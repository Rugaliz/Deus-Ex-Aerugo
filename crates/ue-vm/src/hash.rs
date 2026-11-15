//! `FCollisionHash` (Engine.dll, UnActCol): the level's hash of colliding actors. Space is cut into 256-unit cells
//! on each axis (index `round((v + 32768) / 256)`, clamped to 0..16383); a cell's bucket is `HashX[x] ^ HashY[y] ^
//! HashZ[z]` of three 16384-entry tables shuffled with `appRand` the first time a hash is made (49152 draws of the
//! game's random generator). Each actor is linked into the bucket of every cell its collision box covers (newest link
//! first); a query walks the cells of its box x, then y, then z, each bucket's chain in order, and takes each actor
//! once (`CollisionTag`) when the link's cell key matches. Result lists are built by prepending (line, point and
//! radius checks: the last found comes first) or appending (encroachment).
//!
//! The engine removes an actor from the cells of its box at the current location (asserting it is where it was
//! added); we remember the cells it was added to, which is the same as long as every move goes through remove/add.

use crate::collision::Vec3;
use crate::Vm;
use ue_registry::FxHashMap;

const CELLS: usize = 0x4000;

/// The three shuffled index tables (`FCollisionHash::HashX/Y/Z`, built once: `FCollisionHash::Inited`).
pub struct HashTables {
    x: Vec<u16>,
    y: Vec<u16>,
    z: Vec<u16>,
}

#[derive(Clone, Copy)]
struct Link {
    actor: u32,
    /// Next link in the bucket (index + 1, 0 = end).
    next: u32,
    /// `(z * 256 + y) * 256 + x` of the cell this link was made for.
    key: i32,
}

/// Inclusive cell ranges: x0, x1, y0, y1, z0, z1.
type Cells = [i32; 6];

#[derive(Default)]
pub struct CollisionHash {
    /// Bucket heads (link index + 1, 0 = empty).
    heads: Vec<u32>,
    links: Vec<Link>,
    /// Free links (index + 1, 0 = none), reused last-freed first.
    free: u32,
    /// The cell boxes each actor was added with.
    filed: FxHashMap<u32, Vec<Cells>>,
}

/// `FCollisionHash::GetHashIndices`: `fistp` of `(v + 32768) * (1 / 256)` (round half to even; out of range gives
/// the integer indefinite, which the clamp turns into 0), clamped to 0..16383.
pub fn hash_index(v: f32) -> i32 {
    let t = ((v as f64 + 32768.0) * 0.00390625).round_ties_even();
    let i = if t.is_finite() && t >= i32::MIN as f64 && t <= i32::MAX as f64 { t as i32 } else { i32::MIN };
    i.clamp(0, 0x3fff)
}

fn key(x: i32, y: i32, z: i32) -> i32 {
    z.wrapping_mul(0x100).wrapping_add(y).wrapping_mul(0x100).wrapping_add(x)
}

fn cells_of(min: Vec3, max: Vec3) -> Cells {
    [hash_index(min[0]), hash_index(max[0]), hash_index(min[1]), hash_index(max[1]), hash_index(min[2]), hash_index(max[2])]
}

impl HashTables {
    /// `FCollisionHash::FCollisionHash` (10357ab0) on first use: identity tables, then for every index a swap with
    /// `appRand() & 0x3fff` in X, Y, Z order.
    fn new(rand: &mut impl FnMut() -> u32) -> HashTables {
        let mut t = HashTables { x: (0..CELLS as u16).collect(), y: (0..CELLS as u16).collect(), z: (0..CELLS as u16).collect() };
        for i in 0..CELLS {
            for tab in [&mut t.x, &mut t.y, &mut t.z] {
                let r = (rand() & 0x3fff) as usize;
                tab.swap(i, r);
            }
        }
        t
    }

    fn bucket(&self, x: i32, y: i32, z: i32) -> usize {
        (self.x[x as usize] ^ self.y[y as usize] ^ self.z[z as usize]) as usize
    }
}

impl CollisionHash {
    fn new() -> CollisionHash {
        CollisionHash { heads: vec![0; CELLS], ..Default::default() }
    }

    fn add(&mut self, t: &HashTables, a: u32, c: Cells) {
        for x in c[0]..=c[1] {
            for y in c[2]..=c[3] {
                for z in c[4]..=c[5] {
                    let b = t.bucket(x, y, z);
                    let link = Link { actor: a, next: self.heads[b], key: key(x, y, z) };
                    let i = if self.free != 0 {
                        let i = self.free as usize - 1;
                        self.free = self.links[i].next;
                        self.links[i] = link;
                        i
                    } else {
                        self.links.push(link);
                        self.links.len() - 1
                    };
                    self.heads[b] = i as u32 + 1;
                }
            }
        }
        self.filed.entry(a).or_default().push(c);
    }

    fn remove(&mut self, t: &HashTables, a: u32) {
        let Some(boxes) = self.filed.remove(&a) else { return };
        for c in boxes {
            for x in c[0]..=c[1] {
                for y in c[2]..=c[3] {
                    for z in c[4]..=c[5] {
                        let b = t.bucket(x, y, z);
                        // unlink every link of `a` in the bucket
                        let mut prev: Option<usize> = None;
                        let mut cur = self.heads[b];
                        while cur != 0 {
                            let i = cur as usize - 1;
                            let next = self.links[i].next;
                            if self.links[i].actor == a {
                                match prev {
                                    None => self.heads[b] = next,
                                    Some(p) => self.links[p].next = next,
                                }
                                self.links[i].next = self.free;
                                self.free = cur;
                            } else {
                                prev = Some(i);
                            }
                            cur = next;
                        }
                    }
                }
            }
        }
    }

    /// Actors linked to the cells of `c`, in the engine's walk order, each once.
    fn walk(&self, t: &HashTables, c: Cells) -> Vec<u32> {
        let mut seen = ue_registry::FxHashSet::default();
        let mut out = Vec::new();
        for x in c[0]..=c[1] {
            for y in c[2]..=c[3] {
                for z in c[4]..=c[5] {
                    let k = key(x, y, z);
                    let mut cur = self.heads[t.bucket(x, y, z)];
                    while cur != 0 {
                        let l = self.links[cur as usize - 1];
                        if l.key == k && seen.insert(l.actor) {
                            out.push(l.actor);
                        }
                        cur = l.next;
                    }
                }
            }
        }
        out
    }
}

impl Vm {
    /// `ULevel::SetActorCollision(1)` (1039d390): a new hash (the tables are shuffled the first time), every actor of
    /// the level's list with `bCollideActors` added in list order.
    pub fn build_collision_hash(&mut self) {
        if self.hash_tables.is_none() {
            let mut rand = || self.next_rand();
            let t = HashTables::new(&mut rand);
            self.hash_tables = Some(std::rc::Rc::new(t));
        }
        self.hash = Some(CollisionHash::new());
        let o = self.offsets();
        for a in self.level_actor_list() {
            if self.is_live(a) && self.flag(a, o.flags, crate::collision::F_COLLIDE_ACTORS) {
                self.hash_add(a);
            }
        }
    }

    /// `ULevel::SetActorCollision(0)`: the hash goes away.
    pub fn drop_collision_hash(&mut self) {
        self.hash = None;
    }

    /// `FCollisionHash::GetActorExtent` (10357e30): the cells of the actor's collision box (its primitive's
    /// `GetCollisionBoundingBox`: a brush's model box through `ABrush::ToWorld` (103f0e70), anything else the
    /// cylinder grown by one unit (103d8460)).
    pub(crate) fn actor_hash_box(&mut self, a: u32) -> (Vec3, Vec3) {
        let o = self.offsets();
        let (loc, rot) = (self.vec_at(a, o.location), self.rot_of(a));
        self.actor_hash_box_at(a, loc, rot)
    }

    /// `actor_hash_box` with the actor placed at `loc`/`rot` (the encroachment test moves it there first).
    pub(crate) fn actor_hash_box_at(&mut self, a: u32, loc: Vec3, rot: [i32; 3]) -> (Vec3, Vec3) {
        if self.is_brush(a) {
            if let Some((bl, bh)) = self.brush_model_box(a) {
                let c = self.brush_to_world(a, loc, rot);
                let (mut lo, mut hi) = ([0.0f32; 3], [0.0f32; 3]);
                for i in 0..8 {
                    let p = [if i & 4 == 0 { bl[0] } else { bh[0] }, if i & 2 == 0 { bl[1] } else { bh[1] }, if i & 1 == 0 { bl[2] } else { bh[2] }];
                    let w = c.transform_point(p);
                    for k in 0..3 {
                        if i == 0 || w[k] < lo[k] {
                            lo[k] = w[k];
                        }
                        if i == 0 || w[k] > hi[k] {
                            hi[k] = w[k];
                        }
                    }
                }
                return (lo, hi);
            }
            // a brush without a model box: the model's stored (unowned) box would be used; ours has none
        }
        let (r, h) = self.cylinder(a);
        let (r, h) = (r + 1.0, h + 1.0);
        ([loc[0] - r, loc[1] - r, loc[2] - h], [loc[0] + r, loc[1] + r, loc[2] + h])
    }

    /// `FCollisionHash::AddActor` (103580b0): links `a` into every cell of its box (a deleted actor is skipped).
    pub(crate) fn hash_add(&mut self, a: u32) {
        let o = self.offsets();
        if self.hash.is_none() || self.flag(a, o.b_delete_me.0, o.b_delete_me.1) {
            return;
        }
        let (lo, hi) = self.actor_hash_box(a);
        let t = self.hash_tables.clone().expect("hash tables");
        self.hash.as_mut().unwrap().add(&t, a, cells_of(lo, hi));
    }

    /// `FCollisionHash::RemoveActor` (103583a0).
    pub(crate) fn hash_remove(&mut self, a: u32) {
        let o = self.offsets();
        if self.flag(a, o.b_delete_me.0, o.b_delete_me.1) {
            return;
        }
        if let (Some(h), Some(t)) = (self.hash.as_mut(), self.hash_tables.clone()) {
            h.remove(&t, a);
        }
    }

    /// Removes `a` if it collides with actors (the engine's `if (bCollideActors && Hash) Hash->RemoveActor`).
    pub(crate) fn hash_remove_colliding(&mut self, a: u32) {
        let o = self.offsets();
        if self.flag(a, o.flags, crate::collision::F_COLLIDE_ACTORS) {
            self.hash_remove(a);
        }
    }

    /// Adds `a` if it collides with actors.
    pub(crate) fn hash_add_colliding(&mut self, a: u32) {
        let o = self.offsets();
        if self.flag(a, o.flags, crate::collision::F_COLLIDE_ACTORS) {
            self.hash_add(a);
        }
    }

    /// Whether `a` is linked in the hash.
    pub fn in_collision_hash(&self, a: u32) -> bool {
        self.hash.as_ref().is_some_and(|h| h.filed.contains_key(&a))
    }

    /// Actors linked to the cells of the box `min`..`max`, in the hash's walk order (each once).
    pub(crate) fn hash_walk(&mut self, min: Vec3, max: Vec3) -> Vec<u32> {
        let (Some(h), Some(t)) = (self.hash.as_ref(), self.hash_tables.as_ref()) else { return Vec::new() };
        h.walk(t, cells_of(min, max))
    }

    /// `FCollisionHash::ActorRadiusCheck` (103589e0): actors whose location is closer than `radius` to `loc`, the last
    /// found first.
    pub fn hash_radius_check(&mut self, loc: Vec3, radius: f32) -> Vec<u32> {
        let o = self.offsets();
        let r3 = [radius; 3];
        let found = self.hash_walk(std::array::from_fn(|k| loc[k] - r3[k]), std::array::from_fn(|k| loc[k] + r3[k]));
        let mut out = Vec::new();
        for a in found {
            let l = self.vec_at(a, o.location);
            let (dz, dy, dx) = (l[2] - loc[2], l[1] - loc[1], l[0] - loc[0]);
            if dz * dz + dy * dy + dx * dx < radius * radius {
                out.insert(0, a);
            }
        }
        out
    }

    /// Colliding actors that are not filed in the hash, filed actors that do not collide, and actors filed under
    /// other cells than their current box covers (each would make the hash disagree with the engine's).
    pub fn collision_hash_mismatches(&mut self) -> Vec<u32> {
        let o = self.offsets();
        let mut bad = Vec::new();
        if self.hash.is_none() {
            return bad;
        }
        for a in self.live_actors() {
            let colliding = self.flag(a, o.flags, crate::collision::F_COLLIDE_ACTORS) && !self.flag(a, o.b_delete_me.0, o.b_delete_me.1);
            let filed = self.hash.as_ref().unwrap().filed.get(&a).cloned();
            match (colliding, filed) {
                (false, None) => {}
                (true, Some(boxes)) => {
                    let (lo, hi) = self.actor_hash_box(a);
                    if boxes != vec![cells_of(lo, hi)] {
                        bad.push(a);
                    }
                }
                _ => bad.push(a),
            }
        }
        bad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_indices_round_half_to_even_and_clamp() {
        assert_eq!(hash_index(0.0), 128);
        // (128 + 32768) / 256 = 128.5 -> 128; (384 + 32768) / 256 = 129.5 -> 130
        assert_eq!(hash_index(128.0), 128);
        assert_eq!(hash_index(384.0), 130);
        assert_eq!(hash_index(-1e9), 0);
        assert_eq!(hash_index(1e9), 0x3fff);
        assert_eq!(hash_index(f32::NAN), 0);
        assert_eq!(hash_index(1e30), 0, "out of range: integer indefinite, clamped to 0");
    }

    #[test]
    fn shuffle_draws_three_numbers_per_index() {
        let mut n = 0u32;
        let mut rand = || {
            n += 1;
            n.wrapping_mul(2654435761) >> 17
        };
        let t = HashTables::new(&mut rand);
        assert_eq!(n, 3 * 16384);
        for tab in [&t.x, &t.y, &t.z] {
            let mut v = tab.clone();
            v.sort();
            assert!(v.iter().enumerate().all(|(i, &x)| x as usize == i), "a permutation");
        }
    }

    #[test]
    fn chains_are_newest_first_and_removal_unlinks() {
        let mut rand = || 0u32;
        let t = HashTables::new(&mut rand);
        let mut h = CollisionHash::new();
        let c = cells_of([0.0; 3], [10.0; 3]);
        h.add(&t, 1, c);
        h.add(&t, 2, c);
        h.add(&t, 3, cells_of([1000.0; 3], [1010.0; 3]));
        assert_eq!(h.walk(&t, c), vec![2, 1]);
        // re-adding moves an actor to the front
        h.remove(&t, 1);
        h.add(&t, 1, c);
        assert_eq!(h.walk(&t, c), vec![1, 2]);
        h.remove(&t, 2);
        assert_eq!(h.walk(&t, c), vec![1]);
        // a box spanning both groups walks x, then y, then z
        assert_eq!(h.walk(&t, cells_of([0.0; 3], [1010.0; 3])), vec![1, 3]);
    }
}

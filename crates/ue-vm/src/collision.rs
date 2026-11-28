//! Collision queries over the VM's actors and the world BSP: cylinder primitives, multi-hit traces, and the property
//! offsets they need (spec: `docs/collision.md`). Ports of `UPrimitive::LineCheck`, `ULevel::MultiLineCheck` and
//! `ULevel::SingleLineCheck`.

use crate::{ref_instance, Vm};
use ue_registry::ClassId;

pub type Vec3 = [f32; 3];

/// `TRACE_*` flags of `SingleLineCheck`.
pub const TRACE_PAWNS: u32 = 0x01;
pub const TRACE_MOVERS: u32 = 0x02;
pub const TRACE_LEVEL: u32 = 0x04;
pub const TRACE_ZONE_CHANGES: u32 = 0x08;
pub const TRACE_OTHERS: u32 = 0x10;
pub const TRACE_ONLY_PROJ_ACTOR: u32 = 0x20;
pub const TRACE_ACTORS: u32 = TRACE_PAWNS | TRACE_MOVERS | TRACE_ZONE_CHANGES | TRACE_OTHERS;
/// Used by `Trace` when `bTraceActors` is set.
pub const TRACE_PROJ_TARGETS: u32 = TRACE_PAWNS | TRACE_OTHERS | TRACE_ONLY_PROJ_ACTOR;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CheckResult {
    /// Actor hit (the level object for the world BSP); 0 for no hit.
    pub actor: u32,
    pub location: Vec3,
    pub normal: Vec3,
    /// Fraction of the way from start to end.
    pub time: f32,
    /// BSP node index for world hits, -1 otherwise.
    pub item: i32,
}

impl CheckResult {
    pub fn none() -> CheckResult {
        CheckResult { actor: 0, location: [0.0; 3], normal: [0.0; 3], time: 1.0, item: -1 }
    }
}

/// Offsets of the `Actor` properties the engine code reads directly, and the classes it tests for.
#[derive(Debug, Clone, Copy)]
pub struct Offsets {
    pub location: usize,
    pub rotation: usize,
    pub velocity: usize,
    pub acceleration: usize,
    pub collision_radius: usize,
    pub collision_height: usize,
    /// Word holding the collision flags.
    pub flags: usize,
    pub owner: usize,
    pub base: usize,
    pub level: usize,
    pub brush: usize,
    pub standing_count: usize,
    pub physics: usize,
    /// `Actor.Role` (a byte).
    pub role: usize,
    pub zone: usize,
    pub b_static: (usize, u32),
    pub b_movable: (usize, u32),
    pub b_delete_me: (usize, u32),
    pub b_hidden: (usize, u32),
    pub touching: usize,
    pub class_pawn: ClassId,
    pub class_mover: ClassId,
    pub class_level_info: ClassId,
    pub class_zone_info: ClassId,
    pub class_projectile: ClassId,
    pub class_brush: ClassId,
    pub class_decoration: ClassId,
    pub class_inventory: ClassId,
    pub class_player_pawn: ClassId,
    /// `LevelInfo.bBegunPlay`.
    pub b_begun_play: (usize, u32),
}

pub const F_COLLIDE_ACTORS: u32 = 0x1;
pub const F_COLLIDE_WORLD: u32 = 0x2;
pub const F_BLOCK_ACTORS: u32 = 0x4;
pub const F_BLOCK_PLAYERS: u32 = 0x8;
pub const F_PROJ_TARGET: u32 = 0x10;

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub use ue_bsp::safe_normal;

/// Swept box (half-extent `extent`) against the cylinder of an actor at `loc`: `UPrimitive::LineCheck` (103d8af0),
/// the engine's default primitive (`UMesh::LineCheck` forwards to it). `end`/`start` are the segment ends. Returns
/// `Some((time, location, normal))` on a hit. The side test keeps the root, both roots, the entry time and the
/// back-off in extended precision (double here) and rounds where the engine stores floats.
pub fn cylinder_line_check(loc: Vec3, radius: f32, height: f32, end: Vec3, start: Vec3, extent: Vec3) -> Option<(f32, Vec3, Vec3)> {
    let h = extent[2] + height;
    let ry = extent[1] + radius;
    let rx = extent[0] + radius;
    if (rx + loc[0] < start[0] && rx + loc[0] < end[0]) || (loc[0] - rx > start[0] && loc[0] - rx > end[0]) {
        return None;
    }
    if (ry + loc[1] < start[1] && ry + loc[1] < end[1]) || (loc[1] - ry > start[1] && loc[1] - ry > end[1]) {
        return None;
    }
    let top = h + loc[2];
    let bottom = loc[2] - h;
    if (top < start[2] && top < end[2]) || (bottom > start[2] && bottom > end[2]) {
        return None;
    }
    let mut t_in = 0.0f32;
    let mut t_out = 1.0f32;
    let mut normal = [0.0f32; 3];
    // top cap
    if start[2] > top && end[2] < top {
        let t = (top - start[2]) / (end[2] - start[2]);
        if t > 0.0 {
            normal = [0.0, 0.0, 1.0];
            t_in = t;
        }
    } else if start[2] < top && end[2] > top {
        let t = (top - start[2]) / (end[2] - start[2]);
        if t < t_out {
            t_out = t;
        }
    }
    // bottom cap
    if start[2] < bottom && end[2] > bottom {
        let t = (bottom - start[2]) / (end[2] - start[2]);
        if t_in < t {
            normal = [0.0, 0.0, -1.0];
            t_in = t;
        }
    } else if start[2] > bottom && end[2] < bottom {
        let t = (bottom - start[2]) / (end[2] - start[2]);
        if t < t_out {
            t_out = t;
        }
    }
    if t_out <= t_in {
        return None;
    }
    // the side: a circle of radius `rx` around the actor in the XY plane (a, b, c and the discriminant are floats)
    let rel = [start[0] - loc[0], start[1] - loc[1]];
    let d = [end[0] - start[0], end[1] - start[1]];
    let a = d[0] * d[0] + d[1] * d[1];
    let b = 2.0 * (d[0] * rel[0] + d[1] * rel[1]);
    let c = rel[0] * rel[0] + rel[1] * rel[1] - rx * rx;
    let disc = ((b as f64) * (b as f64) - (c as f64) * (a as f64) * 4.0) as f32;
    if c >= 1.0 || start[2] <= bottom || top <= start[2] {
        if disc < 0.0 {
            return None;
        }
        if a as f64 >= 1e-8 {
            let sq = (disc as f64).sqrt();
            let k = 0.5 / a as f64;
            let t1 = (sq - b as f64) * k;
            if t_out as f64 > t1 {
                t_out = t1 as f32;
            }
            let t0 = -((sq + b as f64) * k);
            if t0 > t_in as f64 {
                t_in = t0 as f32;
                let nx = (((d[0] as f64 * t0) as f32) + start[0]) - loc[0];
                let ny = (((d[1] as f64 * t0) as f32) + start[1]) - loc[1];
                let (x, y) = (nx as f64, ny as f64);
                let l2 = x * x + y * y;
                normal = if l2 >= 1e-8 {
                    let s = 1.0 / l2.sqrt();
                    [(s * x) as f32, (s * y) as f32, 0.0]
                } else {
                    [nx, ny, 0.0]
                };
            }
            if t_out <= t_in {
                return None;
            }
        } else if c > 0.0 {
            return None;
        }
        // back off 0.001 (double), clamp to 0..1 (NaN gives 0); the location uses the unrounded time
        let mut t = t_in as f64 - 0.001;
        t = if t < 0.0 || t.is_nan() { 0.0 } else if t < 1.0 { t } else { 1.0 };
        let dy = end[1] - start[1];
        let dz = end[2] - start[2];
        let location = [((d[0] as f64 * t) as f32) + start[0], ((dy as f64 * t) as f32) + start[1], (dz as f64 * t + start[2] as f64) as f32];
        Some((t as f32, location, normal))
    } else {
        // starting inside the cylinder: only a hit when moving toward its axis (double -0.1)
        if -0.1 <= d[1] as f64 * rel[1] as f64 + d[0] as f64 * rel[0] as f64 {
            return None;
        }
        Some((0.0, start, safe_normal([rel[0], rel[1], 0.0])))
    }
}

/// Box at `loc` (half-extent `extent`) against an actor's cylinder (`UPrimitive::PointCheck`, 103d87e0; Y extent
/// unused); `Some((location, normal))` pushes out. A steep normal (|z| > 0.5) puts the box at `Owner.Z - Extent.Z`
/// for both caps; otherwise the height is `Owner.Z - Extent.X * flat.Z + Location.Z` (as the asm computes it).
pub fn cylinder_point_check(owner_loc: Vec3, radius: f32, height: f32, loc: Vec3, extent: Vec3) -> Option<(Vec3, Vec3)> {
    let dz = owner_loc[2] - loc[2];
    let reach_z = extent[2] + height;
    if reach_z * reach_z <= dz * dz {
        return None;
    }
    let (dx, dy) = (owner_loc[0] - loc[0], owner_loc[1] - loc[1]);
    let reach = extent[0] + radius;
    if reach * reach <= dx * dx + dy * dy {
        return None;
    }
    let normal = safe_normal(sub(loc, owner_loc));
    if normal[2] < -0.5 || normal[2] > 0.5 {
        return Some(([loc[0], loc[1], owner_loc[2] - extent[2]], normal));
    }
    let flat = safe_normal([normal[0], normal[1], 0.0]);
    Some(([owner_loc[0] - extent[0] * flat[0], owner_loc[1] - extent[0] * flat[1], owner_loc[2] - extent[0] * flat[2] + loc[2]], normal))
}

impl Vm {
    /// Property offsets and classes the collision code needs (computed once).
    pub fn offsets(&mut self) -> Offsets {
        if let Some(o) = self.offsets {
            return o;
        }
        let actor = self.reg.class_by_name("Actor").expect("Actor class");
        let p = |reg: &ue_registry::Registry, n: &str| reg.find_prop(actor, n).unwrap_or_else(|| panic!("Actor.{n}")).clone();
        let off = |reg: &ue_registry::Registry, n: &str| p(reg, n).offset as usize;
        let bit = |reg: &ue_registry::Registry, n: &str| {
            let d = p(reg, n);
            (d.offset as usize, d.bit_mask)
        };
        let cls = |reg: &ue_registry::Registry, n: &str| reg.class_by_name(n).unwrap_or_else(|| panic!("class {n}"));
        let r = &self.reg;
        let o = Offsets {
            location: off(r, "Location"),
            rotation: off(r, "Rotation"),
            velocity: off(r, "Velocity"),
            acceleration: off(r, "Acceleration"),
            collision_radius: off(r, "CollisionRadius"),
            collision_height: off(r, "CollisionHeight"),
            flags: bit(r, "bCollideActors").0,
            owner: off(r, "Owner"),
            base: off(r, "Base"),
            level: off(r, "Level"),
            brush: off(r, "Brush"),
            standing_count: off(r, "StandingCount"),
            physics: off(r, "Physics"),
            role: off(r, "Role"),
            zone: off(r, "Region"),
            b_static: bit(r, "bStatic"),
            b_movable: bit(r, "bMovable"),
            b_delete_me: bit(r, "bDeleteMe"),
            b_hidden: bit(r, "bHidden"),
            touching: off(r, "Touching"),
            class_pawn: cls(r, "Pawn"),
            class_mover: cls(r, "Mover"),
            class_level_info: cls(r, "LevelInfo"),
            b_begun_play: {
                let li = r.class_by_name("LevelInfo").expect("LevelInfo class");
                let d = r.find_prop(li, "bBegunPlay").expect("LevelInfo.bBegunPlay");
                (d.offset as usize, d.bit_mask)
            },
            class_zone_info: cls(r, "ZoneInfo"),
            class_projectile: cls(r, "Projectile"),
            class_brush: cls(r, "Brush"),
            class_decoration: cls(r, "Decoration"),
            class_inventory: cls(r, "Inventory"),
            class_player_pawn: cls(r, "PlayerPawn"),
        };
        self.offsets = Some(o);
        o
    }

    fn raw(&self, obj: u32) -> Option<&[u8]> {
        let o = self.objects.get(ref_instance(obj)?)?;
        o.alive.then_some(&o.mem.data[..])
    }

    pub fn f32_at(&self, obj: u32, off: usize) -> f32 {
        self.raw(obj).map(|d| f32::from_le_bytes(d[off..off + 4].try_into().unwrap())).unwrap_or(0.0)
    }

    pub fn u32_at(&self, obj: u32, off: usize) -> u32 {
        self.raw(obj).map(|d| u32::from_le_bytes(d[off..off + 4].try_into().unwrap())).unwrap_or(0)
    }

    pub fn vec_at(&self, obj: u32, off: usize) -> Vec3 {
        [self.f32_at(obj, off), self.f32_at(obj, off + 4), self.f32_at(obj, off + 8)]
    }

    pub fn set_vec_at(&mut self, obj: u32, off: usize, v: Vec3) {
        if let Some(i) = ref_instance(obj) {
            if let Some(o) = self.objects.get_mut(i) {
                for k in 0..3 {
                    if let Some(d) = o.mem.data.get_mut(off + k * 4..off + k * 4 + 4) {
                        d.copy_from_slice(&v[k].to_le_bytes());
                    }
                }
            }
        }
    }

    pub fn u8_at(&self, obj: u32, off: usize) -> u8 {
        self.raw(obj).map(|d| d[off]).unwrap_or(0)
    }

    pub fn set_u8_at(&mut self, obj: u32, off: usize, v: u8) {
        if let Some(i) = ref_instance(obj) {
            if let Some(o) = self.objects.get_mut(i) {
                // (a slot of a level left behind has no memory any more)
                if let Some(d) = o.mem.data.get_mut(off) {
                    *d = v;
                }
            }
        }
    }

    pub fn set_u32_at(&mut self, obj: u32, off: usize, v: u32) {
        if let Some(i) = ref_instance(obj) {
            if let Some(o) = self.objects.get_mut(i) {
                if let Some(d) = o.mem.data.get_mut(off..off + 4) {
                    d.copy_from_slice(&v.to_le_bytes());
                }
            }
        }
    }

    pub fn flag(&self, obj: u32, word: usize, mask: u32) -> bool {
        self.u32_at(obj, word) & mask != 0
    }

    /// Is `obj` an instance of `class` (or a subclass)?
    pub fn is_instance_of(&self, obj: u32, class: ClassId) -> bool {
        self.class_of(obj).map(|c| self.is_a(c, class)).unwrap_or(false)
    }

    /// Location of an actor (zero for none).
    pub fn location(&mut self, obj: u32) -> Vec3 {
        let o = self.offsets();
        self.vec_at(obj, o.location)
    }

    /// Collision cylinder (radius, height) of an actor.
    pub fn cylinder(&mut self, obj: u32) -> (f32, f32) {
        let o = self.offsets();
        (self.f32_at(obj, o.collision_radius), self.f32_at(obj, o.collision_height))
    }

    /// Refiles an actor in the collision hash after its collision properties were changed directly (engine-side code
    /// and tests; scripts go through `SetCollision`/`SetCollisionSize`): out, then back in if it collides.
    pub fn refresh_collision(&mut self, obj: u32) {
        self.hash_remove(obj);
        if self.is_live(obj) {
            self.hash_add_colliding(obj);
        }
    }

    /// `ULevel::SetActorCollision(1)`: a fresh collision hash from the level's actor list.
    pub fn rebuild_collision_set(&mut self) {
        self.build_collision_hash();
    }

    fn actor_class(&self) -> ClassId {
        self.reg.class_by_name("Actor").expect("Actor")
    }

    /// The level's actor list in order (`GLevel->Actors`).
    pub(crate) fn level_actor_list(&self) -> Vec<u32> {
        self.live_actors()
    }

    /// The model `BoundingBox` of a brush actor's `Brush` (cached by brush reference).
    pub(crate) fn brush_model_box(&mut self, a: u32) -> Option<(Vec3, Vec3)> {
        let o = self.offsets();
        let brush = self.u32_at(a, o.brush);
        if brush == 0 {
            return None;
        }
        if let Some(b) = self.brush_boxes.get(&brush) {
            return *b;
        }
        let path = self.reg.obj_path(ue_registry::ObjId(brush)).to_string();
        let bx = self.reg.export_by_path(&path).and_then(|(pi, ei)| {
            let m = ue_assets::model::Model::load(&self.reg.packages[pi].pkg, ei).ok()?;
            let b = &m.bounding_box;
            (b.is_valid).then_some((b.min, b.max))
        });
        self.brush_boxes.insert(brush, bx);
        bx
    }

    /// Traces against the world BSP and the colliding actors; hits are returned nearest first
    /// (`ULevel::MultiLineCheck`). `level` includes the world BSP, `actors` the collision hash.
    pub fn multi_line_check(&mut self, start: Vec3, end: Vec3, extent: Vec3, level: bool, actors: bool, extra_flags: u32) -> Vec<CheckResult> {
        let o = self.offsets();
        let mut results: Vec<CheckResult> = Vec::new();
        let mut end = end;
        let mut scale = 1.0f32;
        let mut only_movers = false;
        let mut level_hit_loc = None;
        if level && self.level != 0 {
            if let Some(bsp) = &self.bsp {
                if let Some(h) = bsp.line_check(start, end, extent, extra_flags) {
                    let d = {
                        let v = sub(h.location, start);
                        dot(v, v).sqrt()
                    };
                    scale = ((d + 5.0) * h.time) / (d + 0.0001);
                    if scale >= 1.0 {
                        scale = 1.0;
                    }
                    end = [
                        (end[0] - start[0]) * scale + start[0],
                        start[1] + (end[1] - start[1]) * scale,
                        (end[2] - start[2]) * scale + start[2],
                    ];
                    if h.time < 0.01 && d < 30.0 {
                        only_movers = true;
                    }
                    level_hit_loc = Some((h.location, h.normal));
                    results.push(CheckResult { actor: self.level, location: h.location, normal: h.normal, time: h.time, item: h.node });
                }
            }
        }
        if actors && self.hash.is_some() {
            // `FCollisionHash::ActorLineCheck` (10359160) on the shortened trace: the box of both ends grown by the
            // extent, every actor found tested with its primitive (a brush's model, else the cylinder), hits
            // prepended to the list
            let bmin: Vec3 = std::array::from_fn(|k| start[k].min(end[k]) - extent[k]);
            let bmax: Vec3 = std::array::from_fn(|k| start[k].max(end[k]) + extent[k]);
            let mut hits: Vec<CheckResult> = Vec::new();
            for a in self.hash_walk(bmin, bmax) {
                if !self.is_live(a) {
                    continue;
                }
                if self.is_brush(a) {
                    if let Some(h) = self.mover_line_check(a, start, end, extent) {
                        hits.push(h);
                    }
                    continue;
                }
                let loc = self.vec_at(a, o.location);
                let (r, h) = (self.f32_at(a, o.collision_radius), self.f32_at(a, o.collision_height));
                if let Some((time, location, normal)) = cylinder_line_check(loc, r, h, end, start, extent) {
                    hits.push(CheckResult { actor: a, location, normal, time, item: -1 });
                }
            }
            hits.reverse();
            // `MultiLineCheck` copies the list in order, 64 hits at most (the world hit counts)
            for mut h in hits {
                if results.len() >= 64 {
                    break;
                }
                let mover = self.is_instance_of(h.actor, o.class_mover);
                if only_movers && !mover {
                    continue;
                }
                // a mover hit on the world hit (same normal, within 2 units) is put 2 units before it along the
                // (shortened) trace, its time measured on that trace (Engine.dll 1039b4ed)
                if let (true, Some((ll, ln))) = (mover, level_hit_loc) {
                    let dl = sub(h.location, ll);
                    if h.normal == ln && dot(dl, dl) < 4.0 {
                        let d = sub(end, start);
                        let len = (dot(d, d) as f64).sqrt();
                        let inv = 1.0 / len;
                        let u: Vec3 = std::array::from_fn(|k| (d[k] as f64 * inv) as f32);
                        h.location = [ll[0] - (u[0] + u[0]), ll[1] - (u[1] + u[1]), ll[2] - (u[2] + u[2])];
                        let r = sub(h.location, start);
                        h.time = (dot(r, r) as f64).sqrt() as f32 / len as f32;
                    }
                }
                h.time *= scale;
                results.push(h);
            }
        }
        // `appQsort` by time (comparator 1039c060): MSVCRT's unstable sort decides the order of equal times
        crate::crt::qsort(&mut results, |v, a, b| crate::crt::x87_cmp(v[a].time, v[b].time));
        results
    }

    /// First hit of a trace honouring `TRACE_*` flags, ignoring `source` and its owner chain
    /// (`ULevel::SingleLineCheck`). `time == 1.0` and `actor == 0` when nothing is hit.
    pub fn single_line_check(&mut self, source: u32, end: Vec3, start: Vec3, flags: u32, extent: Vec3, extra_flags: u32) -> CheckResult {
        let o = self.offsets();
        let list = self.multi_line_check(start, end, extent, flags & TRACE_LEVEL != 0, flags & 0x17 != 0, extra_flags);
        'next: for h in list {
            // skip the source and everything that owns it
            let mut cur = source;
            while cur != 0 {
                if cur == h.actor {
                    continue 'next;
                }
                cur = self.u32_at(cur, o.owner);
            }
            let class_ok = if self.is_instance_of(h.actor, o.class_level_info) {
                flags & TRACE_LEVEL != 0
            } else if self.is_instance_of(h.actor, o.class_pawn) {
                flags & TRACE_PAWNS != 0
            } else if self.is_instance_of(h.actor, o.class_mover) {
                flags & TRACE_MOVERS != 0
            } else if self.is_instance_of(h.actor, o.class_zone_info) {
                flags & TRACE_ZONE_CHANGES != 0
            } else if flags & TRACE_OTHERS != 0 {
                let f = self.u32_at(h.actor, o.flags);
                flags & TRACE_ONLY_PROJ_ACTOR == 0 || f & F_PROJ_TARGET != 0 || (f & F_BLOCK_ACTORS != 0 && f & F_BLOCK_PLAYERS != 0)
            } else {
                false
            };
            if class_ok {
                return h;
            }
        }
        CheckResult::none()
    }
}

/// A BSP surface's texture (name, group as registry name ids) and surface flags.
#[derive(Debug, Clone)]
pub struct SurfInfo {
    pub texture_name: u32,
    pub texture_group: u32,
    pub flags: u32,
    /// Object path of the surface's texture.
    pub texture_path: Option<std::rc::Rc<str>>,
}

impl Vm {
    /// `Friction` of the texture of the surface of BSP node `node` (`None` without a texture): the value stored with
    /// the texture, else 1.0 (`UTexture::UTexture` 103ed3d0 sets it after the class defaults, which read 0).
    pub(crate) fn surface_friction(&mut self, node: i32) -> Option<f32> {
        let bsp = self.bsp.as_ref()?;
        let n = bsp.nodes.get(usize::try_from(node).ok()?)?;
        let path = self.surf_info.get(usize::try_from(n.i_surf).ok()?)?.texture_path.clone()?;
        if let Some(&f) = self.texture_friction.get(&*path) {
            return Some(f);
        }
        self.reg.ensure_package_for_path(&path);
        let f = self
            .reg
            .export_by_path(&path)
            .and_then(|(pi, ei)| ue_assets::property_values(&self.reg.packages[pi].pkg, ei).ok())
            .and_then(|props| props.into_iter().find(|v| v.name.eq_ignore_ascii_case("Friction") && v.data.len() >= 4))
            .map(|v| f32::from_le_bytes([v.data[0], v.data[1], v.data[2], v.data[3]]))
            .unwrap_or(1.0);
        self.texture_friction.insert(path.to_string(), f);
        Some(f)
    }

    /// Texture and flags of the surface of BSP node `node`.
    pub(crate) fn surface_of_node(&self, node: i32) -> Option<((u32, u32), u32)> {
        let bsp = self.bsp.as_ref()?;
        let n = bsp.nodes.get(usize::try_from(node).ok()?)?;
        let s = self.surf_info.get(usize::try_from(n.i_surf).ok()?)?;
        Some(((s.texture_name, s.texture_group), s.flags))
    }
}

impl Vm {
    /// Texture slots of a mesh object (`Mesh` property value), as object path ids; empty if unknown.
    pub(crate) fn mesh_texture_slots(&mut self, mesh_ref: u32) -> Vec<u32> {
        if let Some(v) = self.mesh_textures.get(&mesh_ref) {
            return v.clone();
        }
        let path = self.reg.obj_path(ue_registry::ObjId(mesh_ref)).to_string();
        let slots = (|| {
            let (pi, ei) = self.reg.export_by_path(&path)?;
            let pkg = &self.reg.packages[pi].pkg;
            let raw: Vec<i32> = match pkg.export_class_name(ei).as_str() {
                "LodMesh" => ue_assets::mesh::LodMesh::load(pkg, ei).ok()?.mesh.textures,
                "Mesh" => ue_assets::mesh::Mesh::load(pkg, ei).ok()?.textures,
                _ => return None,
            };
            Some((pi, raw))
        })();
        let ids = match slots {
            Some((pi, raw)) => raw.iter().map(|&r| self.reg.ref_path(pi, r).map(|p| self.reg.obj_id(&p).0).unwrap_or(0)).collect(),
            None => Vec::new(),
        };
        self.mesh_textures.insert(mesh_ref, ids.clone());
        ids
    }

    /// `Actor.GetMeshTexture(texnum)`: the texture the actor draws mesh slot `texnum` with.
    pub fn get_mesh_texture(&mut self, a: u32, texnum: i32) -> u32 {
        let actor = self.actor_class();
        let off = |vm: &Vm, n: &str| vm.reg.find_prop(actor, n).map(|p| p.offset as usize);
        let (Some(mesh_off), Some(skin_off), Some(multi_off)) = (off(self, "Mesh"), off(self, "Skin"), off(self, "MultiSkins")) else { return 0 };
        let mesh = self.u32_at(a, mesh_off);
        if mesh == 0 {
            return 0;
        }
        let slots = self.mesh_texture_slots(mesh);
        if (0..8).contains(&texnum) {
            let m = self.u32_at(a, multi_off + texnum as usize * 4);
            if m != 0 {
                return m;
            }
        }
        if texnum > 0 && (texnum as usize) < slots.len() && slots[texnum as usize] != 0 {
            return slots[texnum as usize];
        }
        let skin = self.u32_at(a, skin_off);
        if skin != 0 {
            return skin;
        }
        if texnum >= 0 && (texnum as usize) < slots.len() {
            return slots[texnum as usize];
        }
        0
    }
}

impl Vm {
    /// The collision BSP of a mover's brush model (cached by brush reference).
    pub fn mover_bsp(&mut self, a: u32) -> Option<std::rc::Rc<ue_bsp::Bsp>> {
        let o = self.offsets();
        let brush = self.u32_at(a, o.brush);
        if brush == 0 {
            return None;
        }
        if let Some(b) = self.mover_bsps.get(&brush) {
            return b.clone();
        }
        let path = self.reg.obj_path(ue_registry::ObjId(brush)).to_string();
        let bsp = self.reg.export_by_path(&path).and_then(|(pi, ei)| {
            let m = ue_assets::model::Model::load(&self.reg.packages[pi].pkg, ei).ok()?;
            Some(std::rc::Rc::new(ue_bsp::Bsp::from_model(&m)))
        });
        self.mover_bsps.insert(brush, bsp.clone());
        bsp
    }

    /// A `Scale` struct property (`Scale`, `SheerRate`, `SheerAxis`); unit and unsheered when missing.
    fn scale_prop(&mut self, a: u32, name: &'static str) -> (Vec3, f32, u8) {
        match self.get_prop_static(a, name, 0) {
            Ok(crate::Value::Struct(_, b)) if b.len() >= 17 => {
                let f = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
                ([f(0), f(4), f(8)], f(12), b[16])
            }
            _ => ([1.0; 3], 0.0, 0),
        }
    }

    /// `ABrush::ToWorld` (1030f310) of the brush actor `a` placed at `loc` with `rot` (the engine's collision code
    /// moves the actor there before asking): unit coords, `*= Location`, `*= PostScale`, `*= Rotation`, `*=
    /// MainScale` (inlined, same as Core's), `*= -PrePivot`.
    pub fn brush_to_world(&mut self, a: u32, loc: Vec3, rot: [i32; 3]) -> ue_bsp::Coords {
        let pre = match self.get_prop_static(a, "PrePivot", 0) {
            Ok(crate::Value::Vector(v)) => v,
            _ => [0.0; 3],
        };
        let (ms, ms_rate, ms_axis) = self.scale_prop(a, "MainScale");
        let (ps, ps_rate, ps_axis) = self.scale_prop(a, "PostScale");
        let mut c = ue_bsp::Coords::UNIT;
        c.translate(loc);
        c.mul_scale(ps, ps_rate, ps_axis);
        c.mul_rotator(rot, crate::natives::sin_tab);
        c.mul_scale(ms, ms_rate, ms_axis);
        c.translate([-pre[0], -pre[1], -pre[2]]);
        c
    }

    /// The level model's nodes as they are now: the moving-brush tracker filters the movers' polygons into the level's
    /// BSP (the renderer updates it each frame), and the level's collision walks that same BSP: zero-extent traces and
    /// `FastLineCheck` stop at a closed door (its nodes are solid once added), box checks do not (no hulls there).
    pub fn sync_world_nodes(&mut self) {
        if let (Some(b), Some(m)) = (self.bsp.as_mut(), self.world_model.as_ref()) {
            b.nodes = ue_bsp::Bsp::nodes_of(m);
        }
    }

    /// `ABrush::BuildCoords` (1032f9e0) of brush actor `a`: the point frame `GMath.UnitCoords * PostScale * Rotation *
    /// MainScale`, the vector frame `(UnitCoords / MainScale / Rotation / PostScale)` transposed, and the orientation
    /// `Sgn(MainScale)` times `PostScale.Orientation()` (Core 10118880: the sign of `Z * Y * X`).
    pub fn brush_coords(&mut self, a: u32) -> (ue_bsp::Coords, ue_bsp::Coords, f32) {
        let (ms, ms_rate, ms_axis) = self.scale_prop(a, "MainScale");
        let (ps, ps_rate, ps_axis) = self.scale_prop(a, "PostScale");
        let rot = self.rot_of(a);
        let mut point = ue_bsp::Coords::UNIT;
        point.mul_scale(ps, ps_rate, ps_axis);
        point.mul_rotator(rot, crate::natives::sin_tab);
        point.mul_scale(ms, ms_rate, ms_axis);
        let mut inv = ue_bsp::Coords::UNIT;
        inv.div_scale(ms, ms_rate, ms_axis);
        inv.div_rotator(rot, crate::natives::sin_tab);
        inv.div_scale(ps, ps_rate, ps_axis);
        let sgn = |s: Vec3| {
            let v = s[2] * s[1] * s[0];
            if v > 0.0 {
                1.0
            } else if v < 0.0 {
                -1.0
            } else {
                0.0
            }
        };
        (point, inv.transpose(), sgn(ms) * sgn(ps))
    }

    /// A trace against a mover's brush (`UModel::LineCheck` with the mover as owner): the brush planes are moved to
    /// the world through `ABrush::ToWorld`; the trace and its box stay in world space.
    fn mover_line_check(&mut self, a: u32, start: Vec3, end: Vec3, extent: Vec3) -> Option<CheckResult> {
        let o = self.offsets();
        let bsp = self.mover_bsp(a)?;
        let (loc, rot) = (self.vec_at(a, o.location), self.rot_of(a));
        let c = self.brush_to_world(a, loc, rot);
        let h = bsp.line_check_owned(start, end, extent, 0, Some(&c))?;
        Some(CheckResult { actor: a, location: h.location, normal: h.normal, time: h.time, item: h.node })
    }
}

#[cfg(test)]
mod cylinder_tests {
    use super::*;

    #[test]
    fn a_line_through_a_cylinder_enters_its_side() {
        // radius 10, half height 20 at the origin; a zero-extent line along x at z = 0 enters at x = -10 (t = 0.45),
        // backed off by 0.001 of the trace (double) from the entry time stored as a float
        let (t, loc, n) = cylinder_line_check([0.0; 3], 10.0, 20.0, [100.0, 0.0, 0.0], [-100.0, 0.0, 0.0], [0.0; 3]).unwrap();
        let te = 0.45f32 as f64 - 0.001;
        assert_eq!(t, te as f32);
        assert_eq!(loc, [((200.0 * te) as f32) - 100.0, 0.0, 0.0]);
        assert_eq!(n, [-1.0, 0.0, 0.0]);
        // a box of extent 5 enters 5 units earlier
        let (t, _, _) = cylinder_line_check([0.0; 3], 10.0, 20.0, [100.0, 0.0, 0.0], [-100.0, 0.0, 0.0], [5.0, 5.0, 5.0]).unwrap();
        assert_eq!(t, (0.425f32 as f64 - 0.001) as f32);
        // passing beside it misses
        assert!(cylinder_line_check([0.0; 3], 10.0, 20.0, [100.0, 11.0, 0.0], [-100.0, 11.0, 0.0], [0.0; 3]).is_none());
    }

    #[test]
    fn a_line_down_onto_a_cylinder_hits_its_top() {
        let (t, loc, n) = cylinder_line_check([0.0; 3], 10.0, 20.0, [0.0, 0.0, -100.0], [0.0, 0.0, 100.0], [0.0; 3]).unwrap();
        assert_eq!(n, [0.0, 0.0, 1.0]);
        assert_eq!(t, (0.4f32 as f64 - 0.001) as f32);
        assert!((loc[2] - 20.2).abs() < 1e-4, "{loc:?}");
    }

    #[test]
    fn starting_inside_only_hits_when_moving_inwards() {
        // start 5 units off the axis, inside: outward misses, inward hits at time 0 with the outward normal
        assert!(cylinder_line_check([0.0; 3], 10.0, 20.0, [50.0, 0.0, 0.0], [5.0, 0.0, 0.0], [0.0; 3]).is_none());
        let (t, loc, n) = cylinder_line_check([0.0; 3], 10.0, 20.0, [-50.0, 0.0, 0.0], [5.0, 0.0, 0.0], [0.0; 3]).unwrap();
        assert_eq!((t, loc, n), (0.0, [5.0, 0.0, 0.0], [1.0, 0.0, 0.0]));
    }

    #[test]
    fn safe_normal_rounds_once() {
        for v in [[1.0f32, 2.0, 3.0], [0.1, -7.3, 1e-3], [12345.6, 0.001, -3.0]] {
            let l = (v[0] as f64 * v[0] as f64 + v[1] as f64 * v[1] as f64 + v[2] as f64 * v[2] as f64).sqrt();
            let n = safe_normal(v);
            for k in 0..3 {
                assert_eq!(n[k], ((1.0 / l) * v[k] as f64) as f32);
            }
        }
        assert_eq!(safe_normal([1e-5, 0.0, 0.0]), [0.0; 3]);
    }

    #[test]
    fn point_check_push_out_heights() {
        // box of extent (5, 5, 10) beside a cylinder (radius 10, half height 20) at the origin: pushed to
        // Owner - Extent.X * flat, height Owner.Z - Extent.X * flat.Z + Location.Z
        let (loc, n) = cylinder_point_check([0.0; 3], 10.0, 20.0, [8.0, 0.0, 3.0], [5.0, 5.0, 10.0]).unwrap();
        assert_eq!(n, safe_normal([8.0, 0.0, 3.0]));
        assert_eq!(loc, [-5.0, 0.0, 3.0]);
        // nearly above it: both caps put the box at Owner.Z - Extent.Z
        let (loc, _) = cylinder_point_check([0.0; 3], 10.0, 20.0, [1.0, 0.0, 25.0], [5.0, 5.0, 10.0]).unwrap();
        assert_eq!(loc, [1.0, 0.0, -10.0]);
        assert!(cylinder_point_check([0.0; 3], 10.0, 20.0, [16.0, 0.0, 0.0], [5.0, 5.0, 10.0]).is_none());
    }
}

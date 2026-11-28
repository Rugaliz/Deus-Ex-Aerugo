//! Actor movement: zones, touching and bases, spot finding, `MoveActor` and friends. Ports of `ULevel::MoveActor`,
//! `ULevel::FarMoveActor`, `ULevel::SetActorZone`, `ULevel::FindSpot`, `AActor::BeginTouch/EndTouch/SetBase/FindBase`,
//! `AActor::moveSmooth` and `AActor::TwoWallAdjust` (spec: `docs/collision.md`).

use crate::collision::*;
use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn nearly_zero(v: Vec3) -> bool {
    v.iter().all(|c| c.abs() < 0.0001)
}

/// Where a point is: the zone actor, BSP leaf and zone number (`PointRegion`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub zone: u32,
    pub leaf: i32,
    pub zone_number: u8,
}

/// Offsets of the `PointRegion` struct members and a few more actor properties.
#[derive(Debug, Clone, Copy)]
pub struct RegionOffsets {
    pub region: usize,
    pub head_region: usize,
    pub foot_region: usize,
    pub leaf: usize,
    pub zone_number: usize,
    pub eye_height: usize,
    pub view_rotation: usize,
    pub old_location: Option<usize>,
    pub just_teleported: Option<(usize, u32)>,
    pub collide_when_placing: (usize, u32),
}

impl Vm {
    pub(crate) fn region_offsets(&mut self) -> RegionOffsets {
        if let Some(r) = self.region_offsets {
            return r;
        }
        let actor = self.reg.class_by_name("Actor").expect("Actor");
        let pawn = self.reg.class_by_name("Pawn").expect("Pawn");
        let find = |reg: &ue_registry::Registry, c, n: &str| reg.find_prop(c, n).cloned();
        let region = find(&self.reg, actor, "Region").expect("Actor.Region");
        let sid = match region.target {
            ue_registry::Target::Struct(s) => s,
            _ => panic!("Region is not a struct"),
        };
        let member = |n: &str| -> usize {
            let id = self.reg.names.get(n).expect("member name");
            self.reg.struct_def(sid).props.iter().find(|p| p.name == id).map(|p| p.offset as usize).expect("region member")
        };
        let r = RegionOffsets {
            region: region.offset as usize,
            head_region: find(&self.reg, pawn, "HeadRegion").expect("HeadRegion").offset as usize,
            foot_region: find(&self.reg, pawn, "FootRegion").expect("FootRegion").offset as usize,
            leaf: member("iLeaf"),
            zone_number: member("ZoneNumber"),
            eye_height: find(&self.reg, pawn, "EyeHeight").expect("EyeHeight").offset as usize,
            view_rotation: find(&self.reg, pawn, "ViewRotation").expect("ViewRotation").offset as usize,
            old_location: find(&self.reg, actor, "OldLocation").map(|p| p.offset as usize),
            just_teleported: find(&self.reg, actor, "bJustTeleported").map(|p| (p.offset as usize, p.bit_mask)),
            collide_when_placing: find(&self.reg, actor, "bCollideWhenPlacing").map(|p| (p.offset as usize, p.bit_mask)).expect("bCollideWhenPlacing"),
        };
        self.region_offsets = Some(r);
        r
    }

    // ---- zones ---------------------------------------------------------------------------------------------

    /// Zone, leaf and zone number at `loc` (`UModel::PointRegion`); the level itself when the model has no zone actor.
    pub fn point_region(&mut self, loc: Vec3) -> Region {
        let Some(bsp) = &self.bsp else { return Region { zone: self.level, leaf: -1, zone_number: 0 } };
        let r = bsp.point_region(loc);
        let zone = self.zone_actors.get(r.zone as usize).copied().filter(|&z| z != 0).unwrap_or(self.level);
        Region { zone, leaf: r.leaf, zone_number: r.zone }
    }

    pub(crate) fn write_region(&mut self, a: u32, off: usize, r: Region) {
        let ro = self.region_offsets();
        self.set_u32_at(a, off, r.zone);
        self.set_u32_at(a, off + ro.leaf, r.leaf as u32);
        if let Some(i) = crate::ref_instance(a) {
            if let Some(d) = self.objects[i].mem.data.get_mut(off + ro.zone_number) {
                *d = r.zone_number;
            }
        }
    }

    fn read_zone(&self, a: u32, off: usize) -> u32 {
        self.u32_at(a, off)
    }

    /// Updates the zone of an actor (and head/foot zones of pawns), sending the zone events unless `test`.
    pub fn set_actor_zone(&mut self, a: u32, test: bool, force_refresh: bool) -> Res<()> {
        let o = self.offsets();
        let ro = self.region_offsets();
        if !self.is_live(a) || self.flag(a, o.b_delete_me.0, o.b_delete_me.1) {
            return Ok(());
        }
        let level = self.level;
        let none = Region { zone: level, leaf: -1, zone_number: 0 };
        if a == level {
            self.write_region(a, ro.region, none);
            return Ok(());
        }
        let is_pawn = self.is_instance_of(a, o.class_pawn);
        if force_refresh {
            self.write_region(a, ro.region, none);
            if is_pawn {
                self.write_region(a, ro.head_region, none);
                self.write_region(a, ro.foot_region, none);
            }
        }
        let loc = self.vec_at(a, o.location);
        let new = self.point_region(loc);
        let old_zone = self.read_zone(a, ro.region);
        if new.zone == old_zone {
            self.write_region(a, ro.region, new);
        } else {
            if !test {
                if old_zone != 0 {
                    self.send_event(old_zone, "ActorLeaving", vec![Value::Obj(a)])?;
                }
                self.send_event(a, "ZoneChange", vec![Value::Obj(new.zone)])?;
            }
            self.write_region(a, ro.region, new);
            if !test {
                self.send_event(new.zone, "ActorEntered", vec![Value::Obj(a)])?;
            }
        }
        if is_pawn {
            let height = self.f32_at(a, o.collision_height);
            let foot = self.point_region([loc[0], loc[1], loc[2] - height]);
            if foot.zone != self.read_zone(a, ro.foot_region) && !test {
                self.send_event(a, "FootZoneChange", vec![Value::Obj(foot.zone)])?;
            }
            self.write_region(a, ro.foot_region, foot);
            let eye = self.f32_at(a, ro.eye_height);
            let head = self.point_region([loc[0], loc[1], loc[2] + eye]);
            if head.zone != self.read_zone(a, ro.head_region) && !test {
                self.send_event(a, "HeadZoneChange", vec![Value::Obj(head.zone)])?;
            }
            self.write_region(a, ro.head_region, head);
        }
        Ok(())
    }

    // ---- relations -----------------------------------------------------------------------------------------

    pub fn is_based_on(&mut self, a: u32, other: u32) -> bool {
        let o = self.offsets();
        let mut cur = a;
        for _ in 0..64 {
            if cur == 0 {
                return false;
            }
            if cur == other {
                return true;
            }
            cur = self.u32_at(cur, o.base);
        }
        false
    }

    /// `AActor::SetBase`: rebases, keeping the standing counts and sending `Detach`/`Attach` (and `BaseChange`).
    pub fn set_base(&mut self, a: u32, new_base: u32, notify: bool) -> Res<()> {
        let o = self.offsets();
        // refuse a base that is based on this actor
        if self.is_based_on(new_base, a) {
            return Ok(());
        }
        let old = self.u32_at(a, o.base);
        if new_base != old {
            if old != 0 {
                let c = self.u8_at(old, o.standing_count);
                self.set_u8_at(old, o.standing_count, c.wrapping_sub(1));
                self.send_event(old, "Detach", vec![Value::Obj(a)])?;
            }
            self.set_u32_at(a, o.base, new_base);
            if new_base != 0 && new_base != self.u32_at(a, o.level) {
                let c = self.u8_at(new_base, o.standing_count);
                self.set_u8_at(new_base, o.standing_count, c.wrapping_add(1));
                self.send_event(new_base, "Attach", vec![Value::Obj(a)])?;
            }
            if notify {
                self.send_event(a, "BaseChange", vec![])?;
            }
        }
        Ok(())
    }

    /// `AActor::SetSupportBase`: ask the supporting actor (script decides) or clear the base.
    pub(crate) fn set_support_base(&mut self, a: u32, support: u32) -> Res<()> {
        if support != 0 {
            self.send_event(support, "SupportActor", vec![Value::Obj(a)])?;
            Ok(())
        } else {
            self.set_base(a, 0, true)
        }
    }

    /// `AActor::FindBase`: looks 8 units below for something to stand on.
    pub fn find_base(&mut self, a: u32) -> Res<()> {
        let o = self.offsets();
        let (r, h) = self.cylinder(a);
        let loc = self.vec_at(a, o.location);
        let hit = self.single_line_check(a, [loc[0], loc[1], loc[2] - 8.0], loc, 0x17, [r, r, h], 0);
        let base = self.u32_at(a, o.base);
        if base != hit.actor && (hit.actor == 0 || !self.is_based_on(hit.actor, a)) {
            self.set_support_base(a, hit.actor)?;
        }
        Ok(())
    }

    /// Is the actor a player-controlled pawn (as `GetPlayerPawn` decides)?
    fn is_player_pawn(&mut self, a: u32) -> bool {
        let o = self.offsets();
        // `AActor::GetPlayerPawn`: a PlayerPawn that has a viewport (its `Player` is set)
        self.is_instance_of(a, o.class_player_pawn) && self.op(a, "Player") != 0
    }

    /// The bounding box of an actor's brush model in the brush's own space (`None` if it has no loadable brush).
    pub fn brush_local_bounds(&mut self, a: u32) -> Option<([f32; 3], [f32; 3])> {
        let o = self.offsets();
        let brush = self.u32_at(a, o.brush);
        if brush == 0 {
            return None;
        }
        if let Some(b) = self.brush_bounds.get(&brush) {
            return *b;
        }
        let path = self.reg.obj_path(ue_registry::ObjId(brush)).to_string();
        let bounds = self.reg.export_by_path(&path).and_then(|(pi, ei)| {
            let m = ue_assets::model::Model::load(&self.reg.packages[pi].pkg, ei).ok()?;
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &m.points {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            (lo[0] <= hi[0]).then_some((lo, hi))
        });
        self.brush_bounds.insert(brush, bounds);
        bounds
    }

    pub(crate) fn is_brush_actor(&mut self, a: u32) -> bool {
        self.is_brush(a)
    }

    pub(crate) fn is_brush(&mut self, a: u32) -> bool {
        let o = self.offsets();
        self.u32_at(a, o.brush) != 0 && self.is_instance_of(a, o.class_brush)
    }

    /// `AActor::IsBlockedBy`: would `a` be stopped by `other`?
    pub fn is_blocked_by(&mut self, a: u32, other: u32) -> bool {
        let o = self.offsets();
        let fa = self.u32_at(a, o.flags);
        let fo = self.u32_at(other, o.flags);
        if other == self.u32_at(a, o.level) {
            return fa & F_COLLIDE_WORLD != 0;
        }
        if self.is_brush(other) {
            if fa & F_COLLIDE_WORLD == 0 {
                return false;
            }
            let bit = if self.is_player_pawn(a) { F_BLOCK_PLAYERS } else { F_BLOCK_ACTORS };
            return fo & bit != 0;
        }
        if self.is_brush(a) {
            if fo & F_COLLIDE_WORLD == 0 {
                return false;
            }
            let bit = if self.is_player_pawn(other) { F_BLOCK_PLAYERS } else { F_BLOCK_ACTORS };
            return fa & bit != 0;
        }
        let a_pp = self.is_player_pawn(a) || self.is_instance_of(a, o.class_projectile);
        if fo & (if a_pp { F_BLOCK_PLAYERS } else { F_BLOCK_ACTORS }) == 0 {
            return false;
        }
        let o_pp = self.is_player_pawn(other) || self.is_instance_of(other, o.class_projectile);
        fa & (if o_pp { F_BLOCK_PLAYERS } else { F_BLOCK_ACTORS }) != 0
    }

    /// `AActor::IsOverlapping`: do the collision cylinders overlap?
    pub fn is_overlapping(&mut self, a: u32, other: u32) -> bool {
        let o = self.offsets();
        if self.is_brush(a) || self.is_brush(other) || other == self.u32_at(a, o.level) {
            return false;
        }
        let (la, lb) = (self.vec_at(a, o.location), self.vec_at(other, o.location));
        let (ca, cb) = (self.cylinder(a), self.cylinder(other));
        let reach = ca.0 + cb.0;
        let rz = ca.1 + cb.1;
        let (dx, dy, dz) = (la[0] - lb[0], la[1] - lb[1], la[2] - lb[2]);
        dx * dx + dy * dy < reach * reach && dz * dz < rz * rz
    }

    // ---- touching ------------------------------------------------------------------------------------------

    pub(crate) fn touching(&self, a: u32, i: usize) -> u32 {
        let o = self.offsets.expect("offsets");
        self.u32_at(a, o.touching + i * 4)
    }

    fn set_touching(&mut self, a: u32, i: usize, v: u32) {
        let o = self.offsets.expect("offsets");
        self.set_u32_at(a, o.touching + i * 4, v);
    }

    /// Adds `other` to `a`'s touching list (making room if needed) and sends `Touch`; false if `a` ended the touch.
    fn add_touching(&mut self, a: u32, other: u32) -> Res<bool> {
        let o = self.offsets();
        let mut free = None;
        for i in 0..4 {
            let t = self.touching(a, i);
            if t == 0 {
                free = Some(i);
            } else if t == other {
                return Ok(true);
            }
        }
        if free.is_none() {
            // evict something: first an actor without physics, then a non-pawn, then (for a pawn "other") an unblocking one
            for i in 0..4 {
                let t = self.touching(a, i);
                if t != 0 && self.u8_at(t, o.physics) == 0 {
                    self.end_touch(a, t, false)?;
                    free = Some(i);
                }
            }
            if free.is_none() && self.is_instance_of(other, o.class_pawn) {
                for i in 0..4 {
                    let t = self.touching(a, i);
                    if t != 0 && !self.is_instance_of(t, o.class_pawn) {
                        self.end_touch(a, t, false)?;
                        free = Some(i);
                        break;
                    }
                }
                if free.is_none() && self.flag(other, o.flags, F_COLLIDE_ACTORS) {
                    return Ok(true);
                }
            }
        }
        if let Some(i) = free {
            self.set_touching(a, i, other);
            self.send_event(a, "Touch", vec![Value::Obj(other)])?;
            if self.touching(a, i) != other {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn begin_touch(&mut self, a: u32, other: u32) -> Res<()> {
        if self.add_touching(a, other)? {
            self.add_touching(other, a)?;
        }
        Ok(())
    }

    pub fn end_touch(&mut self, a: u32, other: u32, no_notify: bool) -> Res<()> {
        for i in 0..4 {
            if self.touching(a, i) == other {
                self.set_touching(a, i, 0);
                if !no_notify {
                    self.send_event(a, "UnTouch", vec![Value::Obj(other)])?;
                }
                break;
            }
        }
        for i in 0..4 {
            if self.touching(other, i) == a {
                self.set_touching(other, i, 0);
                self.send_event(other, "UnTouch", vec![Value::Obj(a)])?;
                break;
            }
        }
        Ok(())
    }

    // ---- point checks and spot finding ----------------------------------------------------------------------

    /// Boxes overlapping solid or other actors at `loc` (`ULevel::MultiPointCheck`, 1039acc0): the world hit first,
    /// then the hash's hits (`ActorPointCheck`, the last found first).
    pub fn multi_point_check(&mut self, loc: Vec3, extent: Vec3, extra: u32, level: bool, actors: bool) -> Vec<CheckResult> {
        let o = self.offsets();
        let mut out = Vec::new();
        if level && self.level != 0 {
            if let Some(h) = self.bsp.as_ref().and_then(|b| b.point_check(loc, extent, extra)) {
                out.push(CheckResult { actor: self.level, location: h.location, normal: h.normal, time: 0.0, item: h.node });
            }
        }
        if actors && self.hash.is_some() {
            let bmin: Vec3 = std::array::from_fn(|k| loc[k] - extent[k]);
            let bmax: Vec3 = std::array::from_fn(|k| loc[k] + extent[k]);
            let first = out.len();
            for a in self.hash_walk(bmin, bmax) {
                if !self.is_live(a) {
                    continue;
                }
                // `FCollisionHash::ActorPointCheck` (103585b0): an actor with a brush is tested with that model
                // (owner = the actor), any other as its cylinder
                if self.is_brush(a) {
                    let Some(bsp) = self.mover_bsp(a) else { continue };
                    let (l, rot) = (self.vec_at(a, o.location), self.rot_of(a));
                    let c = self.brush_to_world(a, l, rot);
                    if let Some(h) = bsp.point_check_owned(loc, extent, 0, Some(&c)) {
                        out.push(CheckResult { actor: a, location: h.location, normal: h.normal, time: 0.0, item: h.node });
                    }
                    continue;
                }
                let (r, h) = self.cylinder(a);
                let l = self.vec_at(a, o.location);
                if let Some((location, normal)) = cylinder_point_check(l, r, h, loc, extent) {
                    out.push(CheckResult { actor: a, location, normal, time: 0.0, item: -1 });
                }
            }
            out[first..].reverse();
        }
        out
    }

    /// `ULevel::SinglePointCheck`: `None` when the box is free, else the hit nearest to `loc`.
    pub fn single_point_check(&mut self, loc: Vec3, extent: Vec3, extra: u32, level: bool, actors: bool) -> Option<CheckResult> {
        let list = self.multi_point_check(loc, extent, extra, level, actors);
        let dist = |h: &CheckResult| {
            let d = sub(h.location, loc);
            dot(d, d)
        };
        let mut best: Option<CheckResult> = None;
        for h in list {
            if best.as_ref().map(|b| dist(&h) < dist(b)).unwrap_or(true) {
                best = Some(h);
            }
        }
        best
    }

    fn adjust_spot(&mut self, adjusted: &mut Vec3, test: Vec3, trace_dist: f32) {
        let hit = self.single_line_check(0, test, *adjusted, TRACE_LEVEL, [0.0; 3], 0);
        if hit.time < 1.0 {
            *adjusted = add(*adjusted, scale(hit.normal, (1.0 - hit.time) * trace_dist));
        }
    }

    /// `ULevel::FindSpot`: nudges `loc` out of walls so a box of half-extent `extent` fits; false if it cannot.
    pub fn find_spot(&mut self, extent: Vec3, loc: &mut Vec3, check_actors: bool, assume_fit: bool) -> bool {
        if extent == [0.0; 3] {
            return self.single_point_check(*loc, extent, 0, true, check_actors).is_none();
        }
        if assume_fit && self.single_point_check(*loc, extent, 0, true, check_actors).is_none() {
            return true;
        }
        let mut adjusted = *loc;
        // `sqrt` in double precision plus 2.0 (double 1042ead0), stored as a float
        let size = ((dot(extent, extent) as f64).sqrt() + 2.0) as f32;
        for i in [-1.0f32, 1.0] {
            let t = add(adjusted, [i * extent[0], 0.0, 0.0]);
            self.adjust_spot(&mut adjusted, t, extent[0]);
            let t = add(adjusted, [0.0, i * extent[1], 0.0]);
            self.adjust_spot(&mut adjusted, t, extent[1]);
            let t = add(adjusted, [0.0, 0.0, i * extent[2]]);
            self.adjust_spot(&mut adjusted, t, extent[2]);
        }
        if self.single_point_check(adjusted, extent, 0, true, check_actors).is_none() {
            *loc = adjusted;
            return true;
        }
        for i in [-1.0f32, 1.0] {
            for j in [-1.0f32, 1.0] {
                for k in [-1.0f32, 1.0] {
                    let t = add(adjusted, [i * extent[0], j * extent[1], k * extent[2]]);
                    self.adjust_spot(&mut adjusted, t, size);
                }
            }
        }
        let d = sub(adjusted, *loc);
        if dot(d, d) <= dot(extent, extent) * 1.5 && self.single_point_check(adjusted, extent, 0, true, check_actors).is_none() {
            *loc = adjusted;
            return true;
        }
        false
    }

    /// `FCollisionHash::ActorEncroachmentCheck` (10358d50): with `a` placed at `loc`/`rot`, the actors linked in the
    /// cells of its collision box (in the hash's walk order, appended) that its primitive overlaps as cylinders (a
    /// mover's own brush, any other actor its cylinder). Non-static brushes are never listed.
    fn encroachment_list(&mut self, a: u32, loc: Vec3, rot: [i32; 3]) -> Vec<u32> {
        let o = self.offsets();
        let is_brush = self.is_brush(a);
        // the brush placed at the test location (`ABrush::ToWorld`)
        let coords = is_brush.then(|| self.brush_to_world(a, loc, rot));
        let (lo, hi) = self.actor_hash_box_at(a, loc, rot);
        let bsp = if is_brush { self.mover_bsp(a) } else { None };
        let (ra, ha) = self.cylinder(a);
        let mut out = Vec::new();
        for x in self.hash_walk(lo, hi) {
            if x == a || !self.is_live(x) || x == self.level {
                continue;
            }
            if self.is_brush(x) && !self.flag(x, o.b_static.0, o.b_static.1) {
                continue;
            }
            let (r, h) = self.cylinder(x);
            let xl = self.vec_at(x, o.location);
            let overlap = if is_brush {
                match &bsp {
                    Some(b) => b.point_check_owned(xl, [r, r, h], 0, coords.as_ref()).is_some(),
                    None => false,
                }
            } else {
                crate::collision::cylinder_point_check(loc, ra, ha, xl, [r, r, h]).is_some()
            };
            if overlap {
                out.push(x);
            }
        }
        out
    }

    /// `ULevel::CheckEncroachment` (Engine.dll 1039a350): `a` is about to move to `loc`/`rot`. Every actor it would
    /// overlap that it blocks (and that blocks it) is either pushed out of the way, when `a` is a non-static brush (a
    /// mover) and the other is not, or reported to `a` through `EncroachingOn`; a true answer stops the move (returns
    /// true). Afterwards each blocked actor gets `EncroachedBy(a)`, and unblocked ones a `Touch` when `touch`.
    pub fn check_encroachment(&mut self, a: u32, loc: Vec3, rot: [i32; 3], touch: bool) -> Res<bool> {
        let o = self.offsets();
        let is_brush = self.is_brush(a);
        let is_static = self.flag(a, o.b_static.0, o.b_static.1);
        let collides = self.flag(a, o.flags, F_COLLIDE_ACTORS) || self.flag(a, o.flags, F_BLOCK_ACTORS) || self.flag(a, o.flags, F_BLOCK_PLAYERS);
        if !collides && (!is_brush || is_static) {
            return Ok(false);
        }
        let list = self.encroachment_list(a, loc, rot);
        for &x in &list {
            if !self.is_blocked_by(a, x) {
                continue;
            }
            let x_moving_brush = self.is_brush(x) && !self.flag(x, o.b_static.0, o.b_static.1);
            if !is_brush || is_static || x_moving_brush {
                if self.encroaching_on(a, x)? {
                    return Ok(true);
                }
            } else {
                // a mover shoves the other actor along with its own move, then checks that it got clear
                let here = self.vec_at(a, o.location);
                let delta = sub(loc, here);
                self.move_smooth(x, delta)?;
                if self.encroachment_list(a, loc, rot).contains(&x) {
                    self.set_vec_at(a, o.location, loc);
                    let back = self.move_smooth(x, [-delta[0], -delta[1], -delta[2]]);
                    self.set_vec_at(a, o.location, here);
                    back?;
                    if self.encroaching_on(a, x)? {
                        return Ok(true);
                    }
                }
            }
        }
        if touch {
            for i in 0..4 {
                let t = self.touching(a, i);
                if t != 0 && !self.is_overlapping(a, t) {
                    self.end_touch(a, t, false)?;
                }
            }
        }
        for &x in &list {
            if x == a || x == self.level {
                continue;
            }
            if self.is_blocked_by(a, x) {
                self.send_event(x, "EncroachedBy", vec![Value::Obj(a)])?;
            } else if touch {
                self.begin_touch(a, x)?;
            }
        }
        Ok(false)
    }

    fn encroaching_on(&mut self, a: u32, x: u32) -> Res<bool> {
        Ok(matches!(self.send_event(a, "EncroachingOn", vec![Value::Obj(x)])?, Some(v) if v.as_bool().unwrap_or(false)))
    }

    // ---- movement ------------------------------------------------------------------------------------------

    /// `Rotation` of an actor (pitch, yaw, roll).
    pub fn rotation(&mut self, a: u32) -> [i32; 3] {
        let o = self.offsets();
        [self.u32_at(a, o.rotation) as i32, self.u32_at(a, o.rotation + 4) as i32, self.u32_at(a, o.rotation + 8) as i32]
    }

    fn set_rotation_raw(&mut self, a: u32, r: [i32; 3]) {
        let o = self.offsets();
        for k in 0..3 {
            self.set_u32_at(a, o.rotation + k * 4, r[k] as u32);
        }
    }

    /// `ULevel::FarMoveActor`: teleports an actor, finding a free spot unless `no_check`.
    /// The live actors whose `Base` is `a`, in actor-list order (the original's loop over `Actors`).
    fn actors_based_on(&self, a: u32, base_off: usize) -> Vec<u32> {
        // read straight from each live actor's memory (this runs for every move)
        let mut out = Vec::new();
        for &i in &self.actor_slots {
            let ob = &self.objects[i];
            if ob.alive && ob.mem.data.get(base_off..base_off + 4).is_some_and(|b| u32::from_le_bytes(b.try_into().unwrap()) == a) {
                out.push(crate::inst_ref(i));
            }
        }
        out
    }

    pub fn far_move_actor(&mut self, a: u32, dest: Vec3, test: bool, no_check: bool) -> Res<bool> {
        let o = self.offsets();
        let ro = self.region_offsets();
        if self.flag(a, o.b_static.0, o.b_static.1) || !self.flag(a, o.b_movable.0, o.b_movable.1) {
            return Ok(false);
        }
        // out of the collision hash for the spot search and the encroachment test, back in afterwards whatever
        // the outcome (10398ba0)
        self.hash_remove_colliding(a);
        let mut dest = dest;
        let mut result = true;
        if !no_check {
            let collide_world = self.flag(a, o.flags, F_COLLIDE_WORLD);
            let placing = self.flag(a, ro.collide_when_placing.0, ro.collide_when_placing.1);
            if collide_world || placing {
                let (r, h) = self.cylinder(a);
                result = self.find_spot([r, r, h], &mut dest, false, false);
            }
        }
        if !test && !no_check && result {
            let rot = self.rotation(a);
            result = !self.check_encroachment(a, dest, rot, true)?;
        }
        if result {
            if !test {
                if self.u8_at(a, o.standing_count) != 0 {
                    let based = self.actors_based_on(a, o.base);
                    for b in based {
                        self.set_base(b, 0, true)?;
                    }
                }
                if let Some((off, mask)) = ro.just_teleported {
                    let w = self.u32_at(a, off) | mask;
                    self.set_u32_at(a, off, w);
                }
            }
            self.set_vec_at(a, o.location, dest);
            if let Some(off) = ro.old_location {
                self.set_vec_at(a, off, dest);
            }
        }
        self.hash_add_colliding(a);
        if result {
            self.set_actor_zone(a, test, false)?;
        }
        Ok(result)
    }

    /// `ULevel::MoveActor`. `hit` receives the blocking hit (time 1 when the whole move succeeded). Returns true if the
    /// actor moved at all.
    #[allow(clippy::too_many_arguments)]
    pub fn move_actor(&mut self, a: u32, delta: Vec3, new_rot: [i32; 3], hit: &mut CheckResult, test: bool, ignore_pawns: bool, ignore_bases: bool, no_fail: bool) -> Res<bool> {
        let o = self.offsets();
        if self.flag(a, o.b_static.0, o.b_static.1) || !self.flag(a, o.b_movable.0, o.b_movable.1) {
            return Ok(false);
        }
        let static_ = self.flag(a, o.b_static.0, o.b_static.1);
        let brush = self.is_brush(a);
        let rot = self.rotation(a);
        if nearly_zero(delta) {
            if new_rot == rot {
                return Ok(true);
            }
            if self.u8_at(a, o.standing_count) == 0 && (!brush || static_) {
                self.set_rotation_raw(a, new_rot);
                return Ok(true);
            }
        }
        *hit = CheckResult::none();

        // direction and the trace, which reaches two units beyond the move
        let dist = dot(delta, delta).sqrt();
        let dir = if delta.iter().any(|c| c.abs() >= 0.0001) { scale(delta, 1.0 / dist) } else { delta };
        let dist = if delta.iter().any(|c| c.abs() >= 0.0001) { dist } else { 0.0 };
        let trace_delta = add(delta, scale(dir, 2.0));
        let loc = self.vec_at(a, o.location);
        let mut first_hits: Vec<CheckResult> = Vec::new();
        // the engine's `iStack_4c`: some candidate hit was found (blocking or not)
        let mut any_hit = false;
        let collide_actors = self.flag(a, o.flags, F_COLLIDE_ACTORS);
        let collide_world = self.flag(a, o.flags, F_COLLIDE_WORLD);
        if (collide_actors || collide_world) && (!brush || static_) && delta != [0.0; 3] {
            let (r, h) = self.cylinder(a);
            let level = collide_world && !(brush && !static_);
            let actors = collide_actors && !(brush && !static_);
            first_hits = self.multi_line_check(loc, add(loc, trace_delta), [r, r, h], level, actors, 0);
            let blocks_something = collide_world || self.flag(a, o.flags, F_BLOCK_ACTORS) || self.flag(a, o.flags, F_BLOCK_PLAYERS);
            if blocks_something {
                for t in first_hits.clone() {
                    if t.actor == a {
                        continue;
                    }
                    if ignore_pawns && !self.flag(t.actor, o.b_static.0, o.b_static.1) && (self.is_instance_of(t.actor, o.class_pawn) || self.is_instance_of(t.actor, o.class_decoration)) {
                        continue;
                    }
                    if (ignore_bases && self.is_based_on(a, t.actor)) || self.is_based_on(t.actor, a) {
                        continue;
                    }
                    any_hit = true;
                    if self.is_blocked_by(a, t.actor) {
                        *hit = t;
                        break;
                    }
                }
            }
        }

        // shorten the move to stop two units short of the obstacle
        let mut applied = delta;
        if hit.time < 1.0 && !no_fail {
            let reach = (dist + 2.0) * hit.time;
            if reach > 2.0 {
                let to_hit = scale(trace_delta, hit.time);
                applied = sub(to_hit, scale(dir, 2.0));
                hit.time = (reach - 2.0) / dist;
            } else {
                applied = [0.0; 3];
                hit.time = 0.0;
            }
        }

        let standing = self.u8_at(a, o.standing_count) != 0;
        if standing && !test {
            // actors standing on this one are carried along (and turned with it)
            let based = self.actors_based_on(a, o.base);
            for b in based {
                let dyaw = new_rot[1] - rot[1];
                let mut extra = [0.0f32; 3];
                if new_rot != rot {
                    let ang = (((new_rot[1] & 0xFFFC) - (rot[1] & 0xFFFC)) & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
                    let (s, c) = ang.sin_cos();
                    let rel = sub(self.vec_at(b, o.location), loc);
                    let rotated = [rel[0] * c - rel[1] * s, rel[0] * s + rel[1] * c, rel[2]];
                    extra = sub(rotated, rel);
                }
                let brot = self.rotation(b);
                let b_new = [brot[0], brot[1] + dyaw, brot[2]];
                let mut bh = CheckResult::none();
                self.move_actor(b, add(applied, extra), b_new, &mut bh, false, false, true, false)?; // riders move with bIgnoreBases set (Engine.dll 10399afd)
                if self.is_instance_of(b, o.class_pawn) {
                    let ro = self.region_offsets();
                    let v = self.u32_at(b, ro.view_rotation + 4) as i32 + dyaw;
                    self.set_u32_at(b, ro.view_rotation + 4, v as u32);
                }
            }
        }
        if !test && !no_fail && !self.is_instance_of(a, o.class_pawn) {
            let target = add(loc, applied);
            // `CheckEncroachment(Actor, Location + Delta, NewRotation, bTouchNotify = 0)` (Engine.dll 10399b9c: `push 0`)
            if self.check_encroachment(a, target, new_rot, false)? {
                return Ok(false);
            }
        }

        // apply
        // out of the collision hash, the new location and rotation, back in (10399c9a)
        let moved_to = add(self.vec_at(a, o.location), applied);
        self.hash_remove_colliding(a);
        self.set_vec_at(a, o.location, moved_to);
        self.set_rotation_raw(a, new_rot);
        self.hash_add_colliding(a);
        if test {
            self.set_actor_zone(a, test, false)?;
            return Ok(hit.time > 0.0);
        }

        // notifications
        if hit.actor != 0 {
            if hit.actor == self.level {
                self.send_event(a, "BumpWall", vec![Value::Vector(hit.location), Value::Vector(hit.normal)])?;
            } else if !self.is_based_on(a, hit.actor) {
                self.send_event(hit.actor, "Bump", vec![Value::Obj(a)])?;
                self.send_event(a, "Bump", vec![Value::Obj(hit.actor)])?;
            }
        }
        let blocks_all = self.flag(a, o.flags, F_BLOCK_ACTORS) && self.flag(a, o.flags, F_BLOCK_PLAYERS);
        if any_hit || !blocks_all {
            for t in &first_hits {
                if t.time >= hit.time {
                    break;
                }
                if t.actor != a
                    && t.actor != self.level
                    && !self.is_based_on(t.actor, a)
                    && !(ignore_bases && self.is_based_on(a, t.actor))
                    && !self.is_blocked_by(a, t.actor)
                {
                    self.begin_touch(a, t.actor)?;
                }
            }
        }
        for i in 0..4 {
            let t = self.touching(a, i);
            if t != 0 && !self.is_overlapping(a, t) {
                self.end_touch(a, t, false)?;
            }
        }
        self.set_actor_zone(a, test, false)?;
        Ok(hit.time > 0.0)
    }

    /// `AActor::TwoWallAdjust`: slides `delta` along two obstacles.
    fn two_wall_adjust(&self, dir: Vec3, delta: &mut Vec3, hit_normal: Vec3, old_hit_normal: Vec3, hit_time: f32) {
        if dot(hit_normal, old_hit_normal) > 0.0 {
            let d = dot(*delta, hit_normal);
            *delta = scale(sub(*delta, scale(hit_normal, d)), 1.0 - hit_time);
            if dot(dir, *delta) <= 0.0 {
                *delta = [0.0; 3];
            }
        } else {
            let along = safe_normal(cross(hit_normal, old_hit_normal));
            let s = (1.0 - hit_time) * dot(along, *delta);
            *delta = scale(along, s);
            if dot(dir, *delta) < 0.0 {
                *delta = scale(*delta, -1.0);
            }
        }
    }

    /// `AActor::moveSmooth`: moves, sliding along walls.
    pub fn move_smooth(&mut self, a: u32, delta: Vec3) -> Res<bool> {
        let o = self.offsets();
        let rot = self.rotation(a);
        let mut hit = CheckResult::none();
        let result = self.move_actor(a, delta, rot, &mut hit, false, false, false, false)?;
        if hit.time < 1.0 {
            let d = dot(delta, hit.normal);
            let mut slide = scale(sub(delta, scale(hit.normal, d)), 1.0 - hit.time);
            if dot(slide, delta) >= 0.0 {
                let old_normal = hit.normal;
                let dir = safe_normal(delta);
                let rot = self.rotation(a);
                self.move_actor(a, slide, rot, &mut hit, false, false, false, false)?;
                if hit.time < 1.0 {
                    self.send_event(a, "HitWall", vec![Value::Vector(hit.normal), Value::Obj(hit.actor)])?;
                    self.two_wall_adjust(dir, &mut slide, hit.normal, old_normal, hit.time);
                    let rot = self.rotation(a);
                    self.move_actor(a, slide, rot, &mut hit, false, false, false, false)?;
                }
            }
        }
        let _ = o;
        Ok(result)
    }
}

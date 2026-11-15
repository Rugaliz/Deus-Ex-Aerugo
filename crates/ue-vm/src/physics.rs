//! Per-actor physics: `performPhysics` and the movement modes (falling, walking, flying, swimming, projectile,
//! rotation). Ports of `AActor::phys*`, `APawn::phys*`, `APawn::calcVelocity`, `APawn::stepUp` and friends
//! (spec: `docs/collision.md`). DX-specific player features (ladders, parachute) are not implemented.

use crate::collision::*;
use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

pub const PHYS_NONE: u8 = 0;
pub const PHYS_WALKING: u8 = 1;
pub const PHYS_FALLING: u8 = 2;
pub const PHYS_SWIMMING: u8 = 3;
pub const PHYS_FLYING: u8 = 4;
pub const PHYS_ROTATING: u8 = 5;
pub const PHYS_PROJECTILE: u8 = 6;
pub const PHYS_ROLLING: u8 = 7;
pub const PHYS_INTERPOLATING: u8 = 8;
pub const PHYS_MOVING_BRUSH: u8 = 9;
pub const PHYS_SPIDER: u8 = 10;
pub const PHYS_TRAILER: u8 = 11;

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn size_sq(a: Vec3) -> f32 {
    dot(a, a)
}
fn is_zero(a: Vec3) -> bool {
    a == [0.0; 3]
}

/// `TwoWallAdjust` (see `movement.rs`), shared here.
pub(crate) fn two_wall_adjust(dir: Vec3, delta: &mut Vec3, hit_normal: Vec3, old_hit_normal: Vec3, hit_time: f32) {
    if dot(hit_normal, old_hit_normal) > 0.0 {
        let d = dot(*delta, hit_normal);
        *delta = mul(sub(*delta, mul(hit_normal, d)), 1.0 - hit_time);
        if dot(dir, *delta) <= 0.0 {
            *delta = [0.0; 3];
        }
    } else {
        let c = [
            hit_normal[1] * old_hit_normal[2] - hit_normal[2] * old_hit_normal[1],
            hit_normal[2] * old_hit_normal[0] - hit_normal[0] * old_hit_normal[2],
            hit_normal[0] * old_hit_normal[1] - hit_normal[1] * old_hit_normal[0],
        ];
        let along = safe_normal(c);
        let s = (1.0 - hit_time) * dot(along, *delta);
        *delta = mul(along, s);
        if dot(dir, *delta) < 0.0 {
            *delta = mul(*delta, -1.0);
        }
    }
}

/// `Vm::prop_fast`: 2^12 slots.
pub(crate) const PROP_FAST_BITS: u32 = 12;
pub(crate) const PROP_FAST_SLOTS: usize = 1 << PROP_FAST_BITS;

impl Vm {
    // ---- property access by name ---------------------------------------------------------------------------

    pub(crate) fn prop_at(&mut self, a: u32, name: &'static str) -> Option<(usize, u32)> {
        let class = self.class_of(a)?;
        let (ptr, len) = (name.as_ptr() as usize, name.len());
        let slot = ((ptr as u64 ^ (class.0 as u64).wrapping_mul(0x9E37_79B9)).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - PROP_FAST_BITS)) as usize;
        let e = self.prop_fast[slot];
        if e.0 == class.0 && e.1 == ptr && e.2 == len {
            return e.3;
        }
        let key = (class, ptr, len);
        let v = match self.prop_cache.get(&key) {
            Some(v) => *v,
            None => {
                let v = self.reg.find_prop(class, name).map(|p| (p.offset as usize, p.bit_mask));
                self.prop_cache.insert(key, v);
                v
            }
        };
        self.prop_fast[slot] = (class.0, ptr, len, v);
        v
    }

    /// Offset and bit mask of a property declared by `Actor` itself (the same in every actor class), cached.
    pub fn actor_prop_offset(&mut self, name: &'static str) -> Option<(usize, u32)> {
        // cached under a class id no class has
        let key = (ue_registry::ClassId(u32::MAX), name.as_ptr() as usize, name.len());
        if let Some(v) = self.prop_cache.get(&key) {
            return *v;
        }
        let v = self.reg.class_by_name("Actor").and_then(|class| self.reg.find_prop(class, name)).map(|p| (p.offset as usize, p.bit_mask));
        self.prop_cache.insert(key, v);
        v
    }

    pub fn fp(&mut self, a: u32, name: &'static str) -> f32 {
        self.prop_at(a, name).map(|(o, _)| self.f32_at(a, o)).unwrap_or(0.0)
    }
    pub fn set_fp(&mut self, a: u32, name: &'static str, v: f32) {
        if let Some((o, _)) = self.prop_at(a, name) {
            self.set_u32_at(a, o, v.to_bits());
        }
    }
    pub fn vp(&mut self, a: u32, name: &'static str) -> Vec3 {
        self.prop_at(a, name).map(|(o, _)| self.vec_at(a, o)).unwrap_or([0.0; 3])
    }
    pub fn set_vp(&mut self, a: u32, name: &'static str, v: Vec3) {
        if let Some((o, _)) = self.prop_at(a, name) {
            self.set_vec_at(a, o, v);
        }
    }
    pub fn bp(&mut self, a: u32, name: &'static str) -> bool {
        self.prop_at(a, name).map(|(o, m)| self.u32_at(a, o) & m != 0).unwrap_or(false)
    }
    pub fn set_bp(&mut self, a: u32, name: &'static str, v: bool) {
        if let Some((o, m)) = self.prop_at(a, name) {
            let w = self.u32_at(a, o);
            self.set_u32_at(a, o, if v { w | m } else { w & !m });
        }
    }
    pub fn u8p(&mut self, a: u32, name: &'static str) -> u8 {
        self.prop_at(a, name).map(|(o, _)| self.u8_at(a, o)).unwrap_or(0)
    }
    pub fn set_u8p(&mut self, a: u32, name: &'static str, v: u8) {
        if let Some((o, _)) = self.prop_at(a, name) {
            self.set_u8_at(a, o, v);
        }
    }
    pub fn ip(&mut self, a: u32, name: &'static str) -> i32 {
        self.prop_at(a, name).map(|(o, _)| self.u32_at(a, o) as i32).unwrap_or(0)
    }
    pub fn op(&mut self, a: u32, name: &'static str) -> u32 {
        self.prop_at(a, name).map(|(o, _)| self.u32_at(a, o)).unwrap_or(0)
    }

    /// The zone actor an actor is in (`Region.Zone`).
    pub fn name_of(&mut self, a: u32) -> String {
        match self.class_of(a) {
            Some(c) => format!("{}#{}", self.reg.class(c).path, a & 0x7FFF_FFFF),
            None => format!("#{a}"),
        }
    }

    pub fn set_ip(&mut self, a: u32, name: &'static str, v: i32) {
        if let Some((o, _)) = self.prop_at(a, name) {
            self.set_u32_at(a, o, v as u32);
        }
    }

    fn next_rand_f32(&mut self) -> f32 {
        self.frand()
    }

    /// The zone actor of an actor (`Region.Zone`).
    pub fn zone_of_actor(&mut self, a: u32) -> u32 {
        self.zone_of(a)
    }

    fn zone_of(&mut self, a: u32) -> u32 {
        let ro = self.region_offsets();
        self.u32_at(a, ro.region)
    }
    pub fn zone_number(&mut self, a: u32) -> u8 {
        let ro = self.region_offsets();
        self.u8_at(a, ro.region + ro.zone_number)
    }
    pub fn zone_vec(&mut self, a: u32, name: &'static str) -> Vec3 {
        let z = self.zone_of(a);
        self.vp(z, name)
    }
    fn zone_f(&mut self, a: u32, name: &'static str) -> f32 {
        let z = self.zone_of(a);
        self.fp(z, name)
    }
    pub(crate) fn zone_b(&mut self, a: u32, name: &'static str) -> bool {
        let z = self.zone_of(a);
        self.bp(z, name)
    }

    pub(crate) fn rot_of(&mut self, a: u32) -> [i32; 3] {
        let o = self.offsets();
        [self.u32_at(a, o.rotation) as i32, self.u32_at(a, o.rotation + 4) as i32, self.u32_at(a, o.rotation + 8) as i32]
    }

    /// `MoveActor` with default flags; returns the hit.
    fn do_move(&mut self, a: u32, delta: Vec3) -> Res<CheckResult> {
        let rot = self.rot_of(a);
        let mut hit = CheckResult::none();
        self.move_actor(a, delta, rot, &mut hit, false, false, false, false)?;
        Ok(hit)
    }

    fn is_deleted(&mut self, a: u32) -> bool {
        let o = self.offsets();
        !self.is_live(a) || self.flag(a, o.b_delete_me.0, o.b_delete_me.1)
    }

    // ---- dispatch ------------------------------------------------------------------------------------------

    /// `AActor::setPhysics`: changes the physics mode, fixing up the base and zeroing motion where needed.
    pub fn set_physics(&mut self, a: u32, mode: u8, floor: u32) -> Res<()> {
        let o = self.offsets();
        if self.u8_at(a, o.physics) == mode {
            return Ok(());
        }
        self.set_u8_at(a, o.physics, mode);
        if matches!(mode, PHYS_WALKING | PHYS_NONE | PHYS_ROLLING | PHYS_ROTATING | PHYS_SPIDER) {
            if floor == 0 {
                self.find_base(a)?;
            } else if self.u32_at(a, o.base) != floor {
                self.send_event(floor, "SupportActor", vec![Value::Obj(a)])?;
            }
        } else if self.u32_at(a, o.base) != 0 {
            self.set_base(a, 0, true)?;
        }
        let now = self.u8_at(a, o.physics);
        if now == PHYS_NONE || now == PHYS_ROTATING {
            self.set_vec_at(a, o.velocity, [0.0; 3]);
            self.set_vec_at(a, o.acceleration, [0.0; 3]);
        }
        Ok(())
    }

    /// `AActor::performPhysics` / `APawn::performPhysics`.
    pub fn perform_physics(&mut self, a: u32, dt: f32) -> Res<()> {
        let o = self.offsets();
        let is_pawn = self.is_instance_of(a, o.class_pawn);
        let phys = self.u8_at(a, o.physics);
        let old_velocity = self.vec_at(a, o.velocity);
        let ro = self.region_offsets();
        let track_velocity = |vm: &mut Vm, f: &dyn Fn(&mut Vm) -> Res<()>| -> Res<()> {
            if let Some(off) = ro.old_location {
                let l = vm.vec_at(a, o.location);
                vm.set_vec_at(a, off, l);
            }
            f(vm)?;
            if let Some(off) = ro.old_location {
                let (l, old) = (vm.vec_at(a, o.location), vm.vec_at(a, off));
                vm.set_vec_at(a, o.velocity, mul(sub(l, old), 1.0 / dt));
            }
            Ok(())
        };
        match (is_pawn, phys) {
            (true, PHYS_WALKING) => self.phys_walking(a, dt, 0)?,
            (_, PHYS_FALLING) => self.phys_falling(a, dt, 0)?,
            (true, PHYS_SWIMMING) => self.phys_swimming(a, dt, 0)?,
            (true, PHYS_FLYING) => self.phys_flying(a, dt, 0)?,
            (false, PHYS_PROJECTILE) => self.phys_projectile(a, dt, 0)?,
            (false, PHYS_ROLLING) => self.phys_rolling(a, dt, 0)?,
            // the path and mover modes leave `Velocity` as the distance covered over the step
            (_, PHYS_INTERPOLATING) => track_velocity(self, &|vm| vm.phys_interpolating(a, dt))?,
            (false, PHYS_MOVING_BRUSH) => track_velocity(self, &|vm| vm.phys_moving_brush(a, dt))?,
            (false, PHYS_TRAILER) => self.physics_unimplemented("Trailer"),
            (true, PHYS_SPIDER) => self.physics_unimplemented("Spider"),
            _ => {}
        }
        if self.is_deleted(a) {
            return Ok(());
        }
        // rotation
        if is_pawn {
            if phys != PHYS_SPIDER {
                let is_player = self.is_instance_of(a, o.class_player_pawn);
                let rot = self.rot_of(a);
                let desired = self.rot_prop(a, "DesiredRotation");
                let rate = self.rot_prop(a, "RotationRate");
                if is_player || rot != desired || rate[2] > 0 {
                    self.pawn_physics_rotation(a, dt, old_velocity)?;
                }
            }
            let t = self.fp(a, "MoveTimer");
            self.set_fp(a, "MoveTimer", t - dt);
            let avg = self.fp(a, "AvgPhysicsTime");
            self.set_fp(a, "AvgPhysicsTime", dt * 0.2 + avg * 0.8);
        } else {
            let rate = self.rot_prop(a, "RotationRate");
            if rate.iter().any(|r| r & 0xFFFF != 0) {
                self.physics_rotation(a, dt)?;
            }
        }
        // actors touched during the move that want to react afterwards
        let pending = self.op(a, "PendingTouch");
        if pending != 0 && self.is_live(pending) {
            self.send_event(pending, "PostTouch", vec![Value::Obj(a)])?;
            let next = self.op(pending, "PendingTouch");
            if let Some((off, _)) = self.prop_at(a, "PendingTouch") {
                self.set_u32_at(a, off, next);
            }
            if let Some((off, _)) = self.prop_at(pending, "PendingTouch") {
                self.set_u32_at(pending, off, 0);
            }
        }
        Ok(())
    }

    /// `AActor::physMovingBrush`: a mover interpolating between key frames. `PhysAlpha` runs 0..1 at `PhysRate` per second; with
    /// `MoverGlideType` = MV_GlideByTime (1) the position follows `3a^2 - 2a^3`. The mover is moved with the usual `MoveActor` (so
    /// actors standing on it are carried, and it can be blocked); when alpha reaches 1 the script's `InterpolateEnd` event runs.
    fn phys_moving_brush(&mut self, a: u32, mut dt: f32) -> Res<()> {
        // only movers move this way (the engine checks the class first)
        if !self.reg.class_by_name("Mover").map(|m| self.class_of(a).map(|c| self.is_a(c, m)).unwrap_or(false)).unwrap_or(false) {
            return Ok(());
        }
        let key = (self.u8p(a, "KeyNum") as usize).min(7) as u32;
        let vec = |vm: &Vm, name: &str, i: u32| match vm.get_prop(a, name, i) {
            Ok(Value::Vector(v)) => v,
            _ => [0.0; 3],
        };
        let rotv = |vm: &Vm, name: &str, i: u32| match vm.get_prop(a, name, i) {
            Ok(Value::Rotator(r)) => r,
            _ => [0; 3],
        };
        while self.bp(a, "bInterpolating") && dt > 0.0 {
            let before = self.fp(a, "PhysAlpha");
            let mut alpha = dt * self.fp(a, "PhysRate") + before;
            if alpha <= 1.0 {
                dt = 0.0;
            } else {
                dt = (alpha - 1.0) / (alpha - before) * dt;
                alpha = 1.0;
            }
            let mut shaped = alpha;
            if self.u8p(a, "MoverGlideType") == 1 {
                shaped = alpha * alpha * 3.0 - 2.0 * alpha * alpha * alpha;
            }
            let (old_pos, base_pos, key_pos) = (vec(self, "OldPos", 0), vec(self, "BasePos", 0), vec(self, "KeyPos", key));
            let (old_rot, base_rot, key_rot) = (rotv(self, "OldRot", 0), rotv(self, "BaseRot", 0), rotv(self, "KeyRot", key));
            let target_pos = add(base_pos, key_pos);
            let new_pos = add(old_pos, mul(sub(target_pos, old_pos), shaped));
            let new_rot: [i32; 3] = std::array::from_fn(|k| {
                let target = base_rot[k].wrapping_add(key_rot[k]);
                old_rot[k] + ((target.wrapping_sub(old_rot[k])) as f32 * shaped) as i32
            });
            let delta = sub(new_pos, self.location(a));
            let mut hit = CheckResult::none();
            let moved = self.move_actor(a, delta, new_rot, &mut hit, false, false, false, false)?;
            if moved {
                self.set_fp(a, "PhysAlpha", alpha);
                if alpha == 1.0 {
                    self.set_bp(a, "bInterpolating", false);
                    self.send_event(a, "InterpolateEnd", vec![Value::Obj(0)])?;
                }
            }
        }
        Ok(())
    }

    /// `AActor::physInterpolating` (named `physPathing` in the decompile): the actor rides a path of `InterpolationPoint`s
    /// (camera paths of cutscenes; the player too). `Target` is the point the current segment starts at, `PhysAlpha` runs
    /// 0..1 along the segment to `Target.Next` at `PhysRate` times the points' rate modifiers per second. With a point before
    /// and two after the segment the position follows a smooth curve (weights `(1 - x^2/4)^2` of the four points at
    /// `alpha + 1, alpha, alpha - 1, alpha - 2`), else a straight line. At the end of a segment both ends hear
    /// `InterpolateEnd` and the next segment starts (the rest of the frame's time carries over); points flagged
    /// `bSkipNextPath` are passed over. Also drives the game speed, the field of view and the screen flash of a player.
    fn phys_interpolating(&mut self, a: u32, mut dt: f32) -> Res<()> {
        let Some(ip) = self.reg.class_by_name("InterpolationPoint") else { return Ok(()) };
        let o = self.offsets();
        let is_player = self.is_instance_of(a, o.class_player_pawn);
        let is_pawn = self.is_instance_of(a, o.class_pawn);
        let point = |vm: &mut Vm, p: u32| p != 0 && vm.is_live(p) && vm.is_instance_of(p, ip);
        let rot_f = |r: [i32; 3], w: f32| -> [i32; 3] { std::array::from_fn(|k| (r[k] as f32 * w) as i32) };
        let add_r = |x: [i32; 3], y: [i32; 3]| -> [i32; 3] { std::array::from_fn(|k| x[k].wrapping_add(y[k])) };
        loop {
            let rate = self.fp(a, "PhysRate");
            if rate == 0.0 || !self.bp(a, "bInterpolating") || dt <= 0.0 {
                return Ok(());
            }
            let t = self.op(a, "Target");
            let dest = if point(self, t) { t } else { 0 };
            let alpha0 = self.fp(a, "PhysAlpha");
            // with a next point the rate modifier, the game speed and (players) flash and fov blend between the two
            // points; without one the rate is 1 and nothing moves
            let mut mult = 1.0f32;
            let next = if dest != 0 { self.op(dest, "Next") } else { 0 };
            let next = if next != 0 && self.is_live(next) { next } else { 0 };
            if dest != 0 && next != 0 {
                let (nr, dr) = (self.fp(next, "RateModifier"), self.fp(dest, "RateModifier"));
                mult = nr * alpha0 + (1.0 - alpha0) * dr;
                let dilation = self.fp(next, "GameSpeedModifier") * alpha0 + (1.0 - alpha0) * self.fp(dest, "GameSpeedModifier");
                if self.level != 0 {
                    let _ = self.set_prop(self.level, "TimeDilation", 0, Value::Float(dilation));
                }
                if is_player {
                    // 103dd9b8..103ddaf9: DesiredFlashScale and FlashScale (all three), DesiredFlashFog and FlashFog,
                    // FovAngle = blended FovModifier times the class default FovAngle
                    let scale = self.fp(dest, "ScreenFlashScale") * (1.0 - alpha0) + self.fp(next, "ScreenFlashScale") * alpha0;
                    let fog = {
                        let (d, n) = match (self.get_prop(dest, "ScreenFlashFog", 0), self.get_prop(next, "ScreenFlashFog", 0)) {
                            (Ok(Value::Vector(d)), Ok(Value::Vector(n))) => (d, n),
                            _ => ([0.0; 3], [0.0; 3]),
                        };
                        std::array::from_fn::<f32, 3, _>(|k| d[k] * (1.0 - alpha0) + n[k] * alpha0)
                    };
                    self.set_fp(a, "DesiredFlashScale", scale);
                    for n in ["FlashScale", "DesiredFlashFog", "FlashFog"] {
                        if let Some((off, _)) = self.prop_at(a, n) {
                            self.set_vec_at(a, off, if n == "FlashScale" { [scale; 3] } else { fog });
                        }
                    }
                    let base = self.class_default_f32(a, "FovAngle").unwrap_or(90.0);
                    let fov = (self.fp(next, "FovModifier") * alpha0 + (1.0 - alpha0) * self.fp(dest, "FovModifier")) * base;
                    self.set_fp(a, "FovAngle", fov);
                }
            }
            let new_alpha = mult * rate * dt + alpha0;
            let alpha = new_alpha.clamp(0.0, 1.0);
            self.set_fp(a, "PhysAlpha", alpha);
            if dest != 0 && next != 0 {
                let prev = self.op(dest, "Prev");
                let after = self.op(next, "Next");
                let (new_pos, new_rot);
                if !point(self, prev) || !point(self, after) {
                    let (lp, ln) = (self.location(dest), self.location(next));
                    new_pos = std::array::from_fn(|k| ln[k] * alpha + lp[k] * (1.0 - alpha));
                    let (rp, rn) = (self.rot_of(dest), self.rot_of(next));
                    new_rot = add_r(rot_f(rn, alpha), rot_f(rp, 1.0 - alpha));
                } else {
                    // (x^2)^2 / 16 - x^2 / 2 + 1 = (1 - x^2/4)^2, in the engine's order
                    let w = |x: f32| {
                        let q = x * x;
                        (q * q * 0.0625 - q * 0.5) + 1.0
                    };
                    let ws = [w(alpha + 1.0), w(alpha), w(alpha - 1.0), w(alpha - 2.0)];
                    let inv = 1.0 / (ws[0] + ws[1] + ws[2] + ws[3]);
                    let pts = [prev, dest, next, after];
                    let locs: Vec<[f32; 3]> = pts.iter().map(|&p| self.location(p)).collect();
                    new_pos = std::array::from_fn(|k| (0..4).map(|i| ws[i] * locs[i][k]).sum::<f32>() * inv);
                    let mut sum = [0i32; 3];
                    for (i, &p) in pts.iter().enumerate() {
                        let r = self.rot_of(p);
                        sum = add_r(sum, rot_f(r, ws[i]));
                    }
                    new_rot = rot_f(sum, inv);
                }
                let delta = sub(new_pos, self.location(a));
                let mut hit = CheckResult::none();
                self.move_actor(a, delta, new_rot, &mut hit, false, false, false, false)?;
                if is_pawn {
                    let r = self.rot_of(a);
                    let _ = self.set_prop(a, "ViewRotation", 0, Value::Rotator(r));
                }
            }
            if rate > 0.0 && new_alpha > 1.0 {
                // the segment is over: both ends are told, the next segment starts with the time that is left
                self.set_fp(a, "PhysAlpha", 0.0);
                dt *= (new_alpha - 1.0) / (new_alpha - alpha0);
                let t = self.op(a, "Target");
                if t != 0 {
                    self.send_event(t, "InterpolateEnd", vec![Value::Obj(a)])?;
                    let t2 = self.op(a, "Target");
                    self.send_event(a, "InterpolateEnd", vec![Value::Obj(t2)])?;
                    if dest != 0 {
                        let mut n = self.op(dest, "Next");
                        self.set_op(a, "Target", n);
                        while point(self, n) && self.bp(n, "bSkipNextPath") {
                            n = self.op(n, "Next");
                            self.set_op(a, "Target", n);
                        }
                    }
                }
            } else if rate < 0.0 && new_alpha < 0.0 {
                self.set_fp(a, "PhysAlpha", 1.0);
                dt *= -new_alpha / (alpha0 - new_alpha);
                let t = self.op(a, "Target");
                if t != 0 {
                    self.send_event(t, "InterpolateEnd", vec![Value::Obj(a)])?;
                    let t2 = self.op(a, "Target");
                    self.send_event(a, "InterpolateEnd", vec![Value::Obj(t2)])?;
                    if dest != 0 {
                        let mut n = self.op(dest, "Prev");
                        self.set_op(a, "Target", n);
                        while point(self, n) && self.bp(n, "bSkipNextPath") {
                            n = self.op(n, "Prev");
                            self.set_op(a, "Target", n);
                        }
                    }
                }
                self.send_event(a, "InterpolateEnd", vec![Value::Obj(0)])?;
            } else {
                return Ok(());
            }
        }
    }

    /// A float property of the actor's class default object.
    pub(crate) fn class_default_f32(&mut self, a: u32, name: &str) -> Option<f32> {
        let c = self.class_of(a)?;
        let off = self.reg.find_prop(c, name).map(|p| p.offset as usize)?;
        let m = self.class_default_mem(c).ok()?;
        Some(f32::from_le_bytes(m.data.get(off..off + 4)?.try_into().ok()?))
    }

    pub(crate) fn physics_unimplemented(&mut self, name: &str) {
        let msg = format!("physics mode {name} is not implemented");
        if !self.log.contains(&msg) {
            self.log.push(msg);
        }
    }

    pub(crate) fn rot_prop(&mut self, a: u32, name: &'static str) -> [i32; 3] {
        match self.prop_at(a, name) {
            Some((o, _)) => [self.u32_at(a, o) as i32, self.u32_at(a, o + 4) as i32, self.u32_at(a, o + 8) as i32],
            None => [0; 3],
        }
    }

    // ---- rotation ------------------------------------------------------------------------------------------

    /// `AActor::fixedTurn`: turns `current` toward `desired` by at most `delta_rate` (angles in 16-bit units).
    pub fn fixed_turn(&mut self, a: u32, current: i32, desired: i32, delta_rate: i32) -> i32 {
        if delta_rate == 0 {
            return current & 0xFFFF;
        }
        let cur = current & 0xFFFF;
        let des = desired & 0xFFFF;
        let fixed = self.bp(a, "bFixedRotationDir");
        let to_desired = self.bp(a, "bRotateToDesired");
        if fixed {
            if !to_desired {
                return (cur + delta_rate) & 0xFFFF;
            }
            if delta_rate < 1 {
                let base = if cur < des { cur + 0x10000 } else { cur };
                let d = (des - base).max(delta_rate);
                return (cur + d) & 0xFFFF;
            }
            let des2 = if des < cur { des + 0x10000 } else { des };
            let d = des2 - cur;
            return (cur + delta_rate.min(d)) & 0xFFFF;
        }
        if to_desired {
            let step = delta_rate.abs();
            if des < cur {
                let d = cur - des;
                if d > 0x7FFF {
                    let dist = des - cur + 0x10000;
                    return (cur + step.min(dist)) & 0xFFFF;
                }
                return (cur - step.min(d)) & 0xFFFF;
            }
            let d = des - cur;
            if d < 0x8000 {
                return (cur + step.min(d)) & 0xFFFF;
            }
            let dist = cur - des + 0x10000;
            return (cur - step.min(dist)) & 0xFFFF;
        }
        cur
    }

    /// `AActor::physicsRotation`: spins an actor by its `RotationRate`, or turns it to `DesiredRotation`.
    pub fn physics_rotation(&mut self, a: u32, dt: f32) -> Res<()> {
        let to_desired = self.bp(a, "bRotateToDesired");
        let fixed = self.bp(a, "bFixedRotationDir");
        let rot = self.rot_of(a);
        let desired = self.rot_prop(a, "DesiredRotation");
        if !((to_desired || fixed) && (!to_desired || rot != desired)) {
            return Ok(());
        }
        let rate = self.rot_prop(a, "RotationRate");
        let d = |x: i32| (x as f32 * dt) as i32; // truncating conversion, like the original `ftol`
        let (dp, dy, dr) = (d(rate[0]), d(rate[1]), d(rate[2]));
        let mut n = rot;
        if dy != 0 && (!to_desired || desired[1] != n[1]) {
            n[1] = self.fixed_turn(a, n[1], desired[1], dy);
        }
        if dp != 0 && (!to_desired || desired[0] != n[0]) {
            n[0] = self.fixed_turn(a, n[0], desired[0], dp);
        }
        if dr != 0 && (!to_desired || desired[2] != n[2]) {
            n[2] = self.fixed_turn(a, n[2], desired[2], dr);
        }
        if n != rot {
            let mut hit = CheckResult::none();
            self.move_actor(a, [0.0; 3], n, &mut hit, false, false, false, false)?;
        }
        if to_desired && self.rot_of(a) == desired {
            self.send_event(a, "EndedRotation", vec![])?;
        }
        Ok(())
    }

    /// `APawn::physicsRotation` (103c9f30): pawns other than players turn their yaw toward `DesiredRotation` at
    /// `RotationRate.Yaw` per second and their pitch at a fixed 32768 per second, toward 0 when walking or falling;
    /// `RotationRate.Pitch` only limits the pitch. Roll is always taken back to 0.
    fn pawn_physics_rotation(&mut self, a: u32, dt: f32, _old_velocity: Vec3) -> Res<()> {
        let o = self.offsets();
        let rot = self.rot_of(a);
        let desired = self.rot_prop(a, "DesiredRotation");
        let rate = self.rot_prop(a, "RotationRate");
        let (mut pitch, mut yaw) = (rot[0], rot[1]);
        if !self.is_instance_of(a, o.class_player_pawn) {
            // `fild` of the rate times the float frame time, truncated (`_ftol`)
            let dy = crate::route::ftol(rate[1] as f64 * dt as f64);
            self.set_bp(a, "bFixedRotationDir", false);
            self.set_bp(a, "bRotateToDesired", true);
            if desired[1] != yaw {
                yaw = self.fixed_turn(a, yaw, desired[1], dy);
            }
            if desired[0] != pitch {
                let dp = crate::route::ftol(dt as f64 * 32768.0);
                let phys = self.u8_at(a, o.physics);
                let target = if phys == PHYS_WALKING || phys == PHYS_FALLING { 0 } else { desired[0] };
                pitch = self.fixed_turn(a, pitch, target, dp);
                // limit the pitch to the rotation rate's pitch range
                if pitch < 0x8000 {
                    if pitch > rate[0] {
                        pitch = rate[0];
                    }
                } else if 0x10000 - rate[0] > pitch {
                    pitch = 0x10000 - rate[0];
                }
            }
        }
        if pitch != rot[0] || yaw != rot[1] || rot[2] != 0 {
            let mut hit = CheckResult::none();
            self.move_actor(a, [0.0; 3], [pitch, yaw, 0], &mut hit, false, false, false, false)?;
        }
        Ok(())
    }

    // ---- projectile and flying -----------------------------------------------------------------------------

    /// `AActor::physProjectile`: straight-line motion with zone drag; bounces when `bBounce`.
    fn phys_projectile(&mut self, a: u32, dt: f32, mut iterations: i32) -> Res<()> {
        let o = self.offsets();
        if self.zone_number(a) == 0 {
            self.destroy(a)?;
            return Ok(());
        }
        let ro = self.region_offsets();
        let loc = self.vec_at(a, o.location);
        if let Some(off) = ro.old_location {
            self.set_vec_at(a, off, loc);
        }
        self.clear_teleported(a);
        let mut remaining = dt;
        let mut bounces = 0;
        while remaining > 0.0 && iterations < 8 {
            iterations += 1;
            if self.zone_b(a, "bWaterZone") {
                let f = 1.0 - self.zone_f(a, "ZoneFluidFriction") * remaining * 0.2;
                let v = self.vec_at(a, o.velocity);
                self.set_vec_at(a, o.velocity, mul(v, f));
            }
            let acc = self.vec_at(a, o.acceleration);
            let v = add(self.vec_at(a, o.velocity), mul(acc, remaining));
            self.set_vec_at(a, o.velocity, v);
            let step = remaining;
            remaining = 0.0;
            if self.is_instance_of(a, o.class_projectile) {
                let max = self.fp(a, "MaxSpeed");
                let v = self.vec_at(a, o.velocity);
                if max * max < size_sq(v) {
                    self.set_vec_at(a, o.velocity, mul(safe_normal(v), max));
                }
            }
            let v = self.vec_at(a, o.velocity);
            let delta = mul(v, dt);
            let hit = self.do_move(a, delta)?;
            if hit.time < 1.0 && !self.is_deleted(a) && !self.just_teleported(a) {
                self.send_event(a, "HitWall", vec![Value::Vector(hit.normal), Value::Obj(hit.actor)])?;
                if self.bp(a, "bBounce") {
                    if bounces < 2 {
                        remaining = (1.0 - hit.time) * step;
                    }
                    bounces += 1;
                    if self.u8_at(a, o.physics) == PHYS_FALLING {
                        self.phys_falling(a, remaining, iterations)?;
                    }
                }
            }
        }
        if !self.bp(a, "bBounce") && !self.just_teleported(a) {
            let (l, old) = (self.vec_at(a, o.location), ro.old_location.map(|off| self.vec_at(a, off)).unwrap_or([0.0; 3]));
            self.set_vec_at(a, o.velocity, mul(sub(l, old), 1.0 / dt));
        }
        Ok(())
    }

    fn just_teleported(&self, a: u32) -> bool {
        let ro = self.region_offsets.expect("region offsets");
        ro.just_teleported.map(|(off, m)| self.u32_at(a, off) & m != 0).unwrap_or(false)
    }

    fn clear_teleported(&mut self, a: u32) {
        let ro = self.region_offsets();
        if let Some((off, m)) = ro.just_teleported {
            let w = self.u32_at(a, off) & !m;
            self.set_u32_at(a, off, w);
        }
    }

    fn store_old_location(&mut self, a: u32) {
        let o = self.offsets();
        let ro = self.region_offsets();
        if let Some(off) = ro.old_location {
            let l = self.vec_at(a, o.location);
            self.set_vec_at(a, off, l);
        }
    }

    fn old_location(&mut self, a: u32) -> Vec3 {
        let o = self.offsets();
        let ro = self.region_offsets();
        ro.old_location.map(|off| self.vec_at(a, off)).unwrap_or_else(|| self.vec_at(a, o.location))
    }

    /// `APawn::physFlying`.
    fn phys_flying(&mut self, a: u32, dt: f32, _iterations: i32) -> Res<()> {
        let o = self.offsets();
        if self.flag(a, o.flags, F_COLLIDE_WORLD) && self.zone_number(a) == 0 {
            // outside the world: pawns that are not players are removed
            if !self.bp(a, "bIsPlayer") {
                self.destroy(a)?;
            }
            return Ok(());
        }
        let acc = self.vec_at(a, o.acceleration);
        let dir = if is_zero(acc) { acc } else { safe_normal(acc) };
        let air_speed = self.fp(a, "AirSpeed");
        let friction = self.zone_f(a, "ZoneFluidFriction");
        self.calc_velocity(a, dir, dt, air_speed, friction, true, false, false);
        self.store_old_location(a);
        self.clear_teleported(a);
        let zone_vel = if self.is_instance_of(a, o.class_player_pawn) || size_sq(self.zone_vec(a, "ZoneVelocity")) > 90000.0 {
            self.zone_vec(a, "ZoneVelocity")
        } else {
            [0.0; 3]
        };
        let delta = mul(add(zone_vel, self.vec_at(a, o.velocity)), dt);
        let hit = self.do_move(a, delta)?;
        if hit.time < 1.0 {
            let grav_z = self.zone_vec(a, "ZoneGravity")[2];
            let down = if grav_z > 0.0 { [0.0, 0.0, 1.0] } else { [0.0, 0.0, -1.0] };
            let dir_move = safe_normal(delta);
            let vel_dir = safe_normal(self.vec_at(a, o.velocity));
            let dz = dot(vel_dir, down);
            if hit.normal[2].abs() >= 0.2 || dz >= 0.5 || dz <= -0.2 {
                self.process_hit_wall(a, hit.normal, hit.actor)?;
                let old_normal = hit.normal;
                let d = dot(delta, hit.normal);
                let mut slide = mul(sub(delta, mul(hit.normal, d)), 1.0 - hit.time);
                if dot(slide, delta) >= 0.0 {
                    let h2 = self.do_move(a, slide)?;
                    if h2.time < 1.0 {
                        self.process_hit_wall(a, h2.normal, h2.actor)?;
                        two_wall_adjust(dir_move, &mut slide, h2.normal, old_normal, h2.time);
                        self.do_move(a, slide)?;
                    }
                }
            } else {
                let remaining = mul(delta, 1.0 - hit.time);
                let z_before = self.vec_at(a, o.location)[2];
                let mut h = hit;
                self.step_up(a, down, dir_move, remaining, &mut h)?;
                let ro = self.region_offsets();
                if let Some(off) = ro.old_location {
                    let mut old = self.vec_at(a, off);
                    old[2] = old[2] - z_before + self.vec_at(a, o.location)[2];
                    self.set_vec_at(a, off, old);
                }
            }
        }
        if !self.just_teleported(a) {
            let (l, old) = (self.vec_at(a, o.location), self.old_location(a));
            self.set_vec_at(a, o.velocity, mul(sub(l, old), 1.0 / dt));
        }
        Ok(())
    }
}

// ---- falling, landing, walking and the helpers they share ---------------------------------------------------------

impl Vm {
    fn mv(&mut self, a: u32, delta: Vec3, hit: &mut CheckResult) -> Res<()> {
        let rot = self.rot_of(a);
        self.move_actor(a, delta, rot, hit, false, false, false, false)?;
        Ok(())
    }

    fn is_pawn_actor(&mut self, a: u32) -> bool {
        let o = self.offsets();
        self.is_instance_of(a, o.class_pawn)
    }

    fn is_player_actor(&mut self, a: u32) -> bool {
        let o = self.offsets();
        self.is_instance_of(a, o.class_player_pawn)
    }

    /// The player-only prelude of the pawn physics modes (`FUN_103cc450`). A player facing a world surface whose texture
    /// is in the `Ladder` group climbs: with movement input the vertical speed follows the view pitch (pushed 6144 units
    /// away from level, so looking straight ahead climbs), `GroundSpeed * sin(pitch) / 2`, and the step is flown; without
    /// input the player hangs still. Returns whether the step was handled. Off a ladder the floor slide runs
    /// (`floor_slide`) and the step is not handled. Not sent: the `WalkTexture` event of both branches (declared in
    /// `Pawn`, implemented by no Deus Ex script, so it does nothing).
    fn pawn_prelude(&mut self, a: u32, dt: f32, iterations: i32) -> Res<bool> {
        if !self.is_player_actor(a) {
            return Ok(false);
        }
        if !self.ladder_ahead(a) {
            self.floor_slide(a, dt)?;
            return Ok(false);
        }
        let o = self.offsets();
        let moving = !is_zero(self.vec_at(a, o.acceleration));
        if moving {
            self.set_bp(a, "bIsWalking", true);
            let pitch = self.rot_prop(a, "ViewRotation")[0];
            let pitch = if pitch >= 0 { pitch + 0x1800 } else { pitch - 0x1800 };
            // the X axis of the rotation (`FCoords / FRotator`): its Z is the engine's table sine of the pitch
            let s = ue_assets::light::sin_tab(pitch);
            let mut v = self.vec_at(a, o.velocity);
            v[2] = self.fp(a, "GroundSpeed") * s * 0.5;
            self.set_vec_at(a, o.velocity, v);
        }
        self.phys_flying(a, dt, iterations)?;
        if is_zero(self.vec_at(a, o.acceleration)) {
            self.set_vec_at(a, o.velocity, [0.0; 3]);
        }
        Ok(true)
    }

    /// `FUN_103ccc40`: whether a ladder is next to the player. From 95% of the collision height below the centre, lines of
    /// twice the collision radius go forward, left, right and back (relative to the yaw), each with its end raised by
    /// 0..32 units in steps of 4; a world surface hit whose texture group is `Ladder` counts.
    fn ladder_ahead(&mut self, a: u32) -> bool {
        let o = self.offsets();
        let loc = self.vec_at(a, o.location);
        let (radius, height) = (self.fp(a, "CollisionRadius"), self.fp(a, "CollisionHeight"));
        let start = [loc[0], loc[1], loc[2] - height * 0.95];
        let yaw = self.rot_of(a)[1];
        let ladder = self.reg.names.intern("Ladder");
        for turn in [0, -0x4000, 0x4000, 0x8000] {
            let ang = ((yaw + turn) & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
            let end = [start[0] + ang.cos() * 2.0 * radius, start[1] + ang.sin() * 2.0 * radius, start[2]];
            for k in (0..=32).step_by(4) {
                let hit = self.single_line_check(a, [end[0], end[1], end[2] + k as f32], start, TRACE_LEVEL | TRACE_MOVERS, [0.0; 3], 0);
                if hit.actor != 0 && hit.actor == self.level {
                    if let Some(((_, group), _)) = self.surface_of_node(hit.item) {
                        if group == ladder {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// The low-friction slide of the player prelude (103cc6cb..103cc9e0): a level and mover trace from the centre
    /// `2 * CollisionRadius + CollisionHeight` down; when walking on a world surface whose texture `Friction` is below
    /// 1, `k = max(4 * Friction, 0.05)` (0.05 when not above it), `V = (dt * ZoneGravity) * (1 / (0.5 k)) * dt`, the
    /// slide `D = V - N (V . N)` along the floor normal, moved with `MoveActor` when `D . V >= 0`. No shipped texture
    /// in any map has a `Friction` below 1 (only the unused `Rocket.Metal.Slippery_Metal`).
    fn floor_slide(&mut self, a: u32, dt: f32) -> Res<()> {
        let o = self.offsets();
        let loc = self.vec_at(a, o.location);
        let (r, h) = self.cylinder(a);
        let end = [loc[0], loc[1], loc[2] - (r + r + h)];
        let hit = self.single_line_check(a, end, loc, crate::collision::TRACE_LEVEL | crate::collision::TRACE_MOVERS, [0.0; 3], 0);
        if hit.actor == 0 || hit.actor != self.level || self.u8_at(a, o.physics) != 1 {
            return Ok(());
        }
        let Some(friction) = self.surface_friction(hit.item) else { return Ok(()) };
        if !((friction as f64) < 1.0) {
            return Ok(());
        }
        let zone = self.zone_of(a);
        let zg = if zone != 0 { self.vp(zone, "ZoneGravity") } else { [0.0, 0.0, -950.0] };
        if let Some(slide) = floor_slide_delta(friction, dt, zg, hit.normal) {
            let rot = self.rotation(a);
            let mut h2 = crate::collision::CheckResult::none();
            self.move_actor(a, slide, rot, &mut h2, false, false, false, false)?;
        }
        Ok(())
    }

    fn set_support_floor(&mut self, a: u32, floor: u32) -> Res<()> {
        self.set_support_base(a, floor)
    }

    /// `APawn::calcVelocity`: integrates acceleration with friction (or brakes when there is no acceleration), then
    /// caps the speed. `fluid` is 1 in water/air, `brake` makes a pawn without acceleration slow down.
    #[allow(clippy::too_many_arguments)]
    pub fn calc_velocity(&mut self, a: u32, dir: Vec3, dt: f32, max_speed: f32, friction: f32, fluid: bool, brake: bool, buoyant: bool) {
        let o = self.offsets();
        let fluid_f: f32 = if fluid { 1.0 } else { 0.0 };
        let decel = fluid_f.max(friction);
        let is_player = self.is_player_actor(a);
        let slow_walk = is_player && self.bp(a, "bIsWalking");
        let net_mode = {
            let l = self.level;
            self.u8p(l, "NetMode")
        };
        let mut acc = self.vec_at(a, o.acceleration);
        let accel_rate = self.fp(a, "AccelRate");
        if !brake || !is_zero(acc) {
            let v = self.vec_at(a, o.velocity);
            let speed = size_sq(v).sqrt();
            if !slow_walk {
                if accel_rate * accel_rate < size_sq(acc) {
                    acc = mul(dir, accel_rate);
                }
            } else if accel_rate * accel_rate * 0.09 < size_sq(acc) {
                acc = mul(mul(dir, accel_rate), 0.3);
            }
            self.set_vec_at(a, o.acceleration, acc);
            let keep = mul(dir, speed);
            let v = sub(v, mul(mul(sub(v, keep), dt), decel));
            self.set_vec_at(a, o.velocity, v);
        } else {
            // braking: damp in 0.03 s slices, keeping only slices that still point the old way
            let old = self.vec_at(a, o.velocity);
            let mut v = old;
            let mut result = [0.0; 3];
            let mut rem = dt;
            while rem > 0.03 {
                v = sub(v, mul(mul(mul(v, 2.0), 0.03), decel));
                if dot(old, v) > 0.0 {
                    result = add(result, mul(mul(v, 0.03), 1.0 / dt));
                }
                rem -= 0.03;
            }
            v = sub(v, mul(mul(mul(v, 2.0), rem), decel));
            if dot(old, v) > 0.0 {
                result = add(result, mul(mul(v, rem), 1.0 / dt));
            }
            v = result;
            if dot(old, v) < 0.0 || size_sq(v) < 100.0 {
                v = [0.0; 3];
            }
            self.set_vec_at(a, o.velocity, v);
        }
        let acc = self.vec_at(a, o.acceleration);
        let v = self.vec_at(a, o.velocity);
        let mut v = add(mul(acc, dt), mul(v, 1.0 - fluid_f * dt * friction));
        let mut max_speed = max_speed;
        if !is_player {
            max_speed *= self.fp(a, "DesiredSpeed");
        }
        if buoyant {
            let g = self.zone_vec(a, "ZoneGravity");
            let (buoyancy, mass) = (self.fp(a, "Buoyancy"), self.fp(a, "Mass"));
            v = add(v, mul(mul(g, dt), 1.0 - buoyancy / mass));
        }
        let mut mult = max_speed;
        let mut scaled = false;
        if slow_walk {
            let (c1, c2) = if net_mode == 0 { (0.09, 0.3) } else { (0.36, 0.6) };
            if max_speed * max_speed * c1 < size_sq(v) {
                let len = size_sq(v).sqrt();
                v = mul(v, 1.0 / len);
                mult = ((1.0 - 2.0 * decel * dt) * len).max(max_speed * c2);
                scaled = true;
            }
        }
        if scaled {
            v = mul(v, mult);
        } else if size_sq(v) > max_speed * max_speed {
            v = mul(safe_normal(v), mult);
        }
        self.set_vec_at(a, o.velocity, v);
    }

    /// `AActor::processHitWall`: pawns that keep walking into a wall give up their move (unless script listens for
    /// `HitWall`); other actors get the `HitWall` event when they are probing for it.
    pub fn process_hit_wall(&mut self, a: u32, normal: Vec3, hit_actor: u32) -> Res<()> {
        let o = self.offsets();
        if hit_actor != 0 && self.is_pawn_actor(hit_actor) {
            return Ok(());
        }
        if self.is_pawn_actor(a) {
            if is_zero(self.vec_at(a, o.acceleration)) {
                return Ok(());
            }
            let dest = self.vp(a, "Destination");
            let loc = self.vec_at(a, o.location);
            let mut dir = safe_normal(sub(dest, loc));
            let mut n = normal;
            if self.u8_at(a, o.physics) == PHYS_WALKING {
                n[2] = 0.0;
                dir[2] = 0.0;
            }
            if self.fp(a, "MinHitWall") < dot(n, dir) {
                return Ok(());
            }
            if !self.is_probing(a, "HitWall") && self.u8_at(a, o.physics) != PHYS_FALLING {
                self.set_fp(a, "MoveTimer", -1.0);
                self.set_bp(a, "bFromWall", true);
                return Ok(());
            }
        } else if !self.is_probing(a, "HitWall") {
            return Ok(());
        }
        self.send_event(a, "HitWall", vec![Value::Vector(normal), Value::Obj(hit_actor)])?;
        Ok(())
    }

    /// `AActor::processLanded`.
    pub fn process_landed(&mut self, a: u32, normal: Vec3, hit_actor: u32, remaining: f32, iterations: i32) -> Res<()> {
        let o = self.offsets();
        let is_pawn = self.is_pawn_actor(a);
        if !is_pawn && self.zone_b(a, "bBounceVelocity") {
            let zv = self.zone_vec(a, "ZoneVelocity");
            if !is_zero(zv) {
                self.set_vec_at(a, o.velocity, [zv[0], zv[1], zv[2] + 80.0]);
                return Ok(());
            }
        }
        if !is_pawn {
            if self.is_instance_of(a, o.class_decoration) {
                if self.ip(a, "numLandings") < 5 {
                    if self.decoration_tumble(a, normal)? {
                        return Ok(());
                    }
                } else {
                    self.set_ip(a, "numLandings", 0);
                }
            }
        } else {
            // a pawn that landed on thin air (nothing close below) is nudged to a free spot
            let (r, h) = self.cylinder(a);
            let loc = self.vec_at(a, o.location);
            let end = [loc[0], loc[1], loc[2] - (r * 0.2 + 8.0)];
            let hit = self.single_line_check(a, end, loc, 0x37, [r * 0.9, r * 0.9, h * 0.9], 0);
            if hit.time == 1.0 {
                let mut spot = loc;
                if self.find_spot([r * 1.1, r * 1.1, h * 1.1], &mut spot, false, false) && spot != loc {
                    self.far_move_actor(a, spot, false, false)?;
                    let v = self.vec_at(a, o.velocity);
                    let (f1, f2) = (self.next_rand_f32(), self.next_rand_f32());
                    self.set_vec_at(a, o.velocity, [v[0] + f1 * 60.0 - 30.0, v[1] + f2 * 60.0 - 30.0, v[2]]);
                    return Ok(());
                }
            }
        }
        self.send_event(a, "Landed", vec![Value::Vector(normal)])?;
        if self.u8_at(a, o.physics) == PHYS_FALLING {
            self.set_physics(a, if is_pawn { PHYS_WALKING } else { PHYS_NONE }, hit_actor)?;
        }
        if self.u8_at(a, o.physics) == PHYS_WALKING && is_pawn {
            let acc = safe_normal(self.vec_at(a, o.acceleration));
            self.set_vec_at(a, o.acceleration, acc);
            if remaining > 0.01 {
                self.phys_walking(a, remaining, iterations)?;
            }
        }
        Ok(())
    }

    /// The decoration part of `processLanded` (numLandings below 5): a box that rests on fewer than two opposite corners
    /// tips over (the corners are probed 8 units down, half a radius in from the edge), and a sliding carcass that lands
    /// on a slope bounces off it. Returns true when the landing was turned into a tumble or bounce (no `Landed` event).
    fn decoration_tumble(&mut self, a: u32, normal: Vec3) -> Res<bool> {
        let o = self.offsets();
        let (r, h) = self.cylinder(a);
        let loc = self.vec_at(a, o.location);
        // something under the middle of the box?
        let start = [loc[0], loc[1], loc[2] - h * 0.8];
        let end = [loc[0], loc[1], loc[2] - (h + r + 8.0)];
        let under = self.single_line_check(a, end, start, 0x37, [0.0; 3], 0);
        if under.actor == 0 {
            let extent = [r * 0.5, r * 0.5, h];
            // free (nothing hit) corners, in the order (+,+), (-,+), (-,-), (+,-)
            let corner = |vm: &mut Vm, sx: f32, sy: f32| -> i32 {
                let s = [loc[0] + sx * r * 0.5, loc[1] + sy * r * 0.5, loc[2]];
                let e = [s[0], s[1], s[2] - 8.0];
                (vm.single_line_check(a, e, s, 0x17, extent, 0).actor == 0) as i32
            };
            let c = [corner(self, 1.0, 1.0), corner(self, -1.0, 1.0), corner(self, -1.0, -1.0), corner(self, 1.0, -1.0)];
            if c.iter().sum::<i32>() > 1 && c[0] + c[2] != 0 && c[1] + c[3] != 0 {
                let n = self.ip(a, "numLandings");
                self.set_ip(a, "numLandings", n + 1);
                let (dx, dy) = ((c[0] + c[3] - c[1] - c[2]) as f32, (c[0] + c[1] - c[2] - c[3]) as f32);
                let fall = -self.vec_at(a, o.velocity)[2];
                let speed = if fall >= 30.0 { fall.min(30.0 + r) } else { 30.0 };
                let k = 2.0 * speed;
                self.set_vec_at(a, o.velocity, [dx * k, dy * k, k * 0.5]);
                return Ok(true);
            }
        }
        if self.is_instance_of(a, self.reg.class_by_name("Carcass").unwrap_or(o.class_decoration)) && normal[2] < 0.9 && self.bp(a, "bSlidingCarcass") {
            if self.next_rand_f32() < 0.2 {
                let n = self.ip(a, "numLandings");
                self.set_ip(a, "numLandings", n + 1);
            }
            let v = mul(normal, 120.0);
            self.set_vec_at(a, o.velocity, [v[0], v[1], 70.0]);
            return Ok(true);
        }
        self.set_ip(a, "numLandings", 0);
        Ok(false)
    }

    /// `AActor::physRolling` (Engine.dll 10301109): a decoration that slides or rolls along the ground. Ground friction
    /// bleeds the velocity toward the acceleration direction, the step is taken in pieces of at most 0.1 s, a wall stops
    /// the slide (or bounces with `bBounce`), and after each piece a probe 16 units along gravity decides whether the
    /// actor is still supported (a floor with normal.z >= 0.7); if not it starts falling.
    fn phys_rolling(&mut self, a: u32, dt: f32, mut iterations: i32) -> Res<()> {
        let o = self.offsets();
        let zone_gf = self.zone_f(a, "ZoneGroundFriction");
        let zone_ff = self.zone_f(a, "ZoneFluidFriction");
        let vel = self.vec_at(a, o.velocity);
        let acc = self.vec_at(a, o.acceleration);
        let (dv, da) = (safe_normal(vel), safe_normal(acc));
        let speed = size_sq(vel).sqrt();
        let mut v = [0.0f32; 3];
        for k in 0..3 {
            v[k] = vel[k] - speed * (dv[k] - da[k]) * dt * zone_gf;
        }
        let drag = 1.0 - dt * zone_ff;
        for k in 0..3 {
            v[k] = drag * v[k] + dt * acc[k];
        }
        self.set_vec_at(a, o.velocity, v);
        let world_v = add(self.zone_vec(a, "ZoneVelocity"), v);
        self.store_old_location(a);
        let gravity = self.zone_vec(a, "ZoneGravity");
        let probe = [0.0, 0.0, if gravity[2] > 0.0 { 1.0 } else { -1.0 } * 16.0];
        self.clear_teleported(a);
        let mut remaining = dt;
        let mut bounces = 0;
        let mut hit;
        loop {
            if remaining <= 0.0 || iterations > 7 {
                if self.just_teleported(a) {
                    return Ok(());
                }
                let (l, old) = (self.vec_at(a, o.location), self.old_location(a));
                self.set_vec_at(a, o.velocity, mul(sub(l, old), 1.0 / dt));
                return Ok(());
            }
            iterations += 1;
            let step = if remaining <= 0.1 { remaining } else { (remaining * 0.5).min(0.1) };
            remaining -= step;
            let mut delta = mul(world_v, step);
            let step_delta = delta;
            let from = self.vec_at(a, o.location);
            if delta.iter().any(|c| c.abs() >= 0.0001) {
                hit = CheckResult::none();
                self.mv(a, delta, &mut hit)?;
                if hit.time < 1.0 {
                    self.send_event(a, "HitWall", vec![Value::Vector(hit.normal), Value::Obj(hit.actor)])?;
                    if !self.bp(a, "bBounce") {
                        let old_normal = hit.normal;
                        let d = dot(hit.normal, delta);
                        delta = mul(sub(delta, mul(hit.normal, d)), 1.0 - hit.time);
                        if dot(delta, step_delta) >= 0.0 {
                            let t = hit.time;
                            self.mv(a, delta, &mut hit)?;
                            if hit.time < 1.0 {
                                self.send_event(a, "HitWall", vec![Value::Vector(hit.normal), Value::Obj(hit.actor)])?;
                                two_wall_adjust(safe_normal(world_v), &mut delta, hit.normal, old_normal, hit.time);
                                let _ = t;
                                self.mv(a, delta, &mut hit)?;
                            }
                        }
                    } else {
                        if bounces < 2 {
                            remaining += (1.0 - hit.time) * step;
                        }
                        bounces += 1;
                    }
                }
            }
            // is there still ground under it?
            hit = CheckResult::none();
            self.mv(a, probe, &mut hit)?;
            let (probe_time, mut floor_z) = (hit.time, hit.normal[2]);
            if hit.time < 1.0 && hit.normal[2] < 1.0 && hit.normal[2] * zone_gf < 3.3 {
                // slide down the slope
                let k = 1.0 / (zone_gf.max(0.5) * 2.0);
                let g = mul(self.zone_vec(a, "ZoneGravity"), k * dt * dt);
                let along = sub(g, mul(hit.normal, dot(g, hit.normal)));
                if dot(along, g) >= 0.0 {
                    // a move too small to count leaves `hit` as the probe left it, as in the engine
                    self.mv(a, along, &mut hit)?;
                    if floor_z < hit.normal[2] {
                        floor_z = hit.normal[2];
                    }
                }
            }
            if probe_time == 1.0 || floor_z < 0.7 {
                // not supported: take the probe back and see how far the slide got
                let mut back = CheckResult::none();
                self.mv(a, mul(probe, -probe_time), &mut back)?;
                let intended = size_sq(step_delta).sqrt();
                let now = self.vec_at(a, o.location);
                let moved = ((now[0] - from[0]).powi(2) + (now[1] - from[1]).powi(2)).sqrt();
                let frac = if intended > 0.0 { (moved / intended).min(1.0) } else { 1.0 };
                remaining += (1.0 - frac) * step;
                self.send_event(a, "Falling", vec![])?;
                if self.u8_at(a, o.physics) == PHYS_ROLLING {
                    self.set_physics(a, PHYS_FALLING, 0)?;
                }
                if self.u8_at(a, o.physics) == PHYS_FALLING {
                    if !self.just_teleported(a) && remaining < dt {
                        let (l, old) = (self.vec_at(a, o.location), self.old_location(a));
                        self.set_vec_at(a, o.velocity, mul(sub(l, old), 1.0 / (dt - remaining)));
                    }
                    let mut v = self.vec_at(a, o.velocity);
                    v[2] = 0.0;
                    self.set_vec_at(a, o.velocity, v);
                    if remaining > 0.005 {
                        self.phys_falling(a, remaining, iterations)?;
                    }
                    return Ok(());
                }
                let d = mul(world_v, remaining);
                let mut h3 = CheckResult::none();
                self.mv(a, d, &mut h3)?;
            } else if hit.actor != self.u32_at(a, o.base) {
                self.set_support_base(a, hit.actor)?;
            }
        }
    }

    /// Velocity after `dt` of gravity, drag in water and the like (the three cases of `physFalling`).
    fn falling_velocity(&mut self, a: u32, old_vel: Vec3, dt: f32) -> Vec3 {
        let o = self.offsets();
        let acc = self.vec_at(a, o.acceleration);
        let grav = self.zone_vec(a, "ZoneGravity");
        if !self.zone_b(a, "bWaterZone") {
            if self.is_instance_of(a, o.class_decoration) && self.bp(a, "bBobbing") {
                return add(old_vel, mul(mul(add(mul(grav, 0.5), acc), 0.5), dt));
            }
            if self.is_player_actor(a) && old_vel[2] < 0.0 {
                let foot_off = self.region_offsets().foot_region;
                let foot_zone = self.region_zone(a, foot_off);
                if self.bp(foot_zone, "bWaterZone") {
                    let drag = 1.0 - dt * self.fp(foot_zone, "ZoneFluidFriction");
                    return add(mul(mul(add(grav, acc), 0.5), dt), mul(old_vel, drag));
                }
            }
            add(old_vel, mul(mul(add(acc, grav), 0.5), dt))
        } else {
            let mass = self.fp(a, "Mass").max(1.0);
            let buoy = 1.0 - self.fp(a, "Buoyancy") / mass;
            let drag = 1.0 - 2.0 * dt * self.zone_f(a, "ZoneFluidFriction");
            add(mul(mul(add(mul(grav, buoy), acc), 0.5), dt), mul(old_vel, drag))
        }
    }

    fn region_zone(&mut self, a: u32, off: usize) -> u32 {
        self.u32_at(a, off)
    }

    /// `AActor::physFalling`.
    pub fn phys_falling(&mut self, a: u32, dt: f32, mut iterations: i32) -> Res<()> {
        let o = self.offsets();
        let is_pawn = self.is_pawn_actor(a);
        let is_player = self.is_player_actor(a);
        if is_pawn && self.pawn_prelude(a, dt, iterations)? {
            return Ok(());
        }
        if self.zone_number(a) == 0 {
            let name = self.name_of(a);
            self.log.push(format!("{name} fell out of the world"));
            self.send_event(a, "FellOutOfWorld", vec![])?;
            return Ok(());
        }
        let old_accel = self.vec_at(a, o.acceleration);
        let mut max_speed2d = 0.0;
        if is_pawn {
            let mut air_control = self.fp(a, "AirControl");
            let accel_rate = self.fp(a, "AccelRate");
            if air_control > 0.15 {
                // would the air-controlled step run into something? then control is reduced
                let vel = self.vec_at(a, o.velocity);
                let dir = safe_normal(old_accel);
                let k = air_control * accel_rate;
                let step = [(k * dir[0] + vel[0]) * dt, (k * dir[1] + vel[1]) * dt, 0.0];
                let loc = self.vec_at(a, o.location);
                let (r, h) = self.cylinder(a);
                let end = [loc[0] + step[0], loc[1] + step[1], loc[2]];
                if self.single_line_check(a, end, loc, 6, [r, r, h], 0).actor != 0 {
                    air_control = 0.05;
                }
            }
            let mut max_accel = air_control * accel_rate;
            let mut acc = old_accel;
            acc[2] = 0.0;
            self.set_vec_at(a, o.acceleration, acc);
            let vel = self.vec_at(a, o.velocity);
            let speed2d = (vel[0] * vel[0] + vel[1] * vel[1]).sqrt();
            if speed2d < 10.0 {
                max_accel += (10.0 - speed2d) / dt;
            } else if speed2d >= self.fp(a, "GroundSpeed") {
                if air_control > 0.05 {
                    max_speed2d = speed2d;
                } else {
                    max_accel = 1.0;
                }
            }
            if max_accel * max_accel < size_sq(acc) {
                self.set_vec_at(a, o.acceleration, mul(safe_normal(acc), max_accel));
            }
        }
        let mut remaining = dt;
        let mut bounces = 0;
        let mut hit = CheckResult::none();
        let mut apex_flag = false;
        let (mut bounced_flag, mut moved_delta);
        loop {
            if remaining <= 0.0 || iterations > 7 {
                self.set_vec_at(a, o.acceleration, old_accel);
                return Ok(());
            }
            iterations += 1;
            let mut tick = if remaining <= 0.1 { remaining } else { (remaining * 0.5).min(0.1) };
            self.store_old_location(a);
            self.clear_teleported(a);
            let mut old_vel = self.vec_at(a, o.velocity);
            remaining -= tick;
            let v = self.falling_velocity(a, old_vel, tick);
            self.set_vec_at(a, o.velocity, v);
            // an apex (vertical velocity changes sign) splits the step so gravity does not overshoot
            let v = self.vec_at(a, o.velocity);
            if !apex_flag && (old_vel[2] > 0.0) != (v[2] > 0.0) {
                if old_vel[2].abs() <= 5.0 || v[2].abs() <= 5.0 {
                    apex_flag = false;
                } else {
                    apex_flag = true;
                    let ratio = old_vel[2].abs() / (v[2].abs() + old_vel[2].abs());
                    let first = ratio * tick;
                    if first > 0.015 {
                        let second = (1.0 - ratio) * tick;
                        if second > 0.015 {
                            remaining += second;
                            tick = first;
                            let v = self.falling_velocity(a, old_vel, tick);
                            self.set_vec_at(a, o.velocity, v);
                        }
                    }
                }
            } else {
                apex_flag = false;
            }
            if max_speed2d != 0.0 {
                let v = self.vec_at(a, o.velocity);
                let flat = [v[0], v[1], 0.0];
                if max_speed2d * max_speed2d < size_sq(flat) {
                    let n = mul(safe_normal(flat), max_speed2d);
                    self.set_vec_at(a, o.velocity, [n[0], n[1], v[2]]);
                }
            }
            let zone_vel = if !is_pawn || is_player || size_sq(self.zone_vec(a, "ZoneVelocity")) > 40000.0 {
                self.zone_vec(a, "ZoneVelocity")
            } else {
                [0.0; 3]
            };
            let delta = mul(add(zone_vel, self.vec_at(a, o.velocity)), tick);
            moved_delta = delta;
            bounced_flag = false;
            self.mv(a, delta, &mut hit)?;
            if self.is_deleted(a) {
                return Ok(());
            }
            if is_pawn && self.u8_at(a, o.physics) == PHYS_SWIMMING {
                let rem = (1.0 - hit.time) * tick + remaining;
                return self.start_swimming(a, old_vel, tick, rem, iterations);
            }
            if hit.time < 1.0 {
                bounced_flag = true;
                if hit.actor != 0 && self.is_player_actor(hit.actor) && self.is_instance_of(a, o.class_decoration) {
                    let n = self.ip(a, "numLandings");
                    self.set_ip(a, "numLandings", (n - 1).max(0));
                }
                if !self.bp(a, "bBounce") {
                    if hit.normal[2] > 0.7 {
                        remaining += (1.0 - hit.time) * tick;
                        let mut land_remaining = remaining;
                        if !self.just_teleported(a) && hit.time > 0.1 && hit.time * tick > 0.003 {
                            let (l, old) = (self.vec_at(a, o.location), self.old_location(a));
                            self.set_vec_at(a, o.velocity, mul(sub(l, old), 1.0 / (hit.time * tick)));
                            land_remaining = remaining;
                        }
                        return self.process_landed(a, hit.normal, hit.actor, land_remaining, iterations);
                    }
                    self.process_hit_wall(a, hit.normal, hit.actor)?;
                    let old_normal = hit.normal;
                    let d = dot(moved_delta, hit.normal);
                    let mut slide = mul(sub(moved_delta, mul(hit.normal, d)), 1.0 - hit.time);
                    let step_time = hit.time;
                    if dot(slide, moved_delta) >= 0.0 {
                        self.mv(a, slide, &mut hit)?;
                        if hit.time < 1.0 {
                            if hit.normal[2] > 0.7 {
                                return self.process_landed(a, hit.normal, hit.actor, 0.0, iterations);
                            }
                            self.process_hit_wall(a, hit.normal, hit.actor)?;
                            two_wall_adjust(safe_normal(moved_delta), &mut slide, hit.normal, old_normal, step_time.min(hit.time).max(hit.time));
                            let creased = old_normal[2] > 0.0 && hit.normal[2] > 0.0 && slide[2] == 0.0 && dot(old_normal, hit.normal) < 0.0;
                            self.mv(a, slide, &mut hit)?;
                            if creased || hit.normal[2] > 0.7 {
                                return self.process_landed(a, hit.normal, hit.actor, 0.0, iterations);
                            }
                        }
                    }
                    // the step's start velocity becomes the horizontal velocity really moved (vertical kept) for the
                    // correction below
                    let (l, old) = (self.vec_at(a, o.location), self.old_location(a));
                    let k = 1.0 / tick;
                    old_vel = [(l[0] - old[0]) * k, (l[1] - old[1]) * k, old_vel[2]];
                } else {
                    self.send_event(a, "HitWall", vec![Value::Vector(hit.normal), Value::Obj(hit.actor)])?;
                    if self.u8_at(a, o.physics) == PHYS_NONE {
                        return Ok(());
                    }
                    if bounces < 2 {
                        remaining += (1.0 - hit.time) * tick;
                    }
                    bounces += 1;
                }
            }
            if (!self.bp(a, "bBounce") || !bounced_flag) && !self.just_teleported(a) {
                let (l, old) = (self.vec_at(a, o.location), self.old_location(a));
                let mut v = sub(mul(sub(l, old), 1.0 / tick), zone_vel);
                // trapezoid rule: the average velocity over the step gives the end velocity
                if v[2] < old_vel[2] || old_vel[2] >= 0.0 {
                    v = sub(mul(v, 2.0), old_vel);
                }
                let term = self.zone_f(a, "ZoneTerminalVelocity");
                if term * term < size_sq(v) {
                    v = mul(safe_normal(v), term);
                }
                self.set_vec_at(a, o.velocity, v);
            }
        }
    }

    /// `APawn::stepUp` (103ce4a0): rises `MaxStepHeight`, moves, and comes back down. A player walking straight into a
    /// pushable decoration (facing it within about 25 degrees) is slowed by the mass ratio instead of stepping (Deus Ex);
    /// a steep low obstacle is stepped up again with the rest of the move, anything else is slid along.
    pub fn step_up(&mut self, a: u32, grav_dir: Vec3, desired_dir: Vec3, delta: Vec3, hit: &mut CheckResult) -> Res<()> {
        let o = self.offsets();
        let down = mul(grav_dir, self.fp(a, "MaxStepHeight"));
        self.mv(a, mul(down, -1.0), hit)?;
        self.mv(a, delta, hit)?;
        if hit.time < 1.0 {
            let pushing = self.is_player_actor(a)
                && hit.actor != 0
                && self.is_instance_of(hit.actor, o.class_decoration)
                && self.bp(hit.actor, "bPushable")
                && (dot(desired_dir, hit.normal) as f64) < -0.9;
            if pushing {
                self.set_just_teleported(a);
                let (mass, other) = (self.fp(a, "Mass"), self.fp(hit.actor, "Mass"));
                let f = mass / (other + mass);
                let v = self.vec_at(a, o.velocity);
                self.set_vec_at(a, o.velocity, mul(v, f));
                self.process_hit_wall(a, hit.normal, hit.actor)?;
                if self.u8_at(a, o.physics) == PHYS_FALLING {
                    return Ok(());
                }
            } else if hit.normal[2].abs() >= 0.2 || size_sq(delta) * hit.time <= 144.0 {
                self.process_hit_wall(a, hit.normal, hit.actor)?;
                let old_normal = hit.normal;
                let d = dot(delta, hit.normal);
                let mut adjusted = mul(sub(delta, mul(hit.normal, d)), 1.0 - hit.time);
                if dot(adjusted, delta) >= 0.0 {
                    self.mv(a, adjusted, hit)?;
                    if hit.time < 1.0 {
                        self.process_hit_wall(a, hit.normal, hit.actor)?;
                        if self.u8_at(a, o.physics) == PHYS_FALLING {
                            return Ok(());
                        }
                        two_wall_adjust(desired_dir, &mut adjusted, hit.normal, old_normal, hit.time);
                        self.mv(a, adjusted, hit)?;
                    }
                }
            } else {
                let rest = mul(delta, 1.0 - hit.time);
                self.step_up(a, grav_dir, desired_dir, rest, hit)?;
                if self.u8_at(a, o.physics) == PHYS_FALLING {
                    return Ok(());
                }
            }
        }
        self.mv(a, down, hit)?;
        if hit.time < 1.0 && hit.normal[2] < 0.5 {
            let d = dot(down, hit.normal);
            let adjusted = mul(sub(down, mul(hit.normal, d)), 1.0 - hit.time);
            if dot(adjusted, down) >= 0.0 {
                self.mv(a, adjusted, hit)?;
            }
        }
        Ok(())
    }

    fn set_just_teleported(&mut self, a: u32) {
        let ro = self.region_offsets();
        if let Some((off, m)) = ro.just_teleported {
            let w = self.u32_at(a, off) | m;
            self.set_u32_at(a, off, w);
        }
    }

    /// `APawn::physWalking`.
    pub fn phys_walking(&mut self, a: u32, dt: f32, mut iterations: i32) -> Res<()> {
        let o = self.offsets();
        if self.pawn_prelude(a, dt, iterations)? {
            return Ok(());
        }
        if self.zone_number(a) == 0 {
            let name = self.name_of(a);
            self.log.push(format!("{name} fell out of the world"));
            self.send_event(a, "FellOutOfWorld", vec![])?;
            return Ok(());
        }
        let mut v = self.vec_at(a, o.velocity);
        v[2] = 0.0;
        self.set_vec_at(a, o.velocity, v);
        let mut acc = self.vec_at(a, o.acceleration);
        acc[2] = 0.0;
        self.set_vec_at(a, o.acceleration, acc);
        let dir = if is_zero(acc) { acc } else { safe_normal(acc) };
        let (ground_speed, ground_friction) = (self.fp(a, "GroundSpeed"), self.zone_f(a, "ZoneGroundFriction"));
        self.calc_velocity(a, dir, dt, ground_speed, ground_friction, false, true, false);
        let v = self.vec_at(a, o.velocity);
        let mut desired = [v[0], v[1], 0.0];
        let zone_vel = self.zone_vec(a, "ZoneVelocity");
        if self.is_player_actor(a) || size_sq(zone_vel) > 90000.0 {
            desired[0] += zone_vel[0] * 25.0 * dt;
            desired[1] += zone_vel[1] * 25.0 * dt;
        }
        let grav_z = if self.zone_vec(a, "ZoneGravity")[2] > 0.0 { 1.0 } else { -1.0 };
        let step_h = self.fp(a, "MaxStepHeight");
        let down = [0.0, 0.0, (step_h + 2.0) * grav_z];
        let grav_dir = [0.0, 0.0, grav_z];
        let (r, h) = self.cylinder(a);
        let mut hit = CheckResult::none();
        self.store_old_location(a);
        self.clear_teleported(a);
        let mut remaining = dt;
        let mut may_fall_sent = false;
        'outer: loop {
            if remaining <= 0.0 || iterations > 7 {
                if !self.just_teleported(a) {
                    let (l, old) = (self.vec_at(a, o.location), self.old_location(a));
                    self.set_vec_at(a, o.velocity, mul(sub(l, old), 1.0 / dt));
                }
                let mut v = self.vec_at(a, o.velocity);
                v[2] = 0.0;
                self.set_vec_at(a, o.velocity, v);
                return Ok(());
            }
            iterations += 1;
            let tick = if remaining > 0.05 && (self.is_player_actor(a) || size_sq(desired) * remaining * remaining > 400.0) {
                (remaining * 0.5).min(0.05)
            } else {
                remaining
            };
            remaining -= tick;
            let delta = mul(desired, tick);
            let loc0 = self.vec_at(a, o.location);
            let moving = delta.iter().any(|c| c.abs() >= 0.0001);
            if !moving {
                remaining = 0.0;
                self.set_bp(a, "bHitSlopedWall", false);
                let feet = [loc0[0], loc0[1], loc0[2] - h];
                let probe = self.single_line_check(a, [feet[0], feet[1], feet[2] - 20.0], feet, 6, [0.0; 3], 0);
                hit = probe;
                if self.u32_at(a, o.base) == hit.actor && hit.time * 20.0 <= 4.6 && hit.time * 20.0 >= 4.1 {
                    continue 'outer;
                }
            } else {
                let mut move_delta = delta;
                if !self.bp(a, "bHitSlopedWall") {
                    self.mv(a, delta, &mut hit)?;
                    let sloped = !(hit.time >= 1.0 || hit.normal[2] < 0.01 || hit.normal[2] >= 0.7);
                    self.set_bp(a, "bHitSlopedWall", sloped);
                } else {
                    let len = size_sq(delta).sqrt();
                    let d = mul(delta, 1.0 / len);
                    let probe_len = (len + 4.0).max(30.0);
                    let end = add(loc0, mul(d, probe_len));
                    hit = self.single_line_check(a, end, loc0, 6, [r, r, h], 0);
                    let sloped = !(hit.time >= 1.0 || hit.normal[2] < 0.01 || hit.normal[2] >= 0.7);
                    self.set_bp(a, "bHitSlopedWall", sloped);
                    if !sloped {
                        if self.is_player_actor(a) {
                            // the player looks ahead at step height: straight on and 45 degrees to either side; a sloped
                            // surface seen there makes the next step slide along it
                            let base = [loc0[0], loc0[1], loc0[2] - h + step_h + 4.0];
                            let is_sloped = |vm: &mut Vm, d: Vec3| {
                                let probe = vm.single_line_check(a, add(base, mul(d, 100.0)), base, 6, [0.0; 3], 0);
                                !(probe.time >= 1.0 || probe.normal[2] < 0.01 || probe.normal[2] >= 0.7)
                            };
                            let mut s = is_sloped(self, d);
                            if !s {
                                s = is_sloped(self, safe_normal([d[0] + d[1], d[1] - d[0], d[2]]));
                            }
                            if !s {
                                s = is_sloped(self, safe_normal([d[0] - d[1], d[0] + d[1], d[2]]));
                            }
                            self.set_bp(a, "bHitSlopedWall", s);
                        }
                    } else {
                        let mut n = hit.normal;
                        n[2] = 0.0;
                        let n = safe_normal(n);
                        let along = dot(delta, n);
                        move_delta = sub(delta, mul(n, along));
                    }
                    self.mv(a, move_delta, &mut hit)?;
                }
                if hit.time < 1.0 {
                    let desired_dir = safe_normal(move_delta);
                    let rest = mul(move_delta, 1.0 - hit.time);
                    self.step_up(a, grav_dir, desired_dir, rest, &mut hit)?;
                    if self.u8_at(a, o.physics) == PHYS_FALLING {
                        let want = size_sq(delta).sqrt();
                        let loc = self.vec_at(a, o.location);
                        let got = ((loc[0] - loc0[0]).powi(2) + (loc[1] - loc0[1]).powi(2)).sqrt();
                        remaining += (1.0 - (got / want).min(1.0)) * tick;
                        self.send_event(a, "Falling", vec![])?;
                        if self.u8_at(a, o.physics) != PHYS_FALLING {
                            if self.u8_at(a, o.physics) != PHYS_FLYING {
                                return Ok(());
                            }
                            let (air_speed, accel_rate) = (self.fp(a, "AirSpeed"), self.fp(a, "AccelRate"));
                            self.set_vec_at(a, o.velocity, [0.0, 0.0, air_speed]);
                            self.set_vec_at(a, o.acceleration, [0.0, 0.0, accel_rate]);
                            if remaining > 0.01 {
                                self.phys_flying(a, remaining, iterations)?;
                            }
                            return Ok(());
                        }
                        if remaining > 0.01 {
                            return self.phys_falling(a, remaining, iterations);
                        }
                        return Ok(());
                    }
                }
                if self.u8_at(a, o.physics) == PHYS_SWIMMING {
                    return self.start_swimming(a, self.vec_at(a, o.velocity), tick, remaining, iterations);
                }
            }

            // is there still ground below?
            let loc = self.vec_at(a, o.location);
            hit = self.single_line_check(a, add(loc, down), loc, 0x17, [r, r, h], 0);
            let dist = (step_h + 2.0) * hit.time;
            let on_base = hit.actor == self.u32_at(a, o.base);
            if hit.time >= 1.0 || (on_base && dist <= 2.4) {
                if dist < 1.9 {
                    let saved = hit;
                    self.mv(a, [0.0, 0.0, 2.1 - dist], &mut hit)?;
                    hit = saved;
                    hit.time = 0.0;
                }
            } else {
                self.mv(a, down, &mut hit)?;
                if hit.actor != self.u32_at(a, o.base) {
                    self.set_support_floor(a, hit.actor)?;
                }
                if self.u8_at(a, o.physics) == PHYS_SWIMMING {
                    return self.start_swimming(a, self.vec_at(a, o.velocity), tick, remaining, iterations);
                }
            }
            if !(hit.time < 1.0 && hit.normal[2] >= 0.7) {
                // walked off an edge
                if self.bp(a, "bCanJump") {
                    if !may_fall_sent && self.is_probing(a, "MayFall") {
                        may_fall_sent = true;
                        self.send_event(a, "MayFall", vec![])?;
                    }
                    if self.bp(a, "bCanJump") {
                        if hit.time < 1.0 {
                            self.set_bp(a, "bHitSlopedWall", true);
                        }
                        let want = size_sq(delta).sqrt();
                        let loc = self.vec_at(a, o.location);
                        if want == 0.0 {
                            remaining = 0.0;
                        } else {
                            let got = ((loc[0] - loc0[0]).powi(2) + (loc[1] - loc0[1]).powi(2)).sqrt();
                            remaining += (1.0 - (got / want).min(1.0)) * tick;
                        }
                        let mut v = self.vec_at(a, o.velocity);
                        v[2] = 0.0;
                        self.set_vec_at(a, o.velocity, v);
                        self.send_event(a, "Falling", vec![])?;
                        if self.u8_at(a, o.physics) == PHYS_WALKING {
                            self.set_physics(a, PHYS_FALLING, 0)?;
                        }
                        if self.u8_at(a, o.physics) == PHYS_FALLING {
                            let keep_z = self.vec_at(a, o.velocity)[2];
                            if !self.just_teleported(a) && remaining < dt {
                                let (l, old) = (self.vec_at(a, o.location), self.old_location(a));
                                self.set_vec_at(a, o.velocity, mul(sub(l, old), 1.0 / (dt - remaining)));
                            }
                            let mut v = self.vec_at(a, o.velocity);
                            v[2] = keep_z;
                            self.set_vec_at(a, o.velocity, v);
                            if remaining > 0.01 {
                                self.phys_falling(a, remaining, iterations)?;
                            }
                            return Ok(());
                        }
                        let rest = mul(desired, remaining);
                        self.mv(a, rest, &mut hit)?;
                        remaining = 0.0;
                        continue 'outer;
                    }
                }
                // cannot walk off: undo the move and stop
                self.set_vec_at(a, o.velocity, [0.0; 3]);
                self.set_vec_at(a, o.acceleration, [0.0; 3]);
                let old = self.old_location(a);
                self.far_move_actor(a, old, false, false)?;
                self.set_fp(a, "MoveTimer", -1.0);
                return Ok(());
            }
            // standing on a slope: slide down it slowly
            let friction = self.zone_f(a, "ZoneGroundFriction");
            if hit.normal[2] < 1.0 && hit.normal[2] * friction < 3.3 {
                let k = 1.0 / (friction.max(0.5) * 2.0);
                let g = self.zone_vec(a, "ZoneGravity");
                let slide = mul(g, k * dt * dt);
                let d = dot(slide, hit.normal);
                let adjusted = sub(slide, mul(hit.normal, d));
                if dot(adjusted, slide) >= 0.0 {
                    self.mv(a, adjusted, &mut hit)?;
                }
                if self.u8_at(a, o.physics) == PHYS_SWIMMING {
                    return self.start_swimming(a, self.vec_at(a, o.velocity), tick, remaining, iterations);
                }
            }
        }
    }

    /// `APawn::findWaterLine`: halves the segment `start`..`end` until it is shorter than one unit, keeping the half
    /// whose ends lie in different kinds of zone (water or not) from the one the pawn is in now. `end` ends up on the
    /// pawn's side of the water surface.
    pub(crate) fn find_water_line(&mut self, a: u32, mut start: Vec3, end: &mut Vec3) {
        let mine = self.zone_b(a, "bWaterZone");
        while size_sq(sub(*end, start)) >= 1.0 {
            let mid = mul(add(start, *end), 0.5);
            let z = self.point_region(mid).zone;
            if self.bp(z, "bWaterZone") == mine {
                *end = mid;
            } else {
                start = mid;
            }
        }
    }

    /// `APawn::Swim`: moves by `delta`; a pawn that left the water is put back at the water surface. Returns the part
    /// of `delta` that was taken back (0 when it stayed in water).
    fn swim(&mut self, a: u32, delta: Vec3, hit: &mut CheckResult) -> Res<f32> {
        let o = self.offsets();
        let start = self.vec_at(a, o.location);
        self.mv(a, delta, hit)?;
        let mut back = 0.0;
        if !self.zone_b(a, "bWaterZone") {
            let loc = self.vec_at(a, o.location);
            let mut line = loc;
            self.find_water_line(a, start, &mut line);
            if line != loc {
                back = size_sq(sub(line, loc)).sqrt() / size_sq(delta).sqrt();
                // the same hit record: the caller sees this move's hit
                self.mv(a, sub(line, loc), hit)?;
            }
        }
        Ok(back)
    }

    /// `APawn::physSwimming`: like flying, at `WaterSpeed` with buoyancy; a strong zone current (or any current for
    /// players) carries the pawn. Leaving the water switches to falling with a small hop upwards.
    fn phys_swimming(&mut self, a: u32, dt: f32, iterations: i32) -> Res<()> {
        if self.pawn_prelude(a, dt, iterations)? {
            return Ok(());
        }
        let o = self.offsets();
        let ro = self.region_offsets();
        let head_zone = self.u32_at(a, ro.head_region);
        let mut vel = self.vec_at(a, o.velocity);
        if !self.bp(head_zone, "bWaterZone") && vel[2] > 100.0 {
            vel[2] *= 1.0 - dt;
            self.set_vec_at(a, o.velocity, vel);
        }
        self.store_old_location(a);
        self.clear_teleported(a);
        let acc = self.vec_at(a, o.acceleration);
        let dir = if is_zero(acc) { acc } else { safe_normal(acc) };
        let water_speed = self.fp(a, "WaterSpeed");
        let friction = self.zone_f(a, "ZoneFluidFriction");
        self.calc_velocity(a, dir, dt, water_speed, friction, true, false, true);
        let vz_before = self.vec_at(a, o.velocity)[2];
        let zone_vel = if self.is_instance_of(a, o.class_player_pawn) || size_sq(self.zone_vec(a, "ZoneVelocity")) > 90000.0 {
            mul(self.zone_vec(a, "ZoneVelocity"), 25.0 * dt)
        } else {
            [0.0; 3]
        };
        let delta = mul(add(zone_vel, self.vec_at(a, o.velocity)), dt);
        let mut hit = CheckResult::none();
        let mut remaining = self.swim(a, delta, &mut hit)? * dt;
        if hit.time < 1.0 {
            let grav_z = self.zone_vec(a, "ZoneGravity")[2];
            let down = if grav_z > 0.0 { [0.0, 0.0, 1.0] } else { [0.0, 0.0, -1.0] };
            let dir_move = safe_normal(delta);
            let vel_dir = safe_normal(self.vec_at(a, o.velocity));
            let dz = vel_dir[2] * down[2];
            if hit.normal[2].abs() >= 0.2 || dz >= 0.5 || dz <= -0.2 {
                self.process_hit_wall(a, hit.normal, hit.actor)?;
                let old_normal = hit.normal;
                let d = dot(delta, hit.normal);
                let mut slide = mul(sub(delta, mul(hit.normal, d)), 1.0 - hit.time);
                if dot(slide, delta) >= 0.0 {
                    let back = self.swim(a, slide, &mut hit)?;
                    // (the original scales the leftover time this way)
                    remaining *= (1.0 - hit.time) * back;
                    if hit.time < 1.0 {
                        self.process_hit_wall(a, hit.normal, hit.actor)?;
                        two_wall_adjust(dir_move, &mut slide, hit.normal, old_normal, hit.time);
                        let back = self.swim(a, slide, &mut hit)?;
                        remaining *= (1.0 - hit.time) * back;
                    }
                }
            } else {
                let rest = mul(delta, 1.0 - hit.time);
                let z_before = self.vec_at(a, o.location)[2];
                self.step_up(a, down, dir_move, rest, &mut hit)?;
                if let Some(off) = ro.old_location {
                    let mut old = self.vec_at(a, off);
                    old[2] = old[2] - z_before + self.vec_at(a, o.location)[2];
                    self.set_vec_at(a, off, old);
                }
            }
        }
        if !self.just_teleported(a) && remaining < dt {
            // a vertical speed changed during the move (by a script) is kept
            let vz_now = self.vec_at(a, o.velocity)[2];
            let (l, old) = (self.vec_at(a, o.location), self.old_location(a));
            let mut v = mul(sub(l, old), 1.0 / (dt - remaining));
            if vz_now != vz_before {
                v[2] = vz_now;
            }
            self.set_vec_at(a, o.velocity, v);
        }
        if !self.zone_b(a, "bWaterZone") {
            if self.u8_at(a, o.physics) == PHYS_SWIMMING {
                self.set_physics(a, PHYS_FALLING, 0)?;
            }
            let mut v = self.vec_at(a, o.velocity);
            if v[2] > 0.0 && v[2] < 160.0 {
                v[2] = (v[0] * v[0] + v[1] * v[1]).sqrt() * 0.4 + 40.0;
                self.set_vec_at(a, o.velocity, v);
            }
        }
        if remaining > 0.01 {
            // two separate tests, as in the engine
            if self.u8_at(a, o.physics) == PHYS_FALLING {
                self.phys_falling(a, remaining, iterations + 1)?;
            }
            if self.u8_at(a, o.physics) == PHYS_FLYING {
                self.phys_flying(a, remaining, iterations + 1)?;
            }
        }
        Ok(())
    }

    /// `APawn::startSwimming`: a pawn that just entered water is moved back to the surface along its path, gets a
    /// velocity from that path (doubled minus the velocity before, at most 4000), sinks a little when it came in slowly
    /// and swims for the rest of the step.
    fn start_swimming(&mut self, a: u32, old_velocity: Vec3, dt: f32, mut remaining: f32, iterations: i32) -> Res<()> {
        let o = self.offsets();
        let loc = self.vec_at(a, o.location);
        let old = self.old_location(a);
        let mut line = loc;
        self.find_water_line(a, old, &mut line);
        let mut back_time = 0.0;
        if line != loc {
            let total = size_sq(sub(loc, old)).sqrt();
            back_time = size_sq(sub(line, loc)).sqrt() * dt / total;
            remaining += back_time;
            let mut h = CheckResult::none();
            self.mv(a, sub(line, loc), &mut h)?;
        }
        if !self.bp(a, "bBounce") && !self.just_teleported(a) {
            let (l, old) = (self.vec_at(a, o.location), self.old_location(a));
            let v = mul(sub(l, old), 1.0 / (dt - back_time));
            let mut v = sub(mul(v, 2.0), old_velocity);
            if size_sq(v) > 16_000_000.0 {
                v = mul(safe_normal(v), 4000.0);
            }
            self.set_vec_at(a, o.velocity, v);
        }
        let mut v = self.vec_at(a, o.velocity);
        if v[2] > -160.0 && v[2] < 0.0 {
            v[2] = -80.0 - (v[0] * v[0] + v[1] * v[1]).sqrt() * 0.7;
            self.set_vec_at(a, o.velocity, v);
        }
        if remaining > 0.01 {
            self.phys_swimming(a, remaining, iterations)?;
        }
        Ok(())
    }
}

/// The slide step of `floor_slide` for a floor texture `friction` below 1: `k = max(4 * friction, 0.05)`,
/// `V = (dt * gravity) * (1 / (0.5 k)) * dt` (the scale in double), `D = V - N (V . N)`; `None` when `D . V < 0`.
fn floor_slide_delta(friction: f32, dt: f32, gravity: [f32; 3], n: [f32; 3]) -> Option<[f32; 3]> {
    let k4 = friction * 4.0;
    let k = if 0.05f32 < k4 { k4 } else { 0.05f32 };
    let g = [dt * gravity[0], dt * gravity[1], dt * gravity[2]];
    let s = 1.0f64 / (k as f64 * 0.5);
    let v1: [f32; 3] = std::array::from_fn(|i| (g[i] as f64 * s) as f32);
    let v: [f32; 3] = std::array::from_fn(|i| v1[i] * dt);
    let d = (v[2] as f64 * n[2] as f64 + v[1] as f64 * n[1] as f64 + v[0] as f64 * n[0] as f64) as f32;
    let slide: [f32; 3] = std::array::from_fn(|i| v[i] - n[i] * d);
    (slide[0] * v[0] + slide[1] * v[1] + slide[2] * v[2] >= 0.0).then_some(slide)
}

#[cfg(test)]
mod floor_slide_tests {
    use super::floor_slide_delta;

    #[test]
    fn a_flat_floor_does_not_slide_and_a_slope_slides_downhill() {
        let g = [0.0, 0.0, -950.0];
        assert_eq!(floor_slide_delta(0.0, 0.1, g, [0.0, 0.0, 1.0]), Some([0.0, 0.0, 0.0]));
        // 45 degree slope facing +x; friction 0 -> k = 0.05: V = (0, 0, -95 * 40) * 0.1 = (0, 0, -380)
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let d = floor_slide_delta(0.0, 0.1, g, [h, 0.0, h]).unwrap();
        assert!((d[0] - 190.0).abs() < 0.01 && (d[2] + 190.0).abs() < 0.01, "{d:?}");
        // friction 0.5 -> k = 2: a twentieth of that
        let d = floor_slide_delta(0.5, 0.1, g, [h, 0.0, h]).unwrap();
        assert!((d[0] - 4.75).abs() < 0.001, "{d:?}");
    }
}


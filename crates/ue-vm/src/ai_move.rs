//! Pawn AI movement natives: alliances (`IsValidEnemy` and friends), turning (`TurnTo`, `TurnToward`), walking toward
//! points and actors (`MoveTo`, `MoveToward` with their per-frame polls), the test-move helpers `walkMove` /
//! `AIDirectionReachable` and `AIPickRandomDestination`. Ports of the original engine's behaviour (constants read from
//! the original code); UNVERIFIED against the running game. Flying and swimming test moves are approximations.

use crate::collision::*;
use crate::physics::*;
use crate::{Latent, Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn size(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

/// `FVector::Rotation` (Core 10144390): `_ftol(atan2(Y, X) * C)` and `_ftol(atan2(Z, sqrt(X^2 + Y^2)) * C)` in double
/// precision with C = 10430.219195527361 (32768 / 3.14164, the double at 10178050); no roll. A zero vector goes
/// through `atan2(0, 0)` like any other (probe `vm.cast.vector.rotator`).
pub fn vec_to_rot(d: Vec3) -> [i32; 3] {
    const C: f64 = 10430.219195527361;
    let (x, y, z) = (d[0] as f64, d[1] as f64, d[2] as f64);
    let yaw = crate::route::ftol(y.atan2(x) * C);
    let pitch = crate::route::ftol(z.atan2((x * x + y * y).sqrt()) * C);
    [pitch, yaw, 0]
}

pub const ALLIANCE_FRIENDLY: i32 = 0;
pub const ALLIANCE_NEUTRAL: i32 = 1;
pub const ALLIANCE_HOSTILE: i32 = 2;

/// Where the pieces of `AlliancesEx[16]` live.
#[derive(Clone, Copy)]
pub(crate) struct AllianceLayout {
    array: usize,
    stride: usize,
    name: usize,
    level: usize,
    agitation: usize,
    /// Offset and mask of `bPermanent`.
    permanent: Option<(usize, u32)>,
}

impl Vm {
    pub(crate) fn set_op(&mut self, a: u32, name: &'static str, v: u32) {
        if let Some((o, _)) = self.prop_at(a, name) {
            self.set_u32_at(a, o, v);
        }
    }

    fn alliance_layout(&mut self, a: u32) -> Option<AllianceLayout> {
        if let Some(l) = self.alliance_layout {
            return Some(l);
        }
        let class = self.class_of(a)?;
        let p = self.reg.find_prop(class, "AlliancesEx")?.clone();
        let ue_registry::Target::Struct(sid) = p.target else { return None };
        let member = |n: &str| -> Option<usize> {
            let id = self.reg.names.get(n)?;
            self.reg.struct_def(sid).props.iter().find(|m| m.name == id).map(|m| m.offset as usize)
        };
        let permanent = {
            let id = self.reg.names.get("bPermanent");
            id.and_then(|id| self.reg.struct_def(sid).props.iter().find(|m| m.name == id).map(|m| (m.offset as usize, m.bit_mask)))
        };
        let l = AllianceLayout { array: p.offset as usize, stride: p.elem_size as usize, name: member("AllianceName")?, level: member("AllianceLevel")?, agitation: member("AgitationLevel")?, permanent };
        self.alliance_layout = Some(l);
        Some(l)
    }

    /// `AScriptedPawn::UpdateAgitation` (DeusEx.dll 10019e30): agitation toward alliances decays once the sustain
    /// timer has run out.
    fn update_agitation(&mut self, a: u32, dt: f32) {
        let t = self.fp(a, "AgitationCheckTimer");
        if t > 0.0 {
            self.set_fp(a, "AgitationCheckTimer", (t - dt).max(0.0));
        }
        let timer = self.fp(a, "AgitationTimer");
        let rate = self.fp(a, "AgitationDecayRate");
        let mut dec = 0.0;
        if timer <= 0.0 {
            dec = dt * rate;
        } else if dt <= timer {
            self.set_fp(a, "AgitationTimer", timer - dt);
        } else {
            self.set_fp(a, "AgitationTimer", 0.0);
            dec = dt * rate * (1.0 - timer / dt);
        }
        if self.bp(a, "bAlliancesChanged") && dec > 0.0 {
            self.set_bp(a, "bAlliancesChanged", false);
            let Some(l) = self.alliance_layout(a) else { return };
            for i in (0..16).rev() {
                let base = l.array + i * l.stride;
                let permanent = l.permanent.map(|(off, m)| self.u32_at(a, base + off) & m != 0).unwrap_or(false);
                let ag = self.f32_at(a, base + l.agitation);
                if self.u32_at(a, base + l.name) != 0 && !permanent && ag > 0.0 {
                    self.set_bp(a, "bAlliancesChanged", true);
                    self.set_u32_at(a, base + l.agitation, (ag - dec).max(0.0).to_bits());
                }
            }
        }
    }

    /// `AScriptedPawn::UpdateFear` (DeusEx.dll 10019f60).
    fn update_fear(&mut self, a: u32, dt: f32) {
        let timer = self.fp(a, "FearTimer");
        let rate = self.fp(a, "FearDecayRate");
        let dec;
        if timer <= 0.0 {
            dec = dt * rate;
        } else if dt <= timer {
            self.set_fp(a, "FearTimer", timer - dt);
            return;
        } else {
            self.set_fp(a, "FearTimer", 0.0);
            dec = dt * rate * (1.0 - timer / dt);
        }
        let level = self.fp(a, "FearLevel");
        if dec > 0.0 && level > 0.0 {
            self.set_fp(a, "FearLevel", (level - dec).max(0.0));
        }
    }

    /// `AScriptedPawn::Tick` (DeusEx.dll 100195a0), which runs before the actor tick of every `ScriptedPawn`: pawns
    /// flagged `bDisappear` vanish once unseen for 5 s, the pre-pivot glides, agitation and fear decay, the many AI
    /// timers count down, the cloak follows health, an obstacle-avoiding turn is given up when it no longer applies,
    /// fire burns out and wounds bleed. Returns false when the pawn was destroyed.
    pub fn scripted_pawn_tick(&mut self, a: u32, dt: f32) -> Res<bool> {
        let o = self.offsets();
        let now = if self.level != 0 { self.get_prop(self.level, "TimeSeconds", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0) } else { 0.0 };
        let unseen = (now - self.fp(a, "LastRenderTime")).max(0.0);
        if self.bp(a, "bDisappear") && (self.ai_in_stasis(a) || unseen > 5.0) {
            self.destroy(a)?;
            return Ok(false);
        }
        let pp_time = self.fp(a, "PrePivotTime");
        if pp_time > 0.0 {
            let desired = self.vp(a, "DesiredPrePivot");
            if dt >= pp_time {
                self.set_vp(a, "PrePivot", desired);
                self.set_fp(a, "PrePivotTime", 0.0);
            } else {
                let pp = self.vp(a, "PrePivot");
                let k = dt * (1.0 / pp_time);
                self.set_vp(a, "PrePivot", [pp[0] + (desired[0] - pp[0]) * k, pp[1] + (desired[1] - pp[1]) * k, pp[2] + (desired[2] - pp[2]) * k]);
                self.set_fp(a, "PrePivotTime", pp_time - dt);
            }
        }
        let low_priority = !(self.bp(a, "bTickVisibleOnly") && self.fp(a, "DistanceFromPlayer") > 1200.0);
        self.update_agitation(a, dt);
        self.update_fear(a, dt);
        let at = self.fp(a, "AlarmTimer") - dt;
        self.set_fp(a, "AlarmTimer", at.max(0.0));
        let weapon = self.op(a, "Weapon");
        if weapon == 0 {
            self.set_fp(a, "WeaponTimer", 0.0);
        } else {
            let w = self.fp(a, "WeaponTimer");
            self.set_fp(a, "WeaponTimer", w + dt);
        }
        let countdown = |vm: &mut Vm, name: &'static str| {
            let t = vm.fp(a, name);
            if t > 0.0 {
                let left = t - dt;
                vm.set_fp(a, name, if left < 0.0 { 0.0 } else { left });
            }
        };
        countdown(self, "FireTimer");
        countdown(self, "SpecialTimer");
        let rt = self.fp(a, "ReloadTimer");
        if rt <= 0.0 || weapon == 0 {
            self.set_fp(a, "ReloadTimer", 0.0);
        } else {
            self.set_fp(a, "ReloadTimer", rt - dt);
        }
        for n in ["AvoidWallTimer", "AvoidBumpTimer", "ObstacleTimer", "CloakEMPTimer", "TakeHitTimer", "CarcassCheckTimer"] {
            countdown(self, n);
        }
        let pe = self.fp(a, "PotentialEnemyTimer");
        if pe > 0.0 {
            let left = pe - dt;
            self.set_fp(a, "PotentialEnemyTimer", left);
            if left <= 0.0 {
                self.set_fp(a, "PotentialEnemyTimer", 0.0);
                let _ = self.set_prop(a, "PotentialEnemyAlliance", 0, Value::Name(0));
            }
        }
        for n in ["BeamCheckTimer", "FutzTimer", "PlayerAgitationTimer"] {
            countdown(self, n);
        }
        let dist = self.fp(a, "DistressTimer");
        if dist >= 0.0 {
            let d = dist + dt;
            let sustain = self.fp(a, "FearSustainTime");
            self.set_fp(a, "DistressTimer", if sustain < d { -1.0 } else { d });
        }
        if self.bp(a, "bHasCloak") {
            let on = self.ip(a, "Health") <= self.ip(a, "CloakThreshold");
            self.call_by_name(a, "EnableCloak", vec![Value::Bool(on)])?;
        }
        if self.bp(a, "bAdvancedTactics") {
            let acc = self.vec_at(a, o.acceleration);
            let turning = self.u8p(a, "TurnDirection");
            if acc == [0.0; 3] || self.u8_at(a, o.physics) != PHYS_WALKING || turning == 0 {
                self.set_bp(a, "bAdvancedTactics", false);
                if turning != 0 {
                    let m = self.fp(a, "MoveTimer");
                    self.set_fp(a, "MoveTimer", m - 4.0);
                }
                let _ = self.set_prop(a, "ActorAvoiding", 0, Value::Obj(0));
                self.set_u8p(a, "NextDirection", 0);
                self.set_u8p(a, "TurnDirection", 0);
                self.set_bp(a, "bClearedObstacle", true);
                self.set_fp(a, "ObstacleTimer", 0.0);
            }
        }
        if self.bp(a, "bOnFire") {
            let b = self.fp(a, "burnTimer") + dt;
            self.set_fp(a, "burnTimer", b);
            if self.fp(a, "BurnPeriod") <= b {
                self.call_by_name(a, "ExtinguishFire", vec![])?;
            }
        }
        if low_priority && self.is_live(a) {
            let rate = self.fp(a, "BleedRate");
            if rate > 0.0 && self.bp(a, "bCanBleed") {
                let adjusted = (1.0 - rate.clamp(0.0, 1.0)) * 1.0 + 0.1;
                let v = self.vec_at(a, o.velocity);
                let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                let period = adjusted / (speed / 512.0).clamp(0.05, 1.0);
                let mut counter = self.fp(a, "DropCounter") + dt;
                while counter >= period && self.is_live(a) {
                    self.set_fp(a, "DropCounter", counter);
                    self.call_by_name(a, "SpurtBlood", vec![])?;
                    counter = self.fp(a, "DropCounter") - period;
                }
                self.set_fp(a, "DropCounter", counter);
                let clot = self.fp(a, "ClotPeriod");
                if clot != 0.0 {
                    self.set_fp(a, "BleedRate", rate - dt / clot);
                }
            }
            if self.fp(a, "BleedRate") <= 0.0 {
                self.set_fp(a, "DropCounter", 0.0);
                self.set_fp(a, "BleedRate", 0.0);
            }
        }
        Ok(self.is_live(a))
    }

    /// `AScriptedPawn::GetAllianceType(name)`: how `pawn` regards an alliance.
    pub fn alliance_type(&mut self, pawn: u32, alliance: u32) -> i32 {
        let mut t = ALLIANCE_NEUTRAL;
        if alliance != 0 {
            if let Some(l) = self.alliance_layout(pawn) {
                for i in 0..16 {
                    let base = l.array + i * l.stride;
                    if self.u32_at(pawn, base + l.name) == alliance {
                        let (level, agitation) = (self.f32_at(pawn, base + l.level), self.f32_at(pawn, base + l.agitation));
                        if level < 0.0 || agitation >= 1.0 {
                            t = ALLIANCE_HOSTILE;
                        } else if level > 0.0 {
                            t = ALLIANCE_FRIENDLY;
                        }
                        break;
                    }
                }
            }
        }
        if self.bp(pawn, "bLikesNeutral") && t == ALLIANCE_NEUTRAL {
            t = ALLIANCE_FRIENDLY;
        }
        if self.bp(pawn, "bReverseAlliances") {
            if t == ALLIANCE_FRIENDLY {
                return ALLIANCE_HOSTILE;
            }
            t = (t != ALLIANCE_HOSTILE) as i32;
        }
        t
    }

    /// `AScriptedPawn::GetPawnAllianceType(other)`: hostile when either side sees the other that way.
    pub fn pawn_alliance_type(&mut self, pawn: u32, other: u32) -> i32 {
        if !self.is_live(other) {
            return ALLIANCE_NEUTRAL;
        }
        let scripted = self.reg.class_by_name("ScriptedPawn");
        let mut theirs = ALLIANCE_NEUTRAL;
        if let (Some(sp), Some(c)) = (scripted, self.class_of(other)) {
            if self.is_a(c, sp) {
                let mine = self.u32_prop(pawn, "Alliance");
                theirs = self.alliance_type(other, mine);
            }
        }
        let alliance = self.u32_prop(other, "Alliance");
        let r = self.alliance_type(pawn, alliance);
        if theirs == ALLIANCE_HOSTILE { ALLIANCE_HOSTILE } else { r }
    }

    fn u32_prop(&mut self, a: u32, name: &'static str) -> u32 {
        self.op(a, name)
    }

    /// `AScriptedPawn::IsValidEnemy`.
    pub fn is_valid_enemy(&mut self, pawn: u32, other: u32, check_alliance: bool) -> bool {
        if !self.is_live(other) || other == pawn || self.bp(other, "bDeleteMe") || !self.bp(other, "bDetectable") || self.ip(other, "Health") < 1 {
            return false;
        }
        !check_alliance || self.pawn_alliance_type(pawn, other) == ALLIANCE_HOSTILE
    }

    // ---- turning and moving ------------------------------------------------------------------------------------

    /// Axis the pawn faces (X axis of its rotation).
    fn facing(&mut self, a: u32) -> Vec3 {
        let r = self.rot_pub(a);
        crate::natives::rotate_vector([1.0, 0.0, 0.0], r, false)
    }

    fn rot_pub(&mut self, a: u32) -> [i32; 3] {
        let o = self.offsets();
        [self.u32_at(a, o.rotation) as i32, self.u32_at(a, o.rotation + 4) as i32, self.u32_at(a, o.rotation + 8) as i32]
    }

    /// Gliders (flying or swimming pawns that cannot strafe) accelerate along their facing while turning.
    fn glide_accel(&mut self, a: u32) {
        let o = self.offsets();
        if self.bp(a, "bCanGlide") && !self.bp(a, "bCanStrafe") && matches!(self.u8_at(a, o.physics), PHYS_FLYING | PHYS_SWIMMING) {
            let f = self.facing(a);
            let rate = self.fp(a, "AccelRate");
            self.set_vec_at(a, o.acceleration, mul(f, rate));
        }
    }

    /// `APawn::rotateToward`: aims `DesiredRotation` at `focus`; true when (nearly) facing it.
    pub fn rotate_toward(&mut self, a: u32, focus: Vec3) -> bool {
        let o = self.offsets();
        let phys = self.u8_at(a, o.physics);
        if phys == PHYS_SPIDER {
            return true;
        }
        let loc = self.vec_at(a, o.location);
        let mut want = vec_to_rot(sub(focus, loc));
        want[1] &= 0xFFFF;
        if phys == PHYS_WALKING {
            let target = self.op(a, "MoveTarget");
            let keep_pitch = self.is_live(target) && self.is_instance_of(target, o.class_pawn);
            if !keep_pitch {
                want[0] = 0;
            }
        }
        self.set_rot_prop(a, "DesiredRotation", want);
        let diff = want[1] - (self.rot_pub(a)[1] & 0xFFFF);
        diff.abs() < 2000 || diff.abs() > 0xF82F
    }

    fn set_rot_prop(&mut self, a: u32, name: &'static str, r: [i32; 3]) {
        if let Some((o, _)) = self.prop_at(a, name) {
            for (i, v) in r.iter().enumerate() {
                self.set_u32_at(a, o + 4 * i, *v as u32);
            }
        }
    }

    /// `APawn::setMoveTimer`: how long a move of `dist` may take at the pawn's current speed.
    pub fn set_move_timer(&mut self, a: u32, dist: f32) {
        let o = self.offsets();
        let speed = match self.u8_at(a, o.physics) {
            PHYS_WALKING | PHYS_FALLING | PHYS_SPIDER => self.fp(a, "GroundSpeed"),
            PHYS_FLYING => self.fp(a, "AirSpeed"),
            PHYS_SWIMMING => self.fp(a, "WaterSpeed"),
            _ => 200.0,
        } * self.fp(a, "DesiredSpeed");
        if speed == 0.0 {
            self.set_fp(a, "MoveTimer", 0.5);
            return;
        }
        self.set_fp(a, "MoveTimer", dist * 1.3 / speed + 1.0);
    }

    /// `APawn::moveToward`: steers acceleration toward `dest`. True when the move is over (arrived or timed out).
    pub fn move_toward(&mut self, a: u32, dest: Vec3) -> Res<bool> {
        let o = self.offsets();
        let loc = self.vec_at(a, o.location);
        let mut dir = sub(dest, loc);
        let phys = self.u8_at(a, o.physics);
        if phys == PHYS_WALKING {
            dir[2] = 0.0;
        } else if phys == PHYS_FALLING {
            // air steering only in a zone whose gravity is weaker than 0.9 of its class default; then the move is over
            // once the pawn falls (vertical speed below 0) with the destination more than 100 units above it
            let roff = self.region_offsets().region;
            let zone = self.u32_at(a, roff);
            let default_g = self.class_of(zone).and_then(|c| {
                let off = self.reg.find_prop(c, "ZoneGravity").map(|p| p.offset as usize)?;
                let m = self.class_default_mem(c).ok()?;
                Some(f32::from_le_bytes(m.data.get(off + 8..off + 12)?.try_into().ok()?))
            });
            let g = self.zone_vec(a, "ZoneGravity")[2];
            if !(default_g.unwrap_or(-950.0) as f64 * 0.9 < g as f64) {
                return Ok(false);
            }
            dir[2] = 0.0;
            let rate = self.fp(a, "AccelRate");
            self.set_vec_at(a, o.acceleration, mul(safe_normal(dir), rate));
            if self.vec_at(a, o.velocity)[2] >= 0.0 {
                return Ok(false);
            }
            return Ok(loc[2] + 100.0 < dest[2]);
        }
        let target = self.op(a, "MoveTarget");
        let target_live = self.is_live(target);
        if target_live && self.is_instance_of(target, o.class_inventory) {
            let tl = self.vec_at(target, o.location);
            let (r, h) = self.cylinder(a);
            if (loc[2] - tl[2]).abs() < h && (loc[0] - tl[0]).powi(2) + (loc[1] - tl[1]).powi(2) < r * r {
                self.send_event(target, "Touch", vec![Value::Obj(a)])?;
            }
        }
        let dist = size(dir);
        let glide = self.bp(a, "bCanGlide") && !self.bp(a, "bCanStrafe") && matches!(phys, PHYS_FLYING | PHYS_SWIMMING);
        if dir[0] * dir[0] + dir[1] * dir[1] < 256.0 {
            let (_, h) = self.cylinder(a);
            if dir[2].abs() < h.max(48.0) {
                if !glide {
                    self.set_vec_at(a, o.acceleration, [0.0; 3]);
                }
                return Ok(true);
            }
        }
        if glide {
            dir = self.facing(a);
        } else if dist > 0.0 {
            dir = mul(dir, 1.0 / dist);
        }
        let rate = self.fp(a, "AccelRate");
        let mut acc = mul(dir, rate);
        self.set_vec_at(a, o.acceleration, acc);
        if self.fp(a, "MoveTimer") < 0.0 {
            return Ok(true);
        }
        if target_live && self.is_instance_of(target, o.class_pawn) {
            let (tr, _) = self.cylinder(target);
            let (r, _) = self.cylinder(a);
            return Ok(self.fp(a, "MeleeRange") * 0.8 + tr + r > dist);
        }
        let vel = self.vec_at(a, o.velocity);
        let speed = size(vel);
        if !glide && speed > 100.0 {
            // steer against sideways drift
            let vn = mul(vel, 1.0 / speed);
            let k = (1.0 - dot(vn, dir)) * speed * 0.2;
            acc = sub(acc, mul(sub(vn, dir), k));
            self.set_vec_at(a, o.acceleration, acc);
        }
        if self.fp(a, "AvgPhysicsTime") * speed * 1.4 <= dist {
            return Ok(false);
        }
        // about to arrive within a step: slow down (once), and never faster than 200 units per step
        if !self.bp(a, "bReducedSpeed") {
            self.set_bp(a, "bReducedSpeed", true);
            let d = self.fp(a, "DesiredSpeed");
            self.set_fp(a, "DesiredSpeed", d * 0.5);
        }
        if speed > 0.0 {
            let d = self.fp(a, "DesiredSpeed");
            self.set_fp(a, "DesiredSpeed", d.min(200.0 / speed));
        }
        // a gliding pawn is done when it gets this close
        Ok(glide)
    }

    fn start_latent(&mut self, name: &str) {
        self.pending_latent = Some(Latent::External(name.into()));
    }

    /// `TurnTo(focus)`.
    pub fn native_turn_to(&mut self, a: u32, focus: Vec3) {
        self.set_op(a, "MoveTarget", 0);
        self.start_latent("TurnTo");
        self.set_vp(a, "Focus", focus);
        self.glide_accel(a);
        self.rotate_toward(a, focus);
    }

    /// `TurnToward(target)`.
    pub fn native_turn_toward(&mut self, a: u32, target: u32) {
        if !self.is_live(target) {
            return;
        }
        self.set_op(a, "FaceTarget", target);
        self.start_latent("TurnToward");
        self.glide_accel(a);
        let o = self.offsets();
        let focus = self.vec_at(target, o.location);
        self.set_vp(a, "Focus", focus);
        self.rotate_toward(a, focus);
    }

    fn desired_speed_for(&mut self, a: u32, speed: f32) {
        let max = self.fp(a, "MaxDesiredSpeed");
        self.set_fp(a, "DesiredSpeed", max.min(speed).max(0.0));
    }

    /// `MoveTo(dest, speed)`.
    pub fn native_move_to(&mut self, a: u32, dest: Vec3, speed: f32) -> Res<()> {
        let o = self.offsets();
        let loc = self.vec_at(a, o.location);
        self.set_op(a, "MoveTarget", 0);
        self.set_bp(a, "bReducedSpeed", false);
        self.desired_speed_for(a, speed);
        self.set_move_timer(a, size(sub(dest, loc)));
        self.start_latent("MoveTo");
        self.set_vp(a, "Destination", dest);
        self.set_vp(a, "Focus", dest);
        self.rotate_toward(a, dest);
        self.move_toward(a, dest)?;
        Ok(())
    }

    /// `MoveToward(target, speed)`.
    pub fn native_move_toward(&mut self, a: u32, target: u32, speed: f32) -> Res<()> {
        if !self.is_live(target) {
            return Ok(());
        }
        let o = self.offsets();
        let (loc, tl) = (self.vec_at(a, o.location), self.vec_at(target, o.location));
        self.set_bp(a, "bReducedSpeed", false);
        self.desired_speed_for(a, speed);
        if self.is_instance_of(target, o.class_pawn) {
            self.set_fp(a, "MoveTimer", 1.2);
        } else {
            self.set_move_timer(a, size(sub(tl, loc)));
        }
        self.set_op(a, "MoveTarget", target);
        self.set_vp(a, "Destination", tl);
        self.set_vp(a, "Focus", tl);
        self.start_latent("MoveToward");
        self.rotate_toward(a, tl);
        self.move_toward(a, tl)?;
        Ok(())
    }

    /// The strafe moves' desired speed: a player always moves at full speed, others at `speed` clamped to their maximum.
    fn strafe_speed(&mut self, a: u32, speed: f32) {
        if self.bp(a, "bIsPlayer") {
            let max = self.fp(a, "MaxDesiredSpeed");
            self.set_fp(a, "DesiredSpeed", max);
        } else {
            self.desired_speed_for(a, speed);
        }
    }

    /// `StrafeTo(dest, focus, speed)`: moves to `dest` while turning toward the point `focus` (`APawn::execStrafeTo`).
    pub fn native_strafe_to(&mut self, a: u32, dest: Vec3, focus: Vec3, speed: f32) -> Res<()> {
        let o = self.offsets();
        let loc = self.vec_at(a, o.location);
        self.set_op(a, "MoveTarget", 0);
        self.set_bp(a, "bReducedSpeed", false);
        self.strafe_speed(a, speed);
        self.set_move_timer(a, size(sub(dest, loc)));
        self.start_latent("StrafeTo");
        self.set_vp(a, "Destination", dest);
        self.set_vp(a, "Focus", focus);
        self.rotate_toward(a, focus);
        self.move_toward(a, dest)?;
        Ok(())
    }

    /// `StrafeFacing(dest, target, speed)`: moves to `dest` while facing `target` (`APawn::execStrafeFacing`).
    pub fn native_strafe_facing(&mut self, a: u32, dest: Vec3, target: u32, speed: f32) -> Res<()> {
        if !self.is_live(target) {
            return Ok(());
        }
        let o = self.offsets();
        let loc = self.vec_at(a, o.location);
        self.set_bp(a, "bReducedSpeed", false);
        self.strafe_speed(a, speed);
        self.set_move_timer(a, size(sub(dest, loc)));
        self.set_vp(a, "Destination", dest);
        self.set_op(a, "FaceTarget", target);
        let focus = self.vec_at(target, o.location);
        self.set_vp(a, "Focus", focus);
        self.start_latent("StrafeFacing");
        self.rotate_toward(a, focus);
        self.move_toward(a, dest)?;
        Ok(())
    }

    /// Per-frame poll of the latent turn and move natives; true when the state code may continue.
    pub fn poll_ai_latent(&mut self, a: u32, name: &str) -> Res<Option<bool>> {
        let o = self.offsets();
        match name {
            "TurnTo" => {
                self.glide_accel(a);
                let focus = self.vp(a, "Focus");
                Ok(Some(self.rotate_toward(a, focus)))
            }
            // done once facing the target (`execPollTurnToward`); at once without a target
            "TurnToward" => {
                let t = self.op(a, "FaceTarget");
                if self.is_live(t) {
                    self.glide_accel(a);
                    let focus = self.vec_at(t, o.location);
                    self.set_vp(a, "Focus", focus);
                    return Ok(Some(self.rotate_toward(a, focus)));
                }
                Ok(Some(true))
            }
            "MoveTo" => {
                let dest = self.vp(a, "Destination");
                if self.bp(a, "bAdvancedTactics") && self.u8_at(a, o.physics) == PHYS_WALKING {
                    self.send_event(a, "AlterDestination", vec![])?;
                }
                let cur = self.vp(a, "Destination");
                self.rotate_toward(a, cur);
                let done = self.move_toward(a, cur)?;
                self.set_vp(a, "Destination", dest);
                Ok(Some(done))
            }
            "StrafeTo" | "StrafeFacing" => {
                if name == "StrafeFacing" {
                    let t = self.op(a, "FaceTarget");
                    if !self.is_live(t) {
                        return Ok(Some(true));
                    }
                    let focus = self.vec_at(t, o.location);
                    self.set_vp(a, "Focus", focus);
                    self.rotate_toward(a, focus);
                }
                let dest = self.vp(a, "Destination");
                if self.bp(a, "bAdvancedTactics") && self.u8_at(a, o.physics) == PHYS_WALKING {
                    self.send_event(a, "AlterDestination", vec![])?;
                }
                if name == "StrafeTo" {
                    let focus = self.vp(a, "Focus");
                    self.rotate_toward(a, focus);
                }
                let cur = self.vp(a, "Destination");
                let done = self.move_toward(a, cur)?;
                self.set_vp(a, "Destination", dest);
                Ok(Some(done))
            }
            "MoveToward" => {
                let t = self.op(a, "MoveTarget");
                if !self.is_live(t) {
                    return Ok(Some(true));
                }
                let mut dest = self.vec_at(t, o.location);
                let phys = self.u8_at(a, o.physics);
                if phys == PHYS_FLYING && self.is_instance_of(t, o.class_pawn) {
                    dest[2] += self.cylinder(t).1 * 0.7;
                }
                if phys == PHYS_SPIDER {
                    let r = self.cylinder(t).0;
                    let floor = self.vp(a, "Floor");
                    dest = sub(dest, mul(floor, r));
                }
                self.set_vp(a, "Destination", dest);
                self.set_vp(a, "Focus", dest);
                let speed = self.fp(a, "DesiredSpeed");
                if self.bp(a, "bAdvancedTactics") && phys == PHYS_WALKING {
                    self.send_event(a, "AlterDestination", vec![])?;
                }
                let cur = self.vp(a, "Destination");
                self.rotate_toward(a, cur);
                let done = self.move_toward(a, cur)?;
                // after the step (`execPollMoveToward`): Destination back on the target; a pawn target gets the speed
                // back, and a pawn that cannot swim gives up on a target in water
                let t = self.op(a, "MoveTarget");
                if self.is_live(t) {
                    let tl = self.vec_at(t, o.location);
                    self.set_vp(a, "Destination", tl);
                    if self.is_instance_of(t, o.class_pawn) {
                        self.set_fp(a, "DesiredSpeed", speed);
                        let roff = self.region_offsets().region;
                        let tz = self.u32_at(t, roff);
                        if !self.bp(a, "bCanSwim") && self.bp(tz, "bWaterZone") {
                            self.set_fp(a, "MoveTimer", -1.0);
                        }
                    }
                }
                Ok(Some(done))
            }
            _ => Ok(None),
        }
    }

    // ---- test moves and random destinations --------------------------------------------------------------------

    pub(crate) fn test_move(&mut self, a: u32, delta: Vec3, hit: &mut CheckResult) -> Res<()> {
        let rot = self.rot_pub(a);
        self.move_actor(a, delta, rot, hit, true, true, false, false)?;
        Ok(())
    }

    /// `APawn::walkMove`: a test step of a walking pawn (with a step-up and a ledge check). Returns 1 for success, 0 when
    /// blocked or too short, -1 when there is no usable floor after the step (the pawn is put back).
    pub fn walk_move(&mut self, a: u32, delta: Vec3, hit: &mut CheckResult, test_actor: u32, threshold: f32, adjust: bool) -> Res<i32> {
        let o = self.offsets();
        let start = self.vec_at(a, o.location);
        let grav = if self.zone_vec(a, "ZoneGravity")[2] > 0.0 { 1.0 } else { -1.0 };
        let step = self.fp(a, "MaxStepHeight");
        let (down_z, up_z) = (step * grav, -step * grav);
        let flat = [delta[0], delta[1], 0.0];
        self.test_move(a, flat, hit)?;
        if test_actor != 0 && hit.actor == test_actor {
            return Ok(5);
        }
        if hit.time < 1.0 {
            let t = hit.time;
            self.test_move(a, [0.0, 0.0, up_z], hit)?;
            self.test_move(a, [flat[0] * (1.0 - t), flat[1] * (1.0 - t), 0.0], hit)?;
            if test_actor != 0 && hit.actor == test_actor {
                return Ok(5);
            }
            self.test_move(a, [0.0, 0.0, down_z], hit)?;
            if test_actor != 0 && hit.actor == test_actor {
                return Ok(5);
            }
            if hit.time < 1.0 && hit.normal[2] < 0.7 {
                self.far_move_actor(a, start, true, true)?;
                return Ok(0);
            }
        }
        let after = self.vec_at(a, o.location);
        self.test_move(a, [0.0, 0.0, (step + 2.0) * grav], hit)?;
        if hit.time == 1.0 {
            self.far_move_actor(a, if adjust { start } else { after }, true, true)?;
            return Ok(-1);
        }
        if hit.normal[2] >= 0.7 {
            let moved = sub(self.vec_at(a, o.location), start);
            if dot(moved, moved) < threshold * threshold {
                if adjust {
                    self.far_move_actor(a, start, true, true)?;
                }
                return Ok(0);
            }
            return Ok(1);
        }
        self.far_move_actor(a, start, true, true)?;
        Ok(-1)
    }

    /// `APawn::flyMove`: a test step of a flying pawn; when blocked, the rest is tried after rising `MaxStepHeight`.
    /// 5 when it touches `goal`, 1 when it moved at least `threshold`, else 0 (put back when `adjust`).
    pub(crate) fn fly_move(&mut self, a: u32, delta: Vec3, goal: u32, threshold: f32, adjust: bool) -> Res<i32> {
        self.free_step(a, delta, goal, threshold, adjust, false)
    }

    /// `APawn::swimMove`: as `flyMove` in water; a step that leaves the water instead ends at the water line (0).
    pub(crate) fn swim_move(&mut self, a: u32, delta: Vec3, goal: u32, threshold: f32, adjust: bool) -> Res<i32> {
        self.free_step(a, delta, goal, threshold, adjust, true)
    }

    fn free_step(&mut self, a: u32, delta: Vec3, goal: u32, threshold: f32, adjust: bool, swim: bool) -> Res<i32> {
        let o = self.offsets();
        let start = self.vec_at(a, o.location);
        let up = [0.0, 0.0, self.fp(a, "MaxStepHeight")];
        let mut hit = CheckResult::none();
        self.test_move(a, delta, &mut hit)?;
        if goal != 0 && hit.actor == goal {
            return Ok(5);
        }
        if swim && !self.zone_b(a, "bWaterZone") {
            let loc = self.vec_at(a, o.location);
            let mut line = loc;
            self.find_water_line(a, start, &mut line);
            if line != loc {
                self.test_move(a, sub(line, loc), &mut hit)?;
            }
            return Ok(0);
        }
        if hit.time < 1.0 {
            let rest = mul(delta, 1.0 - hit.time);
            self.test_move(a, up, &mut hit)?;
            self.test_move(a, rest, &mut hit)?;
            if goal != 0 && hit.actor == goal {
                return Ok(5);
            }
        }
        let moved = sub(self.vec_at(a, o.location), start);
        if threshold * threshold <= dot(moved, moved) {
            return Ok(1);
        }
        if adjust {
            self.far_move_actor(a, start, true, true)?;
        }
        Ok(0)
    }

    /// `APawn::AIDirectionReachable`: walks (as a test) from the pawn's spot along `yaw`/`pitch` in short steps and
    /// reports a spot where the distance to `focus` lies between `min_dist` and `max_dist`.
    #[allow(clippy::too_many_arguments)]
    pub fn ai_direction_reachable(&mut self, a: u32, focus: Vec3, yaw: i32, pitch: i32, min_dist: f32, max_dist: f32, best: &mut Vec3) -> Res<bool> {
        let o = self.offsets();
        let (min_sq, max_sq) = (min_dist * min_dist, max_dist * max_dist);
        let old_loc = self.vec_at(a, o.location);
        let old_vel = self.vec_at(a, o.velocity);
        let mut step = self.cylinder(a).0.clamp(5.0, 25.0);
        let classify = |d: f32| if d < min_sq { -1 } else if d > max_sq { 1 } else { 0 };
        let dist_sq = |l: Vec3| dot(sub(l, focus), sub(l, focus));
        let water = self.zone_b(a, "bWaterZone");
        let phys = self.u8_at(a, o.physics);
        // modes: 1 walk (walking, or swimming out of water), 2 fly, 3 swim (in a water zone); any other physics
        // (falling...) tests nothing and leaves `best` as it is
        let (mode, dir) = if water {
            (3, crate::natives::rotate_vector([1.0, 0.0, 0.0], [pitch, yaw, 0], false))
        } else if phys == PHYS_WALKING || phys == PHYS_SWIMMING {
            (1, crate::natives::rotate_vector([1.0, 0.0, 0.0], [0, yaw, 0], false))
        } else if phys == PHYS_FLYING {
            (2, crate::natives::rotate_vector([1.0, 0.0, 0.0], [pitch, yaw, 0], false))
        } else {
            return Ok(false);
        };
        let mut found = false;
        *best = old_loc;
        let mut prev = classify(dist_sq(old_loc));
        let mut budget = 100;
        let mut ok = true;
        let mut hit = CheckResult::none();
        while ok {
            let delta = mul(dir, step);
            let mut r = match mode {
                1 => self.walk_move(a, delta, &mut hit, 0, 4.1, false)?,
                2 => self.fly_move(a, delta, 0, 4.1, false)?,
                _ => self.swim_move(a, delta, 0, 4.1, false)?,
            };
            if r != 1 {
                if self.zone_number(a) == 0 {
                    r = 0;
                } else if mode == 1 && r == -1 && self.fp(a, "MaxStepHeight") < step {
                    step = self.fp(a, "MaxStepHeight");
                    r = 1;
                }
            }
            ok = r == 1;
            let foot_off = self.region_offsets().foot_region;
            let foot_zone = self.u32_at(a, foot_off);
            if self.bp(foot_zone, "bPainZone") && self.op(foot_zone, "DamageType") != self.op(a, "ReducedDamageType") {
                ok = false;
            }
            let in_water = self.zone_b(a, "bWaterZone");
            if (mode == 3 && !in_water) || (mode != 3 && in_water) {
                ok = false;
            }
            budget -= 1;
            if budget < 0 || !ok {
                break;
            }
            let loc = self.vec_at(a, o.location);
            let cur = classify(dist_sq(loc));
            if cur == 0 {
                *best = loc;
                found = true;
                if prev > 0 {
                    ok = false;
                }
            } else if prev != cur {
                if prev != 0 {
                    *best = loc;
                    found = true;
                }
                ok = false;
            }
            prev = cur;
        }
        self.far_move_actor(a, old_loc, true, true)?;
        self.set_vec_at(a, o.velocity, old_vel);
        Ok(found)
    }

    /// `AActor::RandomBiasedRotation`: a rotation near (`yaw`, `pitch`); a bigger distribution value concentrates the
    /// result nearer the centre.
    pub fn random_biased_rotation(&mut self, yaw: i32, yaw_dist: f32, pitch: i32, pitch_dist: f32) -> [i32; 3] {
        let spread = |vm: &mut Vm, dist: f32, neg: f32, pos: f32| -> i32 {
            let d = (dist.clamp(0.0, 1.0) + 1.0) * 0.5;
            let r = vm.frand() * 2.0 - 1.0;
            let a = r.abs();
            let f = if a <= d { a * (1.0 - d) / d } else { (1.0 - d) + (a - d) * d / (1.0 - d) };
            (f * if r < 0.0 { neg } else { pos }) as i32
        };
        let dy = spread(self, yaw_dist, -32768.0, 32767.0);
        let dp = spread(self, pitch_dist, -16384.0, 16383.0);
        [dp + pitch, dy + yaw, 0]
    }

    /// `AIPickRandomDestination`: tries up to `tries` random headings (biased around the given yaw and pitch) and returns
    /// the first reachable spot between `min_dist` and `max_dist` away. `multiplier` shrinks the distances for a first
    /// look and then re-checks at that scaled distance.
    #[allow(clippy::too_many_arguments)]
    pub fn ai_pick_random_destination(&mut self, a: u32, min_dist: f32, max_dist: f32, yaw: i32, yaw_dist: f32, pitch: i32, pitch_dist: f32, tries: i32, multiplier: f32, dest: &mut Vec3) -> Res<bool> {
        let o = self.offsets();
        let mut tries = tries.max(1);
        let mut mult = multiplier;
        if mult < 0.0001 {
            mult = 0.0001;
        } else if mult >= 1.0 {
            mult = 1.0;
        }
        let loc = self.vec_at(a, o.location);
        let mut best = loc;
        let phys = self.u8_at(a, o.physics);
        let can_pitch = phys != PHYS_WALKING && (phys != PHYS_SWIMMING || self.zone_b(a, "bWaterZone"));
        let mut ok = false;
        while tries > 0 {
            let rot = self.random_biased_rotation(yaw, yaw_dist, pitch, pitch_dist);
            let p = if can_pitch { rot[0] } else { 0 };
            ok = self.ai_direction_reachable(a, loc, rot[1], p, min_dist / mult, max_dist / mult, &mut best)?;
            if ok {
                if mult < 1.0 {
                    let d = size(sub(best, loc)) * mult;
                    ok = self.ai_direction_reachable(a, loc, rot[1], p, d, d, &mut best)?;
                }
                if ok {
                    break;
                }
            }
            tries -= 1;
        }
        *dest = best;
        Ok(ok)
    }
}

//! Pawn perception natives: the AI's light model (`AIGetLightLevel`, `AIVisibility`) and `AICanSee`. Ports of the original
//! behaviour. Light comes from light actors and the zone's ambient light (not from lightmaps), as in the original.

use crate::ai_move::vec_to_rot;
use crate::collision::*;
use crate::VmError;
use crate::Vm;
use crate::Value;

type Res<T> = Result<T, VmError>;

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Brightness byte and saturation byte as one 0..1 intensity.
fn light_value(brightness: u8, saturation: u8) -> f32 {
    brightness as f32 * (1.0 / 255.0) * (saturation as f32 + 127.5) * (1.0 / 382.5)
}

/// Field-of-view falloff: 1 inside the view cone, fading to 0 across the target's angular width (angles in degrees).
fn fov_factor(angle_deg: f32, fov: f32, spread: f32) -> f32 {
    let mut a = angle_deg;
    while a <= -180.0 {
        a += 360.0;
    }
    while a > 180.0 {
        a -= 360.0;
    }
    let a = a.abs();
    let half = (fov - spread) * 0.5;
    if half + spread < a {
        0.0
    } else if half < a {
        1.0 - (a - half) / spread
    } else {
        1.0
    }
}

impl Vm {
    /// `AActor::AIGetLightLevel(loc)`: lit by every light actor in range that has a clear line to the spot (static
    /// lights only count when unobstructed), half of their sum plus twice the zone's ambient light, clamped to 0..1.
    pub fn ai_light_level(&mut self, a: u32, loc: Vec3) -> f32 {
        let o = self.offsets();
        let region_off = self.region_offsets().region;
        let zone = self.u32_at(a, region_off);
        let lit;
        if !self.bp(a, "bUnlit") {
            let mut sum = 0.0;
            let lights: Vec<u32> = (0..self.objects.len()).map(crate::inst_ref).filter(|&r| r != a && self.is_live(r) && self.class_of(r).map(|c| self.is_actor_class(c)).unwrap_or(false)).collect();
            for l in lights {
                if self.u8p(l, "LightType") == 0 || self.u8p(l, "LightBrightness") == 0 {
                    continue;
                }
                let radius = (self.u8p(l, "LightRadius") as f32 + 1.0) * 25.0;
                let lp = self.vec_at(l, o.location);
                let d = sub(loc, lp);
                let dist_sq = dot(d, d);
                if dist_sq >= radius * radius {
                    continue;
                }
                if self.bp(l, "bStatic") {
                    let hit = self.single_line_check(a, lp, loc, 6, [0.0; 3], 0);
                    if hit.time < 1.0 {
                        continue;
                    }
                }
                let falloff = 1.0 - (dist_sq / (radius * radius)).sqrt();
                let v = light_value(self.u8p(l, "LightBrightness"), self.u8p(l, "LightSaturation"));
                sum += v * falloff;
            }
            lit = sum * 0.5;
        } else {
            lit = 1.0;
        }
        let ambient = light_value(self.u8p(zone, "AmbientBrightness"), self.u8p(zone, "AmbientSaturation"));
        (ambient + ambient + lit).clamp(0.0, 1.0)
    }

    /// `AIVisibility(bIncludeVelocity)`: the light level at the actor, smoothed over 0.25 s steps and refreshed every
    /// half second, brightened by up to half when it moves faster than 30 units a second.
    pub fn ai_visibility(&mut self, a: u32, include_velocity: bool) -> f32 {
        let o = self.offsets();
        // `Level.TimeSeconds` (LevelInfo+0x478)
        let now = self.fp(self.level, "TimeSeconds");
        let loc = self.vec_at(a, o.location);
        let since = now - self.fp(a, "VisUpdateTime");
        let mut v;
        if since <= 0.5 {
            if since <= 0.25 {
                let (cur, last) = (self.fp(a, "CurrentVisibility"), self.fp(a, "LastVisibility"));
                v = (since / 0.25) * (cur - last) + last;
            } else {
                let cur = self.fp(a, "CurrentVisibility");
                self.set_fp(a, "LastVisibility", cur);
                self.set_fp(a, "VisUpdateTime", now);
                let lvl = self.ai_light_level(a, loc);
                self.set_fp(a, "CurrentVisibility", lvl);
                v = self.fp(a, "LastVisibility");
            }
        } else {
            self.set_fp(a, "VisUpdateTime", now);
            v = self.ai_light_level(a, loc);
            self.set_fp(a, "CurrentVisibility", v);
            self.set_fp(a, "LastVisibility", v);
        }
        if include_velocity {
            let vel = self.vec_at(a, o.velocity);
            let speed = dot(vel, vel).sqrt();
            let k = ((speed - 30.0) / 170.0).clamp(0.0, 1.0);
            v = k * v * 0.5 + v;
        }
        v.clamp(0.0, 1.0)
    }

    /// Is the line from `from` to `to` free of sight blockers? The world blocks; actors block when `bBlockSight`
    /// (and are not hidden); the looker's owner chain and the target never block.
    fn sight_clear(&mut self, looker: u32, target: u32, to: Vec3, from: Vec3) -> bool {
        let o = self.offsets();
        let level = self.level;
        for h in self.multi_line_check(from, to, [0.0; 3], true, true, 0) {
            let mut owner = looker;
            let mut skip = false;
            while owner != 0 {
                if owner == h.actor {
                    skip = true;
                    break;
                }
                owner = self.u32_at(owner, o.owner);
            }
            if skip || h.actor == target {
                continue;
            }
            if h.actor == level {
                return false;
            }
            if self.bp(h.actor, "bBlockSight") && !self.bp(h.actor, "bHidden") {
                return false;
            }
        }
        true
    }

    /// `APawn::AICanSee`: how well this pawn sees `other` (0..1): angular size, field of view, light, then line of sight.
    #[allow(clippy::too_many_arguments)]
    pub fn ai_can_see(&mut self, a: u32, other: u32, visibility: f32, check_visibility: bool, check_dir: bool, check_cylinder: bool, check_los: bool) -> Res<f32> {
        let o = self.offsets();
        if !self.is_live(other) || visibility <= 0.0 || !self.bp(other, "bDetectable") {
            return Ok(0.0);
        }
        let eye_h = self.fp(a, "BaseEyeHeight");
        let loc = self.vec_at(a, o.location);
        let eye = [loc[0], loc[1], loc[2] + eye_h];
        let ol = self.vec_at(other, o.location);
        let d = sub(ol, eye);
        let mut dist_sq = dot(d, d);
        if dist_sq < 1.0 {
            dist_sq = 1.0;
        }
        let (r, h) = self.cylinder(other);
        let size = (h * h + r * r) / dist_sq;
        if size <= self.fp(a, "MinAngularSize") {
            return Ok(0.0);
        }
        let mut check_cylinder = check_cylinder;
        if size < 0.0003046792916483 {
            check_cylinder = false;
        }
        let mut vis = visibility * size * 64.0;
        if check_dir && vis > 0.0 {
            let aspect = self.fp(a, "AspectRatio");
            let hfov = self.fp(a, "AIHorizontalFov");
            let vfov = if aspect > 0.0 { hfov / aspect } else { 0.0 };
            let h_spread = (r * r / dist_sq).sqrt().atan() * 114.59155902616465;
            let v_spread = (h * h / dist_sq).sqrt().atan() * 114.59155902616465;
            let base = if self.is_instance_of(a, o.class_player_pawn) { self.rot_prop(a, "ViewRotation") } else { self.rot_of(a) };
            let add = self.rot_prop(a, "AIAddViewRotation");
            let view = [base[0] + add[0], base[1] + add[1], base[2] + add[2]];
            let local = crate::natives::rotate_vector(d, view, true);
            let rot = vec_to_rot(local);
            let to_deg = |u: i32| u as f32 * 360.0 * 1.52587890625e-05;
            let fh = fov_factor(to_deg(rot[1]), hfov, h_spread);
            let mut fv = fov_factor(to_deg(rot[0]), vfov, v_spread);
            if dist_sq < 22500.0 && fv < 0.75 {
                fv = 0.75;
            }
            vis = fv * (fh * vis);
        }
        if check_visibility && vis > 0.0 {
            vis *= self.ai_visibility(other, true);
        }
        vis = (vis - self.fp(a, "VisibilityThreshold")).clamp(0.0, 1.0);
        if !check_los || vis <= 0.0 {
            return Ok(vis);
        }
        let seen = if !check_cylinder {
            self.sight_clear(a, other, ol, eye)
        } else {
            let player = self.is_instance_of(other, o.class_player_pawn);
            let mut seen = false;
            if player {
                let eh = self.fp(other, "EyeHeight");
                seen = self.sight_clear(a, other, [ol[0], ol[1], ol[2] + eh], eye);
            }
            if !seen {
                seen = self.sight_clear(a, other, [ol[0], ol[1], ol[2] + h], eye) || self.sight_clear(a, other, [ol[0], ol[1], ol[2] - h], eye);
            }
            if !seen && !player {
                let flat = {
                    let n = crate::collision::safe_normal([d[0], d[1], 0.0]);
                    [n[0], n[1]]
                };
                seen = self.sight_clear(a, other, [ol[0] + flat[1] * r, ol[1] - flat[0] * r, ol[2]], eye)
                    || self.sight_clear(a, other, [ol[0] - flat[1] * r, ol[1] + flat[0] * r, ol[2]], eye);
            }
            seen
        };
        Ok(if seen { vis } else { 0.0 })
    }

    /// `APawn::LineOfSightTo(Other, bPeripheral, bIgnoreDistance)` (Engine.dll 103beb50). Script `LineOfSightTo` calls it
    /// with `bPeripheral` false and `CanSee` with it true. Only the level's BSP blocks the rays (`UModel::FastLineCheck`):
    /// actors and movers do not. Side effects: flips `bLOSflag` (peripheral calls on a non-enemy), writes `Stimulus`
    /// (peripheral calls) and `LastSeeingPos`/`LastSeenPos` (when it sees its enemy).
    pub fn line_of_sight_to(&mut self, a: u32, other: u32, mut peripheral: bool, ignore_distance: bool) -> bool {
        if other == 0 {
            return false;
        }
        let o = self.offsets();
        let enemy = match self.get_prop(a, "Enemy", 0) {
            Ok(crate::Value::Obj(e)) => e,
            _ => 0,
        };
        if other == enemy {
            peripheral = false;
        } else if peripheral {
            let f = self.bp(a, "bLOSflag");
            self.set_bp(a, "bLOSflag", !f);
        }
        let loc = self.vec_at(a, o.location);
        let ol = self.vec_at(other, o.location);
        let delta = sub(ol, loc);
        let dist_sq = dot(delta, delta);
        let is_player = self.bp(a, "bIsPlayer");
        let other_pawn = self.is_instance_of(other, o.class_pawn);
        let visibility = if other_pawn { self.u8p(other, "Visibility") as f32 } else { 0.0 };
        // `dist_sq` becomes the "effective" squared distance in the peripheral branch
        let mut dist_sq = dist_sq;
        let max_sq;
        if !peripheral {
            max_sq = if other_pawn {
                let s = ((visibility + 16.0) * 0.015).min(1.0);
                if is_player { (s * 5000.0 * s * 5000.0).min(25_000_000.0) } else { (s * 4000.0 * s * 4000.0).min(16_000_000.0) }
            } else if is_player {
                16_000_000.0
            } else {
                9_000_000.0
            };
            if max_sq < dist_sq && !ignore_distance {
                return false;
            }
        } else {
            let sight = self.fp(a, "SightRadius");
            max_sq = if other_pawn {
                let s = (visibility * 0.0078125).min(1.0) * sight;
                let m = s * s;
                if is_player { m.min(16_000_000.0) } else { m.min(12_000_000.0) }
            } else {
                sight * sight
            };
            if max_sq < dist_sq {
                return false;
            }
            let dir = crate::collision::safe_normal(delta);
            let rot = self.rot_of(a);
            let forward = crate::natives::rotate_vector(dir, rot, true)[0];
            let s = forward - self.fp(a, "PeripheralVision");
            let k = if s > 0.0 { 0.8f64 } else { 0.17f64 };
            let stim = (s as f64 * k + 0.2f64) as f32;
            self.set_fp(a, "Stimulus", stim);
            if stim <= 0.0 {
                return false;
            }
            let dz = (ol[2] - loc[2]).abs();
            let skill = (self.fp(a, "Skill") + 1.0).max(1.0);
            dist_sq = ((dz / skill) * (dz / skill) + dist_sq) / (stim * stim);
            if max_sq < dist_sq && !ignore_distance {
                return false;
            }
            self.set_fp(a, "Stimulus", 1.0);
        }
        let eye = [loc[0], loc[1], loc[2] + self.fp(a, "BaseEyeHeight")];
        let fast = |vm: &mut Vm, end: Vec3, start: Vec3| vm.bsp.as_ref().map(|b| b.fast_line_check(end, start)).unwrap_or(true);
        if other == enemy {
            if fast(self, ol, eye) || fast(self, ol, loc) {
                let seen = self.vec_at(other, o.location);
                self.set_vp(a, "LastSeeingPos", loc);
                self.set_vp(a, "LastSeenPos", seen);
                return true;
            }
            if 1_000_000.0 < dist_sq && !ignore_distance {
                return false;
            }
        } else if 1_000_000.0 < dist_sq && !ignore_distance {
            // far away: only pawns get the cheap alternating test; everything else is one ray to its middle
            if other_pawn {
                if !self.bp(a, "bLOSflag") && max_sq * 0.5 < dist_sq {
                    return false;
                }
                if !is_player && self.frand() < 0.5 {
                    return false;
                }
            }
            return fast(self, ol, eye);
        }
        if !peripheral || !self.bp(a, "bLOSflag") {
            let h = self.fp(other, "CollisionHeight");
            if fast(self, [ol[0], ol[1], ol[2] + h * 0.8], eye) {
                return true;
            }
        }
        if 250_000.0 < dist_sq && !ignore_distance {
            return false;
        }
        if !other_pawn {
            return false;
        }
        // the four corners of the target's box at its height, tried in an order the engine derives from each corner's
        // squared distance to the WORLD ORIGIN (not to the looker): the two extremes are skipped
        let r = self.fp(other, "CollisionRadius");
        let pts = [
            [ol[0] - r, ol[1] + r, ol[2]],
            [ol[0] + r, ol[1] + r, ol[2]],
            [ol[0] - r, ol[1] - r, ol[2]],
            [ol[0] + r, ol[1] - r, ol[2]],
        ];
        let mag = |p: Vec3| p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
        let (mut lo, mut hi) = (mag(pts[0]), mag(pts[0]));
        let (mut lo_i, mut hi_i) = (0usize, 0usize);
        for (i, p) in pts.iter().enumerate().skip(1) {
            let m = mag(*p);
            if m > hi {
                hi = m;
                hi_i = i;
            } else if m < lo {
                lo = m;
                lo_i = i;
            }
        }
        let mut skip_next = self.bp(a, "bLOSflag");
        for (i, p) in pts.iter().enumerate() {
            if i == lo_i || i == hi_i {
                continue;
            }
            if skip_next && peripheral {
                skip_next = false;
                continue;
            }
            skip_next = true;
            if fast(self, *p, eye) {
                return true;
            }
        }
        false
    }

    /// `AActor::TestCanSeeMe`: can the local player `player` see actor `a`? The player's view target always can; others
    /// must be within about 1400 units (`(radius + 3.6) * 100000` squared), not more than 60 degrees off the view axis
    /// (the original tests the square of the cosine, so an actor straight behind passes too) and in line of sight.
    pub fn test_can_see_me(&mut self, a: u32, player: u32) -> bool {
        if player == 0 || !self.is_live(player) {
            return false;
        }
        let o = self.offsets();
        if self.op(player, "ViewTarget") == a {
            return true;
        }
        let (pl, al) = (self.vec_at(player, o.location), self.vec_at(a, o.location));
        let rel = sub(al, pl);
        let d2 = dot(rel, rel);
        if d2 < (self.fp(a, "CollisionRadius") + 3.6) * 100_000.0 {
            if !self.bp(player, "bBehindView") {
                let view = self.rot_prop(player, "ViewRotation");
                let local = crate::natives::rotate_vector(rel, view, true);
                if local[0] * local[0] < d2 * 0.25 {
                    return false;
                }
            }
            return self.line_of_sight_to(player, a, false, false);
        }
        false
    }

    /// `PlayerCanSeeMe` in a standalone game: does the local player (the viewport's actor) see `a`?
    pub fn player_can_see_me(&mut self, a: u32) -> bool {
        let o = self.offsets();
        let Some(player) = (0..self.objects.len()).map(crate::inst_ref).find(|&r| self.is_live(r) && self.is_instance_of(r, o.class_player_pawn) && self.op(r, "Player") != 0) else { return false };
        self.test_can_see_me(a, player)
    }

    /// `Pawn.PickTarget`: of the pawns in `Level.PawnList` (not `a`, alive, `bProjTarget`; in a team game not a team
    /// mate), the one in front of `proj_start` along `fire_dir`, closer than 2000 units, nearest to the aim line (cosine
    /// above `best_aim`) and in line of sight. Nothing without a game info. Returns it with the new best aim and distance.
    pub fn pick_target(&mut self, a: u32, best_aim: f32, best_dist: f32, fire_dir: Vec3, proj_start: Vec3) -> (u32, f32, f32) {
        let mut best = (0, best_aim, best_dist);
        let game = self.op(self.level, "Game");
        if game == 0 {
            return best;
        }
        let team_game = self.bp(game, "bTeamGame");
        let my_pri = self.op(a, "PlayerReplicationInfo");
        for p in self.pawn_list() {
            if p == a || self.get_prop(p, "Health", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0) <= 0 || !self.bp(p, "bProjTarget") {
                continue;
            }
            let pri = self.op(p, "PlayerReplicationInfo");
            if team_game && my_pri != 0 && pri != 0 {
                let team = |vm: &mut Vm, r: u32| vm.get_prop(r, "Team", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0);
                if team(self, my_pri) == team(self, pri) {
                    continue;
                }
            }
            self.consider_target(a, p, fire_dir, proj_start, &mut best);
        }
        best
    }

    /// `Pawn.PickAnyTarget`: as `PickTarget`, over every actor of the level that is a projectile target (`bProjTarget`)
    /// and not a pawn (decorations, ...).
    pub fn pick_any_target(&mut self, a: u32, best_aim: f32, best_dist: f32, fire_dir: Vec3, proj_start: Vec3) -> (u32, f32, f32) {
        let o = self.offsets();
        let mut best = (0, best_aim, best_dist);
        for p in self.live_actors() {
            if !self.bp(p, "bProjTarget") || self.is_instance_of(p, o.class_pawn) {
                continue;
            }
            self.consider_target(a, p, fire_dir, proj_start, &mut best);
        }
        best
    }

    /// The test both target pickers share: ahead (positive dot product), within 2000 units, a larger aim cosine than the
    /// best so far and in line of sight of `a` (no peripheral vision).
    fn consider_target(&mut self, a: u32, p: u32, fire_dir: Vec3, proj_start: Vec3, best: &mut (u32, f32, f32)) {
        let o = self.offsets();
        let d = sub(self.vec_at(p, o.location), proj_start);
        let along = dot(d, fire_dir);
        if along <= 0.0 {
            return;
        }
        let dist_sq = dot(d, d);
        if dist_sq >= 4_000_000.0 {
            return;
        }
        let dist = dist_sq.sqrt();
        let aim = along / dist;
        if aim > best.1 && self.line_of_sight_to(a, p, false, false) {
            *best = (p, aim, dist);
        }
    }

    /// `Pawn.FindStairRotation`: the view pitch a walking player gets when looking up or down stairs (`bLookUpStairs`).
    /// Traced from the eye along the horizontal view direction (movers and world only, a box of the collision radius and 1
    /// unit high unless stated): forward over twice the height `h` (collision height + eye height); with nothing within
    /// 0.8 h, the target is 0. Otherwise from half way to the hit, down 3 h, measures the floor drop `a`; then from the hit
    /// point down 3 h measures `b`. Floor ahead higher (`a` < 0.7 h - 6) and higher again further on (`b` < `a` - 10):
    /// pitch 5400 (else the current pitch if it is positive, otherwise 0). Floor ahead lower (`a` > 0.7 h + 6), a clear
    /// line (zero extent) from the body to 0.9 of the hit distance and lower again further on (`b` > `a` + 10): -5000
    /// (else the current pitch if it is negative, otherwise 0). The returned pitch moves from the current one towards
    /// the target by `min(1, dt * k)`, `k` = 8000 / gap (integer division) for gaps below 1000, 8 above. With `dt` above
    /// 0.33 the current pitch comes back unchanged; otherwise a pitch above 32768 is first folded into -32768..32768.
    pub fn find_stair_rotation(&mut self, a: u32, dt: f32) -> Result<i32, VmError> {
        let o = self.offsets();
        let mut view = self.rot_prop(a, "ViewRotation");
        if dt as f64 > 0.33 {
            return Ok(view[0]);
        }
        if view[0] > 0x8000 {
            view[0] = (view[0] & 0xffff) - 0x10000;
            self.set_prop(a, "ViewRotation", 0, Value::Rotator(view))?;
        }
        let pitch = view[0];
        let dir = crate::natives::rotate_vector([1.0, 0.0, 0.0], [0, view[1], view[2]], false);
        let loc = self.vec_at(a, o.location);
        let eye = self.fp(a, "BaseEyeHeight");
        let radius = self.fp(a, "CollisionRadius");
        let view_point = [loc[0], loc[1], loc[2] + eye];
        let h = self.fp(a, "CollisionHeight") + eye;
        let ext = [radius, radius, 1.0];
        let flags = TRACE_MOVERS | TRACE_LEVEL;
        let along = |d: f32| [view_point[0] + dir[0] * d, view_point[1] + dir[1] * d, view_point[2] + dir[2] * d];
        let hit = self.single_line_check(a, along(2.0 * h), view_point, flags, ext, 0);
        let dist = 2.0 * h * hit.time;
        let mut target = 0;
        if dist > h * 0.8 {
            let down = h * 3.0;
            let mid = along(dist * 0.5);
            let hit = self.single_line_check(a, [mid[0], mid[1], mid[2] - down], mid, flags, ext, 0);
            if hit.time < 1.0 {
                let drop_a = down * hit.time;
                let level = h * 0.7;
                let ahead = along(dist);
                let ahead_down = [ahead[0], ahead[1], ahead[2] - down];
                if level - 6.0 > drop_a {
                    // up the stairs
                    let hit = self.single_line_check(a, ahead_down, ahead, flags, ext, 0);
                    target = pitch.max(0);
                    if drop_a - 10.0 > down * hit.time {
                        target = 5400;
                    }
                } else if level + 6.0 < drop_a {
                    // down the stairs, when the way ahead is clear at body height
                    let d = dist * 0.9;
                    let body_ahead = [loc[0] + dir[0] * d, loc[1] + dir[1] * d, loc[2] + dir[2] * d];
                    let clear = self.single_line_check(a, body_ahead, loc, flags, [0.0; 3], 0);
                    if clear.time == 1.0 {
                        let hit = self.single_line_check(a, ahead_down, ahead, flags, ext, 0);
                        target = pitch.min(0);
                        if drop_a + 10.0 < down * hit.time {
                            target = -5000;
                        }
                    }
                }
            }
        }
        let gap = (pitch - target).abs();
        if gap <= 0 {
            return Ok(target);
        }
        // x87 arithmetic (double precision), truncated
        let rate = if gap < 1000 { (8000 / gap) as f64 } else { 8.0 };
        let f = (rate * dt as f64).min(1.0);
        Ok(((1.0 - f) * pitch as f64 + target as f64 * f) as i32)
    }
}

//! Navigation: direct reachability tests (`pointReachable`, `actorReachable`, `Reachable` with the walk/jump/fly/swim
//! tests, ported from Engine.dll) and the reach spec data. The path search itself (`FindPathTo`, `FindPathToward`) is
//! in `route.rs`.

use crate::collision::*;
use crate::physics::*;
use crate::{Vm, VmError};

type Res<T> = Result<T, VmError>;

/// A precomputed link between two navigation points (`Level.ReachSpecs`), with references resolved to actors.
#[derive(Debug, Clone)]
pub struct ReachSpecRt {
    pub distance: i32,
    pub start: u32,
    pub end: u32,
    pub radius: i32,
    pub height: i32,
    pub flags: i32,
    pub pruned: bool,
}

pub const R_WALK: i32 = 1;
pub const R_FLY: i32 = 2;
pub const R_SWIM: i32 = 4;
pub const R_JUMP: i32 = 8;
pub const R_DOOR: i32 = 16;
pub const R_SPECIAL: i32 = 32;
pub const R_PLAYERONLY: i32 = 64;

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Vm {
    /// Puts a navigation point at the head of `Level.NavigationPointList` (linked through `nextNavigationPoint`), as the
    /// engine does when one is created; scripts walk this list.
    pub fn register_nav_point(&mut self, a: u32) {
        let Some(nav) = self.reg.class_by_name("NavigationPoint") else { return };
        if self.level == 0 || !self.class_of(a).map(|c| self.is_a(c, nav)).unwrap_or(false) {
            return;
        }
        let head = self.op(self.level, "NavigationPointList");
        if head == a {
            return;
        }
        self.set_u32_prop(a, "nextNavigationPoint", head);
        self.set_u32_prop(self.level, "NavigationPointList", a);
    }

    /// All live navigation points (the original walks `Level.NavigationPointList`).
    pub fn nav_points(&mut self) -> Vec<u32> {
        let Some(nav) = self.reg.class_by_name("NavigationPoint") else { return Vec::new() };
        (0..self.objects.len()).map(crate::inst_ref).filter(|&r| self.is_live(r) && self.class_of(r).map(|c| self.is_a(c, nav)).unwrap_or(false)).collect()
    }

    fn zone_is_pain_for(&mut self, pawn: u32, zone: u32) -> bool {
        self.bp(zone, "bPainZone") && self.op(zone, "DamageType") != self.op(pawn, "ReducedDamageType")
    }

    /// `APawn::walkReachable` (103c1b70): walks test steps of the pawn's radius (128 for jumpers with a smaller radius)
    /// toward `dest`, the last one with an 8-unit threshold, until within `threshold` horizontally. Arrival counts when
    /// the height difference is under the pawn's (or the goal's, if taller) height, or, on a slope (last floor normal
    /// 0.7..0.95), when the difference fits the slope over the radii. A blocked step makes a flyer try `flyReachable`, a
    /// jumper try `FindBestJump` from where the floor ended, and others retry with `MaxStepHeight` steps; a hurting
    /// floor fails, water hands over to `swimReachable` (swimmers outside pain zones) or ends the test. A
    /// `WarpZoneMarker` goal counts as reached when the pawn is in its zone. The pawn and its velocity are restored.
    pub fn walk_reachable(&mut self, a: u32, dest: Vec3, threshold: f32, goal: u32) -> Res<bool> {
        let o = self.offsets();
        let thr_sq = threshold * threshold;
        let start_loc = self.vec_at(a, o.location);
        let start_vel = self.vec_at(a, o.velocity);
        let radius = self.cylinder(a).0;
        let mut step = if self.bp(a, "bCanJump") { if radius <= 128.0 { 128.0 } else { radius } } else { radius };
        let step_sq = step * step;
        let mut height = self.cylinder(a).1;
        if goal != 0 {
            height = height.max(self.cylinder(goal).1);
        }
        let mut success = false;
        let mut budget = 100;
        let mut hit = CheckResult::none();
        let mut r = 1;
        while r == 1 {
            let loc = self.vec_at(a, o.location);
            let d = sub(dest, loc);
            let mut flat = [d[0], d[1], 0.0];
            let d2 = d[0] * d[0] + d[1] * d[1];
            if d[2] > height && (d[2] - height) * (d[2] - height) * 0.8 > d2 {
                break; // too far above
            }
            if d2 <= thr_sq {
                r = 0;
                if d[2].abs() < height {
                    success = true;
                } else {
                    let nz = hit.normal[2] as f64;
                    if nz < 0.95 && nz > 0.7 {
                        let slope = (1.0 / (nz * nz) - 1.0).sqrt();
                        let (pr, ph) = self.cylinder(a);
                        if d[2] < 0.0 && (pr as f64 * slope + ph as f64) > -d[2] as f64 {
                            success = true;
                        } else {
                            let gr = if goal != 0 { self.cylinder(goal).0 } else { 46.0 };
                            if pr < gr && ((gr + 15.0 - pr) as f64 * slope + height as f64) > d[2] as f64 {
                                success = true;
                            }
                        }
                    }
                }
            } else {
                r = if step_sq <= d2 {
                    flat = crate::collision::safe_normal(flat);
                    self.walk_move(a, [flat[0] * step, flat[1] * step, flat[2] * step], &mut hit, goal, 4.1, false)?
                } else {
                    self.walk_move(a, flat, &mut hit, goal, 8.0, false)?
                };
                if r == 5 {
                    r = 0;
                    success = true;
                } else if r != 1 {
                    if self.zone_number(a) == 0 {
                        r = 0;
                        success = false;
                    } else if self.bp(a, "bCanFly") {
                        r = 0;
                        success = self.fly_reachable(a, dest, threshold, goal)?;
                    } else if self.bp(a, "bCanJump") {
                        if r == -1 {
                            let dir = crate::collision::safe_normal(flat);
                            let gs = self.fp(a, "GroundSpeed");
                            r = if self.find_best_jump(a, dest, [dir[0] * gs, dir[1] * gs, dir[2] * gs], true)?.0 { 1 } else { 0 };
                        }
                    } else if r == -1 && step > self.fp(a, "MaxStepHeight") {
                        step = self.fp(a, "MaxStepHeight");
                        r = 1;
                    }
                }
                let foot = { let off = self.region_offsets().foot_region; self.u32_at(a, off) };
                if self.zone_is_pain_for(a, foot) {
                    r = 0;
                    success = false;
                }
                let zone = { let off = self.region_offsets().region; self.u32_at(a, off) };
                if self.bp(zone, "bWaterZone") {
                    r = 0;
                    if self.bp(a, "bCanSwim") && !self.zone_is_pain_for(a, zone) {
                        success = self.swim_reachable(a, dest, threshold, goal)?;
                    }
                }
            }
            budget -= 1;
            if budget < 0 {
                r = 0;
            }
        }
        if !success && goal != 0 && self.reg.class_by_name("WarpZoneMarker").map(|c| self.class_of(goal).map(|k| self.is_a(k, c)).unwrap_or(false)).unwrap_or(false) {
            let zone = { let off = self.region_offsets().region; self.u32_at(a, off) };
            success = zone == self.op(goal, "markedWarpZone");
        }
        self.far_move_actor(a, start_loc, true, true)?;
        self.set_vec_at(a, o.velocity, start_vel);
        Ok(success)
    }

    /// `APawn::SuggestJumpVelocity`: horizontal aim at `dest` for a jump starting with vertical speed `vel[2]`: the arc
    /// is followed in 0.05 s steps (zone gravity, -100 unless it pulls down) until it is back at the destination height
    /// and falling; the horizontal speed covers the distance in that time, at most `GroundSpeed`.
    pub fn suggest_jump_velocity(&mut self, a: u32, dest: Vec3, vel: Vec3) -> Vec3 {
        let o = self.offsets();
        let loc = self.vec_at(a, o.location);
        let mut g = self.zone_vec(a, "ZoneGravity")[2] as f64;
        if g >= 0.0 {
            g = -100.0;
        }
        let dz = (dest[2] - loc[2]) as f64;
        let mut t = 0.0f64;
        let start_vz = vel[2];
        let mut vz = vel[2] as f64;
        let mut z = 0.0f64;
        while z > dz || vz > 0.0 {
            vz = (vz + g * 0.05) as f32 as f64;
            t = (t + 0.05) as f32 as f64;
            z += vz * 0.05;
        }
        if vz.abs() > 1.0 {
            t -= (z - dz) / vz;
        }
        let gs = self.fp(a, "GroundSpeed") as f64;
        let mut v = [dest[0] - loc[0], dest[1] - loc[1], 0.0];
        let speed = if t as f32 > 0.0 {
            let len = ((v[0] * v[0] + v[1] * v[1]) as f64).sqrt();
            if len > 0.0 {
                let k = 1.0 / len;
                v = [(v[0] as f64 * k) as f32, (v[1] as f64 * k) as f32, 0.0];
            }
            (len / t as f32 as f64).min(gs)
        } else {
            v = crate::collision::safe_normal(v);
            gs
        };
        [(speed * v[0] as f64) as f32, (speed * v[1] as f64) as f32, start_vz]
    }

    /// `APawn::FindBestJump`: jumps (`JumpZ` up, aimed by `SuggestJumpVelocity`) and follows the fall (`jumpLanding`);
    /// good when the landing is more than 8 units nearer to `dest`, not in a hurting zone, not in water for a non-swimmer
    /// and less than 350 units below the start. The pawn stays at the landing when `keep`. Returns (good, landing).
    pub fn find_best_jump(&mut self, a: u32, dest: Vec3, vel: Vec3, keep: bool) -> Res<(bool, Vec3)> {
        let o = self.offsets();
        let start = self.vec_at(a, o.location);
        let jump_z = self.fp(a, "JumpZ");
        let v = self.suggest_jump_velocity(a, dest, [vel[0], vel[1], jump_z]);
        let landing = self.jump_landing(a, v, true)?;
        let loc = self.vec_at(a, o.location);
        let foot = { let off = self.region_offsets().foot_region; self.u32_at(a, off) };
        let zone = { let off = self.region_offsets().region; self.u32_at(a, off) };
        let good = if self.zone_is_pain_for(a, foot) || (!self.bp(a, "bCanSwim") && self.bp(zone, "bWaterZone")) {
            false
        } else {
            let len = |p: Vec3| {
                let d = [(dest[0] - p[0]) as f64, (dest[1] - p[1]) as f64, (dest[2] - p[2]) as f64];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            };
            let gained = len(start) as f32 - len(loc) as f32;
            if start[2] - loc[2] < 350.0 {
                gained as f64 > 8.0
            } else {
                self.log.push("FindBestJump: jump would drop too far".into());
                false
            }
        };
        if !keep {
            self.far_move_actor(a, start, true, true)?;
        }
        Ok((good, landing))
    }

    /// `APawn::flyReachable` (103c1190): straight `flyMove` steps of `max(radius, 200)` (the last with an 8-unit
    /// threshold) until within `threshold` and the pawn's height; entering water hands over to `swimReachable` (for
    /// swimmers outside pain zones) or ends the test.
    pub(crate) fn fly_reachable(&mut self, a: u32, dest: Vec3, threshold: f32, goal: u32) -> Res<bool> {
        self.free_reachable(a, dest, threshold, goal, false)
    }

    /// `APawn::swimReachable` (103c15a0): as `flyReachable` with `swimMove`; leaving the water ends the test, except
    /// that a flyer goes on with `flyReachable`, and a walker whose goal is less than `MaxStepHeight` + 50 above it
    /// rises out (by its height plus `MaxStepHeight`, or to the goal height) and, if that is clear, goes on with
    /// `flyReachable`. A pain zone fails.
    pub(crate) fn swim_reachable(&mut self, a: u32, dest: Vec3, threshold: f32, goal: u32) -> Res<bool> {
        self.free_reachable(a, dest, threshold, goal, true)
    }

    fn free_reachable(&mut self, a: u32, dest: Vec3, threshold: f32, goal: u32, swim: bool) -> Res<bool> {
        let o = self.offsets();
        let thr_sq = threshold * threshold;
        let start_loc = self.vec_at(a, o.location);
        let start_vel = self.vec_at(a, o.velocity);
        let step = self.cylinder(a).0.max(200.0);
        let step_sq = step * step;
        let mut success = false;
        let mut budget = 100;
        let mut r = 1;
        while r != 0 {
            let loc = self.vec_at(a, o.location);
            let d = sub(dest, loc);
            let d2 = dot(d, d);
            if d2 <= thr_sq && d[2].abs() <= self.cylinder(a).1 {
                r = 0;
                success = true;
            } else {
                let (delta, thr) = if step_sq <= d2 { ({ let n = crate::collision::safe_normal(d); [n[0] * step, n[1] * step, n[2] * step] }, 4.1) } else { (d, 8.0) };
                r = if swim { self.swim_move(a, delta, goal, thr, false)? } else { self.fly_move(a, delta, goal, thr, false)? };
                if r == 5 {
                    r = 0;
                    success = true;
                }
                let zone = {
                    let off = self.region_offsets().region;
                    self.u32_at(a, off)
                };
                if swim {
                    if !self.bp(zone, "bWaterZone") {
                        r = 0;
                        if self.bp(a, "bCanFly") {
                            success = self.fly_reachable(a, dest, threshold, goal)?;
                        } else if self.bp(a, "bCanWalk") && dest[2] < self.fp(a, "MaxStepHeight") + self.vec_at(a, o.location)[2] + 50.0 {
                            let now = self.vec_at(a, o.location);
                            let rise = (self.cylinder(a).1 + self.fp(a, "MaxStepHeight")).max(dest[2] - now[2]);
                            let mut hit = CheckResult::none();
                            self.test_move(a, [0.0, 0.0, rise], &mut hit)?;
                            if hit.time == 1.0 {
                                success = self.fly_reachable(a, dest, threshold, goal)?;
                            }
                        }
                    } else if self.zone_is_pain_for(a, zone) {
                        r = 0;
                        success = false;
                    }
                } else if r != 0 && self.bp(zone, "bWaterZone") {
                    r = 0;
                    if self.bp(a, "bCanSwim") && !self.zone_is_pain_for(a, zone) {
                        success = self.swim_reachable(a, dest, threshold, goal)?;
                    }
                }
            }
            budget -= 1;
            if budget < 0 {
                r = 0;
            }
        }
        if !success && goal != 0 && self.reg.class_by_name("WarpZoneMarker").map(|c| self.class_of(goal).map(|k| self.is_a(k, c)).unwrap_or(false)).unwrap_or(false) {
            let zone = {
                let off = self.region_offsets().region;
                self.u32_at(a, off)
            };
            success = zone == self.op(goal, "markedWarpZone");
        }
        self.far_move_actor(a, start_loc, true, true)?;
        self.set_vec_at(a, o.velocity, start_vel);
        Ok(success)
    }

    /// `APawn::Reachable`: dispatches on the pawn's zone and physics (anything but walking, swimming or flying outside
    /// water is not reachable).
    pub fn reachable(&mut self, a: u32, dest: Vec3, threshold: f32, goal: u32) -> Res<bool> {
        let o = self.offsets();
        if self.zone_b(a, "bWaterZone") {
            return self.swim_reachable(a, dest, threshold, goal);
        }
        match self.u8_at(a, o.physics) {
            PHYS_WALKING | PHYS_SWIMMING => self.walk_reachable(a, dest, threshold, goal),
            PHYS_FLYING => self.fly_reachable(a, dest, threshold, goal),
            _ => Ok(false),
        }
    }

    /// Snaps `p` to the nearest spot the pawn fits at (a test move there), restoring the pawn afterwards.
    fn settle_point(&mut self, a: u32, p: Vec3) -> Res<Vec3> {
        let o = self.offsets();
        let old = self.vec_at(a, o.location);
        if self.far_move_actor(a, p, true, false)? {
            let at = self.vec_at(a, o.location);
            self.far_move_actor(a, old, true, true)?;
            return Ok(at);
        }
        Ok(p)
    }

    /// `pointReachable(point)`: can the pawn get to the point directly?
    pub fn point_reachable(&mut self, a: u32, point: Vec3) -> Res<bool> {
        self.point_reachable_kv(a, point, false)
    }

    /// `APawn::pointReachable(point, bKnowVisible)`: `know_visible` skips the eye-to-point visibility test.
    pub fn point_reachable_kv(&mut self, a: u32, point: Vec3, know_visible: bool) -> Res<bool> {
        let o = self.offsets();
        let loc = self.vec_at(a, o.location);
        let (dx, dy) = (point[0] - loc[0], point[1] - loc[1]);
        if dx * dx + dy * dy > 1_000_000.0 {
            return Ok(false);
        }
        let zone = self.point_region(point).zone;
        if !self.zone_b(a, "bWaterZone") && !self.bp(a, "bCanSwim") && self.bp(zone, "bWaterZone") {
            return Ok(false);
        }
        let foot_off = self.region_offsets().foot_region;
        let foot = self.u32_at(a, foot_off);
        if !self.bp(foot, "bPainZone") && self.zone_is_pain_for(a, zone) {
            return Ok(false);
        }
        let eye = [loc[0], loc[1], loc[2] + self.fp(a, "BaseEyeHeight")];
        if !know_visible && !self.bsp.as_ref().map(|b| b.fast_line_check(point, eye)).unwrap_or(true) {
            return Ok(false);
        }
        let p = self.settle_point(a, point)?;
        self.reachable(a, p, 15.0, 0)
    }

    /// `actorReachable(actor)`: can the pawn get to the actor directly?
    pub fn actor_reachable(&mut self, a: u32, goal: u32) -> Res<bool> {
        self.actor_reachable_kv(a, goal, false)
    }

    /// `APawn::actorReachable(actor, bKnowVisible)`. A pawn goal may be any distance away (the 800-unit limit and the
    /// pain zone test apply to other actors; for pawns a pain zone counts only when its damage type hurts);
    /// `know_visible` skips the line-of-sight trace.
    pub fn actor_reachable_kv(&mut self, a: u32, goal: u32, know_visible: bool) -> Res<bool> {
        let o = self.offsets();
        if !self.is_live(goal) {
            return Ok(false);
        }
        let loc = self.vec_at(a, o.location);
        let gl = self.vec_at(goal, o.location);
        let d = sub(gl, loc);
        let dist_sq = dot(d, d);
        let region_off = self.region_offsets().region;
        let gzone = self.u32_at(goal, region_off);
        if self.is_instance_of(goal, o.class_pawn) {
            if self.zone_is_pain_for(a, gzone) {
                return Ok(false);
            }
        } else {
            if dist_sq > 640_000.0 {
                return Ok(false);
            }
            if self.zone_is_pain_for(a, gzone) {
                return Ok(false);
            }
        }
        if self.bp(gzone, "bWaterZone") && !self.bp(a, "bCanSwim") {
            return Ok(false);
        }
        let eye = [loc[0], loc[1], loc[2] + self.fp(a, "BaseEyeHeight")];
        if !know_visible {
            let t = self.single_line_check(a, gl, eye, 6, [0.0; 3], 0);
            if t.time < 1.0 && t.actor != goal {
                return Ok(false);
            }
        }
        let (r, _) = self.cylinder(a);
        let (gr, _) = self.cylinder(goal);
        let mut threshold = 15.0;
        if self.is_instance_of(goal, o.class_pawn) {
            let melee = (r * 1.5).min(self.fp(a, "MeleeRange"));
            let touch = melee + gr + r;
            if dist_sq <= touch * touch {
                return Ok(true);
            }
            threshold = touch;
            if dist_sq > 640_000.0 {
                threshold = threshold.max(dist_sq.sqrt() - 800.0);
            }
        } else if self.is_instance_of(goal, o.class_inventory) || self.reg.class_by_name("Trigger").map(|t| self.class_of(goal).map(|c| self.is_a(c, t)).unwrap_or(false)).unwrap_or(false) {
            threshold = (gr + r) - 2.0;
        }
        let dest = self.settle_point(a, gl)?;
        let test = if self.bp(goal, "bBlockActors") { goal } else { 0 };
        self.reachable(a, dest, threshold, test)
    }

    fn reach_flags_of(&mut self, a: u32) -> i32 {
        self.calc_move_flags(a)
    }

    /// `APawn::GetPathnodeList` (Engine.dll 10302086): the navigation points a pawn can reach from where it stands, with
    /// their path distance, nearest first when `sort`. From a start node (`from` if it is one, else the node the pawn
    /// overlaps) these are the ends of its reach specs that fit the pawn's size and abilities (`use_pruned` adds the
    /// pruned paths); with no start node they are the pawn's own line-of-walk neighbours within 1000 units.
    pub fn get_pathnode_list(&mut self, a: u32, max: usize, from: u32, use_pruned: bool, sort: bool) -> Res<Vec<(u32, f32)>> {
        let o = self.offsets();
        let flags = self.reach_flags_of(a);
        let (radius, height) = {
            let (r, h) = self.cylinder(a);
            (r as i32, h as i32)
        };
        let nav_class = self.reg.class_by_name("NavigationPoint");
        let is_nav = |vm: &Vm, x: u32| x != 0 && nav_class.map(|n| vm.class_of(x).map(|c| vm.is_a(c, n)).unwrap_or(false)).unwrap_or(false);
        let mut refpos = self.vec_at(a, o.location);
        let mut start = 0u32;
        if from != 0 {
            if is_nav(self, from) {
                start = from;
            } else {
                refpos = self.vec_at(from, o.location);
            }
        }
        if start == 0 {
            let mt = self.op(a, "MoveTarget");
            if is_nav(self, mt) && self.is_overlapping(a, mt) {
                start = mt;
            }
        }
        let nodes = self.nav_points();
        if start == 0 {
            // the first navigation point (list order: the newest first) the pawn overlaps
            let mut ordered = nodes.clone();
            ordered.reverse();
            start = ordered.into_iter().find(|&n| self.is_overlapping(a, n)).unwrap_or(0);
        }
        let mut out: Vec<(u32, f32)> = Vec::new();
        if start != 0 {
            for prop in if use_pruned { vec!["Paths", "PrunedPaths"] } else { vec!["Paths"] } {
                let Some((off, _)) = self.prop_at(start, prop) else { continue };
                for i in 0..16 {
                    if out.len() >= max {
                        break;
                    }
                    let idx = self.u32_at(start, off + 4 * i) as i32;
                    if idx < 0 {
                        break;
                    }
                    let Some(spec) = self.reach_specs.get(idx as usize).cloned() else { continue };
                    if is_nav(self, spec.end) && spec.radius >= radius && spec.height >= height && (spec.flags & flags) == spec.flags {
                        out.push((spec.end, spec.distance as f32));
                    }
                }
            }
        } else {
            // no start node: the closest navigation points within 1000 units that the pawn can walk to
            let mut near: Vec<(u32, f32)> = Vec::new();
            let mut ordered = nodes.clone();
            ordered.reverse();
            for n in ordered {
                let d = sub(self.vec_at(n, o.location), refpos);
                let d2 = dot(d, d);
                if d2 < 1_000_000.0 {
                    let dist = d2.sqrt();
                    if near.len() < max {
                        near.push((n, dist));
                    } else {
                        // replaces the nearest entry that is farther than this one
                        let mut best: Option<usize> = None;
                        for (j, e) in near.iter().enumerate() {
                            if dist < e.1 && best.map(|b| e.1 < near[b].1).unwrap_or(true) {
                                best = Some(j);
                            }
                        }
                        if let Some(b) = best {
                            near[b] = (n, dist);
                        }
                    }
                }
            }
            for (n, d) in near {
                if self.actor_reachable(a, n)? {
                    out.push((n, d));
                }
            }
        }
        if sort && out.len() > 1 {
            // `appQsort` by distance (comparator 103c6a70); ties in MSVCRT's order
            crate::crt::qsort(&mut out, |v, a, b| crate::crt::x87_cmp(v[a].1, v[b].1));
        }
        Ok(out)
    }
}

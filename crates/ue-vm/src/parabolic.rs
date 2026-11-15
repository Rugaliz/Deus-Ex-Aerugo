//! `AActor::ParabolicTrace` (Engine.dll 10301a73): follows the arc a thrown or falling body would take, bouncing off
//! surfaces, and reports where and after how long it comes to rest. The AI uses it to pick grenade throws and to see
//! whether a projectile in the air is coming its way.

use crate::collision::*;
use crate::{Vm, VmError};

type Res<T> = Result<T, VmError>;

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

/// The two-wall slide of `AActor::TwoWallAdjust`.
fn two_wall_adjust(dir: Vec3, delta: &mut Vec3, hit_normal: Vec3, old_hit_normal: Vec3, hit_time: f32) {
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

impl Vm {
    /// `FUN_1036ca90`: moves a point or cylinder `pos` by `delta`, stopping two units before the first blocking hit
    /// (the level, or an actor that blocks actors, other than something based on `ignore`). `hit` is left alone for a
    /// move too small to count. Returns whether the move got anywhere.
    fn trace_step(&mut self, ignore: u32, pos: &mut Vec3, delta: Vec3, extent: Vec3, check_actors: bool, hit: &mut CheckResult) -> bool {
        if delta.iter().all(|c| c.abs() < 0.0001) {
            return true;
        }
        *hit = CheckResult::none();
        let dist = dot(delta, delta).sqrt();
        let dir = mul(delta, 1.0 / dist);
        let trace_delta = add(delta, mul(dir, 2.0));
        let level = self.level;
        let list = self.multi_line_check(*pos, add(*pos, trace_delta), extent, true, check_actors, 0);
        for h in list {
            if (ignore == 0 || !self.is_based_on(h.actor, ignore)) && (h.actor == level || self.bp(h.actor, "bBlockActors")) {
                *hit = h;
                break;
            }
        }
        let mut applied = delta;
        if hit.time < 1.0 {
            let reach = (dist + 2.0) * hit.time;
            if reach > 2.0 {
                applied = sub(mul(trace_delta, hit.time), mul(dir, 2.0));
                hit.time = (reach - 2.0) / dist;
            } else {
                applied = [0.0; 3];
                hit.time = 0.0;
            }
        }
        *pos = add(*pos, applied);
        hit.time > 0.0
    }

    /// Returns (time until it comes to rest, final location); time 0 means the arc ran out of the world, took too long
    /// or ended in water, and the location is then the start.
    #[allow(clippy::too_many_arguments)]
    pub fn parabolic_trace(&mut self, a: u32, start_loc: Vec3, start_vel: Vec3, max_time: f32, elasticity: f32, bounce: bool, landing_speed: f32, extent: Vec3, check_actors: bool, granularity: f32) -> Res<(f32, Vec3)> {
        let dt = if granularity < 0.005 {
            0.005
        } else if granularity >= 1.0 {
            1.0
        } else {
            granularity
        };
        let mut pos = start_loc;
        let mut vel = start_vel;
        let mut hit = CheckResult::none();
        let mut region = self.point_region(pos);
        let mut iterations = 0i32;
        let mut done = false;
        let landed = |vel: Vec3| bounce.then(|| dot(vel, vel) < landing_speed * landing_speed).unwrap_or(true);
        loop {
            if done {
                return Ok((iterations as f32 * dt, pos));
            }
            let mut hit_something = false;
            let gravity = self.vp(region.zone, "ZoneGravity");
            let zone_vel = self.vp(region.zone, "ZoneVelocity");
            vel = add(vel, mul(gravity, dt));
            let step = mul(add(vel, zone_vel), dt);
            self.trace_step(a, &mut pos, step, extent, check_actors, &mut hit);
            if !self.bp(region.zone, "bWaterZone") {
                if hit.time < 1.0 {
                    hit_something = true;
                    if hit.normal[2] > 0.7 && landed(vel) {
                        done = true;
                    } else {
                        // bounce: reflect the velocity and spend the rest of the step sliding along the surface
                        let n1 = hit.normal;
                        let t1 = hit.time;
                        vel = mul(add(vel, mul(n1, dot(vel, n1) * -2.0)), elasticity);
                        let d = dot(step, n1);
                        let mut slide = mul(sub(step, mul(n1, d)), 1.0 - t1);
                        if dot(slide, step) >= 0.0 {
                            self.trace_step(a, &mut pos, slide, extent, check_actors, &mut hit);
                            if hit.time < 1.0 {
                                let n2 = hit.normal;
                                vel = mul(add(vel, mul(n2, dot(vel, n2) * -2.0)), elasticity);
                                if hit.normal[2] > 0.7 && landed(vel) {
                                    done = true;
                                }
                                two_wall_adjust(safe_normal(step), &mut slide, n2, n1, hit.time);
                                self.trace_step(a, &mut pos, slide, extent, check_actors, &mut hit);
                                if !done && hit.time < 1.0 && hit.normal[2] > 0.7 && landed(slide) {
                                    done = true;
                                }
                            }
                        }
                    }
                }
            } else {
                // water ends the arc without a landing
                iterations = 0;
                pos = start_loc;
                done = true;
            }
            if !bounce || !hit_something {
                let term = self.fp(region.zone, "ZoneTerminalVelocity");
                if dot(vel, vel) > term * term {
                    vel = mul(safe_normal(vel), term);
                }
            }
            iterations += 1;
            region = self.point_region(pos);
            if region.zone_number == 0 || max_time < iterations as f32 * dt || dot(step, step) > 2_500_000.0 {
                iterations = 0;
                done = true;
                pos = start_loc;
            }
        }
    }
}

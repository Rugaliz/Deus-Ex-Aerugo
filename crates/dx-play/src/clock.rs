//! Fixed-step simulation with interpolated drawing. The game (scripts, physics, AI) always advances in steps of the same
//! length, whatever the frame rate, so its behaviour does not depend on the display (very short frames at 144-240 fps are
//! where engines of this age misbehave); each drawn frame shows the world between the last two steps, blended by how far
//! real time has got into the next step. The view direction adds the mouse movement that is not simulated yet, so looking
//! around has no extra lag.

use std::collections::HashMap;

use crate::Session;
use dx_soft::Camera;
use ue_vm::Value;

/// Where an actor was: location and rotation.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub pos: [f32; 3],
    pub rot: [i32; 3],
}

/// Farther than this between two steps is a teleport: drawn at once, not blended.
const TELEPORT: f32 = 512.0;
/// After a stall (a level load, a window drag) at most this many steps are caught up; the rest is dropped.
const MAX_STEPS: usize = 8;

pub struct FrameClock {
    /// Length of one simulation step in seconds (`DX_SIM_HZ`, default 60 steps per second).
    pub step: f32,
    accum: f32,
    /// How far real time is into the next step (0..1): the blend between the previous and the current state.
    pub alpha: f32,
    /// False until `advance` runs: callers that step the world themselves (tests, headless runs) draw the current state.
    active: bool,
    prev: HashMap<u32, Pose>,
    prev_eye: Option<[f32; 3]>,
    /// Real seconds passed through `advance` (the viewport clock procedural textures run by).
    real_time: f64,
}

impl Default for FrameClock {
    fn default() -> FrameClock {
        let hz = std::env::var("DX_SIM_HZ").ok().and_then(|v| v.parse::<f32>().ok()).filter(|h| (20.0..=240.0).contains(h)).unwrap_or(60.0);
        FrameClock { step: 1.0 / hz, accum: 0.0, alpha: 1.0, active: false, prev: HashMap::new(), prev_eye: None, real_time: 0.0 }
    }
}

/// Rotator units of view turn per unit of mouse axis at `MouseSensitivity` 1 and a 90 degree field of view (measured; the
/// scripts' `PlayerInput` scales mouse movement by `MouseSensitivity * DesiredFOV / 90`, and the step length cancels out).
pub const LOOK_PER_MOUSE_UNIT: f32 = 153.35;

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Blends two rotator components the short way round (65536 units per turn).
fn lerp_angle(a: i32, b: i32, t: f32) -> i32 {
    let d = ((b.wrapping_sub(a) & 0xFFFF) as i32 + 32768) % 65536 - 32768;
    a.wrapping_add((d as f32 * t) as i32)
}

impl Session {
    /// Advances the game by `real_dt` seconds of real time in fixed steps; returns how many steps ran.
    pub fn advance(&mut self, real_dt: f32) -> usize {
        self.clock.active = true;
        self.clock.accum += real_dt.max(0.0);
        self.clock.real_time += real_dt.max(0.0) as f64;
        let mut n = 0;
        while self.clock.accum >= self.clock.step {
            if n == MAX_STEPS {
                self.clock.accum = 0.0;
                break;
            }
            self.snapshot_poses();
            let step = self.clock.step;
            // only the last step of a drawn frame does the frame's drawing work (the visibility pass with its
            // render stamps, overlays, viewport windows): the original draws once per tick, and doing it for every
            // caught-up step made a slow frame slower still
            let hidden_root = self.vm.ui_root_hidden(self.player);
            let last = hidden_root || self.clock.accum - step < step || n + 1 == MAX_STEPS;
            self.draw_this_step = last;
            self.step_bound(step);
            self.draw_this_step = true;
            self.clock.accum -= step;
            n += 1;
            // the original ticks once per drawn frame, and the Save screen counts on it: it hides the root window,
            // waits two ticks (so that a frame without the menus is on screen) and then reads the screen for the save's
            // picture. While the root is hidden, one step per frame (the time left over is dropped: the game is
            // paused under the menus).
            if self.vm.ui_root_hidden(self.player) {
                self.clock.accum %= step;
                break;
            }
        }
        self.clock.alpha = (self.clock.accum / self.clock.step).clamp(0.0, 1.0);
        n
    }

    /// The viewport clock in seconds: real time when the window loop drives the game (keeps running in menus, like the
    /// original's), the level time when a caller steps the world itself.
    pub fn viewport_time(&mut self) -> f64 {
        if self.clock.active {
            self.clock.real_time
        } else {
            self.level_time() as f64
        }
    }

    /// Remembers where every actor and the eye are before a step.
    fn snapshot_poses(&mut self) {
        self.clock.prev.clear();
        for a in self.vm.live_actors() {
            let (pos, rot) = (self.vm.location(a), self.vm.rotation(a));
            self.clock.prev.insert(a, Pose { pos, rot });
        }
        self.clock.prev_eye = Some(self.camera().pos);
    }

    /// How far actor `a` is drawn from where it stands now: the blend between its pose before the last step and now,
    /// minus the current pose (zero when not blending, for new actors and teleports). Added to whatever is drawn for it.
    pub fn pose_offset(&mut self, a: u32) -> ([f32; 3], [i32; 3]) {
        let t = self.clock.alpha;
        if !self.clock.active || t >= 1.0 {
            return ([0.0; 3], [0; 3]);
        }
        let Some(p) = self.clock.prev.get(&a).copied() else { return ([0.0; 3], [0; 3]) };
        let (pos, rot) = (self.vm.location(a), self.vm.rotation(a));
        let d = ((pos[0] - p.pos[0]).powi(2) + (pos[1] - p.pos[1]).powi(2) + (pos[2] - p.pos[2]).powi(2)).sqrt();
        if d > TELEPORT {
            return ([0.0; 3], [0; 3]);
        }
        let l = lerp(p.pos, pos, t);
        let r: [i32; 3] = std::array::from_fn(|k| lerp_angle(p.rot[k], rot[k], t).wrapping_sub(rot[k]));
        ([l[0] - pos[0], l[1] - pos[1], l[2] - pos[2]], r)
    }

    /// Rotator units the view turns per unit of pending `aMouseX`/`aMouseY`, with the player's sensitivity, field of view
    /// and invert setting.
    pub fn look_per_mouse(&mut self) -> [f32; 2] {
        let p = self.player;
        let f = |s: &mut Session, n: &str, d: f32| s.vm.get_prop(p, n, 0).ok().and_then(|v| v.as_f32()).unwrap_or(d);
        let sens = f(self, "MouseSensitivity", 3.0);
        let fov = f(self, "DesiredFOV", 90.0);
        let invert = matches!(self.vm.get_prop(p, "bInvertMouse", 0), Ok(ue_vm::Value::Bool(true)));
        let k = LOOK_PER_MOUSE_UNIT * sens * fov * 0.01111;
        [k, if invert { -k } else { k }]
    }

    /// The camera of the drawn frame: the eye blended between the last two steps, looking where the player looks now plus
    /// the mouse movement the next step will apply.
    pub fn render_camera(&mut self) -> Camera {
        let mut cam = self.camera();
        if !self.clock.active {
            return cam;
        }
        if let Some(prev) = self.clock.prev_eye {
            let d = ((cam.pos[0] - prev[0]).powi(2) + (cam.pos[1] - prev[1]).powi(2) + (cam.pos[2] - prev[2]).powi(2)).sqrt();
            if d <= TELEPORT {
                cam.pos = lerp(prev, cam.pos, self.clock.alpha);
            }
        }
        // mouse look shows ahead of the simulation only in the player's own view
        // (not while a camera path moves the player: the path sets the view rotation every step, so the predicted turn
        // would be taken back, a jitter toward the mouse; the logo cutscene is such a path)
        let interpolating = matches!(self.vm.get_prop(self.player, "Physics", 0), Ok(Value::Byte(8)));
        let own_view = self.calc_view.map(|(_, _, _, first)| first).unwrap_or(true) && !interpolating;
        // and only while the player's scripts turn the view by the mouse (the last step with mouse movement did): a
        // paralysed player in a cutscene, a conversation, any state that ignores the mouse shows no turn
        if own_view && self.mouse_turns_view && !self.ui_wants_mouse() {
            let k = self.look_per_mouse();
            cam.yaw = cam.yaw.wrapping_add((self.input.pending_axis("aMouseX") * k[0]) as i32);
            cam.pitch = (cam.pitch + (self.input.pending_axis("aMouseY") * k[1]) as i32).clamp(-16384, 16383);
        }
        cam
    }
}

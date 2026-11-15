//! Turns key, axis and stick events into per-frame player input.

use crate::bindings::{Bindings, Command};
use std::collections::{BTreeMap, HashMap};

/// Factor of raw axis events such as mouse movement.
const AXIS_EVENT_SCALE: f32 = 0.01;
/// All accumulated axes are multiplied by this over the frame time.
const RATE_SCALE: f32 = 20.0;

/// What the host applies to the player before `PlayerInput`/`PlayerTick` of one frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameOut {
    /// Byte variables (`bFire`, `bDuck`, ...) with their value.
    pub buttons: Vec<(String, u8)>,
    /// Float variables (`aBaseY`, `aMouseX`, ...) with the value for this frame.
    pub axes: Vec<(String, f32)>,
    /// Exec functions to call (`Jump`, `ActivateBelt 3`), in the order they were pressed.
    pub execs: Vec<String>,
}

pub struct InputProcessor {
    pub bindings: Bindings,
    held: [bool; 256],
    /// Stick/trigger deflection by key code, replaced on each update.
    analog: HashMap<u8, f32>,
    axes: BTreeMap<String, f32>,
    buttons: BTreeMap<String, u8>,
    pulses: Vec<String>,
    execs: Vec<String>,
}

impl InputProcessor {
    pub fn new(bindings: Bindings) -> InputProcessor {
        InputProcessor { bindings, held: [false; 256], analog: HashMap::new(), axes: BTreeMap::new(), buttons: BTreeMap::new(), pulses: Vec::new(), execs: Vec::new() }
    }

    /// A key or button goes down or up. Repeated presses of a held key and releases of a key that is not down are ignored.
    pub fn key(&mut self, code: u8, down: bool) {
        if self.held[code as usize] == down {
            return;
        }
        self.held[code as usize] = down;
        for c in self.bindings.commands(code) {
            match (c, down) {
                (Command::Button(b), true) => {
                    self.buttons.insert(b, 1);
                }
                (Command::Button(b), false) => {
                    self.buttons.insert(b, 0);
                }
                (Command::Pulse(b), true) => {
                    self.buttons.insert(b.clone(), 1);
                    self.pulses.push(b);
                }
                (Command::Toggle(b), true) => {
                    let v = self.buttons.entry(b).or_insert(0);
                    *v ^= 0x80;
                }
                (Command::OnRelease(cmd), false) => self.execs.push(cmd),
                (Command::Exec(cmd), true) => self.execs.push(cmd),
                _ => {}
            }
        }
    }

    /// A raw axis movement (mouse delta, wheel step as a key press is better): `Axis` commands of the key add
    /// `delta * Speed * 0.01`.
    pub fn axis(&mut self, code: u8, delta: f32) {
        for c in self.bindings.commands(code) {
            if let Command::Axis { name, speed } = c {
                *self.axes.entry(name).or_insert(0.0) += delta * speed * AXIS_EVENT_SCALE;
            }
        }
    }

    /// A stick or trigger position in -1..1 (0 when released): it acts like a key held down in proportion, for every frame
    /// until updated. Dead zones belong to the caller.
    pub fn analog(&mut self, code: u8, value: f32) {
        if value == 0.0 {
            self.analog.remove(&code);
        } else {
            self.analog.insert(code, value.clamp(-1.0, 1.0));
        }
    }

    /// What raw axis movement (mouse) has gathered for `name` since the last `end_frame` (the rate `end_frame` will report
    /// is this times `RATE_SCALE / dt`). Lets a renderer show mouse look before the next simulation step applies it.
    pub fn pending_axis(&self, name: &str) -> f32 {
        self.axes.get(name).copied().unwrap_or(0.0)
    }

    /// Every key that is down goes up as a real release (`OnRelease` commands run, buttons drop to 0): the window
    /// system took an event (`XInputExt::Process`). Queued commands and stick positions stay.
    pub fn release_all(&mut self) {
        for code in 0..=255u8 {
            if self.held[code as usize] {
                self.key(code, false);
            }
        }
    }

    /// Releases everything (window focus lost).
    pub fn reset(&mut self) {
        self.held = [false; 256];
        self.analog.clear();
        self.axes.clear();
        self.buttons.clear();
        self.pulses.clear();
        self.execs.clear();
    }

    /// Ends the frame of `dt` seconds: adds the held keys' axis contributions, converts to rates and hands over the state.
    pub fn end_frame(&mut self, dt: f32) -> FrameOut {
        let dt = dt.max(1e-4);
        for code in 0..=255u8 {
            let weight = if self.held[code as usize] { 1.0 } else { self.analog.get(&code).copied().unwrap_or(0.0).abs() };
            let sign = self.analog.get(&code).map(|v| v.signum()).unwrap_or(1.0);
            if weight == 0.0 {
                continue;
            }
            for c in self.bindings.commands(code) {
                if let Command::Axis { name, speed } = c {
                    *self.axes.entry(name).or_insert(0.0) += speed * sign * weight * dt;
                }
            }
        }
        let out = FrameOut {
            buttons: self.buttons.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            axes: self.axes.iter().map(|(k, v)| (k.clone(), v * RATE_SCALE / dt)).collect(),
            execs: std::mem::take(&mut self.execs),
        };
        self.axes.clear();
        for p in self.pulses.drain(..) {
            self.buttons.insert(p, 0);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_code;

    fn proc() -> InputProcessor {
        InputProcessor::new(Bindings::defaults())
    }

    fn axis(f: &FrameOut, n: &str) -> f32 {
        f.axes.iter().find(|(k, _)| k == n).map(|(_, v)| *v).unwrap_or(0.0)
    }

    #[test]
    fn a_held_movement_key_gives_the_same_rate_at_any_frame_time() {
        for dt in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 144.0] {
            let mut p = proc();
            p.key(key_code("W").unwrap(), true);
            let f = p.end_frame(dt);
            assert!((axis(&f, "aBaseY") - 6000.0).abs() < 1.0, "{}", axis(&f, "aBaseY"));
        }
        let mut p = proc();
        p.key(key_code("W").unwrap(), true);
        p.key(key_code("S").unwrap(), true);
        assert_eq!(axis(&p.end_frame(0.01), "aBaseY"), 0.0, "opposite keys cancel");
    }

    #[test]
    fn mouse_motion_is_a_rate_and_a_repeat_press_is_ignored() {
        let mut p = proc();
        p.axis(key_code("MouseX").unwrap(), 10.0);
        let f = p.end_frame(1.0 / 60.0);
        // 10 counts * speed 6 * 0.01 = 0.6, over the frame time, times 20
        assert!((axis(&f, "aMouseX") - 0.6 * 20.0 * 60.0).abs() < 0.01);
        assert_eq!(axis(&p.end_frame(1.0 / 60.0), "aMouseX"), 0.0, "motion does not carry over");
        let mut p = proc();
        let space = key_code("Space").unwrap();
        p.key(space, true);
        p.key(space, true);
        assert_eq!(p.end_frame(0.016).execs, vec!["Jump"]);
        p.key(space, false);
        p.key(space, false);
        assert!(p.end_frame(0.016).execs.is_empty());
    }

    #[test]
    fn buttons_hold_toggle_and_exec_with_arguments() {
        let mut p = proc();
        p.key(key_code("LeftMouse").unwrap(), true);
        let f = p.end_frame(0.016);
        assert_eq!(f.execs, vec!["ParseLeftClick", "Fire"]);
        assert!(f.buttons.contains(&("bFire".to_string(), 1)));
        p.key(key_code("LeftMouse").unwrap(), false);
        assert!(p.end_frame(0.016).buttons.contains(&("bFire".to_string(), 0)));
        let c = key_code("C").unwrap();
        p.key(c, true);
        p.key(c, false);
        assert!(p.end_frame(0.016).buttons.contains(&("bDuck".to_string(), 0x80)));
        p.key(c, true);
        p.key(c, false);
        assert!(p.end_frame(0.016).buttons.contains(&("bDuck".to_string(), 0)));
        p.key(key_code("4").unwrap(), true);
        assert_eq!(p.end_frame(0.016).execs, vec!["ActivateBelt 4"]);
    }

    #[test]
    fn a_stick_acts_like_a_proportional_held_key() {
        let mut p = proc();
        p.analog(key_code("JoyY").unwrap(), -0.5);
        let f = p.end_frame(1.0 / 60.0);
        // JoyY pushed forward (negative) with Speed=-300 on aBaseY: half deflection, half the keyboard rate
        assert!((axis(&f, "aBaseY") - 3000.0).abs() < 1.0, "{}", axis(&f, "aBaseY"));
        p.analog(key_code("JoyY").unwrap(), 0.0);
        assert_eq!(axis(&p.end_frame(1.0 / 60.0), "aBaseY"), 0.0);
    }
}

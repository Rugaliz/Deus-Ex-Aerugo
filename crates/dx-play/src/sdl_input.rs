//! SDL3 events -> the engine's key/axis events (`dx_input::InputProcessor`): keyboard, mouse buttons, wheel, relative
//! mouse motion and game controllers.

use dx_input::key_code;
use dx_play::Session;
use sdl3::event::Event;
use sdl3::gamepad::{Axis, Button, Gamepad};
use sdl3::keyboard::Scancode;
use sdl3::mouse::MouseButton;
use std::collections::HashMap;

/// Names of the engine key(s) a physical key stands for. Shift, control and alt send the specific key and the generic one,
/// so a binding on either works.
fn scancode_names(sc: Scancode) -> Vec<String> {
    use Scancode::*;
    let one = |s: &str| vec![s.to_string()];
    match sc {
        LShift => vec!["LShift".into(), "Shift".into()],
        RShift => vec!["RShift".into(), "Shift".into()],
        LCtrl => vec!["LControl".into(), "Ctrl".into()],
        RCtrl => vec!["RControl".into(), "Ctrl".into()],
        LAlt | RAlt => one("Alt"),
        Space => one("Space"),
        Return => one("Enter"),
        Tab => one("Tab"),
        Backspace => one("Backspace"),
        CapsLock => one("CapsLock"),
        Up => one("Up"),
        Down => one("Down"),
        Left => one("Left"),
        Right => one("Right"),
        PageUp => one("PageUp"),
        PageDown => one("PageDown"),
        Home => one("Home"),
        End => one("End"),
        Insert => one("Insert"),
        Delete => one("Delete"),
        Pause => one("Pause"),
        PrintScreen => one("PrintScrn"),
        KpPlus => one("GreyPlus"),
        KpMinus => one("GreyMinus"),
        KpMultiply => one("GreyStar"),
        KpDivide => one("GreySlash"),
        KpPeriod => one("NumPadPeriod"),
        Semicolon => one("Semicolon"),
        Equals => one("Equals"),
        Comma => one("Comma"),
        Minus => one("Minus"),
        Period => one("Period"),
        Slash => one("Slash"),
        Grave => one("Tilde"),
        LeftBracket => one("LeftBracket"),
        RightBracket => one("RightBracket"),
        Backslash => one("Backslash"),
        Apostrophe => one("SingleQuote"),
        other => {
            // letters, digits, F-keys and the keypad digits are named like SDL names them
            let n = format!("{other:?}");
            let n = if let Some(d) = n.strip_prefix("Kp").filter(|d| d.len() == 1 && d.chars().all(|c| c.is_ascii_digit())) {
                format!("NumPad{d}")
            } else if let Some(d) = n.strip_prefix("Num").or_else(|| n.strip_prefix('_')).filter(|d| d.len() == 1 && d.chars().all(|c| c.is_ascii_digit())) {
                // the number row (the SDL binding calls them `_1`..`_0`)
                d.to_string()
            } else {
                n
            };
            vec![n]
        }
    }
}

/// The key the window system sees for a physical key: WinDrv passes the generic virtual key for the modifiers
/// (`IK_Shift` 0x10, `IK_Ctrl` 0x11, `IK_Alt` 0x12), not the left/right ones.
fn ui_key_code(names: &[String]) -> Option<u8> {
    for generic in ["Shift", "Ctrl", "Alt"] {
        if names.iter().any(|n| n == generic) {
            return key_code(generic);
        }
    }
    names.first().and_then(|n| key_code(n))
}

pub struct SdlInput {
    gamepad_subsystem: Option<sdl3::GamepadSubsystem>,
    pads: HashMap<u32, Gamepad>,
    /// Stick positions (left x/y, right x/y) in -1..1 after the dead zone.
    sticks: [f32; 4],
    triggers: [bool; 2],
}

const DEAD_ZONE: f32 = 0.18;

fn dead_zone(v: i16) -> f32 {
    let f = v as f32 / 32767.0;
    if f.abs() < DEAD_ZONE {
        0.0
    } else {
        f.signum() * ((f.abs() - DEAD_ZONE) / (1.0 - DEAD_ZONE)).min(1.0)
    }
}

/// A typed character through `UEngine::Key` (`XGameEngineExt::Key`): the console's `KeyType`, then the root when it
/// handles the keyboard. True when either took it.
fn type_char(session: &mut Session, ch: char) -> bool {
    if session.console_key_type(ch) {
        return true;
    }
    if session.ui_wants_keyboard() {
        session.ui_char(ch);
        return true;
    }
    false
}

/// The character the original's window procedure gets for a key that SDL gives no text for: with Alt held (and not
/// Ctrl) the key's own character (`WM_SYSCHAR`, Shift and Caps Lock applied; Alt+Enter is the fullscreen switch, no
/// character), with Ctrl a letter's control character (1..26), and Backspace, Tab and Escape (8, 9, 27).
fn synthesized_char(sc: Scancode, keymod: sdl3::keyboard::Mod) -> Option<char> {
    use sdl3::keyboard::Mod;
    let alt = keymod.intersects(Mod::LALTMOD | Mod::RALTMOD);
    let ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD);
    match sc {
        Scancode::Backspace => return Some('\u{8}'),
        Scancode::Tab => return Some('\t'),
        Scancode::Escape => return Some('\u{1b}'),
        Scancode::Return | Scancode::KpEnter => return None,
        _ => {}
    }
    let plain = keymod - (Mod::LALTMOD | Mod::RALTMOD | Mod::LCTRLMOD | Mod::RCTRLMOD | Mod::LGUIMOD | Mod::RGUIMOD);
    let k = sdl3::keyboard::Keycode::from_scancode(sc, sdl3::sys::keycode::SDL_Keymod(plain.bits()), true)?.to_ll().0;
    let ch = char::from_u32(k)?;
    if ctrl && !alt {
        let l = ch.to_ascii_lowercase();
        return l.is_ascii_lowercase().then(|| char::from(l as u8 - b'a' + 1));
    }
    (alt && !ctrl && !ch.is_control() && (ch as u32) < 0x4000_0000).then_some(ch)
}

impl SdlInput {
    pub fn new(sdl: &sdl3::Sdl) -> SdlInput {
        let gamepad_subsystem = sdl.gamepad().ok();
        let mut s = SdlInput { gamepad_subsystem, pads: HashMap::new(), sticks: [0.0; 4], triggers: [false; 2] };
        if let Some(g) = &s.gamepad_subsystem {
            for id in g.gamepads().unwrap_or_default() {
                if let Ok(p) = g.open(id) {
                    s.pads.insert(id.into(), p);
                }
            }
        }
        s
    }

    /// Feeds one SDL event. Returns false for events that are not input.
    pub fn event(&mut self, session: &mut Session, window_size: (f32, f32), e: &Event) -> bool {
        // the window system gets every key and button first (`XInputExt::Process`, 1002d0b0): the root keeps its
        // key-down map from all of them and takes them only while it handles the keyboard or mouse; an event it takes
        // never reaches the bindings, and every key the bindings have down is released (as a real release)
        match e {
            Event::KeyDown { scancode: Some(sc), .. } | Event::KeyUp { scancode: Some(sc), .. } => {
                let down = matches!(e, Event::KeyDown { .. });
                let repeat = matches!(e, Event::KeyDown { repeat: true, .. });
                // the character Windows sends after the key (`WM_CHAR`/`WM_SYSCHAR`) that SDL does not: Alt+letter and
                // the control characters; it reaches `UEngine::Key` whatever happens to the key event
                let typed = match e {
                    Event::KeyDown { keymod, .. } => synthesized_char(*sc, *keymod),
                    _ => None,
                };
                // the console's `KeyEvent` first (`UEngine::InputEvent`, autorepeats included): an event it takes goes
                // no further (`XInputExt::PreProcess` does nothing: not the window system, not the bindings)
                let code = ui_key_code(&scancode_names(*sc));
                let mut handled = false;
                if let Some(c) = code {
                    handled = session.console_key_event(c, if down { 1 } else { 3 }, 0.0);
                }
                if !handled {
                    if let Some(c) = code {
                        if session.ui_key(c, down) {
                            session.input.release_all();
                            handled = true;
                        }
                    }
                }
                if let Some(ch) = typed {
                    type_char(session, ch);
                }
                // a held key's repeats are for the console and the window system only (the bindings ignore a press of
                // a key that is down)
                if handled || repeat {
                    return true;
                }
            }
            Event::TextInput { text, .. } => {
                // `UEngine::Key`: the console's `KeyType` first, then the window system
                let mut any = false;
                for ch in text.chars() {
                    any |= type_char(session, ch);
                }
                if any || session.ui_wants_keyboard() {
                    return true;
                }
            }
            Event::MouseButtonDown { mouse_btn, .. } | Event::MouseButtonUp { mouse_btn, .. } => {
                let down = matches!(e, Event::MouseButtonDown { .. });
                let code = match mouse_btn {
                    MouseButton::Left => Some("LeftMouse"),
                    MouseButton::Right => Some("RightMouse"),
                    MouseButton::Middle => Some("MiddleMouse"),
                    _ => None,
                };
                if let Some(c) = code.and_then(key_code) {
                    let took = session.console_key_event(c, if down { 1 } else { 3 }, 0.0);
                    if took {
                        return true;
                    }
                    if session.ui_key(c, down) {
                        session.input.release_all();
                        return true;
                    }
                }
            }
            Event::MouseWheel { y, .. } => {
                // WinDrv: the wheel's axis (`IK_MouseW`, 120 a notch) and one press and release per notch; each through
                // the console's `KeyEvent` first, then the window system while it has the mouse
                let name = if *y > 0.0 { "MouseWheelUp" } else { "MouseWheelDown" };
                let notches = y.abs().round().max(1.0) as u32;
                let mut took = session.console_key_event(0xE6, 4, *y * 120.0);
                if !took && session.ui_wants_mouse() {
                    session.ui_axis(0xE6);
                }
                if let Some(c) = key_code(name) {
                    for _ in 0..notches {
                        for down in [true, false] {
                            if session.console_key_event(c, if down { 1 } else { 3 }, 0.0) {
                                took = true;
                            } else if session.ui_wants_mouse() {
                                session.ui_key(c, down);
                            }
                        }
                    }
                }
                if session.ui_wants_mouse() {
                    session.input.release_all();
                    return true;
                }
                if took {
                    return true;
                }
            }
            Event::MouseMotion { x, y, xrel, yrel, .. } => {
                // the mouse axes (`IK_MouseX`, `IK_MouseY`, up positive) through the console's `KeyEvent`, then the root
                // (`RawMouseButtonPressed` with action 4) while it has the mouse: taken, they release the held keys
                let mut took = false;
                for (code, d) in [(0xE4u8, *xrel), (0xE5u8, -*yrel)] {
                    if d != 0.0 && session.console_key_event(code, 4, d) {
                        took = true;
                    }
                }
                if session.ui_wants_mouse() {
                    session.ui_mouse_abs(*x, *y, window_size.0, window_size.1);
                    for (code, d) in [(0xE4u8, *xrel), (0xE5u8, -*yrel)] {
                        if d != 0.0 {
                            session.ui_axis(code);
                        }
                    }
                    session.input.release_all();
                    return true;
                }
                if took {
                    return true;
                }
            }
            _ => {}
        }
        // mouse counts scaled to the 400 DPI mice the game was tuned for (`AerugoSettings.MouseDPI`)
        let mouse_scale = session.mouse_scale;
        let input = &mut session.input;
        let key = |input: &mut dx_input::InputProcessor, name: &str, down: bool| {
            if let Some(c) = key_code(name) {
                input.key(c, down);
            }
        };
        match e {
            Event::KeyDown { scancode: Some(sc), repeat: false, .. } | Event::KeyUp { scancode: Some(sc), .. } => {
                let down = matches!(e, Event::KeyDown { .. });
                for n in scancode_names(*sc) {
                    key(input, &n, down);
                }
            }
            Event::MouseButtonDown { mouse_btn, .. } | Event::MouseButtonUp { mouse_btn, .. } => {
                let down = matches!(e, Event::MouseButtonDown { .. });
                match mouse_btn {
                    MouseButton::Left => key(input, "LeftMouse", down),
                    MouseButton::Right => key(input, "RightMouse", down),
                    MouseButton::Middle => key(input, "MiddleMouse", down),
                    _ => {}
                }
            }
            Event::MouseWheel { y, .. } => {
                // each notch is a press and release of the wheel "key" (press runs the binding)
                let name = if *y > 0.0 { "MouseWheelUp" } else { "MouseWheelDown" };
                for _ in 0..y.abs().round().max(1.0) as u32 {
                    key(input, name, true);
                    key(input, name, false);
                }
            }
            Event::MouseMotion { xrel, yrel, .. } => {
                // the engine's mouse axes: right and up are positive
                if let Some(c) = key_code("MouseX") {
                    input.axis(c, *xrel * mouse_scale);
                }
                if let Some(c) = key_code("MouseY") {
                    input.axis(c, -*yrel * mouse_scale);
                }
            }
            Event::GamepadAdded { which, .. } => {
                if let Some(g) = &self.gamepad_subsystem {
                    if let Ok(p) = g.open(*which) {
                        self.pads.insert((*which).into(), p);
                    }
                }
            }
            Event::GamepadRemoved { which, .. } => {
                self.pads.remove(&(*which).into());
                self.sticks = [0.0; 4];
                self.apply_sticks(input);
            }
            Event::GamepadButtonDown { button, .. } | Event::GamepadButtonUp { button, .. } => {
                let down = matches!(e, Event::GamepadButtonDown { .. });
                let name = match button {
                    Button::South => "Joy1",
                    Button::East => "Joy2",
                    Button::West => "Joy3",
                    Button::North => "Joy4",
                    Button::LeftShoulder => "Joy5",
                    Button::RightShoulder => "Joy6",
                    Button::Back => "Joy7",
                    Button::Start => "Joy8",
                    Button::LeftStick => "Joy9",
                    Button::RightStick => "Joy10",
                    Button::DPadUp => "JoyPovUp",
                    Button::DPadDown => "JoyPovDown",
                    Button::DPadLeft => "JoyPovLeft",
                    Button::DPadRight => "JoyPovRight",
                    _ => return true,
                };
                // gamepad buttons are ordinary keys: the console, then the window system see them first
                if let Some(c) = key_code(name) {
                    if session.console_key_event(c, if down { 1 } else { 3 }, 0.0) {
                        return true;
                    }
                    if session.ui_key(c, down) {
                        session.input.release_all();
                        return true;
                    }
                }
                key(&mut session.input, name, down);
            }
            Event::GamepadAxisMotion { axis, value, .. } => match axis {
                Axis::LeftX => self.stick(input, 0, *value),
                Axis::LeftY => self.stick(input, 1, *value),
                Axis::RightX => self.stick(input, 2, *value),
                Axis::RightY => self.stick(input, 3, *value),
                // triggers press the digital buttons Joy11 (right) and Joy12 (left) past half travel
                Axis::TriggerRight => self.trigger(input, 0, "Joy11", *value),
                Axis::TriggerLeft => self.trigger(input, 1, "Joy12", *value),
            },
            _ => return false,
        }
        true
    }

    fn stick(&mut self, input: &mut dx_input::InputProcessor, i: usize, v: i16) {
        self.sticks[i] = dead_zone(v);
        self.apply_sticks(input);
    }

    fn apply_sticks(&self, input: &mut dx_input::InputProcessor) {
        for (i, name) in ["JoyX", "JoyY", "JoyZ", "JoyR"].iter().enumerate() {
            if let Some(c) = key_code(name) {
                input.analog(c, self.sticks[i]);
            }
        }
    }

    fn trigger(&mut self, input: &mut dx_input::InputProcessor, i: usize, name: &str, v: i16) {
        let down = v > 16000;
        if self.triggers[i] != down {
            self.triggers[i] = down;
            if let Some(c) = key_code(name) {
                input.key(c, down);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_row_and_keypad_keys_have_engine_names() {
        for (sc, name) in [(Scancode::_1, "1"), (Scancode::_0, "0"), (Scancode::Kp5, "NumPad5"), (Scancode::A, "A"), (Scancode::F1, "F1")] {
            let n = scancode_names(sc);
            assert_eq!(n, vec![name.to_string()], "{sc:?}");
            assert!(key_code(name).is_some(), "{name} is not an engine key");
        }
    }
}

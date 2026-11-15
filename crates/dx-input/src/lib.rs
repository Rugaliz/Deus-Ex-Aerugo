//! Input layer of the engine without any window code: key names, `[Engine.Input]` bindings with aliases, and the
//! processing that turns key and axis events into the player's input variables.
//!
//! The model follows the original input subsystem: a key press runs the commands of its binding once
//! (`Button`/`Pulse`/`Toggle` set a byte variable, anything else is an exec function of the player), a held key adds
//! `Speed * dt` to the axis of each `Axis` command every frame, a raw axis event (mouse) adds `delta * Speed * 0.01`,
//! and at the end of the frame all accumulated axes are multiplied by `20 / dt`, so the player's script sees rates.

mod bindings;
mod keys;
mod processor;

pub use bindings::{Bindings, Command, DEFAULT_INI};
pub use keys::KEYS;
pub use processor::{FrameOut, InputProcessor};

/// Key code by name (case-insensitive), e.g. `"LeftMouse"`, `"W"`, `"F3"`, `"MouseWheelUp"`.
pub fn key_code(name: &str) -> Option<u8> {
    KEYS.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(c, _)| *c)
}

/// Name of a key code.
pub fn key_name(code: u8) -> Option<&'static str> {
    KEYS.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

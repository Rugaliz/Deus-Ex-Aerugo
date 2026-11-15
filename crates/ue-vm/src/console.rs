//! Console commands as the engine routes them (`ConsoleCommand` of `Actor`, `PlayerPawn` and `Console`).
//!
//! * `Actor.ConsoleCommand` (Engine 1030184d) hands the line to the game engine (`XLevel->Engine->Exec`).
//! * `PlayerPawn.ConsoleCommand` (103ba200) hands it to its viewport (`Player->Exec`); without a viewport it returns "".
//! * `Console.ConsoleCommand` (10379900) hands it to the console's viewport with the console itself as the output
//!   device (each piece of output becomes a `Message` event) and returns whether anything took it.
//!
//! The chain, synchronous, first taker wins (output collected as `FStringOutputDevice` does: pieces appended):
//! * the window client's viewport (WinDrv 11106d90) runs `UViewport::Exec` first and only then its own commands
//!   (`ToggleFullscreen`, `GetRes`, `SetRes`...: the host, [`HostExec::Window`]);
//! * `UViewport::Exec` (10373a60): the input system (`UInput::Exec` 10393e10: key names, bindings, aliases: the host),
//!   then `GETPING`, `INJECT`, `NETSPEED`, `LANSPEED`, `SHOWALL`, `REPORT`, `SHOT`, `SHOWACTORS`, `HIDEACTORS`, `RMODE`,
//!   `EXEC`, the console object's exec functions, then `UPlayer::Exec`;
//! * `UPlayer::Exec` (103d7ed0): the level (`ULevel::Exec` 1039dfe0: demos), the game's exec functions, the HUD's
//!   (`PlayerPawn.myHUD`), the pawn's own, those of each item of its inventory, then the engine;
//! * the engine (DeusEx `DDeusExGameEngine::Exec` 1000ed70): `SAVEGAME`/`DELETEGAME`, then `UGameEngine::Exec` 1038a030
//!   (`OPEN`, `START`, `DISCONNECT`, ..., the level, the game's exec functions with no executor), then `UEngine::Exec`
//!   10380ee0 (`USystem::Exec` 1016b790: `EXIT`/`QUIT`/`RELAUNCH`; `UObject::StaticExec` 10153110: `GET`/`SET`; the
//!   client: `BRIGHTNESS`; `FLUSH`).
//!
//! Exec functions are called as `UObject::ScriptConsoleExec` (Core 10150860) does: the first word names the function
//! (an exec function, the current state's version first), its parameters are read from the rest of the line one after
//! the other by their types' `ImportText` (Core), the first object parameter takes the executing pawn when it fits.

use crate::{inst_ref, Value, Vm, VmError};
use ue_package::object::PropertyKind as K;
use ue_registry::{PropDef, Target};

type Res<T> = Result<T, VmError>;

const FUNC_EXEC: u32 = 0x200;
const CPF_PARM: u32 = 0x80;
const CPF_OPTIONAL_PARM: u32 = 0x10;
const CPF_RETURN_PARM: u32 = 0x400;
/// `PlayerPawn.ShowFlags`' `SHOW_Actors` bit.
const SHOW_ACTORS: i32 = 0x8;

/// The parts of the chain that live in the host (window, input and client layers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostExec {
    /// `UInput::Exec`: `KEYNAME`, `KEYBINDING`, `BUTTON`/`PULSE`/`TOGGLE`/`AXIS`. Aliases are asked with [`HostExec::Alias`].
    Input,
    /// The alias table of the input system: the command text an alias name stands for (`None`: not an alias).
    Alias,
    /// `UWindowsViewport::Exec`'s own commands (display modes).
    Window,
    /// `GET INPUT <key>` / `SET INPUT <key> <command>`: the key bindings (properties of the class `Input`).
    InputConfig,
}

/// The host's part of the chain: `Some(output)` when it took the line.
pub type HostConsole = Box<dyn FnMut(&mut Vm, HostExec, &str) -> Option<String>>;

/// What the engine does after the frame (the original sets flags the main loop or the viewport acts on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineRequest {
    /// `EXIT`/`QUIT`: `GIsRequestingExit`.
    Exit,
    /// `RELAUNCH <args>`: start the game again with these arguments, then exit.
    Relaunch(String),
    /// `SHOT`: a screenshot of the next frame.
    Shot,
    /// `FLUSH`: `UEngine::Flush` (caches dropped and rebuilt).
    Flush,
    /// `RECONNECT`: travel to the current level's URL again.
    Reconnect,
}

/// `ParseCommand` (Core 10147a20): skips spaces and tabs; on a match of `word` (any case) not followed by a letter or
/// digit, moves past it and the spaces after it. The leading spaces stay skipped either way.
pub fn parse_command(s: &mut &str, word: &str) -> bool {
    *s = s.trim_start_matches([' ', '\t']);
    let n = word.len();
    if s.len() >= n && s.is_char_boundary(n) && s[..n].eq_ignore_ascii_case(word) {
        let rest = &s[n..];
        if !rest.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
            *s = rest.trim_start_matches([' ', '\t']);
            return true;
        }
    }
    false
}

/// `ParseToken(Str, Result, MaxLen, UseEscape)` (Core 10147ba0): spaces and tabs skipped, then a quoted string (a
/// backslash takes the next character when `escape`) or everything up to the next space or tab. At most `max - 1`
/// characters are kept. `None` when the token is empty.
pub fn parse_token(s: &mut &str, max: usize, escape: bool) -> Option<String> {
    *s = s.trim_start_matches([' ', '\t']);
    let mut out = String::new();
    let mut n = 1;
    let mut it = s.char_indices().peekable();
    let end;
    if s.starts_with('"') {
        it.next();
        let mut stop = s.len();
        while let Some((i, c)) = it.next() {
            if c == '"' || n >= max {
                stop = i;
                break;
            }
            let mut c = c;
            if c == '\\' && escape {
                match it.next() {
                    Some((_, e)) => c = e,
                    None => {
                        stop = s.len();
                        break;
                    }
                }
            }
            out.push(c);
            n += 1;
        }
        let rest = &s[stop.min(s.len())..];
        end = s.len() - rest.strip_prefix('"').unwrap_or(rest).len();
    } else {
        let mut stop = s.len();
        for (i, c) in it {
            if c == ' ' || c == '\t' {
                stop = i;
                break;
            }
            if n < max {
                out.push(c);
                n += 1;
            }
        }
        end = stop;
    }
    *s = &s[end..];
    (!out.is_empty()).then_some(out)
}

/// `ParseNext` (Core 10147b50): skips white space and `;` comments up to the end of their line.
fn parse_next(s: &mut &str) {
    loop {
        *s = s.trim_start_matches([' ', '\t', '\r', '\n']);
        if !s.starts_with(';') {
            return;
        }
        *s = s.find(['\r', '\n']).map(|i| &s[i..]).unwrap_or("");
    }
}

/// `ReadToken` of the property importers (Core 10164cb0): a quoted string (`\\` is a backslash, `\xx` a hex
/// character code), an identifier (letters, digits, `_`, `-`, and `.` when `dotted`), or else the one character
/// under the cursor (not consumed). `None` for an unterminated or overlong token.
pub(crate) fn read_token<'a>(s: &'a str, max: usize, dotted: bool) -> Option<(String, &'a str)> {
    let mut out = String::new();
    if let Some(body) = s.strip_prefix('"') {
        let hex = |c: Option<char>| c.and_then(|c| c.to_digit(16)).unwrap_or(0);
        let mut chars = body.chars();
        let mut rest = body;
        let mut count = 0;
        loop {
            let c = match chars.clone().next() {
                Some(c) if c != '"' && c != '\r' && c != '\n' && count < max - 1 => c,
                _ => break,
            };
            if c == '\\' {
                chars.next();
                let mut look = chars.clone();
                if look.next() == Some('\\') {
                    chars.next();
                    out.push('\\');
                } else {
                    let hi = hex(chars.next());
                    let lo = hex(chars.next());
                    out.push(char::from_u32(hi * 16 + lo).unwrap_or('\0'));
                }
            } else {
                chars.next();
                out.push(c);
            }
            count += 1;
            rest = chars.as_str();
        }
        if count == max - 1 {
            return None;
        }
        return rest.strip_prefix('"').map(|r| (out, r));
    }
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || (dotted && c == '.');
    if s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
        let end = s.find(|c: char| !ident(c)).unwrap_or(s.len());
        if end >= max - 1 {
            return None;
        }
        return Some((s[..end].to_string(), &s[end..]));
    }
    // one character, the cursor stays
    out.extend(s.chars().next());
    Some((out, s))
}

/// `appAtoi`/`_wtoi`: optional white space, a sign, then digits; 0 when there are none; wraps like the CRT on overflow.
pub(crate) fn atoi(s: &str) -> i32 {
    let t = s.trim_start();
    let (neg, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    // the system C runtime's `_wtol` saturates at the int range
    let mut v: i64 = 0;
    for b in t.bytes().take_while(u8::is_ascii_digit) {
        v = (v * 10 + (b - b'0') as i64).min(1 << 40);
    }
    let v = if neg { -v } else { v };
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// `appAtof`/`_wtof`: the longest leading decimal number (white space first), 0 when there is none.
pub(crate) fn atof(s: &str) -> f32 {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > s
    };
    let mut any = digits(&mut i);
    if i < b.len() && b[i] == b'.' {
        i += 1;
        any |= digits(&mut i);
    }
    if !any {
        return 0.0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if digits(&mut j) {
            i = j;
        }
    }
    t[..i].parse::<f64>().map(|v| v as f32).unwrap_or(0.0)
}

/// Output of one command line, as the original's output device receives it (pieces in order).
#[derive(Default)]
pub struct ExecOut(pub Vec<String>);

impl ExecOut {
    fn log(&mut self, s: impl Into<String>) {
        self.0.push(s.into());
    }

    /// `FStringOutputDevice`: the pieces appended.
    pub fn text(&self) -> String {
        self.0.concat()
    }
}

impl Vm {
    fn host_exec(&mut self, stage: HostExec, cmd: &str) -> Option<String> {
        let mut host = self.host_console.take()?;
        let out = host(self, stage, cmd);
        // a host that installed another hook meanwhile keeps the new one
        if self.host_console.is_none() {
            self.host_console = Some(host);
        }
        out
    }

    /// `Actor.ConsoleCommand`: the line goes to the game engine; its output is returned.
    pub fn console_command_actor(&mut self, cmd: &str) -> Res<String> {
        let mut out = ExecOut::default();
        self.engine_exec(cmd, &mut out)?;
        Ok(out.text())
    }

    /// `PlayerPawn.ConsoleCommand`: the line goes to the pawn's viewport (none: nothing happens, "" is returned).
    pub fn console_command_player(&mut self, pawn: u32, cmd: &str) -> Res<String> {
        let viewport = self.op(pawn, "Player");
        let mut out = ExecOut::default();
        if viewport != 0 {
            self.viewport_exec(viewport, cmd, &mut out)?;
        }
        Ok(out.text())
    }

    /// `Console.ConsoleCommand`: the line goes to the console's viewport; each piece of output is a `Message` to the
    /// console. Returns whether something took the line.
    pub fn console_command_console(&mut self, console: u32, cmd: &str) -> Res<bool> {
        let viewport = self.op(console, "Viewport");
        let mut out = ExecOut::default();
        let taken = viewport != 0 && self.viewport_exec(viewport, cmd, &mut out)?;
        for piece in out.0 {
            let none = Value::Name(0);
            self.send_event(console, "Message", vec![Value::Obj(0), Value::Str(piece), none])?;
        }
        Ok(taken)
    }

    /// Runs a line as typed at the console or bound to a key (the input system hands it to the viewport). Returns
    /// whether something took it and its output.
    pub fn exec_viewport_line(&mut self, pawn: u32, cmd: &str) -> Res<(bool, String)> {
        let viewport = self.op(pawn, "Player");
        let mut out = ExecOut::default();
        let taken = viewport != 0 && self.viewport_exec(viewport, cmd, &mut out)?;
        Ok((taken, out.text()))
    }

    /// `UWindowsViewport::Exec`: the engine's viewport chain first, then the window's own commands.
    fn viewport_exec(&mut self, viewport: u32, cmd: &str, out: &mut ExecOut) -> Res<bool> {
        if self.uviewport_exec(viewport, cmd, out)? {
            return Ok(true);
        }
        Ok(match self.host_exec(HostExec::Window, cmd) {
            Some(s) => {
                if !s.is_empty() {
                    out.log(s);
                }
                true
            }
            None => false,
        })
    }

    /// `UViewport::Exec` (Engine 10373a60).
    fn uviewport_exec(&mut self, viewport: u32, cmd: &str, out: &mut ExecOut) -> Res<bool> {
        let actor = self.op(viewport, "Actor");
        // the input system: its own commands, then aliases (`ExecInputCommands`, an alias never expands inside another)
        if let Some(s) = self.host_exec(HostExec::Input, cmd) {
            if !s.is_empty() {
                out.log(s);
            }
            return Ok(true);
        }
        if !self.alias_running {
            let mut c = cmd;
            if let Some(name) = parse_token(&mut c, 0x100, false) {
                if let Some(text) = self.host_exec(HostExec::Alias, &name) {
                    self.alias_running = true;
                    let r = self.exec_input_commands(viewport, &text, out);
                    self.alias_running = false;
                    r?;
                    return Ok(true);
                }
            }
        }
        // (the render device's own commands: none that this port's renderer has)
        let mut s = cmd;
        if parse_command(&mut s, "GETPING") {
            // no network connection: a standalone game's ping
            out.log("0");
            return Ok(true);
        }
        // network commands: a standalone game has no connection or net driver; the speeds are network settings
        if parse_command(&mut s, "INJECT") || parse_command(&mut s, "NETSPEED") || parse_command(&mut s, "LANSPEED") {
            return Ok(true);
        }
        if parse_command(&mut s, "SHOWALL") {
            if let Some(actor_class) = self.reg.class_by_name("Actor") {
                let all: Vec<u32> = (0..self.objects.len()).map(inst_ref).filter(|&r| self.is_live(r) && self.is_instance_of(r, actor_class)).collect();
                for a in all {
                    let _ = self.set_prop(a, "bHidden", 0, Value::Bool(false));
                }
            }
            return Ok(true);
        }
        // REPORT copies a report of the view to the clipboard
        if parse_command(&mut s, "REPORT") {
            return Ok(true);
        }
        if parse_command(&mut s, "SHOT") {
            self.engine_requests.push(EngineRequest::Shot);
            return Ok(true);
        }
        let show = parse_command(&mut s, "SHOWACTORS");
        if show || parse_command(&mut s, "HIDEACTORS") {
            let flags = self.get_prop(actor, "ShowFlags", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0);
            let flags = if show { flags | SHOW_ACTORS } else { flags & !SHOW_ACTORS };
            let _ = self.set_prop(actor, "ShowFlags", 0, Value::Int(flags));
            return Ok(true);
        }
        if parse_command(&mut s, "RMODE") {
            let n = atoi(s);
            if (1..=19).contains(&n) {
                let _ = self.set_prop(actor, "RendMap", 0, Value::Int(n));
            }
            return Ok(true);
        }
        if parse_command(&mut s, "EXEC") {
            // a command file (`UViewport::ExecMacro`): this port ships none and reads none from the game folder
            return Ok(true);
        }
        let console = self.op(viewport, "Console");
        if console != 0 && self.script_console_exec(console, cmd, out, actor)? {
            return Ok(true);
        }
        self.uplayer_exec(actor, cmd, out)
    }

    /// `UInput::ExecInputCommands`: the parts of an alias, split at `|`, each run through the viewport.
    fn exec_input_commands(&mut self, viewport: u32, text: &str, out: &mut ExecOut) -> Res<()> {
        for part in text.split('|') {
            let part = part.trim_start_matches([' ', '\t']);
            if !part.is_empty() {
                self.viewport_exec(viewport, part, out)?;
            }
        }
        Ok(())
    }

    /// `UPlayer::Exec` (Engine 103d7ed0).
    fn uplayer_exec(&mut self, actor: u32, cmd: &str, out: &mut ExecOut) -> Res<bool> {
        if actor == 0 {
            return Ok(false);
        }
        if level_exec(cmd) {
            return Ok(true);
        }
        let game = self.game_info();
        if game != 0 && self.script_console_exec(game, cmd, out, actor)? {
            return Ok(true);
        }
        let hud = self.op(actor, "myHUD");
        if hud != 0 && self.script_console_exec(hud, cmd, out, actor)? {
            return Ok(true);
        }
        if self.script_console_exec(actor, cmd, out, actor)? {
            return Ok(true);
        }
        let mut item = self.op(actor, "Inventory");
        let mut guard = 0;
        while item != 0 && guard < 4096 {
            if self.script_console_exec(item, cmd, out, actor)? {
                return Ok(true);
            }
            item = self.op(item, "Inventory");
            guard += 1;
        }
        self.engine_exec(cmd, out)
    }

    /// The pawn of the client's first viewport (a `PlayerPawn` with a `Player`).
    fn local_player(&mut self) -> u32 {
        let Some(pp) = self.reg.class_by_name("PlayerPawn") else { return 0 };
        let all: Vec<u32> = (0..self.objects.len()).map(inst_ref).filter(|&r| self.is_live(r) && self.is_instance_of(r, pp)).collect();
        all.into_iter().find(|&r| self.op(r, "Player") != 0).unwrap_or(0)
    }

    /// `LevelInfo.Game` of the level.
    fn game_info(&mut self) -> u32 {
        if self.level == 0 {
            return 0;
        }
        self.op(self.level, "Game")
    }

    /// The game engine's `Exec`: DeusEx's (1000ed70), the game engine's (1038a030), then the engine's (10380ee0).
    fn engine_exec(&mut self, cmd: &str, out: &mut ExecOut) -> Res<bool> {
        let mut s = cmd;
        if parse_command(&mut s, "SAVEGAME") {
            if s.starts_with(|c: char| c.is_ascii_digit()) {
                self.save_request = Some((atoi(s), String::new()));
            }
            return Ok(true);
        }
        if parse_command(&mut s, "DELETEGAME") {
            if s.starts_with(|c: char| c.is_ascii_digit()) {
                if let Some(d) = self.save_slot_dir(atoi(s)) {
                    let _ = std::fs::remove_dir_all(d);
                }
            }
            return Ok(true);
        }
        // `OPEN` keeps the options of the current URL (`TRAVEL_Partial`), `START` replaces them (`TRAVEL_Absolute`);
        // both travel after the frame (`SetClientTravel`)
        if parse_command(&mut s, "OPEN") || parse_command(&mut s, "START") {
            self.travel_request = Some(crate::travel::TravelRequest { url: s.to_string(), items: false });
            return Ok(true);
        }
        // SERVERTRAVEL and SAY only on a server
        if parse_command(&mut s, "DISCONNECT") {
            // back to the entry level
            self.travel_request = Some(crate::travel::TravelRequest { url: "dx.dx".into(), items: false });
            return Ok(true);
        }
        if parse_command(&mut s, "RECONNECT") {
            self.engine_requests.push(EngineRequest::Reconnect);
            return Ok(true);
        }
        if parse_command(&mut s, "GETCURRENTTICKRATE") || parse_command(&mut s, "GETMAXTICKRATE") {
            // `%f` of the engine's `CurrentTickRate` / `GetMaxTickRate()`: this port keeps neither (a standalone game
            // runs unlimited; UNVERIFIED for the current rate)
            out.log("0.000000");
            return Ok(true);
        }
        if parse_command(&mut s, "GSPYLITE") {
            return Ok(true);
        }
        if parse_command(&mut s, "CANCEL") {
            // no pending level to cancel: the progress message is cleared
            return Ok(true);
        }
        if level_exec(cmd) {
            return Ok(true);
        }
        let game = self.game_info();
        if game != 0 && self.script_console_exec(game, cmd, out, 0)? {
            return Ok(true);
        }
        self.uengine_exec(cmd, out)
    }

    /// `UEngine::Exec` (Engine 10380ee0): the system, the object system, the client, then `FLUSH`.
    fn uengine_exec(&mut self, cmd: &str, out: &mut ExecOut) -> Res<bool> {
        let mut s = cmd;
        // `USystem::Exec` (Core 1016b790)
        if parse_command(&mut s, "EXIT") || parse_command(&mut s, "QUIT") {
            out.log("Closing by request");
            self.engine_requests.push(EngineRequest::Exit);
            return Ok(true);
        }
        if parse_command(&mut s, "RELAUNCH") {
            self.engine_requests.push(EngineRequest::Relaunch(s.to_string()));
            self.engine_requests.push(EngineRequest::Exit);
            return Ok(true);
        }
        if parse_command(&mut s, "MEMSTAT") || parse_command(&mut s, "CONFIGHASH") {
            return Ok(true);
        }
        // `UObject::StaticExec` (Core 10153110). The input system's key bindings are properties of the class `Input`
        // made by its constructor (one per key name): the host keeps them
        let (mut g, mut t) = (cmd, cmd);
        if (parse_command(&mut g, "GET") && parse_command(&mut g, "INPUT")) || (parse_command(&mut t, "SET") && parse_command(&mut t, "INPUT")) {
            if let Some(text) = self.host_exec(HostExec::InputConfig, cmd) {
                if !text.is_empty() {
                    out.log(text);
                }
                return Ok(true);
            }
        }
        if let Some(text) = self.config_console(cmd) {
            if !text.is_empty() {
                out.log(text);
            }
            return Ok(true);
        }
        // the client (`UClient::Exec` 10375940; WinDrv's adds nothing): `BRIGHTNESS` steps the brightness by a tenth,
        // back to 0 from 1, saves it, flushes and tells the player
        let mut c = cmd;
        if parse_command(&mut c, "BRIGHTNESS") {
            const SECTION: &str = "WinDrv.WindowsClient";
            let old: f32 = self.get_config(SECTION, "Brightness").trim().parse().unwrap_or(0.5);
            // `fadd QWORD 0.1` then `fst DWORD`; the comparison with 1.0 uses the unrounded sum
            let sum = old as f64 + 0.1;
            let new = if sum >= 1.0 { 0.0 } else { sum as f32 };
            if let Some(cfg) = self.reg.config.as_mut() {
                cfg.set("System", SECTION, "Brightness", &format!("{new:.6}"));
                let _ = cfg.save();
            }
            self.engine_requests.push(EngineRequest::Flush);
            let viewer = self.local_player();
            if viewer != 0 {
                // `ftol(Brightness * 10 + 1)`
                let level = (new as f64 * 10.0 + 1.0) as i32;
                self.call_by_name(viewer, "ClientMessage", vec![Value::Str(format!("Brightness level {level}/10")), Value::Name(0)])?;
            }
            return Ok(true);
        }
        let mut f = cmd;
        if parse_command(&mut f, "FLUSH") {
            self.engine_requests.push(EngineRequest::Flush);
            return Ok(true);
        }
        Ok(false)
    }

    /// `UObject::ScriptConsoleExec` (Core 10150860): runs `cmd` as an exec function of `obj`. Returns false when the
    /// first word names no exec function; true otherwise, also when a parameter could not be read (the call is then
    /// skipped and the error goes to the output).
    pub fn script_console_exec(&mut self, obj: u32, cmd: &str, out: &mut ExecOut, executor: u32) -> Res<bool> {
        let mut s = cmd;
        let Some(word) = parse_token(&mut s, 0x40, true) else { return Ok(false) };
        let Some(id) = self.reg.names.get(&word) else { return Ok(false) };
        let Some(loc) = self.resolve_function(obj, id) else { return Ok(false) };
        let (flags, params) = {
            let f = self.reg.function(loc);
            let params: Vec<PropDef> = f.props.iter().take_while(|p| p.flags & (CPF_PARM | CPF_RETURN_PARM) == CPF_PARM).cloned().collect();
            (f.flags, params)
        };
        if flags & FUNC_EXEC == 0 {
            return Ok(false);
        }
        let mut args = Vec::with_capacity(params.len());
        let mut executor = executor;
        for (i, p) in params.iter().enumerate() {
            if i == 0 && executor != 0 {
                if let K::Object = p.kind {
                    let fits = match &p.target {
                        Target::Object(c) => {
                            let path = self.reg.obj_path(*c).to_string();
                            match self.reg.class_by_path(&path) {
                                Some(want) => self.class_of_ref(executor).is_some_and(|have| self.is_a(have, want)),
                                None => true,
                            }
                        }
                        _ => true,
                    };
                    if fits {
                        args.push(Value::Obj(executor));
                        continue;
                    }
                } else {
                    executor = 0;
                }
            }
            parse_next(&mut s);
            match self.import_param(p, s) {
                Some((v, rest)) => {
                    args.push(v);
                    s = rest;
                }
                None if p.flags & CPF_OPTIONAL_PARM != 0 => break,
                None => {
                    // Core.int `BadProperty`, with the command's name (as the name table spells it) and the parameter's
                    out.log(format!("'{}': Bad or missing property '{}'", self.reg.names.str(id), self.reg.names.str(p.name)));
                    return Ok(true);
                }
            }
        }
        // parameters not read are zero (the parameter block starts cleared)
        for p in params.iter().skip(args.len()) {
            args.push(zero_of(p));
        }
        self.call_function(obj, loc, &mut args)?;
        Ok(true)
    }

    /// One parameter's `ImportText` (Core, `PPF_Localized`): its value and the rest of the line, `None` on failure.
    fn import_param<'a>(&mut self, p: &PropDef, s: &'a str) -> Option<(Value, &'a str)> {
        let num_end = |s: &'a str, ok: fn(char) -> bool| &s[s.find(|c: char| !ok(c)).unwrap_or(s.len())..];
        Some(match p.kind {
            // `UIntProperty::ImportText` 10165f00: digits or a minus sign start a number; the run of them is skipped
            K::Int => {
                let v = if s.starts_with(|c: char| c == '-' || c.is_ascii_digit()) { atoi(s) } else { 0 };
                (Value::Int(v), num_end(s, |c| c == '-' || c.is_ascii_digit()))
            }
            // `UFloatProperty::ImportText` 10166940: the number, then everything up to a comma, a parenthesis or the end
            K::Float => (Value::Float(atof(s)), &s[s.find([',', ')', '\r', '\n']).unwrap_or(s.len())..]),
            // `UBoolProperty::ImportText` 10166380: 1/True/0/False (and the localised words: English here)
            K::Bool => {
                let (t, rest) = read_token(s, 0x400, false)?;
                let on = ["1", "True"].iter().any(|w| t.eq_ignore_ascii_case(w));
                let off = ["0", "False"].iter().any(|w| t.eq_ignore_ascii_case(w));
                if !on && !off {
                    return None;
                }
                (Value::Bool(on), rest)
            }
            // `UByteProperty::ImportText` 10165aa0 (asm): an enum reads a token first; a value name is taken, else
            // digits must follow the token (so an enum never takes a number)
            K::Byte => {
                let mut at = s;
                if let Target::Enum(e) = &p.target {
                    let e = e.clone();
                    let (t, rest) = read_token(s, 0x400, false)?;
                    if let Some(names) = self.reg.enum_value_names(&e) {
                        if let Some(i) = names.iter().position(|&n| self.reg.names.str(n).eq_ignore_ascii_case(&t)) {
                            return Some((Value::Byte(i as u8), rest));
                        }
                    }
                    at = rest;
                }
                if !at.starts_with(|c: char| c.is_ascii_digit()) {
                    return None;
                }
                (Value::Byte(atoi(at) as u8), num_end(at, |c| c.is_ascii_digit()))
            }
            // `UNameProperty::ImportText` 101674d0
            K::Name => {
                let (t, rest) = read_token(s, 0x400, false)?;
                let v = if t.is_empty() { Value::Name(0) } else { self.name_value(&t) };
                (v, rest)
            }
            // `UStrProperty::ImportText` 10167980 without `PPF_Delimited`: the whole rest, the cursor stays
            K::Str | K::String => (Value::Str(s.to_string()), s),
            // `UObjectProperty::ImportText` 10166e20 / `UClassProperty::ImportText` 10167110
            K::Object | K::Class => {
                let (t, rest) = read_token(s, 0x400, true)?;
                if t.eq_ignore_ascii_case("None") {
                    return Some((Value::Obj(0), rest));
                }
                let rest = rest.trim_start_matches(' ');
                let mut chars = rest.chars();
                let quote = chars.next();
                let after = chars.as_str();
                let (class_name, name, rest) = if quote == Some('\'') {
                    let (o, r) = read_token(after, 0x400, true)?;
                    let r = r.strip_prefix('\'')?;
                    (Some(t), o, r)
                } else {
                    // the original steps over one more character here
                    (None, t, after)
                };
                let v = self.find_object_for(p, class_name.as_deref(), &name)?;
                (Value::Obj(v), rest)
            }
            _ => return None,
        })
    }

    /// `StaticFindObject(Class, ANY_PACKAGE, Name)` for an object or class parameter; a class parameter keeps only
    /// classes derived from its meta class (else None, still a success).
    fn find_object_for(&mut self, p: &PropDef, class_name: Option<&str>, name: &str) -> Option<u32> {
        let short = name.rsplit('.').next().unwrap_or(name);
        let class_param = matches!(p.target, Target::Class { .. });
        let wants_class = class_param || class_name.is_some_and(|c| c.eq_ignore_ascii_case("Class")) || matches!(&p.target, Target::Object(c) if self.reg.obj_path(*c).eq_ignore_ascii_case("Core.Class"));
        if wants_class {
            let c = self.reg.class_by_path(name).or_else(|| self.reg.class_by_name(short))?;
            if let Target::Class { meta, .. } = &p.target {
                let meta_path = self.reg.obj_path(*meta).to_string();
                if let Some(m) = self.reg.class_by_path(&meta_path) {
                    if !self.is_a(c, m) {
                        return Some(0);
                    }
                }
            }
            let path = self.reg.class(c).path.clone();
            return Some(self.reg.obj_id(&path).0);
        }
        // a live object of the parameter's class with that name
        let want = match (&p.target, class_name) {
            (_, Some(cn)) => self.reg.class_by_name(cn.rsplit('.').next().unwrap_or(cn)),
            (Target::Object(c), None) => {
                let path = self.reg.obj_path(*c).to_string();
                self.reg.class_by_path(&path)
            }
            _ => None,
        };
        let name_id = self.reg.names.get(short)?;
        (0..self.objects.len()).map(inst_ref).find(|&r| {
            self.is_live(r)
                && want.is_none_or(|w| self.class_of_ref(r).is_some_and(|c| self.is_a(c, w)))
                && matches!(self.get_prop(r, "Name", 0), Ok(Value::Name(n)) if n == name_id)
        })
    }
}

/// `ULevel::Exec` (Engine 1039dfe0): the net driver's and demo driver's commands (none in a standalone game), then the
/// demo commands, which this port does not have: they are taken and do nothing.
fn level_exec(cmd: &str) -> bool {
    let mut s = cmd;
    parse_command(&mut s, "DEMOREC") || parse_command(&mut s, "DEMOPLAY")
}

/// A parameter left unread: zero of its type.
fn zero_of(p: &PropDef) -> Value {
    match p.kind {
        K::Int => Value::Int(0),
        K::Float => Value::Float(0.0),
        K::Bool => Value::Bool(false),
        K::Byte => Value::Byte(0),
        K::Name => Value::Name(0),
        K::Str | K::String => Value::Str(String::new()),
        K::Object | K::Class => Value::Obj(0),
        _ => Value::Void,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_needs_a_word_boundary() {
        let mut s = "  rmode 5";
        assert!(parse_command(&mut s, "RMODE"));
        assert_eq!(s, "5");
        let mut s = "RMODEX 5";
        assert!(!parse_command(&mut s, "RMODE"));
        assert_eq!(s, "RMODEX 5");
        let mut s = "\tSHOT";
        assert!(parse_command(&mut s, "SHOT"));
        assert_eq!(s, "");
        // a dot or underscore is no letter: the command still matches
        let mut s = "get_x";
        assert!(parse_command(&mut s, "GET"));
        assert_eq!(s, "_x");
    }

    #[test]
    fn parse_token_reads_words_and_quotes() {
        let mut s = "  ActivateBelt 3";
        assert_eq!(parse_token(&mut s, 64, true).as_deref(), Some("ActivateBelt"));
        assert_eq!(s, " 3");
        let mut s = "\"a b\\\"c\" rest";
        assert_eq!(parse_token(&mut s, 64, true).as_deref(), Some("a b\"c"));
        assert_eq!(s, " rest");
        let mut s = "   ";
        assert_eq!(parse_token(&mut s, 64, true), None);
        // a long word is cut to max - 1 characters, the cursor still passes it
        let mut s = "abcdef x";
        assert_eq!(parse_token(&mut s, 4, false).as_deref(), Some("abc"));
        assert_eq!(s, " x");
    }

    #[test]
    fn read_token_forms() {
        assert_eq!(read_token("Foo.Bar baz", 0x400, true), Some(("Foo.Bar".into(), " baz")));
        assert_eq!(read_token("Foo.Bar baz", 0x400, false), Some(("Foo".into(), ".Bar baz")));
        assert_eq!(read_token("\"a\\41\\\\\" x", 0x400, false), Some(("aA\\".into(), " x")));
        // one character, not consumed
        assert_eq!(read_token("(x", 0x400, false), Some(("(".into(), "(x")));
        assert_eq!(read_token("", 0x400, false), Some(("".into(), "")));
        assert_eq!(read_token("\"open", 0x400, false), None);
    }

    #[test]
    fn crt_numbers() {
        assert_eq!(atoi("12abc"), 12);
        assert_eq!(atoi("--5"), 0);
        assert_eq!(atoi(" -7"), -7);
        assert_eq!(atof("1.5 2"), 1.5);
        assert_eq!(atof("x"), 0.0);
        assert_eq!(atof("2e3,"), 2000.0);
        assert_eq!(atof(".5"), 0.5);
    }

    #[test]
    fn parse_next_skips_comments() {
        let mut s = "  ; note\n  5";
        parse_next(&mut s);
        assert_eq!(s, "5");
    }
}

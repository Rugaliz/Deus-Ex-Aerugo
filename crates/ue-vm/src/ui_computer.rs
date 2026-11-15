//! `ComputerWindow`: the typewriter display (the InfoLink display of the HUD is the one user in the game scripts).
//! Everything printed is queued as timed events; `Tick` plays the due events into a display buffer of character cells
//! (word wrap, scrolling, fade-in per character) which `Draw` paints. Ported in behaviour from the `XComputerWindow`
//! functions of Extension.dll. Sounds go through the queue too (`SetTextSound`, `PlaySoundLater`): each character
//! plays the current text sound at the text cursor. Not done: queueing input (`GetInput`/`GetChar`; the answer to a
//! single-key wait is in `VirtualKeyPressed`); graphics queued by
//! `PrintGraphic` are drawn at their cell or pixel position.

use crate::ui_widgets::color_arg;
use crate::{Value, Vm, VmError};
use std::collections::VecDeque;

type Res<T> = Result<T, VmError>;

fn fl(a: &[Value], i: usize, d: f32) -> f32 {
    match a.get(i) {
        None | Some(Value::Void) => d,
        Some(v) => v.as_f32().unwrap_or(d),
    }
}
fn iv(a: &[Value], i: usize) -> i32 {
    a.get(i).and_then(Value::as_i32).unwrap_or(0)
}
fn bl(a: &[Value], i: usize, d: bool) -> bool {
    match a.get(i) {
        Some(Value::Void) | None => d,
        Some(v) => v.as_bool().unwrap_or(d),
    }
}
fn ob(a: &[Value], i: usize) -> u32 {
    match a.get(i) {
        Some(Value::Obj(r)) => *r,
        _ => 0,
    }
}

#[derive(Debug, Clone)]
enum What {
    Char(char, bool),
    Color([u8; 4]),
    Position(i32, i32),
    NewLine,
    /// Texture, height, column/x, row/y (-1: at the text position), pixel position (the width is stored, never read).
    Graphic(u32, i32, i32, i32, bool),
    Clear,
    FadeOut(f32),
    /// The sound each character plays from now on (`SetTextSound`).
    TextSound(u32),
    /// A sound played when its turn comes (`PlaySoundLater`).
    Sound(u32),
}

#[derive(Debug, Clone)]
struct Ev {
    time: f32,
    what: What,
}

#[derive(Debug, Clone)]
enum CellKind {
    Char(char),
    /// Texture, drawn at pixel (`col`, `row`) rather than at that cell.
    Graphic(u32, bool),
}

#[derive(Debug, Clone)]
struct Cell {
    kind: CellKind,
    col: i32,
    row: i32,
    color: [u8; 4],
    /// Seconds of fade-in left (0 = fully shown).
    fade: f32,
}

#[derive(Debug, Default)]
pub struct ComputerData {
    queue: VecDeque<Ev>,
    cells: Vec<Cell>,
    tx: i32,
    ty: i32,
    color: [u8; 4],
}

impl Vm {
    /// Characters and graphics currently displayed by all computer windows (for tests).
    pub fn ui_computer_cells(&self) -> usize {
        self.ui.computers.values().map(|d| d.cells.len()).sum()
    }

    fn cd(&mut self, w: u32) -> &mut ComputerData {
        self.ui.computers.entry(w).or_default()
    }

    pub(crate) fn computer_init(&mut self, w: u32) {
        for (n, v) in [("eventTimeInterval", 0.02), ("timeNextEvent", 0.0), ("timeLastEvent", 0.0), ("timeCurrent", 0.0), ("fadeSpeed", 0.0), ("throttle", 1.0), ("fadeOutTimer", 0.0), ("fadeOutStart", 0.0), ("computerSoundVolume", 1.0), ("cursorBlinkSpeed", 0.5), ("cursorNextEvent", 0.5)] {
            self.set_f_pub(w, n, v);
        }
        for (n, v) in [("textCols", 80), ("textRows", 25), ("fontWidth", 8), ("fontHeight", 16), ("cursorWidth", 8), ("cursorHeight", 2), ("textX", 0), ("textY", 0)] {
            self.set_i_pub(w, n, v);
        }
        for (n, v) in [("bWordWrap", false), ("bCursorVisible", true), ("bShowCursor", true), ("bWaitingForKey", false), ("bPauseProcessing", false)] {
            self.set_b_pub(w, n, v);
        }
        let white = [255, 255, 255, 255];
        let c = self.color_value_pub(white);
        let _ = self.set_prop(w, "fontColor", 0, c.clone());
        let _ = self.set_prop(w, "cursorColor", 0, c.clone());
        let _ = self.set_prop(w, "colGraphicTile", 0, c);
        let d = self.cd(w);
        d.color = white;
        // the rest of `XComputerWindow::Init` (1000d530): the player, the pause rule, the text window (full size)
        // and the hidden input edit window
        let root = self.ui_root_of_pub(w);
        let pawn = if root != 0 { self.wo_pub(root, "parentPawn") } else { 0 };
        self.set_o_pub(w, "player", pawn);
        if pawn != 0 && self.level != 0 && matches!(self.get_prop(self.level, "Pauser", 0), Ok(Value::Str(ref p)) if !p.is_empty()) {
            self.set_b_pub(w, "bIgnoreGamePaused", true);
        }
        let blink = self.wf_pub(w, "cursorBlinkSpeed");
        self.set_f_pub(w, "cursorNextEvent", blink);
        let window_class = self.reg.obj_id("Extension.Window").0;
        if let Ok(tw) = self.ui_create_window(window_class, w, true) {
            self.set_o_pub(w, "textWindow", tw);
            if tw != 0 {
                let _ = self.call_by_name(tw, "SetWindowAlignments", vec![Value::Byte(3), Value::Byte(3), Value::Float(0.0), Value::Float(0.0), Value::Float(0.0), Value::Float(0.0)]);
            }
        }
        let edit_class = self.reg.obj_id("Extension.EditWindow").0;
        if let Ok(iw) = self.ui_create_window(edit_class, w, false) {
            self.set_o_pub(w, "inputWindow", iw);
            if iw != 0 {
                let _ = self.call_by_name(iw, "SetTextMargins", vec![Value::Float(0.0), Value::Float(0.0)]);
                let _ = self.call_by_name(iw, "SetInsertionPointBlinkRate", vec![Value::Float(0.0), Value::Float(blink + blink)]);
            }
        }
        self.ui_set_selectability(w, true);
    }

    fn computer_resize(&mut self, w: u32) {
        if self.wo_ref_pub(w, "textFont") != 0 {
            let (fw, fh, cols, rows) = (self.wi_pub(w, "fontWidth"), self.wi_pub(w, "fontHeight"), self.wi_pub(w, "textCols"), self.wi_pub(w, "textRows"));
            let _ = self.native_ui(w, "SetSize", &mut vec![Value::Float((fw * cols) as f32), Value::Float((fh * rows) as f32)]);
        }
    }

    /// Queues an event at `timeNextEvent`; `step` advances the time for the next one (characters do).
    fn computer_queue(&mut self, w: u32, what: What, step: bool) {
        let t = self.wf_pub(w, "timeNextEvent");
        let interval = self.wf_pub(w, "eventTimeInterval");
        let now = self.wf_pub(w, "timeCurrent");
        self.cd(w).queue.push_back(Ev { time: t, what });
        self.set_f_pub(w, "timeLastEvent", now);
        if step {
            self.set_f_pub(w, "timeNextEvent", t + interval);
        }
    }

    pub(crate) fn native_computer(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        use Value::*;
        let v = match name {
            "SetBackgroundTextures" => {
                for i in 0..6 {
                    // the six texture slots of the script array
                    let r = ob(a, i);
                    if let Some((off, _)) = self.prop_at(this, "backgroundTextures") {
                        self.set_u32_at(this, off + 4 * i, r);
                    }
                }
                Void
            }
            "SetTextSize" => {
                self.set_i_pub(this, "textCols", iv(a, 0));
                self.set_i_pub(this, "textRows", iv(a, 1));
                self.computer_resize(this);
                Void
            }
            "SetTextWindowPosition" => {
                // the text window fills the computer window here, so there is nothing to place
                Void
            }
            "SetTextFont" => {
                self.set_o_pub(this, "textFont", ob(a, 0));
                self.set_i_pub(this, "fontWidth", iv(a, 1));
                self.set_i_pub(this, "fontHeight", iv(a, 2));
                let c = color_arg(a, 3);
                self.computer_queue(this, What::Color(c), false);
                self.computer_resize(this);
                Void
            }
            "SetFontColor" => {
                let c = color_arg(a, 0);
                self.computer_queue(this, What::Color(c), false);
                Void
            }
            "SetTextTiming" => {
                self.set_f_pub(this, "eventTimeInterval", fl(a, 0, 0.0));
                Void
            }
            "SetFadeSpeed" => {
                self.set_f_pub(this, "fadeSpeed", fl(a, 0, 0.0));
                Void
            }
            "SetCursorTexture" => {
                self.set_o_pub(this, "cursorTexture", ob(a, 0));
                self.set_i_pub(this, "cursorWidth", iv(a, 1));
                self.set_i_pub(this, "cursorHeight", iv(a, 2));
                Void
            }
            "SetCursorColor" => {
                let c = self.color_value_pub(color_arg(a, 0));
                let _ = self.set_prop(this, "cursorColor", 0, c);
                Void
            }
            "SetCursorBlinkSpeed" => {
                let s = fl(a, 0, 0.5);
                self.set_f_pub(this, "cursorBlinkSpeed", s);
                self.set_f_pub(this, "cursorNextEvent", s);
                Void
            }
            "ShowTextCursor" => {
                self.set_b_pub(this, "bShowCursor", bl(a, 0, true));
                Void
            }
            "SetTextSound" => {
                // queued: it applies to the characters printed after it (`XComputerWindow::SetTextSound`)
                self.computer_queue(this, What::TextSound(ob(a, 0)), false);
                Void
            }
            "PlaySoundLater" => {
                self.computer_queue(this, What::Sound(ob(a, 0)), false);
                Void
            }
            "PrintImmediate" => {
                // `XComputerWindow::PrintImmediate`: no timing and no text sound from now on, characters without fade
                self.set_f_pub(this, "eventTimeInterval", 0.0);
                self.set_o_pub(this, "textSound", 0);
                let text = match a.first() {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                for c in text.chars() {
                    let what = if c == '^' { What::NewLine } else { What::Char(c, true) };
                    self.computer_queue(this, what, true);
                }
                Void
            }
            "SetTypingSound" => {
                self.set_o_pub(this, "typingSound", ob(a, 0));
                Void
            }
            "SetComputerSoundVolume" => {
                self.set_f_pub(this, "computerSoundVolume", fl(a, 0, 1.0));
                Void
            }
            "SetTypingSoundVolume" => Void,
            "ClearScreen" => {
                self.computer_queue(this, What::Clear, false);
                Void
            }
            "ClearLine" => {
                let row = iv(a, 0);
                self.cd(this).cells.retain(|c| !(matches!(c.kind, CellKind::Char(_)) && c.row == row));
                Void
            }
            "Print" => {
                let text = match a.first() {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                self.set_b_pub(this, "bWaitingForKey", false);
                for c in text.chars() {
                    let what = if c == '^' { What::NewLine } else { What::Char(c, false) };
                    self.computer_queue(this, what, true);
                }
                if bl(a, 1, false) {
                    self.computer_queue(this, What::NewLine, true);
                }
                Void
            }
            "PrintLn" => {
                self.computer_queue(this, What::NewLine, true);
                Void
            }
            "PrintGraphic" => {
                // (Graphic, Width, Height, posX, posY, bStatic, bPixelPos): positions default to -1 (`execPrintGraphic`);
                // bStatic is stored and never read
                let opt = |i: usize| match a.get(i) {
                    Some(Value::Void) | None => -1,
                    Some(v) => v.as_i32().unwrap_or(-1),
                };
                let what = What::Graphic(ob(a, 0), iv(a, 2), opt(3), opt(4), bl(a, 6, false));
                self.computer_queue(this, what, false);
                Void
            }
            "SetTextPosition" => {
                self.computer_queue(this, What::Position(iv(a, 0), iv(a, 1)), false);
                Void
            }
            "IsBufferFlushed" => Bool(self.cd(this).queue.is_empty()),
            // `XComputerWindow::Pause` (1000db20): -1 (also an omitted length) stops processing until `Resume`; any other
            // length delays the next event by that much (probe `computer.paused`)
            "Pause" => {
                let t = fl(a, 0, -1.0);
                if t == -1.0 {
                    self.set_b_pub(this, "bPauseProcessing", true);
                } else {
                    let n = self.wf_pub(this, "timeNextEvent");
                    self.set_f_pub(this, "timeNextEvent", n + t);
                }
                Void
            }
            "Resume" => {
                self.set_b_pub(this, "bPauseProcessing", false);
                Void
            }
            "IsPaused" => Bool(self.wb_pub(this, "bPauseProcessing")),
            "SetThrottle" => {
                self.set_f_pub(this, "throttle", fl(a, 0, 1.0));
                Void
            }
            "GetThrottle" => Float(self.wf_pub(this, "throttle")),
            "ResetThrottle" => {
                self.set_f_pub(this, "throttle", 1.0);
                Void
            }
            "EnableWordWrap" => {
                self.set_b_pub(this, "bWordWrap", bl(a, 0, true));
                Void
            }
            "FadeOutText" => {
                let d = fl(a, 0, 2.0);
                self.computer_queue(this, What::FadeOut(d), false);
                Void
            }
            // input and the sound queue are not part of this port yet
            "GetInput" | "GetChar" => Void,
            _ => return None,
        };
        Some(Ok(v))
    }

    /// `XComputerWindow::InvokeComputerInputFinished` (1000ef80): processing pauses while `ComputerInputFinished(key,
    /// value)` goes to the window and up its parents until one returns true; then the queue resumes from now.
    fn computer_input_finished(&mut self, w: u32, key: &str, value: &str) {
        self.set_b_pub(w, "bPauseProcessing", true);
        let mut cur = w;
        while cur != 0 {
            let (r, _) = self.ui_script_event(cur, "ComputerInputFinished", vec![Value::Str(key.to_string()), Value::Str(value.to_string())]);
            if r.as_bool().unwrap_or(false) {
                break;
            }
            cur = self.wo_pub(cur, "winParent");
        }
        let now = self.wf_pub(w, "timeCurrent");
        self.set_f_pub(w, "timeNextEvent", now);
        self.set_b_pub(w, "bPauseProcessing", false);
    }

    /// The window's native events: `Tick` plays the queue (`Draw` is in `computer_draw`), `VisibilityChanged` arms the
    /// `ComputerStart` event, `VirtualKeyPressed` answers a wait for a single key.
    pub(crate) fn computer_event(&mut self, w: u32, name: &str, args: &mut Vec<Value>) -> Option<Value> {
        match name {
            "Tick" => {}
            // `XComputerWindow::VisibilityChanged` (1000ee70): the first showing asks the next tick for `ComputerStart`
            "VisibilityChanged" => {
                let (_, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                if bl(args, 0, false) && !self.wb_pub(w, "bComputerStartInvoked") {
                    self.set_b_pub(w, "bInvokeComputerStart", true);
                }
                return Some(Value::Void);
            }
            // `XComputerWindow::VirtualKeyPressed` (1000f530): while waiting for a key (`GetChar`) any key but Esc is the
            // answer: echoed when asked (at once: no interval, no text sound, no fade), the typing sound at the text
            // cursor, then `ComputerInputFinished(inputKey, key)`. The wait is not ended here. Otherwise the wait ends and
            // the script's event decides.
            "VirtualKeyPressed" => {
                let key = iv(args, 0);
                if self.wb_pub(w, "bWaitingForKey") && key != 0x1b {
                    let text: String = char::from_u32((key & 0xffff) as u32).filter(|&c| c != '\0').map(String::from).unwrap_or_default();
                    if self.wb_pub(w, "bEchoKey") {
                        let (interval, sound) = (self.wf_pub(w, "eventTimeInterval"), self.wo_pub(w, "textSound"));
                        self.set_f_pub(w, "eventTimeInterval", 0.0);
                        self.set_o_pub(w, "textSound", 0);
                        for c in text.chars() {
                            let what = if c == '^' { What::NewLine } else { What::Char(c, true) };
                            self.computer_queue(w, what, true);
                        }
                        self.set_f_pub(w, "eventTimeInterval", interval);
                        self.set_o_pub(w, "textSound", sound);
                        self.set_b_pub(w, "bEchoKey", false);
                    }
                    let s = self.wo_pub(w, "typingSound");
                    if s != 0 {
                        self.computer_sound(w, s);
                    }
                    let input_key = match self.get_prop(w, "inputKey", 0) {
                        Ok(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    self.computer_input_finished(w, &input_key, &text);
                    return Some(Value::Bool(true));
                }
                self.set_b_pub(w, "bWaitingForKey", false);
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                return Some(r);
            }
            _ => return None,
        }
        // `XComputerWindow::Tick` (1000f7b0): nothing for a window being destroyed (`bIgnoreTick`); the first tick after
        // creation only clears `bFirstTick` (on in the class defaults), so the terminal starts one frame later
        if self.wb_pub(w, "bIgnoreTick") {
            return Some(Value::Void);
        }
        if self.wb_pub(w, "bFirstTick") {
            self.set_b_pub(w, "bFirstTick", false);
            return Some(Value::Void);
        }
        // the game's pause holds the terminal: on the first tick of a pause processing stops and nothing else
        // happens; when the pause ends, processing resumes (unless the window was created during a pause)
        if !self.wb_pub(w, "bIgnoreGamePaused") {
            let player = self.wo_pub(w, "player");
            let level = if player != 0 { self.wo_pub(player, "Level") } else { 0 };
            if level != 0 {
                let paused = matches!(self.get_prop(level, "Pauser", 0), Ok(Value::Str(ref p)) if !p.is_empty());
                if paused && !self.wb_pub(w, "bGamePaused") {
                    self.set_b_pub(w, "bGamePaused", true);
                    self.set_b_pub(w, "bPauseProcessing", true);
                    return Some(Value::Void);
                }
                if !paused && self.wb_pub(w, "bGamePaused") {
                    self.set_b_pub(w, "bGamePaused", false);
                    self.set_b_pub(w, "bPauseProcessing", false);
                }
            }
        }
        // `ComputerStart` once, after the first showing: to the window and every parent having it (results ignored)
        if self.wb_pub(w, "bInvokeComputerStart") {
            self.set_b_pub(w, "bInvokeComputerStart", false);
            self.set_b_pub(w, "bComputerStartInvoked", true);
            let mut cur = w;
            while cur != 0 {
                self.ui_script_event(cur, "ComputerStart", vec![]);
                cur = self.wo_pub(cur, "winParent");
            }
        }
        if self.wb_pub(w, "bGamePaused") {
            return Some(Value::Void);
        }
        let dt = fl(args, 0, 0.0);
        // cursor blink
        let blink = self.wf_pub(w, "cursorBlinkSpeed");
        if blink > 0.0 {
            let left = self.wf_pub(w, "cursorNextEvent") - dt;
            if left < 0.0 {
                self.set_f_pub(w, "cursorNextEvent", left + blink);
                let v = self.wb_pub(w, "bCursorVisible");
                self.set_b_pub(w, "bCursorVisible", !v);
            } else {
                self.set_f_pub(w, "cursorNextEvent", left);
            }
        }
        if !self.wb_pub(w, "bPauseProcessing") {
            let now = self.wf_pub(w, "timeCurrent") + dt * self.wf_pub(w, "throttle");
            self.set_f_pub(w, "timeCurrent", now);
            let interval = self.wf_pub(w, "eventTimeInterval");
            if interval < now - self.wf_pub(w, "timeLastEvent") {
                self.set_f_pub(w, "timeNextEvent", now + interval);
            }
            self.computer_process(w, now);
            let fade = self.wf_pub(w, "fadeOutTimer");
            if fade > 0.0 {
                let left = (fade - dt).max(0.0);
                self.set_f_pub(w, "fadeOutTimer", left);
                if left == 0.0 {
                    let mut cur = w;
                    while cur != 0 {
                        self.ui_script_event(cur, "ComputerFadeOutCompleted", vec![]);
                        cur = self.wo_pub(cur, "winParent");
                    }
                }
            }
        }
        if self.wf_pub(w, "fadeSpeed") > 0.0 {
            for c in &mut self.cd(w).cells {
                c.fade = (c.fade - dt).max(0.0);
            }
        }
        Some(Value::Void)
    }

    fn computer_process(&mut self, w: u32, now: f32) {
        let (cols, rows) = (self.wi_pub(w, "textCols").max(1), self.wi_pub(w, "textRows").max(1));
        let wrap = self.wb_pub(w, "bWordWrap");
        let fade_speed = self.wf_pub(w, "fadeSpeed");
        let blink = self.wf_pub(w, "cursorBlinkSpeed");
        loop {
            let due = self.cd(w).queue.front().map(|e| e.time < now).unwrap_or(false);
            if !due {
                break;
            }
            let ev = self.cd(w).queue.pop_front().unwrap();
            if let What::Char(..) = ev.what {
                self.computer_char(w, ev.what, cols, rows, wrap, fade_speed, blink);
                // every character plays the text sound at the cursor, placed or not (`ProcessQueuedBuffer`)
                let s = self.wo_pub(w, "textSound");
                self.computer_sound(w, s);
                continue;
            }
            match ev.what {
                What::TextSound(s) => self.set_o_pub(w, "textSound", s),
                What::Sound(s) => self.computer_sound(w, s),
                What::Char(..) => {}
                What::Color(c) => self.cd(w).color = c,
                What::Position(x, y) => {
                    let d = self.cd(w);
                    d.tx = x;
                    d.ty = y;
                }
                What::NewLine => {
                    let d = self.cd(w);
                    d.tx = 0;
                    d.ty += 1;
                    if d.ty == rows {
                        d.ty = rows - 1;
                        Self::computer_scroll(d);
                    }
                }
                What::Graphic(tex, height, x, y, pixels) => {
                    // `ProcessQueuedBuffer` case 5: the cursor shows again, the entry is placed
                    self.set_b_pub(w, "bCursorVisible", true);
                    self.set_f_pub(w, "cursorNextEvent", blink);
                    let fh = self.wi_pub(w, "fontHeight");
                    let d = self.cd(w);
                    d.cells.push(Cell { kind: CellKind::Graphic(tex, pixels), col: x, row: y, color: [255; 4], fade: 0.0 });
                    Self::computer_place_graphic(d, fh, rows, height);
                }
                What::Clear => {
                    self.set_b_pub(w, "bWaitingForKey", false);
                    let d = self.cd(w);
                    d.cells.clear();
                    d.tx = 0;
                    d.ty = 0;
                }
                What::FadeOut(t) => {
                    self.set_f_pub(w, "fadeOutStart", t);
                    self.set_f_pub(w, "fadeOutTimer", t);
                }
            }
        }
        self.set_i_pub(w, "textX", self.ui.computers.get(&w).map(|d| d.tx).unwrap_or(0));
        self.set_i_pub(w, "textY", self.ui.computers.get(&w).map(|d| d.ty).unwrap_or(0));
    }

    /// A due character (`ProcessQueuedBuffer` case 0, `CalculateCharDisplayPosition`): word wrap, scrolling, the cell.
    #[allow(clippy::too_many_arguments)]
    fn computer_char(&mut self, w: u32, what: What, cols: i32, rows: i32, wrap: bool, fade_speed: f32, blink: f32) {
        let What::Char(c, immediate) = what else { return };
        self.set_b_pub(w, "bCursorVisible", true);
        self.set_f_pub(w, "cursorNextEvent", blink);
        // does the rest of the word fit in the line?
        let d = self.cd(w);
        if d.tx == 0 && c == ' ' && wrap {
            return;
        }
        let mut wrap_now = false;
        if wrap {
            let mut x = d.tx;
            let mut chars = std::iter::once(c).chain(d.queue.iter().map_while(|e| match &e.what {
                What::Char(c, _) => Some(*c),
                _ => None,
            }));
            wrap_now = true;
            while x < cols {
                match chars.next() {
                    Some(ch) if ch != ' ' => x += 1,
                    _ => {
                        wrap_now = false;
                        break;
                    }
                }
            }
        }
        let d = self.cd(w);
        let (col, row);
        if wrap_now || d.tx + 1 == cols {
            d.tx = 0;
            d.ty += 1;
            if d.ty == rows {
                d.ty = rows - 1;
                Self::computer_scroll(d);
            }
            if wrap && c == ' ' {
                return;
            }
            col = 0;
            row = d.ty;
        } else {
            col = d.tx;
            row = d.ty;
        }
        d.tx = col + 1;
        let color = d.color;
        d.cells.push(Cell { kind: CellKind::Char(c), col, row, color, fade: if immediate { 0.0 } else { fade_speed } });
    }

    /// `XComputerWindow::PlaySoundNow` (10010ae0): a window sound at the centre of the text cursor's cell.
    fn computer_sound(&mut self, w: u32, sound: u32) {
        let (fw, fh) = (self.wi_pub(w, "fontWidth"), self.wi_pub(w, "fontHeight"));
        let (tx, ty) = self.ui.computers.get(&w).map(|d| (d.tx, d.ty)).unwrap_or((0, 0));
        let x = fw as f32 * 0.5 + (tx * fw) as f32;
        let y = fh as f32 * 0.5 + (ty * fh) as f32;
        let v = self.wf_pub(w, "soundVolume");
        self.ui_play_sound_at(w, sound, v, 1.0, x, y);
    }

    /// `CalculateGraphicDisplayPosition` (10010990) for the graphic just added. Without a row it goes at the text row: the
    /// text moves down by its height in whole rows, or, when it would pass the bottom, the screen scrolls
    /// `fontHeight * TextY - textRows + height` times (the original mixes pixels and rows here) and the graphic's row
    /// becomes `fontHeight * textRows - height`, written to its old slot (lost when a scroll removed entries before it).
    /// With a row, the text row goes under it. Without a column it takes the text column.
    fn computer_place_graphic(d: &mut ComputerData, fh: i32, rows: i32, height: i32) {
        let i = d.cells.len() - 1;
        let fh_div = if fh == 0 { 1 } else { fh };
        if d.cells[i].row == -1 {
            d.cells[i].row = d.ty;
            if fh * d.ty + height <= fh * rows {
                d.ty += height / fh_div;
            } else {
                let n = fh * d.ty - (fh * rows) / fh_div + height;
                for _ in 0..n.max(0) {
                    Self::computer_scroll(d);
                }
                if let Some(c) = d.cells.get_mut(i) {
                    c.row = fh * rows - height;
                }
                d.ty = rows - 1;
            }
        } else {
            d.ty = (height + d.cells[i].row) / fh_div + 1;
        }
        let tx = d.tx;
        if let Some(c) = d.cells.get_mut(i) {
            if c.col == -1 {
                c.col = tx;
            }
        }
    }

    /// One line up: rows shift, what leaves the top is dropped.
    fn computer_scroll(d: &mut ComputerData) {
        for c in &mut d.cells {
            c.row -= 1;
        }
        d.cells.retain(|c| c.row >= 0);
    }

    /// `XComputerWindow::Draw`: background textures, then the display buffer and the cursor.
    pub(crate) fn computer_draw(&mut self, w: u32, g: u32) {
        let font = self.wo_ref_pub(w, "textFont");
        let (fw, fh) = (self.wi_pub(w, "fontWidth") as f32, self.wi_pub(w, "fontHeight") as f32);
        // background: six 256-pixel tiles, translucent
        let bg: Vec<u32> = match self.prop_at(w, "backgroundTextures") {
            Some((off, _)) => (0..6).map(|i| self.u32_at(w, off + 4 * i)).collect(),
            None => Vec::new(),
        };
        // translucent whatever the background (graphics are drawn with it too)
        self.gc_mut(g).set_style(3);
        if bg.first().copied().unwrap_or(0) != 0 {
            let st = self.gc_state(g);
            for (i, t) in bg.iter().enumerate() {
                if *t != 0 {
                    let (tw, th) = self.texture_size(*t);
                    self.gc_draw_stretched_icon(&st, (i % 3) as f32 * 256.0, (i / 3) as f32 * 256.0, tw, th, *t);
                }
            }
        }
        {
            let st = self.gc_mut(g);
            // the alignments and word wrap stay as the GC has them
            st.normal_font = font;
            st.bold_font = font;
        }
        let fade_speed = self.wf_pub(w, "fadeSpeed");
        let fade_out = self.wf_pub(w, "fadeOutTimer");
        let fade_out_start = self.wf_pub(w, "fadeOutStart").max(0.0001);
        let tile = self.color_bytes(w, "colGraphicTile");
        let cells = self.cd(w).cells.clone();
        for c in &cells {
            match &c.kind {
                CellKind::Char(ch) => {
                    // fade in; the whole screen fades out together
                    // the screen's fade-out, else the character's own fade-in (never both)
                    let k = if fade_out > 0.0 {
                        fade_out / fade_out_start
                    } else if fade_speed > 0.0 && c.fade > 0.0 {
                        (fade_speed - c.fade) / fade_speed
                    } else {
                        1.0
                    };
                    let col = [(c.color[0] as f32 * k) as u8, (c.color[1] as f32 * k) as u8, (c.color[2] as f32 * k) as u8, c.color[3]];
                    self.gc_mut(g).text_color = col;
                    let st = self.gc_state(g);
                    self.gc_draw_text(&st, c.col as f32 * fw, c.row as f32 * fh, fw, fh, &ch.to_string());
                }
                CellKind::Graphic(t, pixels) => {
                    self.gc_mut(g).tile_color = tile;
                    let st = self.gc_state(g);
                    let (tw, th) = self.texture_size(*t);
                    // `DrawIcon` at the pixel position when the entry's pixel flag is set, else at the cell
                    let (x, y) = if *pixels { (c.col as f32, c.row as f32) } else { (c.col as f32 * fw, c.row as f32 * fh) };
                    self.gc_draw_stretched_icon(&st, x, y, tw, th, *t);
                }
            }
        }
        // the cursor block after the last character
        // not while the input window shows (it draws its own caret)
        let ctex = self.wo_ref_pub(w, "cursorTexture");
        let input = self.wo_pub(w, "inputWindow");
        if ctex != 0 && input != 0 && !self.ui_visible_recursive(input) && self.wb_pub(w, "bCursorVisible") && self.wb_pub(w, "bShowCursor") {
            let (cw, ch) = (self.wi_pub(w, "cursorWidth") as f32, self.wi_pub(w, "cursorHeight") as f32);
            let (tx, ty) = self.ui.computers.get(&w).map(|d| (d.tx, d.ty)).unwrap_or((0, 0));
            self.gc_mut(g).set_style(2);
            self.gc_mut(g).tile_color = self.color_bytes(w, "cursorColor");
            let st = self.gc_state(g);
            self.gc_draw_pattern(&st, tx as f32 * fw, (ty + 1) as f32 * fh - ch, cw, ch, 0.0, 0.0, ctex);
        }
    }
}

#[cfg(test)]
mod graphic_tests {
    use super::*;

    fn with(ty: i32, tx: i32, col: i32, row: i32) -> ComputerData {
        let mut d = ComputerData { ty, tx, ..Default::default() };
        d.cells.push(Cell { kind: CellKind::Char('a'), col: 0, row: 0, color: [255; 4], fade: 0.0 });
        d.cells.push(Cell { kind: CellKind::Graphic(1, false), col, row, color: [255; 4], fade: 0.0 });
        d
    }

    /// `CalculateGraphicDisplayPosition`'s three paths (font height 10, five rows, a graphic 25 high).
    #[test]
    fn a_graphic_is_placed_as_calculate_graphic_display_position_does() {
        // fits under the text row: placed there, the text moves down two whole rows, the column is the text column
        let mut d = with(1, 3, -1, -1);
        Vm::computer_place_graphic(&mut d, 10, 5, 25);
        assert_eq!((d.cells[1].col, d.cells[1].row, d.ty), (3, 1, 3));
        // does not fit: 10*4 - 5 + 25 = 60 scrolls clear the screen (the graphic too), the text row is the last one
        let mut d = with(4, 0, -1, -1);
        Vm::computer_place_graphic(&mut d, 10, 5, 25);
        assert!(d.cells.is_empty());
        assert_eq!(d.ty, 4);
        // an explicit row: the text goes under the graphic
        let mut d = with(0, 0, 6, 2);
        Vm::computer_place_graphic(&mut d, 10, 5, 25);
        assert_eq!((d.cells[1].col, d.cells[1].row, d.ty), (6, 2, 3));
    }
}

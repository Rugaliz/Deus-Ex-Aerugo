//! `EditWindow`: an editable text field (single or multi line) with an insertion point, a selection, undo and a blinking
//! caret. Ported in behaviour from the `XEditWindow` functions of Extension.dll, with these simplifications: lines break
//! only at newlines (the caret is not placed inside wrapped lines), the clipboard is internal to the port, no sounds.

use crate::ui_gc::GcState;
use crate::ui_widgets::color_arg;
use crate::{Value, Vm, VmError};

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

/// One undo record (`XEditWindow::AddUndo`): at `pos`, the text `old` was replaced by `new`.
#[derive(Debug, Clone, Default)]
pub struct UndoRec {
    pub pos: usize,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Default)]
pub struct EditData {
    /// The undo records (`bufferList`); `cur` is `currentUndo` (records before it are done), `unchanged` is
    /// `unchangedUndo` (-1: changed whatever the position). The text has changed when they differ.
    pub undo: Vec<UndoRec>,
    pub cur: i32,
    pub unchanged: i32,
    pub max_undos: i32,
    pub blink_delay: f32,
    pub cursor_showing: bool,
    pub dragging: bool,
    /// `bSelectWords`: while set (from a Ctrl+click or an even click count until the left button goes up) the
    /// selection grows to whole words.
    pub select_words: bool,
    /// `dragDelay`: time to the next drag step while the button is held (`Tick`).
    pub drag_delay: f32,
}

impl Vm {
    fn ed(&self, w: u32) -> Option<&EditData> {
        self.ui.edits.get(&w)
    }
    fn edm(&mut self, w: u32) -> Option<&mut EditData> {
        self.ui.edits.get_mut(&w)
    }

    pub(crate) fn edit_init(&mut self, w: u32) {
        let _ = self.set_prop(w, "hAlign", 0, Value::Byte(0));
        let _ = self.set_prop(w, "vAlign", 0, Value::Byte(0));
        self.set_f_pub(w, "hMargin", 3.0);
        self.set_f_pub(w, "vMargin", 0.0);
        for (n, v) in [("bEditable", true), ("bUppercaseOnly", false), ("bSingleLine", false), ("bSpecialText", false)] {
            self.set_b_pub(w, n, v);
        }
        for (n, v) in [("insertPos", 0), ("insertHookPos", 0), ("selectStart", 0), ("selectEnd", 0), ("maxSize", 200000)] {
            self.set_i_pub(w, n, v);
        }
        let _ = self.set_prop(w, "insertType", 0, Value::Byte(0));
        let (black, white, grey) = (self.color_value_pub([0, 0, 0, 255]), self.color_value_pub([255, 255, 255, 255]), self.color_value_pub([196, 196, 196, 255]));
        let _ = self.set_prop(w, "inverseColor", 0, black);
        let _ = self.set_prop(w, "insertColor", 0, white.clone());
        let _ = self.set_prop(w, "selectColor", 0, grey);
        let _ = self.set_prop(w, "editCursorColor", 0, white);
        for n in ["insertTexture", "selectTexture", "editCursor", "editCursorShadow", "typeSound", "deleteSound", "enterSound", "moveSound"] {
            self.set_o_pub(w, n, 0);
        }
        for (n, v) in [("blinkStart", 0.75), ("blinkPeriod", 1.0), ("insertPrefWidth", 0.0), ("insertPrefHeight", 0.0)] {
            self.set_f_pub(w, n, v);
        }
        let g = self.wo_ref_pub(w, "wGc");
        if g != 0 {
            self.gc_mut(g).parse_meta = false;
        }
        self.ui.edits.insert(w, EditData { undo: Vec::new(), cur: 0, unchanged: 0, max_undos: 64, blink_delay: 0.75, cursor_showing: true, dragging: false, select_words: false, drag_delay: 0.0 });
        self.ui_set_selectability(w, true);
    }

    fn edit_text(&self, w: u32) -> String {
        match self.get_prop(w, "text", 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        }
    }

    fn edit_set_text_var(&mut self, w: u32, s: &str) {
        let _ = self.set_prop(w, "text", 0, Value::Str(s.to_string()));
    }

    fn edit_gc(&self, w: u32) -> GcState {
        let g = self.wo_ref_pub(w, "wGc");
        let mut s = if g != 0 { self.gc_state(g) } else { GcState::default() };
        s.normal_font = self.wo_ref_pub(w, "normalFont");
        s.bold_font = s.normal_font;
        s.word_wrap = false;
        s.parse_meta = false;
        s
    }

    fn edit_line_height(&mut self, w: u32) -> f32 {
        let s = self.edit_gc(w);
        let h = self.font_height(&s, false);
        self.set_f_pub(w, "lineHeight", h);
        h
    }

    pub(crate) fn edit_gc_pub(&self, w: u32) -> GcState {
        self.edit_gc(w)
    }

    pub(crate) fn edit_line_height_pub(&mut self, w: u32) -> f32 {
        self.edit_line_height(w)
    }

    /// The rows of the text (`GenerateLines` with the edit's GC) and the line height.
    fn edit_rows(&mut self, w: u32) -> (Vec<u32>, Vec<crate::ui_paint::TextRow>, f32) {
        let gc = self.edit_gc(w);
        let lh = self.edit_line_height(w);
        let (s, rows) = self.large_text_rows(w, &gc);
        (s, rows, lh)
    }

    /// `PosToRowCol`: the row holding a character position (the last row starting at or before it; row 0 without
    /// rows) and the column in it.
    fn edit_row_of(rows: &[crate::ui_paint::TextRow], pos: usize) -> (usize, usize) {
        let r = rows.iter().rposition(|row| row.start <= pos).unwrap_or(0);
        let start = rows.get(r).map(|row| row.start).unwrap_or(0);
        (r, pos.saturating_sub(start))
    }

    /// `RowToY`: the top of a row (rows counted as at least one for the layout).
    fn edit_row_y(&mut self, w: u32, rows: usize, lh: f32, r: usize) -> f32 {
        let y0 = self.large_text_top(w, rows.max(1), lh);
        y0 + r as f32 * (lh + self.wf_pub(w, "vSpace"))
    }

    /// `ColToX`: the row's left edge (`hAlign` 0: the truncated margin; the row width is not clamped here) plus the
    /// pixel offset of the column from the row start (`CharToPixel`).
    fn edit_col_x(&mut self, w: u32, s: &[u32], rows: &[crate::ui_paint::TextRow], r: usize, col: usize) -> f32 {
        let (start, rw) = rows.get(r).map(|row| (row.start, row.width)).unwrap_or((0, 0.0));
        let base = if self.wi_pub(w, "hAlign") == 0 { self.wf_pub(w, "hMargin").trunc() } else { self.large_text_row_x(w, rw) };
        let end = (start + col).min(s.len());
        if end <= start {
            return base;
        }
        let prefix: String = s[start..end].iter().filter_map(|&c| char::from_u32(c)).collect();
        let gc = self.edit_gc(w);
        base + self.text_extent(&gc, 0.0, &prefix).0
    }

    /// The character position nearest to a point (`XYToPos`): the row under y (clamped to the rows), then the
    /// column whose boundary is nearest to x.
    fn edit_xy_pos(&mut self, w: u32, x: f32, y: f32) -> usize {
        let (s, rows, lh) = self.edit_rows(w);
        if rows.is_empty() {
            return 0;
        }
        let y0 = self.edit_row_y(w, rows.len(), lh, 0);
        let pitch = (lh + self.wf_pub(w, "vSpace")).max(1.0);
        let r = (((y - y0) / pitch).floor().max(0.0) as usize).min(rows.len() - 1);
        let (start, len) = (rows[r].start, rows[r].len);
        let base = self.edit_col_x(w, &s, &rows, r, 0);
        let gc = self.edit_gc(w);
        let target = x - base;
        let mut prev = 0.0f32;
        for i in start..start + len {
            let prefix: String = s[start..=i].iter().filter_map(|&c| char::from_u32(c)).collect();
            let wd = self.text_extent(&gc, 0.0, &prefix).0;
            if target < (prev + wd) * 0.5 {
                return i;
            }
            prev = wd;
        }
        start + len
    }

    /// `ComputeCursorConfig` (1001f680): the caret rectangle (`insertX/Y/Width/Height`, also the `showArea`) and the
    /// selection's rows and x positions.
    fn edit_cursor_config(&mut self, w: u32) {
        let (s, rows, lh) = self.edit_rows(w);
        let gc = self.edit_gc(w);
        let pos = (self.wi_pub(w, "insertPos").max(0) as usize).min(s.len());
        let c = s.get(pos).copied().filter(|&c| !((9..=13).contains(&c) || c == 32)).and_then(char::from_u32).unwrap_or(' ');
        let cw = self.text_extent(&gc, 0.0, &c.to_string()).0;
        let (pw, ph) = (self.wf_pub(w, "insertPrefWidth"), self.wf_pub(w, "insertPrefHeight"));
        let v = self.wf_pub(w, "vSpace");
        let kind = self.wi_pub(w, "insertType");
        let tex = self.wo_ref_pub(w, "insertTexture");
        let (tw, th) = self.texture_size(tex);
        let (iw, ih) = match kind {
            0 => (if pw < 1.0 { 2.0 } else { pw }, lh),
            1 => (cw.max(1.0), if ph < 1.0 { 2.0 } else { ph }),
            2 => (cw.max(1.0), lh + v - (v * 0.5).trunc()),
            _ => (if pw < 1.0 { tw } else { pw }, if ph < 1.0 { th } else { ph }),
        };
        let (r, col) = Self::edit_row_of(&rows, pos);
        let mut x = self.edit_col_x(w, &s, &rows, r, col);
        let mut y = self.edit_row_y(w, rows.len(), lh, r);
        if kind == 0 || kind == 3 {
            x -= (iw * 0.5).trunc();
            y += ((lh - ih) * 0.5).trunc();
        } else {
            x += ((cw - iw) * 0.5).trunc();
            y += (lh - ih).trunc();
        }
        for (n, val) in [("insertX", x), ("insertY", y), ("insertWidth", iw), ("insertHeight", ih), ("showAreaX", x), ("showAreaY", y), ("showAreaWidth", iw), ("showAreaHeight", ih)] {
            self.set_f_pub(w, n, val);
        }
        let (a, b) = self.edit_selection(w);
        let (mut sr, mut er, mut sx, mut ex) = (-1i32, -1i32, 0.0f32, 0.0f32);
        if a < b {
            let (r0, c0) = Self::edit_row_of(&rows, a.min(s.len()));
            let (r1, c1) = Self::edit_row_of(&rows, b.min(s.len()));
            sx = self.edit_col_x(w, &s, &rows, r0, c0);
            ex = self.edit_col_x(w, &s, &rows, r1, c1);
            sr = r0 as i32;
            er = r1 as i32;
        }
        self.set_i_pub(w, "selectStartRow", sr);
        self.set_i_pub(w, "selectEndRow", er);
        self.set_f_pub(w, "selectStartX", sx);
        self.set_f_pub(w, "selectEndX", ex);
    }

    fn edit_show_cursor(&mut self, w: u32) {
        let bs = self.wf_pub(w, "blinkStart");
        if let Some(d) = self.edm(w) {
            d.cursor_showing = true;
            d.blink_delay = bs;
        }
        // the cursor's area (`ComputeCursorConfig`): edit sounds play at its centre
        self.edit_cursor_config(w);
        let area = ["showAreaX", "showAreaY", "showAreaWidth", "showAreaHeight"].map(|n| self.wf_pub(w, n));
        self.ui_ask_parent_to_show_area(w, area[0], area[1], area[2], area[3]);
    }

    /// `XEditWindow::PlayEditSound` (Extension.dll 1001dca0): a window sound at the centre of the cursor's area.
    fn edit_play_sound(&mut self, w: u32, sound: u32, volume: f32, pitch: f32) {
        let x = self.wf_pub(w, "showAreaWidth") * 0.5 + self.wf_pub(w, "showAreaX");
        let y = self.wf_pub(w, "showAreaHeight") * 0.5 + self.wf_pub(w, "showAreaY");
        self.ui_play_sound_at(w, sound, volume, pitch, x, y);
    }

    fn edit_modified(&self, w: u32) -> bool {
        self.ed(w).map(|d| d.cur != d.unchanged).unwrap_or(false)
    }

    fn edit_notify(&mut self, w: u32, event: &str) {
        let m = self.edit_modified(w);
        let mut cur = w;
        while cur != 0 {
            let (r, _) = self.ui_event(cur, event, vec![Value::Obj(w), Value::Bool(m)]);
            if r.as_bool().unwrap_or(false) {
                break;
            }
            cur = self.wo_pub(cur, "winParent");
        }
    }

    /// `XEditWindow::AddUndo`: a pure insertion right after the previous one (nothing replaced) joins it; records
    /// after the current one are dropped; at most `maxUndos` are kept (`ClampUndo`).
    fn edit_add_undo(&mut self, w: u32, pos: usize, old: &str, new: &str) {
        let Some(d) = self.edm(w) else { return };
        let (ol, nl) = (old.chars().count(), new.chars().count());
        if ol == 0 && nl == 0 {
            return;
        }
        let mut push = true;
        if (ol < 1 || nl < 1) && d.cur > 0 && d.cur != d.unchanged {
            let prev = &mut d.undo[d.cur as usize - 1];
            if prev.old.is_empty() && ol < 1 && pos == prev.pos + prev.new.chars().count() {
                prev.new.push_str(new);
                push = false;
            }
        }
        if (d.cur as usize) < d.undo.len() {
            d.undo.truncate(d.cur as usize);
            if d.cur < d.unchanged {
                d.unchanged = -1;
            }
        }
        if push {
            d.undo.push(UndoRec { pos, old: old.to_string(), new: new.to_string() });
            d.cur += 1;
        }
        if d.max_undos > -1 && (d.max_undos as usize) < d.undo.len() {
            let n = d.undo.len() - d.max_undos as usize;
            d.undo.drain(..n);
            d.cur -= n as i32;
            d.unchanged -= n as i32;
            if d.cur < 0 {
                d.cur = 0;
            }
            if d.unchanged < 0 {
                d.unchanged = -1;
            }
        }
    }

    /// `ClearUndo`: no records; the text counts as changed unless it was unchanged.
    fn edit_clear_undo(&mut self, w: u32) {
        if let Some(d) = self.edm(w) {
            d.unchanged = if d.cur != d.unchanged { -1 } else { 0 };
            d.cur = 0;
            d.undo.clear();
        }
    }

    /// `SetTextChangedFlag`.
    fn edit_set_changed(&mut self, w: u32, on: bool) {
        if let Some(d) = self.edm(w) {
            d.unchanged = if on { -1 } else { d.cur };
        }
    }

    /// `FilterString`: line breaks dropped on a single line, each character through the character filter (upper case
    /// only, the script's `FilterChar`). `None` when something went in and nothing came out.
    fn edit_filter(&mut self, w: u32, text: &str) -> Option<String> {
        let upper = self.wb_pub(w, "bUppercaseOnly");
        let single = self.wb_pub(w, "bSingleLine");
        let mut out = String::new();
        for c in text.chars() {
            if (c == '\n' && single) || (c < ' ' && c != '\n') {
                continue;
            }
            let c = if upper { c.to_ascii_uppercase() } else { c };
            let (r, outs) = self.ui_script_event(w, "FilterChar", vec![Value::Str(c.to_string())]);
            if let Value::Bool(false) = r {
                continue;
            }
            match outs.first() {
                Some(Value::Str(s)) if !s.is_empty() => out.push(s.chars().next().unwrap_or(c)),
                _ => out.push(c),
            }
        }
        if !text.is_empty() && out.is_empty() {
            return None;
        }
        Some(out)
    }

    /// `InsertText(text, bUndo, bSelect)` (Extension 1001e3c0): filtered; without undo the undo records go and the
    /// text counts as changed.
    fn edit_insert_text(&mut self, w: u32, text: &str, undo: bool, select: bool) -> bool {
        let Some(text) = self.edit_filter(w, &text.replace("|n", "\n")) else { return false };
        if !undo {
            self.edit_clear_undo(w);
            self.edit_set_changed(w, true);
        }
        self.edit_replace_text(w, &text, undo, select);
        true
    }

    fn edit_set_selection(&mut self, w: u32, a: i32, b: i32) {
        self.set_i_pub(w, "selectStart", a.min(b));
        self.set_i_pub(w, "selectEnd", a.max(b));
    }

    /// `ReplaceText(text, bUndo, bSelect)` (Extension 1001fe20): the selection (or nothing, at the insertion point)
    /// becomes `text`, cut to what `maxSize` leaves room for; an undo record when asked; the insertion point goes after
    /// the new text, selecting it from the old insertion point when `select`.
    fn edit_replace_text(&mut self, w: u32, text: &str, undo: bool, select: bool) {
        let mut chars: Vec<char> = self.edit_text(w).chars().collect();
        let (ss, se) = (self.wi_pub(w, "selectStart"), self.wi_pub(w, "selectEnd"));
        let sel = if ss < se { (se - ss) as usize } else { 0 };
        let mut ins: Vec<char> = text.chars().collect();
        let mut select = select;
        let max = self.wi_pub(w, "maxSize");
        if max > 0 {
            let max = max as usize;
            if max < chars.len() {
                chars.truncate(max);
                let t: String = chars.iter().collect();
                self.edit_set_text_var(w, &t);
                self.edit_clear_undo(w);
                self.edit_set_changed(w, true);
                self.ui_ask_parent_for_reconfigure(w);
                select = false;
            }
            if sel < ins.len() {
                let excess = chars.len() as i64 - max as i64 - sel as i64 + ins.len() as i64;
                if excess > 0 {
                    let keep = (ins.len() as i64 - excess).max(0) as usize;
                    ins.truncate(keep);
                }
            }
        }
        if sel < 1 && ins.is_empty() {
            return;
        }
        let mut pos = (self.wi_pub(w, "insertPos").max(0) as usize).min(chars.len());
        if ss < se {
            pos = (ss.max(0) as usize).min(chars.len());
        }
        let end = (pos + sel).min(chars.len());
        if undo {
            let old: String = chars[pos..end].iter().collect();
            let new: String = ins.iter().collect();
            self.edit_add_undo(w, pos, &old, &new);
        }
        let n = ins.len();
        chars.splice(pos..end, ins);
        let s: String = chars.iter().collect();
        self.edit_set_text_var(w, &s);
        self.set_i_pub(w, "insertPos", pos as i32);
        self.set_i_pub(w, "insertHookPos", pos as i32);
        self.edit_notify(w, "TextChanged");
        self.ui_ask_parent_for_reconfigure(w);
        self.edit_set_insertion(w, (pos + n) as i32, select);
    }

    /// `Undo`/`Redo`: the record's new (old) text is selected and replaced by its old (new) text, which stays selected.
    fn edit_undo(&mut self, w: u32, redo: bool) {
        let Some((pos, old, new)) = self.ed(w).and_then(|d| {
            let i = if redo { d.cur } else { d.cur - 1 };
            (i >= 0 && (i as usize) < d.undo.len()).then(|| {
                let r = &d.undo[i as usize];
                (r.pos, r.old.clone(), r.new.clone())
            })
        }) else {
            return;
        };
        let (gone, back) = if redo { (old, new) } else { (new, old) };
        if let Some(d) = self.edm(w) {
            d.cur += if redo { 1 } else { -1 };
        }
        self.set_i_pub(w, "selectStart", pos as i32);
        self.set_i_pub(w, "selectEnd", (pos + gone.chars().count()) as i32);
        self.set_i_pub(w, "insertPos", pos as i32);
        self.edit_replace_text(w, &back, false, true);
    }

    fn edit_selection(&self, w: u32) -> (usize, usize) {
        (self.wi_pub(w, "selectStart").max(0) as usize, self.wi_pub(w, "selectEnd").max(0) as usize)
    }

    /// `XEditWindow::ComputeSelectArea` (1001f010): the selection between the insertion point and the hook (none when
    /// they meet). In word mode the selection always exists and reaches out to whole words: its start back to the
    /// beginning of its word (`StartOfWord`, 1001f300), its end on to the end of its word (`EndOfWord`, 1001f350; words
    /// end at 9..13 and spaces), and the insertion point moves to the grown end on its side; the hook stays.
    fn edit_compute_select_area(&mut self, w: u32) {
        let (pos, hook) = (self.wi_pub(w, "insertPos"), self.wi_pub(w, "insertHookPos"));
        let words = self.ed(w).map(|d| d.select_words).unwrap_or(false);
        if pos == hook && !words {
            // no selection: -1, -1 (`GetSelectedArea` reports it as 0, 0)
            self.edit_set_selection(w, -1, -1);
            return;
        }
        let (mut lo, mut hi) = (pos.min(hook), pos.max(hook));
        if words {
            let chars: Vec<char> = self.edit_text(w).chars().collect();
            let space = |c: char| (9..=13).contains(&(c as u32)) || c == ' ';
            let mut s = lo.clamp(0, chars.len() as i32) as usize;
            while s > 0 && !space(chars[s - 1]) {
                s -= 1;
            }
            let mut e = hi.max(0) as usize;
            while e < chars.len() && !space(chars[e]) {
                e += 1;
            }
            (lo, hi) = (s as i32, e as i32);
            self.set_i_pub(w, "insertPos", if pos >= hook { hi } else { lo });
        }
        self.edit_set_selection(w, lo, hi);
    }

    /// `SetInsertionPoint` / `SetCursorPoint` (1001d4a0, 1001f140): the insertion point clamped to the text, the hook
    /// with it unless `drag`, then the selection (`ComputeSelectArea`) and the caret.
    fn edit_set_insertion(&mut self, w: u32, pos: i32, drag: bool) {
        let len = self.edit_text(w).chars().count() as i32;
        let pos = pos.clamp(0, len);
        self.set_i_pub(w, "insertPos", pos);
        if !drag {
            self.set_i_pub(w, "insertHookPos", pos);
        }
        self.edit_compute_select_area(w);
        let pos = self.wi_pub(w, "insertPos");
        // `ComputeCursorConfig(1)`: the caret's x becomes the preferred x for moves between rows
        let (st, rows, _) = self.edit_rows(w);
        let (r, col) = Self::edit_row_of(&rows, pos as usize);
        let x = self.edit_col_x(w, &st, &rows, r, col);
        self.set_f_pub(w, "insertPreferredCol", x);
        self.edit_show_cursor(w);
    }

    /// `MoveInsertionPoint` (1001d250) over the rows: up/down/page by rows keeping the preferred x
    /// (`SetCursorRow`), left/right by one character, word jumps and row ends within the row, text start/end.
    fn edit_move(&mut self, w: u32, how: i32, drag: bool) {
        let (s, rows, lh) = self.edit_rows(w);
        let pos = self.wi_pub(w, "insertPos");
        let (r, _) = Self::edit_row_of(&rows, (pos.max(0) as usize).min(s.len()));
        let r = r as i32;
        let n = rows.len();
        // a page: the rows the window's height holds, at least one
        let page = if n < 1 { 1 } else { ((self.wf_pub(w, "height") / (lh + self.wf_pub(w, "vSpace"))) as i32).max(1) };
        let is_space = |c: u32| (9..=13).contains(&c) || c == 32;
        // a row's end position (`start + consumed length - 1`; the text's end for the last row)
        let row_end = |ri: usize| rows.get(ri + 1).map(|nx| nx.start as i32 - 1).unwrap_or(s.len() as i32);
        if n < 1 && !matches!(how, 2 | 3 | 10 | 11) {
            return;
        }
        let new = match how {
            0 => return self.edit_set_cursor_row(w, r - 1, drag),
            1 => return self.edit_set_cursor_row(w, r + 1, drag),
            8 => return self.edit_set_cursor_row(w, r - page, drag),
            9 => return self.edit_set_cursor_row(w, r + page, drag),
            2 => pos - 1,
            3 => pos + 1,
            // `PrevWord` (1001f550): back over spaces, then over the word, not past the row start; at the row start
            // one character back
            4 => {
                let p = (pos.max(0) as usize).min(s.len());
                let start = rows[r as usize].start;
                if p == start {
                    p as i32 - 1
                } else {
                    let mut q = p;
                    let mut seen_word = false;
                    while q > start {
                        if is_space(s[q - 1]) {
                            if seen_word {
                                break;
                            }
                        } else {
                            seen_word = true;
                        }
                        q -= 1;
                    }
                    q as i32
                }
                .max(0)
            }
            // `NextWord` (1001f3b0): over the word, then the spaces, to the next word on the row; else the row's end
            5 => {
                let p = (pos.max(0) as usize).min(s.len());
                let row = &rows[r as usize];
                let vis_end = row.start + row.len;
                if p >= vis_end {
                    rows.get(r as usize + 1).map(|nx| nx.start as i32).unwrap_or(s.len() as i32 + 1)
                } else {
                    let mut q = p;
                    let mut seen_space = false;
                    let mut out = None;
                    while q < vis_end {
                        if is_space(s[q]) {
                            seen_space = true;
                        } else if seen_space {
                            out = Some(q as i32);
                            break;
                        }
                        q += 1;
                    }
                    out.unwrap_or_else(|| row_end(r as usize))
                }
            }
            6 => rows[r as usize].start as i32,
            7 => row_end(r as usize),
            10 => 0,
            11 => s.len() as i32,
            _ => return,
        };
        self.edit_set_insertion(w, new, drag);
    }

    /// `SetCursorRow` (1001f200): the row clamped to the rows, the column nearest the preferred x (`XToCol`), which
    /// stays as it was.
    fn edit_set_cursor_row(&mut self, w: u32, row: i32, drag: bool) {
        let (s, rows, _) = self.edit_rows(w);
        let n = rows.len() as i32;
        let row = row.min(n - 1).max(0);
        let mut pos = 0i32;
        if row < n {
            let r = &rows[row as usize];
            let (hm, width) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "width"));
            let base = match self.wi_pub(w, "hAlign") {
                0 => hm as i32 as f32,
                2 => (width - r.width - hm) as i32 as f32,
                _ => ((width - r.width) * 0.5) as i32 as f32,
            };
            let target = self.wf_pub(w, "insertPreferredCol") - base;
            let gc = self.edit_gc(w);
            let mut col = 0usize;
            let mut pen = 0i32;
            // `PixelToChar`: whole-pixel pen, a character counts once x passes its middle
            for i in r.start..r.start + r.len {
                let wd = self.text_extent(&gc, 0.0, &char::from_u32(s[i]).unwrap_or(' ').to_string()).0;
                if target < wd * 0.5 + pen as f32 {
                    break;
                }
                pen = (wd + pen as f32) as i32;
                col = i + 1 - r.start;
                if pen as f32 >= target {
                    break;
                }
            }
            pos = (r.start + col) as i32;
        }
        let pos = pos.clamp(0, s.len() as i32);
        self.set_i_pub(w, "insertPos", pos);
        if !drag {
            self.set_i_pub(w, "insertHookPos", pos);
        }
        self.edit_compute_select_area(w);
        self.edit_show_cursor(w);
    }

    /// `DeleteChar(bBefore, bUndo)` / `DeleteText` (1001e560, 100201d0): without undo the records go and the text
    /// counts as changed; the selection is deleted, else the character before or after the insertion point.
    fn edit_delete_char(&mut self, w: u32, before: bool, undo: bool) {
        if !undo {
            self.edit_clear_undo(w);
            self.edit_set_changed(w, true);
        }
        let (ss, se) = (self.wi_pub(w, "selectStart"), self.wi_pub(w, "selectEnd"));
        if se <= ss {
            let len = self.edit_text(w).chars().count() as i32;
            let pos = self.wi_pub(w, "insertPos");
            let start = if before { pos - 1 } else { pos };
            if (before && pos >= 1) || (!before && pos < len) {
                self.set_i_pub(w, "selectStart", start);
                self.set_i_pub(w, "selectEnd", start + 1);
            }
        }
        self.edit_replace_text(w, "", undo, false);
    }

    pub(crate) fn edit_event(&mut self, w: u32, name: &str, args: &mut Vec<Value>) -> Option<Value> {
        match name {
            "ParentRequestedPreferredSize" => {
                let (mut pw, mut ph) = (fl(args, 1, 0.0), fl(args, 3, 0.0));
                let (w_spec, h_spec) = (bl(args, 0, false), bl(args, 2, false));
                self.text_preferred_size(w, w_spec, &mut pw, h_spec, &mut ph);
                if self.wb_pub(w, "bSingleLine") && !h_spec {
                    ph = self.wf_pub(w, "vMargin") * 2.0 + self.edit_line_height(w);
                }
                args[1] = Value::Float(pw);
                args[3] = Value::Float(ph);
                Some(Value::Void)
            }
            "KeyPressed" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                let ch = match args.first() {
                    Some(Value::Str(s)) => s.chars().next(),
                    _ => None,
                };
                if let Some(c) = ch {
                    if self.wb_pub(w, "bEditable") && c != '`' && c != '~' && c >= ' ' {
                        if self.edit_insert_text(w, &c.to_string(), true, false) {
                            // `XEditWindow::KeyPressed`: the type sound after a character went in
                            let s = self.wo_pub(w, "typeSound");
                            let v = self.wf_pub(w, "soundVolume");
                            self.edit_play_sound(w, s, v, 1.0);
                            return Some(Value::Bool(true));
                        }
                    }
                }
                Some(r)
            }
            "VirtualKeyPressed" => {
                if iv(args, 0) == 0x0d {
                    // `XEditWindow::VirtualKeyPressed`: Enter plays the enter sound first
                    let s = self.wo_pub(w, "enterSound");
                    let v = self.wf_pub(w, "soundVolume");
                    self.edit_play_sound(w, s, v, 1.0);
                }
                if iv(args, 0) == 0x0d && self.wb_pub(w, "bSingleLine") && self.wb_pub(w, "bEditable") {
                    self.edit_notify(w, "EditActivated");
                    return Some(Value::Bool(true));
                }
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                Some(r)
            }
            "VisibilityChanged" => {
                let (_, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                if args.first().and_then(Value::as_bool).unwrap_or(false) {
                    self.ui_ask_parent_for_reconfigure(w);
                    self.edit_show_cursor(w);
                }
                Some(Value::Void)
            }
            "MouseButtonPressed" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                if iv(args, 2) != 1 || !self.wb_pub(w, "bEditable") {
                    return Some(r);
                }
                // `XEditWindow::MouseButtonPressed` (10021570): Ctrl or an even click count (double, quadruple, ...)
                // selects by words until the button goes up; the drag repeat starts 0.1 s later (the original adds
                // `GetTickOffset`, the wall-clock time since the root's tick; 0 on the port's step clock)
                let ctrl = self.edit_key_down(w, 0x11);
                let even = (iv(args, 3) - 1) % 2 == 1;
                if let Some(d) = self.edm(w) {
                    d.select_words = ctrl || even;
                    d.dragging = true;
                    d.drag_delay = 0.1;
                }
                let (x, y) = (fl(args, 0, 0.0), fl(args, 1, 0.0));
                let pos = self.edit_xy_pos(w, x, y) as i32;
                let shift = self.edit_key_down(w, 0x10);
                self.edit_set_insertion(w, pos, shift);
                Some(Value::Bool(true))
            }
            "MouseMoved" => {
                self.ui_script_event(w, name, args.clone());
                if self.ed(w).map(|d| d.dragging).unwrap_or(false) {
                    let pos = self.edit_xy_pos(w, fl(args, 0, 0.0), fl(args, 1, 0.0)) as i32;
                    self.edit_set_insertion(w, pos, true);
                }
                Some(Value::Void)
            }
            "MouseButtonReleased" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                // `XEditWindow::MouseButtonReleased` (10021760): the left button ends the drag and the word mode and is
                // taken when the text is editable
                if iv(args, 2) == 1 {
                    if let Some(d) = self.edm(w) {
                        d.dragging = false;
                        d.select_words = false;
                    }
                    if self.wb_pub(w, "bEditable") {
                        return Some(Value::Bool(true));
                    }
                }
                Some(r)
            }
            "Tick" => {
                // `XEditWindow::Tick` (100207d0): the caret blink, then while dragging a drag step every 0.1 s at the
                // cursor (`MouseMoved`, script event included), so a selection keeps growing with the mouse held still
                let dt = fl(args, 0, 0.0);
                let step = match self.edm(w) {
                    Some(d) if d.dragging => {
                        d.drag_delay -= dt;
                        if d.drag_delay < 0.0 {
                            d.drag_delay += 0.1;
                            if d.drag_delay < 0.0 {
                                d.drag_delay = 0.1;
                            }
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                };
                if self.wb_pub(w, "bEditable") {
                    let (period, dt) = (self.wf_pub(w, "blinkPeriod"), fl(args, 0, 0.0));
                    if let Some(d) = self.edm(w) {
                        d.blink_delay -= dt;
                        if d.blink_delay < 0.0 {
                            d.blink_delay += period * 0.5;
                            if d.blink_delay < 0.0 {
                                d.blink_delay = period * 0.5;
                            }
                            d.cursor_showing = !d.cursor_showing;
                        }
                    }
                }
                if step {
                    let root = self.ui_root_of_pub(w);
                    if root != 0 {
                        let g = self.wo_ref_pub(w, "wGc");
                        let o = self.gc_state(g).clip;
                        let (x, y) = (self.wf_pub(root, "mouseX") - o[0], self.wf_pub(root, "mouseY") - o[1]);
                        let mut a = vec![Value::Float(x), Value::Float(y)];
                        self.edit_event(w, "MouseMoved", &mut a);
                    }
                }
                Some(Value::Void)
            }
            _ => None,
        }
    }

    fn edit_key_down(&self, w: u32, key: u32) -> bool {
        let root = self.ui_root_of_pub(w);
        root != 0 && matches!(self.get_prop(root, "keyDownMap", key.min(254)), Ok(Value::Byte(b)) if b != 0)
    }

    /// `XEditWindow::Draw` (10020d00). Not editable or not the focus window: `XLargeTextWindow::Draw`. Otherwise the
    /// script's `DrawWindow`, the rows with the selection (a bar per selected row in `selectColor`, its text in
    /// `inverseColor`) clipped to the margins, then the caret on top.
    pub(crate) fn edit_draw(&mut self, w: u32, g: u32) {
        let focused = {
            let root = self.ui_root_of_pub(w);
            root != 0 && self.wo_pub(root, "focusWindow") == w
        };
        if !self.wb_pub(w, "bEditable") || !focused {
            return self.large_text_draw(w, g);
        }
        self.ui_event(w, "DrawWindow", vec![Value::Obj(g)]);
        self.edit_cursor_config(w);
        let (s, rows, lh) = self.edit_rows(w);
        let v = self.wf_pub(w, "vSpace");
        let so = v - (v * 0.5).trunc();
        let (hm, vm) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "vMargin"));
        let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
        let y0 = self.large_text_top(w, rows.len(), lh);
        let pitch = lh + v;
        let select = self.color_bytes(w, "selectColor");
        self.gc_mut(g).tile_color = select;
        let outer = self.gc_state(g);
        let mut st = outer.clone();
        st.parse_meta = false;
        st.intersect(hm, vm, width - (hm + hm), height - (vm + vm));
        let inverse = {
            let mut t = st.clone();
            t.text_color = self.color_bytes(w, "inverseColor");
            t.text_plane()
        };
        let tex = self.wo_ref_pub(w, "selectTexture");
        let (sr, er) = (self.wi_pub(w, "selectStartRow"), self.wi_pub(w, "selectEndRow"));
        let (sx, ex) = (self.wf_pub(w, "selectStartX"), self.wf_pub(w, "selectEndX"));
        for (r, row) in rows.iter().enumerate() {
            let y = y0 + r as f32 * pitch;
            if y + pitch < 0.0 || y > height {
                continue;
            }
            let rw = row.width.max(4.0);
            let x = self.large_text_row_x(w, rw);
            let ri = r as i32;
            if ri == sr || ri == er {
                let xs = if ri == sr { sx } else { x };
                let xe = if ri == er { ex } else { x + rw };
                self.gc_draw_pattern(&st, xs, y - so, xe - xs, lh + v, xs, y - so, tex);
                self.gc_draw_text_line(&st, x, y, &s, row.start, row.len, &row.state, Some((xs, xe, inverse)));
            } else if sr >= 0 && ri > sr && ri < er {
                self.gc_draw_pattern(&st, x, y - so, rw, lh + v, x, y - so, tex);
                self.gc_draw_text_line(&st, x, y, &s, row.start, row.len, &row.state, Some((f32::MIN, f32::MAX, inverse)));
            } else {
                self.gc_draw_text_line(&st, x, y, &s, row.start, row.len, &row.state, None);
            }
        }
        let caret = self.wo_ref_pub(w, "insertTexture");
        if caret != 0 && self.ed(w).map(|d| d.cursor_showing).unwrap_or(false) {
            let ins = self.color_bytes(w, "insertColor");
            {
                let gm = self.gc_mut(g);
                gm.set_style(2);
                gm.tile_color = ins;
            }
            let st = self.gc_state(g);
            let r = ["insertX", "insertY", "insertWidth", "insertHeight"].map(|n| self.wf_pub(w, n));
            self.gc_draw_pattern(&st, r[0], r[1], r[2], r[3], 0.0, 0.0, caret);
        }
    }

    /// The script natives of `EditWindow`.
    pub(crate) fn native_edit(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        use Value::*;
        let v = match name {
            // `XEditWindow::SetText` (1001e260): everything selected, the insertion point at the end, `InsertText(text,
            // no undo, no select)` (the undo records go, the text counts as changed), then the insertion point to 0
            "SetText" => {
                let text = match a.first() {
                    Some(Str(s)) => s.clone(),
                    Some(v) => self.value_text(v),
                    None => String::new(),
                };
                let len = self.edit_text(this).chars().count() as i32;
                self.set_i_pub(this, "selectStart", 0);
                self.set_i_pub(this, "selectEnd", len);
                self.set_i_pub(this, "insertPos", len);
                self.edit_insert_text(this, &text, false, false);
                self.edit_set_insertion(this, 0, false);
                Void
            }
            "AppendText" => {
                let r = self.native_widgets_pub(this, name, a);
                let len = self.edit_text(this).chars().count() as i32;
                let ip = self.wi_pub(this, "insertPos").min(len);
                self.set_i_pub(this, "insertPos", ip);
                self.set_i_pub(this, "insertHookPos", ip);
                self.edit_set_selection(this, 0, 0);
                return r;
            }
            "MoveInsertionPoint" => {
                self.edit_move(this, iv(a, 0), bl(a, 1, false));
                Void
            }
            "SetInsertionPoint" => {
                self.edit_set_insertion(this, iv(a, 0), bl(a, 1, false));
                Void
            }
            "GetInsertionPoint" => Int(self.wi_pub(this, "insertPos")),
            "SetSelectedArea" => {
                let len = self.edit_text(this).chars().count() as i32;
                let start = iv(a, 0).clamp(0, len);
                let end = (start + iv(a, 1).max(0)).min(len);
                self.edit_set_selection(this, start, end);
                self.set_i_pub(this, "insertHookPos", start);
                self.set_i_pub(this, "insertPos", end);
                Void
            }
            "GetSelectedArea" => {
                let (s, e) = self.edit_selection(this);
                a[0] = Int(s as i32);
                a[1] = Int(e.saturating_sub(s) as i32);
                Void
            }
            "EnableEditing" => {
                self.set_b_pub(this, "bEditable", bl(a, 0, true));
                Void
            }
            "IsEditingEnabled" => Bool(self.wb_pub(this, "bEditable")),
            "EnableSingleLineEditing" => {
                let on = bl(a, 0, true);
                self.set_b_pub(this, "bSingleLine", on);
                if on {
                    self.set_b_pub(this, "bWordWrap", false);
                    let t = self.edit_text(this).replace('\n', " ");
                    self.edit_set_text_var(this, &t);
                }
                self.ui_ask_parent_for_reconfigure(this);
                Void
            }
            "IsSingleLineEditingEnabled" => Bool(self.wb_pub(this, "bSingleLine")),
            "EnableUppercaseOnly" => {
                let on = bl(a, 0, true);
                self.set_b_pub(this, "bUppercaseOnly", on);
                if on {
                    let t = self.edit_text(this).to_uppercase();
                    self.edit_set_text_var(this, &t);
                }
                Void
            }
            "ClearTextChangedFlag" => {
                self.edit_set_changed(this, false);
                Void
            }
            "SetTextChangedFlag" => {
                self.edit_set_changed(this, bl(a, 0, true));
                Void
            }
            "HasTextChanged" => Bool(self.edit_modified(this)),
            "SetMaxSize" => {
                let n = iv(a, 0).max(0);
                self.set_i_pub(this, "maxSize", n);
                let t: String = self.edit_text(this).chars().take(n as usize).collect();
                if t != self.edit_text(this) {
                    self.edit_set_text_var(this, &t);
                    let len = t.chars().count() as i32;
                    let ip = self.wi_pub(this, "insertPos").min(len);
                    self.set_i_pub(this, "insertPos", ip);
                }
                Void
            }
            "SetMaxUndos" => {
                if let Some(d) = self.edm(this) {
                    d.max_undos = iv(a, 0);
                }
                Void
            }
            "InsertText" => {
                let text = match a.first() {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                Bool(self.edit_insert_text(this, &text, bl(a, 1, false), bl(a, 2, false)))
            }
            "DeleteChar" => {
                self.edit_delete_char(this, bl(a, 0, false), bl(a, 1, false));
                Void
            }
            "SetInsertionPointBlinkRate" => {
                self.set_f_pub(this, "blinkStart", fl(a, 0, 0.75));
                self.set_f_pub(this, "blinkPeriod", fl(a, 1, 1.0));
                Void
            }
            "SetInsertionPointTexture" => {
                self.set_o_pub(this, "insertTexture", ob(a, 0));
                let c = self.color_value_pub(color_arg(a, 1));
                let _ = self.set_prop(this, "insertColor", 0, c);
                Void
            }
            "SetInsertionPointType" => {
                let _ = self.set_prop(this, "insertType", 0, Byte(iv(a, 0) as u8));
                self.set_f_pub(this, "insertPrefWidth", fl(a, 1, 0.0));
                self.set_f_pub(this, "insertPrefHeight", fl(a, 2, 0.0));
                Void
            }
            "SetSelectedAreaTexture" => {
                self.set_o_pub(this, "selectTexture", ob(a, 0));
                let c = self.color_value_pub(color_arg(a, 1));
                let _ = self.set_prop(this, "selectColor", 0, c);
                Void
            }
            "SetSelectedAreaTextColor" => {
                let c = self.color_value_pub(color_arg(a, 0));
                let _ = self.set_prop(this, "inverseColor", 0, c);
                Void
            }
            "SetEditCursor" => {
                self.set_o_pub(this, "editCursor", ob(a, 0));
                self.set_o_pub(this, "editCursorShadow", ob(a, 1));
                let c = self.color_value_pub(color_arg(a, 2));
                let _ = self.set_prop(this, "editCursorColor", 0, c);
                Void
            }
            "Undo" | "Redo" => {
                self.edit_undo(this, name == "Redo");
                Void
            }
            "ClearUndo" => {
                self.edit_clear_undo(this);
                Void
            }
            "Copy" | "Cut" => {
                let (s, e) = self.edit_selection(this);
                if s < e {
                    let t: String = self.edit_text(this).chars().skip(s).take(e - s).collect();
                    self.ui.clipboard = t;
                    if name == "Cut" {
                        self.edit_replace_text(this, "", true, false);
                    }
                }
                Void
            }
            "Paste" => {
                let clip = self.ui.clipboard.clone();
                self.edit_insert_text(this, &clip, true, false);
                Void
            }
            "SetEditSounds" => {
                // `XEditWindow::SetEditSounds`: type, delete, enter, move (an omitted one is none)
                for (i, n) in ["typeSound", "deleteSound", "enterSound", "moveSound"].iter().enumerate() {
                    self.set_o_pub(this, n, ob(a, i));
                }
                Void
            }
            "PlayEditSound" => {
                self.edit_play_sound(this, ob(a, 0), fl(a, 1, -1.0), fl(a, 2, 1.0));
                Void
            }
            _ => return None,
        };
        Some(Ok(v))
    }
}

//! `ListWindow`: rows of text fields in columns with selection, sorting, hot keys and mouse/keyboard handling. Ported from
//! the `XListWindow` functions of Extension.dll. The original keeps rows and columns in native arrays and uses the row
//! pointer as the row id; here the rows live in `UiState::lists` and a row id is a counter value (never reused, 0 = none).

use std::cmp::Ordering;

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
fn st(a: &[Value], i: usize) -> String {
    match a.get(i) {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

#[derive(Clone, Debug)]
pub struct ListCol {
    pub title: String,
    /// Sort priority (higher sorts first; -1 = not a sort column).
    pub order: i32,
    pub reverse: bool,
    pub case_sensitive: bool,
    pub hidden: bool,
    pub width: f32,
    pub align: u8,
    pub color: [u8; 4],
    pub font: u32,
    /// 0 string, 1 float, 2 time.
    pub ctype: u8,
    pub fmt: String,
}

#[derive(Clone, Debug, Default)]
pub struct ListField {
    pub text: String,
    pub value: f32,
    /// Size of the text in the column's font.
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug)]
pub struct ListRow {
    pub id: i32,
    pub selected: bool,
    pub fields: Vec<ListField>,
    /// The client data: an integer or an object reference (the original has one slot for both).
    pub client: u32,
}

#[derive(Debug, Default)]
pub struct ListData {
    pub rows: Vec<ListRow>,
    pub cols: Vec<ListCol>,
    pub next_id: i32,
    pub num_selected: i32,
    pub focus: i32,
    pub anchor: i32,
    pub dragging: bool,
    pub last_index: i32,
    pub remaining: f32,
    pub hot: String,
    pub hot_timer: f32,
}

impl ListData {
    fn index_of(&self, id: i32) -> Option<usize> {
        if id == 0 {
            return None;
        }
        self.rows.iter().position(|r| r.id == id)
    }
}

/// `StringToFloat`: a number with an optional sign, decimal point or comma, `0x` hex prefix; reading stops at the first
/// character that cannot belong to it.
fn string_to_float(s: &str) -> f32 {
    let t = s.trim_start();
    let (neg, t) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v = 0.0f64;
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        for c in h.chars() {
            match c.to_digit(16) {
                Some(d) => v = v * 16.0 + d as f64,
                None => break,
            }
        }
    } else {
        let mut scale = 0.0f64;
        for c in t.chars() {
            if let Some(d) = c.to_digit(10) {
                if scale > 0.0 {
                    scale *= 10.0;
                    v += d as f64 / scale;
                } else {
                    v = v * 10.0 + d as f64;
                }
            } else if (c == '.' || c == ',') && scale == 0.0 {
                scale = 1.0;
            } else {
                break;
            }
        }
    }
    (if neg { -v } else { v }) as f32
}

impl Vm {
    fn ld(&self, w: u32) -> Option<&ListData> {
        self.ui.lists.get(&w)
    }
    fn ldm(&mut self, w: u32) -> Option<&mut ListData> {
        self.ui.lists.get_mut(&w)
    }

    pub(crate) fn list_init(&mut self, w: u32) {
        let _ = self.set_prop(w, "delimiter", 0, Value::Str(";".into()));
        let (black, white, grey) = (self.color_value_pub([0, 0, 0, 255]), self.color_value_pub([255, 255, 255, 255]), self.color_value_pub([128, 128, 128, 255]));
        let _ = self.set_prop(w, "inverseColor", 0, black);
        let _ = self.set_prop(w, "highlightColor", 0, white);
        let _ = self.set_prop(w, "focusColor", 0, grey);
        for (n, v) in [("bAutoSort", false), ("bMultiSelect", true), ("bAutoExpandColumns", true), ("bHotKeys", true)] {
            self.set_b_pub(w, n, v);
        }
        for (n, v) in [("focusThickness", 1.0), ("colMargin", 3.0), ("rowMargin", 1.0), ("lineSize", 0.0), ("remainingDelay", 0.0), ("hotKeyTimer", 0.0)] {
            self.set_f_pub(w, n, v);
        }
        self.set_i_pub(w, "maxClicks", 2);
        for n in ["highlightTexture", "focusTexture", "activateSound", "moveSound"] {
            self.set_o_pub(w, n, 0);
        }
        self.set_i_pub(w, "hotKeyCol", 0);
        self.set_i_pub(w, "numSelected", 0);
        self.ui.lists.insert(w, ListData { next_id: 1, ..Default::default() });
        self.list_set_num_columns(w, 1);
        self.list_reset_sort(w, true);
        self.ui_set_selectability(w, true);
    }

    // ---- text sizes and columns ------------------------------------------------------------------------------

    fn list_gc(&self, w: u32, font: u32) -> GcState {
        let g = self.wo_ref_pub(w, "wGc");
        let mut s = if g != 0 { self.gc_state(g) } else { GcState::default() };
        s.normal_font = font;
        s.bold_font = font;
        s.word_wrap = false;
        s.parse_meta = false;
        s.h_align = 0;
        s.v_align = 0;
        s
    }

    fn list_field_size(&mut self, w: u32, font: u32, text: &str) -> (f32, f32) {
        if font == 0 {
            return (0.0, 0.0);
        }
        let s = self.list_gc(w, font);
        self.text_extent(&s, 0.0, text)
    }

    /// `ComputeRowSize`: a row is as high as the tallest column font plus the row margins.
    fn list_compute_row_size(&mut self, w: u32) {
        let fonts: Vec<u32> = self.ld(w).map(|d| d.cols.iter().map(|c| c.font).collect()).unwrap_or_default();
        let mut h = 0.0f32;
        for f in fonts {
            if f != 0 {
                let s = self.list_gc(w, f);
                h = h.max(self.font_height(&s, false));
            }
        }
        let rm = self.wf_pub(w, "rowMargin");
        self.set_f_pub(w, "lineSize", h + rm * 2.0);
    }

    fn list_new_col(&self, w: u32, index: usize) -> ListCol {
        let _ = index;
        ListCol {
            title: String::new(),
            order: 0,
            reverse: false,
            case_sensitive: false,
            hidden: false,
            width: self.wf_pub(w, "colMargin") * 2.0 + 20.0,
            align: 0,
            color: self.color_bytes(w, "textColor"),
            font: self.wo_ref_pub(w, "normalFont"),
            ctype: 0,
            fmt: String::new(),
        }
    }

    fn list_set_num_columns(&mut self, w: u32, n: i32) {
        let n = n.max(0) as usize;
        let have = self.ld(w).map(|d| d.cols.len()).unwrap_or(0);
        let new: Vec<ListCol> = (have..n).map(|i| self.list_new_col(w, i)).collect();
        if let Some(d) = self.ldm(w) {
            d.cols.truncate(n);
            d.cols.extend(new);
            for r in &mut d.rows {
                r.fields.resize(n, ListField::default());
            }
        }
        self.list_compute_row_size(w);
        self.ui_ask_parent_for_reconfigure(w);
    }

    /// `ExpandColumnByField`: the column is at least as wide as the field plus margins. Returns whether it grew.
    fn list_expand_column(&mut self, w: u32, row: usize, col: usize) -> bool {
        let cm = self.wf_pub(w, "colMargin");
        let Some(d) = self.ldm(w) else { return false };
        let need = cm * 2.0 + d.rows[row].fields[col].w;
        if d.cols[col].width < need {
            d.cols[col].width = need;
            true
        } else {
            false
        }
    }

    fn list_resize_columns(&mut self, w: u32, expand_only: bool) {
        let cm = self.wf_pub(w, "colMargin");
        if let Some(d) = self.ldm(w) {
            if !expand_only {
                for c in &mut d.cols {
                    c.width = cm * 2.0;
                }
            }
        }
        let (rows, cols) = self.ld(w).map(|d| (d.rows.len(), d.cols.len())).unwrap_or((0, 0));
        for r in 0..rows {
            for c in 0..cols {
                self.list_expand_column(w, r, c);
            }
        }
        self.ui_ask_parent_for_reconfigure(w);
    }

    // ---- fields ----------------------------------------------------------------------------------------------

    /// `FieldConvertToString`: a number as text (only a float column uses its own format).
    fn list_value_text(&self, w: u32, col: usize, v: f32) -> String {
        let (ctype, fmt) = self.ld(w).map(|d| (d.cols[col].ctype, d.cols[col].fmt.clone())).unwrap_or((0, String::new()));
        let f = if ctype == 1 { fmt } else { "%f".to_string() };
        crate::crt::sprintf_double(&f, v as f64)
    }

    /// `SetFieldByString`.
    fn list_set_field_text(&mut self, w: u32, row: usize, col: usize, text: &str) {
        let ctype = self.ld(w).map(|d| d.cols[col].ctype).unwrap_or(0);
        if ctype == 0 {
            if let Some(d) = self.ldm(w) {
                d.rows[row].fields[col].value = 0.0;
                d.rows[row].fields[col].text = text.to_string();
            }
        } else {
            let v = string_to_float(text);
            let t = self.list_value_text(w, col, v);
            if let Some(d) = self.ldm(w) {
                d.rows[row].fields[col].value = v;
                d.rows[row].fields[col].text = t;
            }
        }
    }

    /// `SetFieldByValue`.
    fn list_set_field_value(&mut self, w: u32, row: usize, col: usize, v: f32) {
        let t = self.list_value_text(w, col, v);
        let ctype = self.ld(w).map(|d| d.cols[col].ctype).unwrap_or(0);
        if let Some(d) = self.ldm(w) {
            d.rows[row].fields[col].text = t;
            d.rows[row].fields[col].value = if ctype == 0 { 0.0 } else { v };
        }
    }

    /// `ComputeFieldSize`.
    fn list_compute_field_size(&mut self, w: u32, row: usize, col: usize) {
        let (font, text) = match self.ld(w) {
            Some(d) => (d.cols[col].font, d.rows[row].fields[col].text.clone()),
            None => return,
        };
        let (fw, fh) = self.list_field_size(w, font, &text);
        if let Some(d) = self.ldm(w) {
            d.rows[row].fields[col].w = fw;
            d.rows[row].fields[col].h = fh;
        }
    }

    /// `FillRow`: splits `text` at the delimiter into the row's fields and widens the columns. Returns whether a column grew.
    fn list_fill_row(&mut self, w: u32, row: usize, text: &str) -> bool {
        let delim = match self.get_prop(w, "delimiter", 0) {
            Ok(Value::Str(s)) => s.chars().next(),
            _ => Some(';'),
        };
        let cols = self.ld(w).map(|d| d.cols.len()).unwrap_or(0);
        let auto = self.wb_pub(w, "bAutoExpandColumns");
        let mut rest: Option<&str> = Some(text);
        let mut grew = false;
        for c in 0..cols {
            let piece = match rest {
                Some(s) => {
                    let end = delim.and_then(|d| s.find(d)).unwrap_or(s.len());
                    let p: String = s[..end].chars().take(0x7ff).collect();
                    rest = if end < s.len() { Some(&s[end + delim.map(|d| d.len_utf8()).unwrap_or(1)..]) } else { Some("") };
                    p
                }
                None => String::new(),
            };
            self.list_set_field_text(w, row, c, &piece);
            self.list_compute_field_size(w, row, c);
            if auto && self.list_expand_column(w, row, c) {
                grew = true;
            }
        }
        grew
    }

    // ---- sorting ---------------------------------------------------------------------------------------------

    fn list_cmp(cols: &[ListCol], keys: &[usize], a: &ListRow, ai: usize, b: &ListRow, bi: usize) -> Ordering {
        for &k in keys {
            let c = &cols[k];
            let o = if c.ctype == 1 || c.ctype == 2 {
                let (x, y) = (a.fields[k].value, b.fields[k].value);
                let o = x.partial_cmp(&y).unwrap_or(Ordering::Equal);
                if c.reverse {
                    o.reverse()
                } else {
                    o
                }
            } else {
                let (x, y) = (&a.fields[k].text, &b.fields[k].text);
                let o = if c.case_sensitive { x.cmp(y) } else { x.to_lowercase().cmp(&y.to_lowercase()) };
                if c.reverse {
                    o.reverse()
                } else {
                    o
                }
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        ai.cmp(&bi)
    }

    /// The sort columns, highest priority first (`SetupSortedColData`).
    fn list_sort_keys(cols: &[ListCol]) -> Vec<usize> {
        let mut k: Vec<usize> = (0..cols.len()).filter(|&i| cols[i].order >= 0).collect();
        k.sort_by(|&a, &b| cols[b].order.cmp(&cols[a].order).then(a.cmp(&b)));
        k
    }

    fn list_sort_rows(&mut self, w: u32) {
        let Some(d) = self.ldm(w) else { return };
        let keys = Self::list_sort_keys(&d.cols);
        let mut tagged: Vec<(usize, ListRow)> = std::mem::take(&mut d.rows).into_iter().enumerate().collect();
        let cols = d.cols.clone();
        tagged.sort_by(|(ai, a), (bi, b)| Self::list_cmp(&cols, &keys, a, *ai, b, *bi));
        d.rows = tagged.into_iter().map(|(_, r)| r).collect();
    }

    /// `InsertRow`: appended, or at its sorted place when the list sorts itself.
    fn list_insert_row(&mut self, w: u32, row: ListRow) -> usize {
        let auto = self.wb_pub(w, "bAutoSort");
        let Some(d) = self.ldm(w) else { return 0 };
        let pos = if auto {
            let keys = Self::list_sort_keys(&d.cols);
            let n = d.rows.len();
            d.rows.iter().enumerate().position(|(i, r)| Self::list_cmp(&d.cols, &keys, r, i, &row, n) == Ordering::Greater).unwrap_or(n)
        } else {
            d.rows.len()
        };
        d.rows.insert(pos, row);
        pos
    }

    /// `CheckPosition`: a changed row moves to its sorted place.
    fn list_check_position(&mut self, w: u32, id: i32) {
        if !self.wb_pub(w, "bAutoSort") {
            return;
        }
        let Some(d) = self.ldm(w) else { return };
        if let Some(i) = d.index_of(id) {
            let r = d.rows.remove(i);
            self.list_insert_row(w, r);
        }
    }

    fn list_reset_sort(&mut self, w: u32, sort: bool) {
        if let Some(d) = self.ldm(w) {
            for c in &mut d.cols {
                c.order = if sort { 0 } else { -1 };
                c.reverse = false;
                c.case_sensitive = false;
            }
        }
        if self.wb_pub(w, "bAutoSort") {
            self.list_sort_rows(w);
        }
    }

    // ---- selection -------------------------------------------------------------------------------------------

    fn list_change_select(&mut self, w: u32, idx: usize, sel: bool, changed: &mut bool) {
        let Some(d) = self.ldm(w) else { return };
        if d.rows[idx].selected != sel {
            d.rows[idx].selected = sel;
            d.num_selected += if sel { 1 } else { -1 };
            *changed = true;
            let n = d.num_selected;
            self.set_i_pub(w, "numSelected", n);
        }
    }

    fn list_notify(&mut self, w: u32, event: &str, args: Vec<Value>) {
        let mut cur = w;
        while cur != 0 {
            let (r, _) = self.ui_event(cur, event, args.clone());
            if r.as_bool().unwrap_or(false) {
                break;
            }
            cur = self.wo_pub(cur, "winParent");
        }
    }

    fn list_selection_changed(&mut self, w: u32) {
        let (n, f) = self.ld(w).map(|d| (d.num_selected, d.focus)).unwrap_or((0, 0));
        self.list_notify(w, "ListSelectionChanged", vec![Value::Obj(w), Value::Int(n), Value::Int(f)]);
    }

    fn list_activate(&mut self, w: u32) {
        if self.ui_visible_recursive(w) {
            let f = self.ld(w).map(|d| d.focus).unwrap_or(0);
            self.list_notify(w, "ListRowActivated", vec![Value::Obj(w), Value::Int(f)]);
        }
    }

    fn list_show_focus(&mut self, w: u32) {
        let (focus, line) = (self.ld(w).map(|d| d.focus).unwrap_or(0), self.wf_pub(w, "lineSize"));
        let idx = self.ld(w).and_then(|d| d.index_of(focus));
        if let Some(i) = idx {
            let width = self.wf_pub(w, "width");
            self.ui_ask_parent_to_show_area(w, 0.0, i as f32 * line, width, line);
        }
    }

    fn list_set_focus(&mut self, w: u32, id: i32, show: bool, anchor: bool) {
        if let Some(d) = self.ldm(w) {
            d.focus = id;
            if anchor {
                d.anchor = id;
            }
        }
        if show {
            self.list_show_focus(w);
        }
    }

    /// `XListWindow::MoveToRow` (Extension.dll 10032e30): moves the focus to a row and changes the selection around it:
    /// `clear` deselects the other rows, `toggle` flips the row instead of selecting it, `span` selects everything from
    /// the anchor (the focus row when there is none) to the row and then skips the single select/toggle step. A focus
    /// change to a row plays the move sound.
    fn list_move_to_row(&mut self, w: u32, id: i32, select: bool, clear: bool, toggle: bool, span: bool, focus: bool) {
        let multi = self.wb_pub(w, "bMultiSelect");
        let (clear, span) = if multi { (clear, span) } else { (true, false) };
        let Some(d) = self.ld(w) else { return };
        let Some(target) = d.index_of(id) else { return };
        let prev_focus = d.focus;
        let focus_idx = d.index_of(d.focus).unwrap_or(0);
        let anchor_idx = d.index_of(d.anchor).unwrap_or(focus_idx);
        let n = d.rows.len();
        let mut changed = false;
        if select {
            if clear {
                for i in 0..n {
                    self.list_change_select(w, i, false, &mut changed);
                }
            } else if span {
                let (lo, hi) = (anchor_idx.min(focus_idx), anchor_idx.max(focus_idx));
                for i in lo..=hi.min(n.saturating_sub(1)) {
                    self.list_change_select(w, i, false, &mut changed);
                }
            }
            if span {
                let (lo, hi) = (anchor_idx.min(target), anchor_idx.max(target));
                for i in lo..=hi.min(n.saturating_sub(1)) {
                    self.list_change_select(w, i, true, &mut changed);
                }
            } else {
                let now = self.ld(w).map(|d| d.rows[target].selected).unwrap_or(false);
                self.list_change_select(w, target, if toggle { !now } else { true }, &mut changed);
            }
        }
        if focus {
            self.list_set_focus(w, id, true, !span);
        }
        if changed {
            self.list_selection_changed(w);
        }
        let now_focus = self.ld(w).map(|d| d.focus).unwrap_or(0);
        if now_focus != prev_focus && now_focus != 0 {
            let s = self.wo_pub(w, "moveSound");
            self.list_play_sound(w, s, -1.0, 1.0);
        }
    }

    /// `XListWindow::PlayListSound` (Extension.dll 10031370): a window sound at the middle of the visible part across
    /// and at the focus row's top plus the row margin down (the middle of the visible part without a focus row; the
    /// window's middle when nothing of it is visible). The point only matters while the window and its parents are
    /// visible (the original plays the plain window sound otherwise).
    pub(crate) fn list_play_sound(&mut self, w: u32, sound: u32, volume: f32, pitch: f32) {
        let g = self.wo_ref_pub(w, "wGc");
        let c = if g != 0 { self.gc_state(g).clip } else { [0.0; 6] };
        let empty = c[4] <= 0.0 || c[5] <= 0.0;
        let x = if empty { self.wf_pub(w, "width") * 0.5 } else { c[4] * 0.5 + c[2] };
        let idx = self.ld(w).and_then(|d| d.index_of(d.focus));
        let y = match idx {
            Some(i) => i as f32 * self.wf_pub(w, "lineSize") + self.wf_pub(w, "rowMargin"),
            None if empty => self.wf_pub(w, "height") * 0.5,
            None => c[5] * 0.5 + c[3],
        };
        self.ui_play_sound_at(w, sound, volume, pitch, x, y);
    }

    fn list_set_row(&mut self, w: u32, id: i32, select: bool, clear: bool, drag: bool) {
        if id != 0 {
            self.list_move_to_row(w, id, select, clear, false, drag, true);
        } else {
            self.list_set_focus(w, 0, true, !drag);
        }
    }

    /// `XListWindow::GetPageSize` (Extension.dll 100311f0): how many rows fit in the visible part of the window (its clip
    /// height from the last `ClipTree`, so a list inside a scroll area pages by what is shown), at least 1.
    fn list_page_size(&self, w: u32) -> i32 {
        let line = self.wf_pub(w, "lineSize");
        if line > 0.0 {
            let g = self.wo_ref_pub(w, "wGc");
            let clip_h = if g != 0 { self.gc_state(g).clip[5] } else { 0.0 };
            let n = (clip_h / line) as i32;
            if n > 0 {
                return n;
            }
        }
        1
    }

    fn list_move_row(&mut self, w: u32, how: i32, select: bool, clear: bool, drag: bool) {
        let Some(d) = self.ld(w) else { return };
        let cur = d.index_of(d.focus).map(|i| i as i32).unwrap_or(-0xffff);
        let n = d.rows.len() as i32;
        let page = self.list_page_size(w);
        let mut i = match how {
            0 => cur - 1,
            1 => cur + 1,
            2 => cur - page,
            3 => cur + page,
            4 => 0,
            _ => n - 1,
        };
        i = i.min(n - 1).max(0);
        if i >= n {
            self.list_set_focus(w, 0, true, !drag);
            return;
        }
        let id = self.ld(w).map(|d| d.rows[i as usize].id).unwrap_or(0);
        self.list_set_row(w, id, select, clear, drag);
    }

    // ---- events ----------------------------------------------------------------------------------------------

    fn list_row_at(&self, w: u32, y: f32) -> Option<usize> {
        let line = self.wf_pub(w, "lineSize");
        let n = self.ld(w)?.rows.len();
        if line <= 0.0 || n == 0 {
            return None;
        }
        Some(((y / line).trunc() as i64).clamp(0, n as i64 - 1) as usize)
    }

    fn list_key_down(&self, w: u32, key: u32) -> bool {
        let root = self.ui_root_of_pub(w);
        root != 0 && matches!(self.get_prop(root, "keyDownMap", key.min(254)), Ok(Value::Byte(b)) if b != 0)
    }

    fn list_clear_hot(&mut self, w: u32) {
        if let Some(d) = self.ldm(w) {
            d.hot.clear();
            d.hot_timer = 0.0;
        }
        self.set_f_pub(w, "hotKeyTimer", 0.0);
    }

    /// `XListWindow::FindRowByKey` (Extension.dll 10030d50), type-ahead: appends the key to the hot key string (cleared
    /// once the hot key timer runs out) and returns the next row whose hot key column starts with it. A string of one
    /// repeated character ("aaa") matches on its first character only, so pressing the same key cycles through the rows
    /// starting with it; the search starts after the focus row for a one-character match and at the focus row
    /// otherwise, wrapping round once. Only A-Z fold to lower case.
    fn list_find_by_key(&mut self, w: u32, ch: char) -> i32 {
        let col = self.wi_pub(w, "hotKeyCol");
        let Some(d) = self.ldm(w) else { return 0 };
        if col < 0 || col as usize >= d.cols.len() || d.rows.is_empty() {
            return 0;
        }
        if d.hot_timer <= 0.0 {
            d.hot.clear();
        }
        d.hot.push(ch);
        d.hot_timer = 1.0;
        let key: Vec<char> = d.hot.chars().map(|c| c.to_ascii_lowercase()).collect();
        let len = if key.iter().all(|&c| c == key[0]) { 1 } else { key.len() };
        let n = d.rows.len();
        let cur = d.index_of(d.focus).unwrap_or(0);
        let start = if len > 1 { cur } else { cur + 1 };
        let mut found = 0;
        for k in 0..n {
            let i = (start + k) % n;
            let mut text = d.rows[i].fields[col as usize].text.chars().map(|c| c.to_ascii_lowercase());
            if key[..len].iter().all(|&c| text.next() == Some(c)) {
                found = d.rows[i].id;
                break;
            }
        }
        found
    }

    pub(crate) fn list_event(&mut self, w: u32, name: &str, args: &mut Vec<Value>) -> Option<Value> {
        match name {
            "ParentRequestedPreferredSize" => {
                let width: f32 = self.ld(w).map(|d| d.cols.iter().filter(|c| !c.hidden).map(|c| c.width).sum()).unwrap_or(0.0);
                let rows = self.ld(w).map(|d| d.rows.len()).unwrap_or(0);
                args[1] = Value::Float(width);
                args[3] = Value::Float(rows as f32 * self.wf_pub(w, "lineSize"));
                Some(Value::Void)
            }
            "ParentRequestedGranularity" => {
                args[0] = Value::Float(1.0);
                args[1] = Value::Float(self.wf_pub(w, "lineSize"));
                Some(Value::Void)
            }
            "MouseButtonPressed" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                if iv(args, 2) != 1 {
                    return Some(r);
                }
                if let Some(i) = self.list_row_at(w, fl(args, 1, 0.0)) {
                    let id = self.ld(w).map(|d| d.rows[i].id).unwrap_or(0);
                    let (shift, ctrl) = (self.list_key_down(w, 0x10), self.list_key_down(w, 0x11));
                    self.list_move_to_row(w, id, true, !ctrl, ctrl, shift, true);
                    if let Some(d) = self.ldm(w) {
                        d.last_index = i as i32;
                        d.dragging = true;
                        d.remaining = 0.1;
                    }
                }
                Some(Value::Bool(true))
            }
            "MouseMoved" => {
                self.ui_script_event(w, name, args.clone());
                let dragging = self.ld(w).map(|d| d.dragging).unwrap_or(false);
                if dragging {
                    if let Some(i) = self.list_row_at(w, fl(args, 1, 0.0)) {
                        if self.ld(w).map(|d| d.last_index) != Some(i as i32) {
                            let id = self.ld(w).map(|d| d.rows[i].id).unwrap_or(0);
                            if let Some(d) = self.ldm(w) {
                                d.last_index = i as i32;
                            }
                            self.list_move_to_row(w, id, true, false, false, true, true);
                        }
                    }
                }
                Some(Value::Void)
            }
            "MouseButtonReleased" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                if iv(args, 2) != 1 {
                    return Some(r);
                }
                if let Some(d) = self.ldm(w) {
                    d.dragging = false;
                }
                // a double click activates the row under the cursor; unlike press and drag the row index is not clamped,
                // so the empty space under the last row activates nothing (`MouseButtonReleased`, 100339c0)
                let line = self.wf_pub(w, "lineSize");
                let n = self.ld(w).map(|d| d.rows.len()).unwrap_or(0) as i64;
                if iv(args, 3) > 1 && line > 0.0 && n > 0 {
                    let i = (fl(args, 1, 0.0) / line) as i64;
                    if (0..n).contains(&i) {
                        let id = self.ld(w).map(|d| d.rows[i as usize].id).unwrap_or(0);
                        self.list_set_focus(w, id, true, true);
                        let s = self.wo_pub(w, "activateSound");
                        self.list_play_sound(w, s, -1.0, 1.0);
                        self.list_activate(w);
                    }
                }
                Some(Value::Bool(true))
            }
            "KeyPressed" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                let ch = match args.first() {
                    Some(Value::Str(s)) => s.chars().next(),
                    _ => None,
                };
                // `IsHotKeyValid` (10031690): ASCII letters, digits and the underscore only
                if let Some(c) = ch.filter(|c| c.is_ascii_alphanumeric() || *c == '_') {
                    if self.wb_pub(w, "bHotKeys") {
                        let id = self.list_find_by_key(w, c);
                        if id != 0 {
                            self.list_set_row(w, id, true, true, false);
                        }
                        return Some(Value::Bool(true));
                    }
                }
                Some(r)
            }
            "VirtualKeyPressed" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                let key = iv(args, 0);
                if matches!(key, 0x25..=0x28 | 0x1b) {
                    self.list_clear_hot(w);
                }
                if key == 0x0d {
                    let s = self.wo_pub(w, "activateSound");
                    self.list_play_sound(w, s, -1.0, 1.0);
                    self.list_activate(w);
                    return Some(Value::Bool(true));
                }
                Some(r)
            }
            "VisibilityChanged" => {
                let (_, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                if args.first().and_then(Value::as_bool).unwrap_or(false) {
                    let first = self.ld(w).and_then(|d| if d.focus == 0 { d.rows.first().map(|r| r.id) } else { None });
                    if let Some(id) = first {
                        self.list_set_row(w, id, true, true, false);
                    }
                }
                Some(Value::Void)
            }
            "Tick" => {
                let dt = fl(args, 0, 0.0);
                let drag = match self.ldm(w) {
                    Some(d) if d.dragging => {
                        d.remaining -= dt;
                        if d.remaining < 0.0 {
                            d.remaining = 0.1;
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                };
                if drag {
                    let root = self.ui_root_of_pub(w);
                    if root != 0 {
                        let g = self.wo_ref_pub(w, "wGc");
                        let o = self.gc_state(g).clip;
                        let (x, y) = (self.wf_pub(root, "mouseX") - o[0], self.wf_pub(root, "mouseY") - o[1]);
                        let mut a = vec![Value::Float(x), Value::Float(y)];
                        self.list_event(w, "MouseMoved", &mut a);
                    }
                }
                let expired = match self.ldm(w) {
                    Some(d) if d.hot_timer > 0.0 => {
                        d.hot_timer -= dt;
                        d.hot_timer <= 0.0
                    }
                    _ => false,
                };
                if expired {
                    self.list_clear_hot(w);
                }
                Some(Value::Void)
            }
            _ => None,
        }
    }

    /// `Draw`: selection bars, the cells of the visible rows and the focus box, then the script's `DrawWindow`.
    pub(crate) fn list_draw(&mut self, w: u32, g: u32) {
        let line = self.wf_pub(w, "lineSize");
        let base = self.gc_state(g);
        let (first, last, ncols) = {
            let Some(d) = self.ld(w) else { return };
            if line <= 0.0 {
                (0, 0, 0)
            } else {
                let (cy, ch) = (base.clip[3], base.clip[5]);
                let f = ((cy / line).floor() as i32).max(0) as usize;
                let l = (((ch + cy) / line).ceil() as i32).max(0) as usize;
                (f, l.min(d.rows.len()), d.cols.len())
            }
        };
        let width = self.wf_pub(w, "width");
        let (cm, rm) = (self.wf_pub(w, "colMargin"), self.wf_pub(w, "rowMargin"));
        let mut s = base.clone();
        s.set_style(2);
        s.word_wrap = false;
        s.h_align = 0;
        s.v_align = 0;
        s.parse_meta = false;
        s.tile_color = self.color_bytes(w, "highlightColor");
        let hl_tex = self.wo_ref_pub(w, "highlightTexture");
        for i in first..last {
            if self.ld(w).map(|d| d.rows[i].selected).unwrap_or(false) {
                self.gc_draw_pattern(&s, 0.0, i as f32 * line, width, line, 0.0, 0.0, hl_tex);
            }
        }
        let inverse = self.color_bytes(w, "inverseColor");
        let mut x0 = 0.0f32;
        for c in 0..ncols {
            let Some(col) = self.ld(w).map(|d| d.cols[c].clone()) else { break };
            if col.hidden {
                continue;
            }
            s.normal_font = col.font;
            s.bold_font = col.font;
            for i in first..last {
                let (selected, field) = match self.ld(w) {
                    Some(d) => (d.rows[i].selected, d.rows[i].fields[c].clone()),
                    None => break,
                };
                s.text_color = if selected { inverse } else { col.color };
                let mut cell = s.clone();
                let top = i as f32 * line;
                cell.intersect(x0 + cm, top + rm, col.width - cm * 2.0, line - rm * 2.0);
                let x = match col.align {
                    0 => x0 + cm,
                    2 => (x0 - cm) + (col.width - field.w),
                    _ => x0 + (col.width - field.w) * 0.5,
                };
                let y = (top - rm) + (line - field.h) + 1.0;
                self.gc_draw_text(&cell, x, y, field.w, field.h, &field.text);
            }
            x0 += col.width;
        }
        let root = self.ui_root_of_pub(w);
        let focused = root != 0 && self.wo_pub(root, "focusWindow") == w;
        if focused {
            if let Some(i) = self.ld(w).and_then(|d| d.index_of(d.focus)) {
                if i >= first && i < last {
                    let mut f = s.clone();
                    f.tile_color = self.color_bytes(w, "focusColor");
                    let (t, tex) = (self.wf_pub(w, "focusThickness"), self.wo_ref_pub(w, "focusTexture"));
                    self.gc_draw_box(&f, 0.0, i as f32 * line, width, line, 0.0, 0.0, t, tex);
                }
            }
        }
        self.ui_event(w, "DrawWindow", vec![Value::Obj(g)]);
    }

    // ---- natives ---------------------------------------------------------------------------------------------

    fn list_col(&self, w: u32, c: i32) -> Option<usize> {
        let n = self.ld(w)?.cols.len();
        if c >= 0 && (c as usize) < n {
            Some(c as usize)
        } else {
            None
        }
    }

    /// The script natives of `ListWindow`.
    pub(crate) fn native_list(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        use Value::*;
        let id = iv(a, 0);
        let v = match name {
            "IndexToRowId" => Int(self.ld(this).and_then(|d| d.rows.get(id.max(0) as usize).filter(|_| id >= 0).map(|r| r.id)).unwrap_or(0)),
            "RowIdToIndex" => Int(self.ld(this).and_then(|d| d.index_of(id)).map(|i| i as i32).unwrap_or(-1)),
            "SetRowClientInt" | "SetRowClientObject" => {
                let v = if name == "SetRowClientInt" { iv(a, 1) as u32 } else { ob(a, 1) };
                if let Some(d) = self.ldm(this) {
                    if let Some(i) = d.index_of(id) {
                        d.rows[i].client = v;
                    }
                }
                Void
            }
            "GetRowClientInt" => Int(self.ld(this).and_then(|d| d.index_of(id).map(|i| d.rows[i].client as i32)).unwrap_or(0)),
            "GetRowClientObject" => Obj(self.ld(this).and_then(|d| d.index_of(id).map(|i| d.rows[i].client)).unwrap_or(0)),
            "AddRow" => {
                let cols = self.ld(this).map(|d| d.cols.len()).unwrap_or(0);
                let rid = {
                    let d = self.ldm(this)?;
                    let rid = d.next_id;
                    d.next_id += 1;
                    rid
                };
                let row = ListRow { id: rid, selected: false, fields: vec![ListField::default(); cols], client: iv(a, 1) as u32 };
                // sizes and fields need the row in place: insert, fill, then move it to its sorted place
                let pos = {
                    let d = self.ldm(this)?;
                    d.rows.push(row);
                    d.rows.len() - 1
                };
                let text = st(a, 0);
                self.list_fill_row(this, pos, &text);
                self.list_check_position(this, rid);
                if !self.wb_pub(this, "bAutoSort") {
                    // appended rows stay where they are
                }
                self.ui_ask_parent_for_reconfigure(this);
                Int(rid)
            }
            "DeleteRow" => {
                let Some(idx) = self.ld(this).and_then(|d| d.index_of(id)) else { return Some(Ok(Void)) };
                let mut changed = false;
                self.list_change_select(this, idx, false, &mut changed);
                if let Some(d) = self.ldm(this) {
                    d.rows.remove(idx);
                    if d.focus == id {
                        d.focus = 0;
                    }
                    if d.anchor == id {
                        d.anchor = 0;
                    }
                }
                if changed {
                    self.list_selection_changed(this);
                }
                self.ui_ask_parent_for_reconfigure(this);
                Void
            }
            "ModifyRow" => {
                if let Some(idx) = self.ld(this).and_then(|d| d.index_of(id)) {
                    let grew = self.list_fill_row(this, idx, &st(a, 1));
                    self.list_check_position(this, id);
                    if grew {
                        self.ui_ask_parent_for_reconfigure(this);
                    }
                }
                Void
            }
            "DeleteAllRows" => {
                let had = self.ld(this).map(|d| d.num_selected > 0).unwrap_or(false);
                if let Some(d) = self.ldm(this) {
                    d.rows.clear();
                    d.num_selected = 0;
                    d.focus = 0;
                    d.anchor = 0;
                }
                self.set_i_pub(this, "numSelected", 0);
                if had {
                    self.list_selection_changed(this);
                }
                self.ui_ask_parent_for_reconfigure(this);
                Void
            }
            "SetField" | "SetFieldValue" => {
                let (Some(idx), Some(col)) = (self.ld(this).and_then(|d| d.index_of(id)), self.list_col(this, iv(a, 1))) else { return Some(Ok(Void)) };
                if name == "SetField" {
                    self.list_set_field_text(this, idx, col, &st(a, 2));
                } else {
                    self.list_set_field_value(this, idx, col, fl(a, 2, 0.0));
                }
                self.list_compute_field_size(this, idx, col);
                let grew = self.wb_pub(this, "bAutoExpandColumns") && self.list_expand_column(this, idx, col);
                self.list_check_position(this, id);
                if grew {
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "GetField" => {
                let r = match (self.ld(this).and_then(|d| d.index_of(id)), self.list_col(this, iv(a, 1))) {
                    (Some(i), Some(c)) => self.ld(this).map(|d| d.rows[i].fields[c].text.clone()).unwrap_or_default(),
                    _ => String::new(),
                };
                Str(r)
            }
            "GetFieldValue" => {
                let r = match (self.ld(this).and_then(|d| d.index_of(id)), self.list_col(this, iv(a, 1))) {
                    (Some(i), Some(c)) => self.ld(this).map(|d| d.rows[i].fields[c].value).unwrap_or(0.0),
                    _ => 0.0,
                };
                Float(r)
            }
            "GetNumRows" => Int(self.ld(this).map(|d| d.rows.len() as i32).unwrap_or(0)),
            "GetNumSelectedRows" => Int(self.ld(this).map(|d| d.num_selected).unwrap_or(0)),
            "SelectRow" => {
                let sel = bl(a, 1, true);
                if let Some(idx) = self.ld(this).and_then(|d| d.index_of(id)) {
                    if self.ld(this).map(|d| d.rows[idx].selected) != Some(sel) {
                        let mut changed = false;
                        if !self.wb_pub(this, "bMultiSelect") && sel {
                            let n = self.ld(this).map(|d| d.rows.len()).unwrap_or(0);
                            for i in 0..n {
                                self.list_change_select(this, i, false, &mut changed);
                            }
                        }
                        self.list_change_select(this, idx, sel, &mut changed);
                        if changed {
                            self.list_selection_changed(this);
                        }
                    }
                }
                Void
            }
            "SelectAllRows" => {
                let sel = bl(a, 0, true) && self.wb_pub(this, "bMultiSelect");
                let n = self.ld(this).map(|d| d.rows.len()).unwrap_or(0);
                let mut changed = false;
                for i in 0..n {
                    self.list_change_select(this, i, sel, &mut changed);
                }
                if changed {
                    self.list_selection_changed(this);
                }
                Void
            }
            "SelectToRow" => {
                if id != 0 {
                    self.list_move_to_row(this, id, true, bl(a, 1, true), bl(a, 2, false), bl(a, 3, false), false);
                }
                Void
            }
            "ToggleRowSelection" => {
                if let Some(idx) = self.ld(this).and_then(|d| d.index_of(id)) {
                    let now = self.ld(this).map(|d| d.rows[idx].selected).unwrap_or(false);
                    let mut a2 = vec![Int(id), Bool(!now)];
                    return self.native_list(this, "SelectRow", &mut a2);
                }
                Void
            }
            "IsRowSelected" => Bool(self.ld(this).and_then(|d| d.index_of(id).map(|i| d.rows[i].selected)).unwrap_or(false)),
            "GetSelectedRow" => {
                let d = self.ld(this)?;
                let f = d.index_of(d.focus).filter(|&i| d.rows[i].selected).map(|i| d.rows[i].id);
                Int(f.or_else(|| d.rows.iter().find(|r| r.selected).map(|r| r.id)).unwrap_or(0))
            }
            "MoveRow" => {
                self.list_move_row(this, id, bl(a, 1, true), bl(a, 2, true), bl(a, 3, false));
                Void
            }
            "SetRow" => {
                self.list_set_row(this, id, bl(a, 1, true), bl(a, 2, true), bl(a, 3, false));
                Void
            }
            "SetFocusRow" => {
                self.list_set_focus(this, id, bl(a, 1, true), bl(a, 2, true));
                Void
            }
            "GetFocusRow" => Int(self.ld(this).map(|d| d.focus).unwrap_or(0)),
            "ShowFocusRow" => {
                self.list_show_focus(this);
                Void
            }
            "SetNumColumns" => {
                self.list_set_num_columns(this, id);
                Void
            }
            "GetNumColumns" => Int(self.ld(this).map(|d| d.cols.len() as i32).unwrap_or(0)),
            "ResizeColumns" => {
                self.list_resize_columns(this, bl(a, 0, false));
                Void
            }
            "SetColumnTitle" | "GetColumnTitle" | "SetColumnWidth" | "GetColumnWidth" | "SetColumnAlignment" | "GetColumnAlignment" | "SetColumnColor" | "GetColumnColor" | "SetColumnFont" | "GetColumnFont" | "SetColumnType" | "GetColumnType"
            | "HideColumn" | "IsColumnHidden" | "SetSortColumn" | "AddSortColumn" | "RemoveSortColumn" => return self.native_list_column(this, name, a),
            "ResetSortColumns" => {
                self.list_reset_sort(this, bl(a, 0, true));
                Void
            }
            "Sort" => {
                self.list_sort_rows(this);
                Void
            }
            "EnableHotKeys" => {
                self.set_b_pub(this, "bHotKeys", bl(a, 0, true));
                Void
            }
            "SetHotKeyColumn" => {
                self.set_i_pub(this, "hotKeyCol", id);
                Void
            }
            "EnableAutoSort" => {
                let on = bl(a, 0, true);
                if self.wb_pub(this, "bAutoSort") != on {
                    self.set_b_pub(this, "bAutoSort", on);
                    if on {
                        self.list_sort_rows(this);
                    }
                }
                Void
            }
            "IsAutoSortEnabled" => Bool(self.wb_pub(this, "bAutoSort")),
            "EnableAutoExpandColumns" => {
                let on = bl(a, 0, true);
                if self.wb_pub(this, "bAutoExpandColumns") != on {
                    self.set_b_pub(this, "bAutoExpandColumns", on);
                    if on {
                        self.list_resize_columns(this, true);
                    }
                }
                Void
            }
            "IsAutoExpandColumnsEnabled" => Bool(self.wb_pub(this, "bAutoExpandColumns")),
            "EnableMultiSelect" => {
                let on = bl(a, 0, true);
                if self.wb_pub(this, "bMultiSelect") != on {
                    self.set_b_pub(this, "bMultiSelect", on);
                    if !on && self.ld(this).map(|d| d.num_selected > 0).unwrap_or(false) {
                        let mut changed = false;
                        let n = self.ld(this).map(|d| d.rows.len()).unwrap_or(0);
                        for i in (0..n).rev() {
                            let sel = self.ld(this).map(|d| d.rows[i].selected && d.num_selected > 1).unwrap_or(false);
                            if sel {
                                self.list_change_select(this, i, false, &mut changed);
                            }
                        }
                        if changed {
                            self.list_selection_changed(this);
                        }
                    }
                }
                Void
            }
            "IsMultiSelectEnabled" => Bool(self.wb_pub(this, "bMultiSelect")),
            "SetFieldMargins" => {
                let (cm, rm) = (fl(a, 0, 0.0), fl(a, 1, 0.0));
                if self.wf_pub(this, "rowMargin") != rm {
                    self.set_f_pub(this, "rowMargin", rm);
                    self.list_compute_row_size(this);
                    self.ui_ask_parent_for_reconfigure(this);
                }
                if self.wf_pub(this, "colMargin") != cm {
                    self.set_f_pub(this, "colMargin", cm);
                    if self.wb_pub(this, "bAutoExpandColumns") {
                        self.list_resize_columns(this, true);
                    }
                }
                Void
            }
            "GetFieldMargins" => {
                a[0] = Float(self.wf_pub(this, "colMargin"));
                a[1] = Float(self.wf_pub(this, "rowMargin"));
                Void
            }
            "GetPageSize" => Int(self.list_page_size(this)),
            "SetDelimiter" => {
                let _ = self.set_prop(this, "delimiter", 0, Str(st(a, 0).chars().take(1).collect()));
                Void
            }
            "SetHighlightTextColor" => {
                let c = self.color_value_pub(color_arg(a, 0));
                let _ = self.set_prop(this, "inverseColor", 0, c);
                Void
            }
            "SetHighlightColor" => {
                let c = self.color_value_pub(color_arg(a, 0));
                let _ = self.set_prop(this, "highlightColor", 0, c);
                Void
            }
            "SetFocusColor" => {
                let c = self.color_value_pub(color_arg(a, 0));
                let _ = self.set_prop(this, "focusColor", 0, c);
                Void
            }
            "SetHighlightTexture" => {
                self.set_o_pub(this, "highlightTexture", ob(a, 0));
                Void
            }
            "SetFocusTexture" => {
                self.set_o_pub(this, "focusTexture", ob(a, 0));
                Void
            }
            "SetFocusThickness" => {
                self.set_f_pub(this, "focusThickness", fl(a, 0, 0.0));
                Void
            }
            "SetListSounds" => {
                self.set_o_pub(this, "activateSound", ob(a, 0));
                self.set_o_pub(this, "moveSound", ob(a, 1));
                Void
            }
            "PlayListSound" => {
                self.list_play_sound(this, ob(a, 0), fl(a, 1, -1.0), fl(a, 2, 1.0));
                Void
            }
            _ => return None,
        };
        Some(Ok(v))
    }

    fn native_list_column(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        use Value::*;
        let Some(c) = self.list_col(this, iv(a, 0)) else {
            return Some(Ok(match name {
                "GetColumnTitle" => Str(String::new()),
                "GetColumnWidth" => Float(0.0),
                "GetColumnAlignment" | "GetColumnType" => Byte(0),
                "GetColumnColor" => {
                    a[1] = self.color_value_pub([0, 0, 0, 0]);
                    Void
                }
                "GetColumnFont" => Obj(0),
                "IsColumnHidden" => Bool(false),
                _ => Void,
            }));
        };
        let auto = self.wb_pub(this, "bAutoExpandColumns");
        let v = match name {
            "SetColumnTitle" => {
                let t = match a.get(1) {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                self.ldm(this)?.cols[c].title = t;
                Void
            }
            "GetColumnTitle" => Str(self.ld(this)?.cols[c].title.clone()),
            "SetColumnWidth" => {
                let wd = fl(a, 1, 0.0);
                if self.ld(this)?.cols[c].width != wd {
                    self.ldm(this)?.cols[c].width = wd;
                    if auto {
                        self.list_resize_columns(this, true);
                    }
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "GetColumnWidth" => Float(self.ld(this)?.cols[c].width),
            "SetColumnAlignment" => {
                self.ldm(this)?.cols[c].align = iv(a, 1) as u8;
                Void
            }
            "GetColumnAlignment" => Byte(self.ld(this)?.cols[c].align),
            "SetColumnColor" => {
                self.ldm(this)?.cols[c].color = color_arg(a, 1);
                Void
            }
            "GetColumnColor" => {
                let col = self.ld(this)?.cols[c].color;
                a[1] = self.color_value_pub(col);
                Void
            }
            "SetColumnFont" => {
                let f = match ob(a, 1) {
                    0 => self.wo_ref_pub(this, "normalFont"),
                    f => f,
                };
                if self.ld(this)?.cols[c].font != f {
                    self.ldm(this)?.cols[c].font = f;
                    let n = self.ld(this)?.rows.len();
                    for r in 0..n {
                        self.list_compute_field_size(this, r, c);
                    }
                    self.list_compute_row_size(this);
                    if auto {
                        self.list_resize_columns(this, true);
                    }
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "GetColumnFont" => Obj(self.ld(this)?.cols[c].font),
            "SetColumnType" => {
                let t = iv(a, 1) as u8;
                let fmt = match a.get(2) {
                    Some(Str(s)) if !s.is_empty() => s.clone(),
                    _ => if t == 1 { "%f".to_string() } else { "%02h:%02m".to_string() },
                };
                {
                    let col = &mut self.ldm(this)?.cols[c];
                    col.ctype = t;
                    col.fmt = fmt;
                }
                let n = self.ld(this)?.rows.len();
                for r in 0..n {
                    let text = self.ld(this)?.rows[r].fields[c].text.clone();
                    self.list_set_field_text(this, r, c, &text);
                    self.list_compute_field_size(this, r, c);
                }
                if auto {
                    self.list_resize_columns(this, true);
                }
                Void
            }
            "GetColumnType" => Byte(self.ld(this)?.cols[c].ctype),
            "HideColumn" => {
                let h = bl(a, 1, true);
                if self.ld(this)?.cols[c].hidden != h {
                    self.ldm(this)?.cols[c].hidden = h;
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "IsColumnHidden" => Bool(self.ld(this)?.cols[c].hidden),
            "SetSortColumn" | "AddSortColumn" | "RemoveSortColumn" => {
                {
                    let d = self.ldm(this)?;
                    match name {
                        "SetSortColumn" => {
                            for col in &mut d.cols {
                                col.order = 0;
                                col.reverse = false;
                                col.case_sensitive = false;
                            }
                        }
                        "AddSortColumn" => {
                            for col in &mut d.cols {
                                if col.order > 0 {
                                    col.order += 1;
                                }
                            }
                        }
                        _ => {}
                    }
                    if name == "RemoveSortColumn" {
                        d.cols[c].order = -1;
                    } else {
                        d.cols[c].order = 1;
                        d.cols[c].reverse = bl(a, 1, false);
                        d.cols[c].case_sensitive = bl(a, 2, false);
                    }
                }
                if self.wb_pub(this, "bAutoSort") {
                    self.list_sort_rows(this);
                }
                Void
            }
            _ => return None,
        };
        Some(Ok(v))
    }
}

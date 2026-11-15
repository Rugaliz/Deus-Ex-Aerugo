//! Painting the window tree into the draw list: clip rectangles, the per-window drawing order and the native `Draw`
//! behaviour of the text, button and border windows (`XRootWindow::PaintWindows`, `XWindow::DrawTree`).

use crate::{Value, Vm};

impl Vm {
    /// `ClipTree`: the absolute origin and visible region of the window and its descendants (kept in each window's `wGc`).
    pub(crate) fn ui_clip_tree(&mut self, w: u32) {
        let parent = self.wo_pub(w, "winParent");
        let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
        let mut clip = [0.0, 0.0, 0.0, 0.0, width, height];
        if parent != 0 {
            let pg = self.wo_ref_pub(parent, "wGc");
            let pc = self.gc_state(pg).clip;
            clip[0] = self.wf_pub(w, "x") + pc[0];
            clip[1] = pc[1] + self.wf_pub(w, "y");
            let (l, t) = (clip[0] + clip[2], clip[1] + clip[3]);
            let (r, b) = (l + clip[4], t + clip[5]);
            let (pl, pt) = (pc[2] + pc[0], pc[3] + pc[1]);
            let (pr, pb) = (pl + pc[4], pt + pc[5]);
            let (nl, nt, nr, nb) = (l.max(pl), t.max(pt), r.min(pr), b.min(pb));
            clip[2] = nl - clip[0];
            clip[3] = nt - clip[1];
            clip[4] = nr - nl;
            clip[5] = nb - nt;
        }
        let g = self.wo_ref_pub(w, "wGc");
        if g != 0 {
            self.gc_mut(g).clip = clip;
        }
        let mut c = self.wo_pub(w, "firstChild");
        while c != 0 {
            self.ui_clip_tree(c);
            c = self.wo_pub(c, "nextSibling");
        }
    }

    /// Creates the window's two GCs (`XWindow::Init`): `wGc` holds the window's own drawing settings, `gGc` is handed out
    /// by `GetGC`.
    pub(crate) fn ui_make_gcs(&mut self, w: u32, parent: u32) {
        let (Ok(wg), Ok(gg)) = (self.new_gc(), self.new_gc()) else { return };
        self.set_o_pub(w, "wGc", wg);
        self.set_o_pub(w, "gGc", gg);
        // the constructor's clip is all zero; only `ClipTree` gives `wGc` the window's region
        let _ = parent;
        self.gc_mut(wg).clip = [0.0; 6];
    }

    /// Fonts of the window changed: its `wGc` follows.
    pub(crate) fn ui_sync_gc_fonts(&mut self, w: u32) {
        let g = self.wo_ref_pub(w, "wGc");
        if g != 0 {
            let (n, b) = (self.wo_ref_pub(w, "normalFont"), self.wo_ref_pub(w, "boldFont"));
            let st = self.gc_mut(g);
            st.normal_font = n;
            st.bold_font = b;
        }
    }

    // ---- painting ----------------------------------------------------------------------------------------

    /// Paints the player's whole window tree and returns the draw list (bottom to top). The screen size is in real pixels;
    /// the root divides it by its multiplier and the draw list is scaled back by it.
    pub fn ui_paint(&mut self, pawn: u32, screen_w: f32, screen_h: f32) -> Vec<crate::ui_gc::DrawCmd> {
        let root = self.ui_set_screen(pawn, screen_w, screen_h);
        self.ui.draw.clear();
        self.ui.views.clear();
        self.ui.actor_cmds.clear();
        self.ui.cmd_seq = 0;
        if root != 0 {
            let (hm, vm) = (self.wi_pub(root, "hMultiplier").max(1), self.wi_pub(root, "vMultiplier").max(1));
            // `PostRender` looks the cursor's window up before the tree is drawn
            let under = self.ui_mouse_window(root, 0);
            self.ui_draw_raw_background(root, hm, vm);
            self.ui_draw_tree(root, hm, vm);
            self.ui_draw_cursor(root, hm, vm, under);
        }
        std::mem::take(&mut self.ui.draw)
    }

    /// Lets `ViewportWindow`s emit 3D views (the caller must draw `ui_views`); without it they show their default texture.
    pub fn ui_enable_views(&mut self, on: bool) {
        self.ui.wants_views = on;
    }

    /// The `GC.DrawActor`/`ClearZ` commands of the last `ui_paint`.
    pub fn ui_actor_cmds(&self) -> &[crate::ui_gc::ActorCmd] {
        &self.ui.actor_cmds
    }

    /// The 3D views of the last `ui_paint`; each lies after `after` quads of its draw list.
    pub fn ui_views(&self) -> &[crate::ui_gc::ViewCmd] {
        &self.ui.views
    }

    /// `XRootWindow::DrawRawBackground` (1003c720), drawn by `PaintWindows` before the tree: the root's `rawBackground`
    /// in normal style tinted by `rawColor` over every part of the root no 3D view and no window without
    /// `bDrawRawBackground` covers (`ClipListGenerate`); stretched, the whole `rawBackgroundWidth` x
    /// `rawBackgroundHeight` source goes into each piece, else it is tiled from each piece's own corner.
    fn ui_draw_raw_background(&mut self, root: u32, hm: i32, vm: i32) {
        let g = self.wo_ref_pub(root, "wGc");
        let tex = self.wo_ref_pub(root, "rawBackground");
        if g == 0 {
            return;
        }
        let abs = |c: [f32; 6]| [c[0] + c[2], c[1] + c[3], c[4], c[5]];
        let root_clip = self.gc_state(g).clip;
        let mut list = vec![abs(root_clip)];
        if self.wb_pub(root, "bRender") {
            let cut = if self.wb_pub(root, "bClipRender") {
                [root_clip[0] + self.wf_pub(root, "renderX"), root_clip[1] + self.wf_pub(root, "renderY"), self.wf_pub(root, "renderWidth"), self.wf_pub(root, "renderHeight")]
            } else {
                abs(root_clip)
            };
            clip_list_subtract(&mut list, cut);
        }
        self.ui_clip_list_generate(root, &mut list);
        let depth = self.gc_push(g);
        {
            let raw = self.color_bytes(root, "rawColor");
            let st = self.gc_mut(g);
            st.canvas = true;
            st.h_mul = hm.max(1);
            st.v_mul = vm.max(1);
            st.draw_enabled = true;
            st.set_style(1);
            st.tile_color = raw;
        }
        let st = self.gc_state(g);
        let stretch = self.wb_pub(root, "bStretchRawBackground");
        let (sw, sh) = (self.wf_pub(root, "rawBackgroundWidth"), self.wf_pub(root, "rawBackgroundHeight"));
        // the pieces are in absolute coordinates; the GC draws relative to the root's origin
        let (ox, oy) = (root_clip[0], root_clip[1]);
        for [x, y, w, h] in list {
            if tex == 0 {
                break;
            }
            let (x, y) = (x - ox, y - oy);
            if stretch {
                self.draw_tile(&st, tex, x, y, w, h, 0.0, 0.0, sw, sh, false, false);
            } else {
                self.gc_draw_pattern(&st, x, y, w, h, x, y, tex);
            }
        }
        self.gc_pop(g, Some(depth));
    }

    /// `ClipListGenerate` (1003c5f0) below the top call: a window without `bDrawRawBackground` takes its clip rectangle
    /// out (its children are not visited); otherwise its visible children, bottom to top.
    fn ui_clip_list_generate(&self, w: u32, list: &mut Vec<[f32; 4]>) {
        if !self.wb_pub(w, "bDrawRawBackground") {
            let g = self.wo_ref_pub(w, "wGc");
            if g != 0 {
                let c = self.gc_state(g).clip;
                clip_list_subtract(list, [c[0] + c[2], c[1] + c[3], c[4], c[5]]);
            }
            return;
        }
        let mut c = self.wo_pub(w, "firstChild");
        while c != 0 {
            if self.wb_pub(c, "bIsVisible") {
                self.ui_clip_list_generate(c, list);
            }
            c = self.wo_pub(c, "nextSibling");
        }
    }

    fn ui_draw_tree(&mut self, w: u32, hm: i32, vm: i32) {
        self.ui_lock_pub(w);
        let g = self.wo_ref_pub(w, "wGc");
        let clip = if g != 0 { self.gc_state(g).clip } else { [0.0; 6] };
        if g != 0 && self.wb_pub(w, "bIsVisible") && clip[4] > 0.0 && clip[5] > 0.0 {
            let depth = self.gc_push(g);
            {
                let st = self.gc_mut(g);
                st.canvas = true;
                st.h_mul = hm.max(1);
                st.v_mul = vm.max(1);
            }
            // the background texture
            let bg = self.wo_ref_pub(w, "background");
            if self.wi_pub(w, "backgroundStyle") != 0 && bg != 0 {
                let (tw, th) = if self.wb_pub(w, "bStretchBackground") { self.texture_size(bg) } else { (0.0, 0.0) };
                let st = self.gc_state(g);
                let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
                self.gc_draw_icon_pattern(&st, 0.0, 0.0, width, height, 0.0, 0.0, tw, th, bg);
            }
            self.ui_draw_event(w, g);
            let mut c = self.wo_pub(w, "firstChild");
            // the next sibling is read after the child is drawn (`DrawTree` 10051490)
            while c != 0 {
                self.ui_draw_tree(c, hm, vm);
                c = self.wo_pub(c, "nextSibling");
            }
            self.ui_event(w, "PostDrawWindow", vec![Value::Obj(g)]);
            self.gc_pop(g, Some(depth));
        }
        self.ui_unlock_pub(w);
    }

    /// `Draw`: the window's own drawing. Native classes draw first or last around the script event, as in the original.
    fn ui_draw_event(&mut self, w: u32, g: u32) {
        use crate::ui_widgets::Kind;
        let kinds = self.ui_kinds(w);
        // the most derived native class decides
        let top = kinds.iter().rev().find(|k| matches!(k, Kind::Border | Kind::Checkbox | Kind::Button | Kind::Toggle | Kind::TextLog | Kind::LargeText | Kind::Text | Kind::Scale | Kind::List | Kind::Edit | Kind::Computer | Kind::Viewport)).copied();
        match top {
            Some(Kind::Border) => {
                let style = self.wi_pub(w, "borderStyle");
                if style != 0 {
                    let smooth = self.wb_pub(w, "bSmoothBorder");
                    {
                        let st = self.gc_mut(g);
                        st.set_style(style as u8);
                        st.enable_smoothing(smooth);
                    }
                    let st = self.gc_state(g);
                    let stretch = self.wb_pub(w, "bStretchBorder");
                    let tex = ["borderTopLeft", "borderTopRight", "borderBottomLeft", "borderBottomRight", "borderLeft", "borderRight", "borderTop", "borderBottom", "center"].map(|n| self.wo_ref_pub(w, n));
                    let m = ["leftMargin", "rightMargin", "topMargin", "bottomMargin"].map(|n| self.wf_pub(w, n));
                    let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
                    self.gc_draw_borders(&st, 0.0, 0.0, width, height, m, tex, stretch, stretch);
                }
            }
            Some(Kind::Button | Kind::Toggle) => {
                let tile = self.color_bytes(w, "curTileColor");
                let text = self.color_bytes(w, "curTextColor");
                {
                    let st = self.gc_mut(g);
                    st.tile_color = tile;
                    st.text_color = text;
                }
                let st = self.gc_state(g);
                let tex = self.wo_ref_pub(w, "curTexture");
                let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
                self.gc_draw_pattern(&st, 0.0, 0.0, width, height, 0.0, 0.0, tex);
                self.ui_text_draw(w, g);
            }
            Some(Kind::Scale) => self.scale_draw(w, g),
            Some(Kind::Edit) => self.edit_draw(w, g),
            Some(Kind::List) => self.list_draw(w, g),
            Some(Kind::Viewport) => self.viewport_draw(w, g),
            Some(Kind::Computer) => {
                self.ui_event(w, "DrawWindow", vec![Value::Obj(g)]);
                self.computer_draw(w, g);
            }
            Some(Kind::TextLog) => self.text_log_draw(w, g),
            Some(Kind::Checkbox) => self.checkbox_draw(w, g),
            Some(Kind::LargeText) => self.large_text_draw(w, g),
            Some(Kind::Text) => self.ui_text_draw(w, g),
            _ => {
                self.ui_event(w, "DrawWindow", vec![Value::Obj(g)]);
            }
        }
    }

    /// `XCheckboxWindow::ComputeTextureSize` (1000a9c0): the box size (the texture size unless given) and the room it
    /// takes beside the text (box width + spacing, 0 without a box).
    pub(crate) fn checkbox_box_size(&mut self, w: u32) -> (f32, f32, f32) {
        let (on, off) = (self.wo_ref_pub(w, "toggleOn"), self.wo_ref_pub(w, "toggleOff"));
        let (on_s, off_s) = (self.texture_size(on), self.texture_size(off));
        let tw = self.wf_pub(w, "textureWidth");
        let th = self.wf_pub(w, "textureHeight");
        let bw = if tw > 0.0 { tw } else { on_s.0.max(off_s.0) };
        let bh = if th > 0.0 { th } else { on_s.1.max(off_s.1) };
        let reserve = if bw > 0.0 { bw + self.wf_pub(w, "checkboxSpacing") } else { 0.0 };
        (bw, bh, reserve)
    }

    /// `XCheckboxWindow::Draw` (1000aad0): the script's `DrawWindow`, the label in `curTextColor` one unit below the
    /// margin, then the box texture (`toggleOn`/`toggleOff`) in `checkboxStyle` and `checkboxColor`, beside the text.
    fn checkbox_draw(&mut self, w: u32, g: u32) {
        let (ha, va, wrap) = (self.wi_pub(w, "hAlign") as u8, self.wi_pub(w, "vAlign") as u8, self.wb_pub(w, "bWordWrap"));
        {
            let st = self.gc_mut(g);
            st.h_align = ha;
            st.v_align = va;
            st.word_wrap = wrap;
        }
        self.ui_event(w, "DrawWindow", vec![Value::Obj(g)]);
        let tex = if self.wb_pub(w, "bButtonPressed") { self.wo_ref_pub(w, "toggleOn") } else { self.wo_ref_pub(w, "toggleOff") };
        let (bw, bh, reserve) = self.checkbox_box_size(w);
        let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
        let (hm, vm) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "vMargin"));
        let box_y = ((height - bh) * 0.5) as i32 as f32;
        let (text_x, box_x) = if self.wb_pub(w, "bRightSide") { (hm, width - (reserve + hm)) } else { (reserve + hm, hm) };
        let text = match self.get_prop(w, "text", 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        };
        let color = self.color_bytes(w, "curTextColor");
        self.gc_mut(g).text_color = color;
        let st = self.gc_state(g);
        self.gc_draw_text(&st, text_x, vm + 1.0, width - (hm + hm + reserve), height - (vm + vm), &text);
        if tex != 0 {
            let style = match self.get_prop(w, "checkboxStyle", 0) {
                Ok(Value::Byte(b)) => b,
                _ => 1,
            };
            let tile = self.color_bytes(w, "checkboxColor");
            {
                let st = self.gc_mut(g);
                st.set_style(style);
                st.tile_color = tile;
            }
            let st = self.gc_state(g);
            self.gc_draw_icon_pattern(&st, box_x, box_y, bw, bh, 0.0, 0.0, bw, bh, tex);
        }
    }

    /// `GenerateLines` (1002da70) at the window's configured width: the text's rows (width, start, length, text state
    /// at the row start), wrapped at `Width - 2 hMargin` when `bWordWrap`, laid out with `gc`.
    pub(crate) fn large_text_rows(&mut self, w: u32, gc: &crate::ui_gc::GcState) -> (Vec<u32>, Vec<TextRow>) {
        let hm = self.wf_pub(w, "hMargin");
        let width = self.wf_pub(w, "width");
        let wrap = if self.wb_pub(w, "bWordWrap") { (width - (hm + hm)).max(0.0) } else { 0.0 };
        let text = match self.get_prop(w, "text", 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        };
        let s: Vec<u32> = text.chars().map(|c| c as u32).collect();
        let mut rows: Vec<TextRow> = Vec::new();
        let mut state = crate::ui_gc::TextState::default();
        let mut pos = 0usize;
        while let Some(line) = self.parse_line(gc, &s, pos, s.len(), &state, wrap) {
            rows.push(TextRow { width: line.width, start: line.start, len: line.len, state: state.clone() });
            state = line.next_state.clone();
            pos = line.next_pos;
            if rows.len() > 100_000 {
                break;
            }
        }
        (s, rows)
    }

    /// The top of the first row for `rows` rows (`Draw`; `RowToY` counts at least one row).
    pub(crate) fn large_text_top(&mut self, w: u32, rows: usize, lh: f32) -> f32 {
        let (vm, height, v) = (self.wf_pub(w, "vMargin"), self.wf_pub(w, "height"), self.wf_pub(w, "vSpace"));
        let n = rows as f32;
        let total = if rows > 1 { n * lh + (n - 1.0) * v } else { n * lh };
        match self.wi_pub(w, "vAlign") {
            0 => vm,
            2 => height - total - vm,
            _ => ((height - total) * 0.5) as i32 as f32,
        }
    }

    /// A row's left edge for its width (`hAlign` 0: the margin, 2: right-aligned, otherwise centred without margin).
    pub(crate) fn large_text_row_x(&mut self, w: u32, rw: f32) -> f32 {
        let (hm, width) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "width"));
        match self.wi_pub(w, "hAlign") {
            0 => hm,
            2 => width - rw - hm,
            _ => ((width - rw) * 0.5) as i32 as f32,
        }
    }

    /// `XLargeTextWindow::Draw` (1002e500): the script's `DrawWindow`, then the rows laid out at the window's width
    /// (`GenerateLines` at configure time) at a fixed pitch of `lineHeight + vSpace`, each aligned on its own, clipped to
    /// the margins. The GC's alignments are not touched.
    pub(crate) fn large_text_draw(&mut self, w: u32, g: u32) {
        self.ui_event(w, "DrawWindow", vec![Value::Obj(g)]);
        let (hm, vm) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "vMargin"));
        let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
        // the rows, with the window's own GC as `ConfigurationChanged` makes them (an edit window's text is literal)
        let (lst, lh) = if self.ui_kinds(w).contains(&crate::ui_widgets::Kind::Edit) {
            let gc = self.edit_gc_pub(w);
            let lh = self.edit_line_height_pub(w);
            (gc, lh)
        } else {
            let wg = self.wo_ref_pub(w, "wGc");
            (self.gc_state(wg), self.large_line_height(w))
        };
        let (s, rows) = self.large_text_rows(w, &lst);
        let v = self.wf_pub(w, "vSpace");
        let y0 = self.large_text_top(w, rows.len(), lh);
        let pitch = lh + v;
        let mut st = self.gc_state(g);
        st.parse_meta = st.parse_meta && lst.parse_meta;
        st.intersect(hm, vm, width - (hm + hm), height - (vm + vm));
        for (r, row) in rows.iter().enumerate() {
            let y = y0 + r as f32 * pitch;
            if y + pitch < 0.0 || y > height {
                continue;
            }
            let x = self.large_text_row_x(w, row.width);
            self.gc_draw_text_line(&st, x, y, &s, row.start, row.len, &row.state, None);
        }
    }

    /// `XTextWindow::Draw`: alignments, the script's `DrawWindow`, then the text.
    fn ui_text_draw(&mut self, w: u32, g: u32) {
        let (ha, va, wrap) = (self.wi_pub(w, "hAlign") as u8, self.wi_pub(w, "vAlign") as u8, self.wb_pub(w, "bWordWrap"));
        {
            let st = self.gc_mut(g);
            st.h_align = ha;
            st.v_align = va;
            st.word_wrap = wrap;
        }
        self.ui_event(w, "DrawWindow", vec![Value::Obj(g)]);
        let text = match self.get_prop(w, "text", 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        };
        let (hm, vm) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "vMargin"));
        let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
        let st = self.gc_state(g);
        self.gc_draw_text(&st, hm, vm, width - (hm + hm), height - (vm + vm), &text);
    }

    pub(crate) fn color_bytes(&self, w: u32, n: &str) -> [u8; 4] {
        match self.get_prop(w, n, 0) {
            Ok(Value::Struct(_, b)) if b.len() >= 4 => [b[0], b[1], b[2], b[3]],
            _ => [255; 4],
        }
    }

    /// `XCheckboxWindow::ParentRequestedPreferredSize` (1000acb0): the text's size with the margins and the box's room,
    /// at least the box plus the margins.
    pub(crate) fn checkbox_preferred_size(&mut self, w: u32, w_spec: bool, pw: &mut f32, h_spec: bool, ph: &mut f32) -> bool {
        let (bw, bh, reserve) = self.checkbox_box_size(w);
        let text = match self.get_prop(w, "text", 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        };
        let (hm, vm) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "vMargin"));
        if text.is_empty() {
            if !self.text_preferred_size(w, w_spec, pw, h_spec, ph) {
                return false;
            }
        } else {
            let g = self.wo_ref_pub(w, "gGc");
            if g == 0 {
                return true;
            }
            self.gc_push(g);
            let wg = self.wo_ref_pub(w, "wGc");
            self.gc_copy(g, wg);
            let st = self.gc_state(g);
            let (min_lines, max_lines) = (self.wi_pub(w, "minLines"), self.wi_pub(w, "maxLines"));
            if !h_spec && (min_lines > -1 || max_lines > -1) {
                let wrap_w = if w_spec { *pw } else { 0.0 };
                let (ex, ey) = self.text_extent(&st, wrap_w, &text);
                if !w_spec {
                    *pw = ex;
                }
                let line_h = self.font_height(&st, true);
                let mut h = ey;
                if min_lines > -1 && h < min_lines as f32 * line_h {
                    h = min_lines as f32 * line_h;
                }
                if max_lines > -1 && (max_lines as f32 * line_h) < h {
                    h = max_lines as f32 * line_h;
                }
                *ph = h;
            } else {
                let wrap_w = if w_spec && self.wb_pub(w, "bWordWrap") { *pw - (hm + hm + reserve) } else { 0.0 };
                let (ex, ey) = self.text_extent(&st, wrap_w, &text);
                *pw = ex;
                *ph = ey;
            }
            *pw += hm + hm + reserve;
            *ph += vm + vm;
            self.gc_pop(g, None);
        }
        *ph = ph.max(bh + vm + vm);
        *pw = pw.max(bw + hm + hm);
        true
    }

    /// `XTextWindow::ParentRequestedPreferredSize`.
    pub(crate) fn text_preferred_size(&mut self, w: u32, w_spec: bool, pw: &mut f32, h_spec: bool, ph: &mut f32) -> bool {
        let g = self.wo_ref_pub(w, "gGc");
        if g == 0 {
            return true;
        }
        self.gc_push(g);
        let wg = self.wo_ref_pub(w, "wGc");
        self.gc_copy(g, wg);
        let st = self.gc_state(g);
        let text = match self.get_prop(w, "text", 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        };
        let (hm, vm) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "vMargin"));
        let (min_lines, max_lines) = (self.wi_pub(w, "minLines"), self.wi_pub(w, "maxLines"));
        let handled = true;
        if !text.eq_ignore_ascii_case("") {
            let wrapped = w_spec && self.wb_pub(w, "bWordWrap");
            let wrap_w = if wrapped { *pw - (hm + hm) } else { 0.0 };
            if !h_spec && (min_lines > -1 || max_lines > -1) {
                let (ex, ey) = self.text_extent(&st, wrap_w, &text);
                if !wrapped {
                    *pw = ex;
                }
                let line_h = self.font_height(&st, true);
                let mut h = ey;
                if min_lines > -1 && h < min_lines as f32 * line_h {
                    h = min_lines as f32 * line_h;
                }
                if max_lines > -1 && (max_lines as f32 * line_h) < h {
                    h = max_lines as f32 * line_h;
                }
                *ph = h;
            } else {
                let (ex, ey) = self.text_extent(&st, wrap_w, &text);
                *pw = ex;
                *ph = ey;
            }
            *pw += hm + hm;
            *ph += vm + vm;
        } else {
            let bg = self.wo_ref_pub(w, "background");
            if bg == 0 {
                // `XWindow::ParentRequestedPreferredSize`: the script event (asm 10046805), then the `MinWidth` clamp
                // below still applies: an unspecified width of -1 becomes `MinWidth` (0), not the window's width
                // (probes `fresh.text`, `scroll.*`)
                let args = vec![Value::Bool(w_spec), Value::Float(*pw), Value::Bool(h_spec), Value::Float(*ph)];
                if let Ok(Some(out)) = self.call_event_out(w, "ParentRequestedPreferredSize", args) {
                    *pw = out.get(1).and_then(Value::as_f32).unwrap_or(*pw);
                    *ph = out.get(3).and_then(Value::as_f32).unwrap_or(*ph);
                }
            } else {
                let (tw, th) = self.texture_size(bg);
                *ph = th;
                if !h_spec {
                    if min_lines > -1 {
                        *ph = min_lines as f32 * self.font_height(&st, true);
                    } else if max_lines > -1 {
                        *ph = max_lines as f32 * self.font_height(&st, true);
                    }
                }
                *pw = tw;
            }
        }
        if !w_spec && *pw < self.wf_pub(w, "minWidth") {
            *pw = self.wf_pub(w, "minWidth");
        }
        self.gc_pop(g, None);
        handled
    }

    /// `XTextWindow::ParentRequestedGranularity` (before the script event adjusts it).
    pub(crate) fn text_granularity(&mut self, w: u32) -> f32 {
        let g = self.wo_ref_pub(w, "wGc");
        let st = self.gc_state(g);
        self.font_height(&st, true)
    }

    /// `XLargeTextWindow::ComputeLineHeight` (1002d9d0): the font's height without spacing, at least 1.
    pub(crate) fn large_line_height(&mut self, w: u32) -> f32 {
        let g = self.wo_ref_pub(w, "wGc");
        if g == 0 {
            return 1.0;
        }
        let st = self.gc_state(g);
        let h = self.font_height(&st, false);
        if h < 1.0 {
            1.0
        } else {
            h
        }
    }

    /// `GenerateLines` + `GetTextExtent` of `XLargeTextWindow` (1002da70, 1002e130): the rows the text breaks into at
    /// `wrap` (0: none): the widest row, and rows x line height + (rows - 1) x `vSpace` (one line height for none).
    fn large_text_extent(&mut self, w: u32, wrap: f32) -> (f32, f32, usize) {
        let lh = self.large_line_height(w);
        let g = self.wo_ref_pub(w, "gGc");
        let wg = self.wo_ref_pub(w, "wGc");
        if g == 0 {
            return (0.0, lh, 0);
        }
        self.gc_push(g);
        self.gc_copy(g, wg);
        let st = self.gc_state(g);
        let text = match self.get_prop(w, "text", 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        };
        let s: Vec<u32> = text.chars().map(|c| c as u32).collect();
        let (mut rows, mut max_w) = (0usize, 0.0f32);
        let mut state = crate::ui_gc::TextState::default();
        let mut pos = 0usize;
        while let Some(line) = self.parse_line(&st, &s, pos, s.len(), &state, wrap) {
            rows += 1;
            max_w = max_w.max(line.width);
            state = line.next_state.clone();
            pos = line.next_pos;
        }
        self.gc_pop(g, None);
        let v = self.wf_pub(w, "vSpace");
        let h = if rows == 0 { lh } else if rows > 1 { (rows - 1) as f32 * v + rows as f32 * lh } else { lh };
        (max_w, h, rows)
    }

    /// `XLargeTextWindow::ParentRequestedPreferredSize` (1002e1d0).
    pub(crate) fn large_text_preferred_size(&mut self, w: u32, w_spec: bool, pw: &mut f32, h_spec: bool, ph: &mut f32) -> bool {
        if self.wo_ref_pub(w, "gGc") == 0 {
            return false;
        }
        let (hm, vm) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "vMargin"));
        let wrap = self.wb_pub(w, "bWordWrap");
        let mut wrap_w = if w_spec && wrap { *pw - (hm + hm) } else { 0.0 };
        if wrap_w < 0.0 {
            wrap_w = 0.0;
        }
        let (ew, eh, _) = self.large_text_extent(w, wrap_w);
        *pw = ew;
        *ph = eh;
        let (min_lines, max_lines) = (self.wi_pub(w, "minLines"), self.wi_pub(w, "maxLines"));
        if !h_spec && (min_lines > -1 || max_lines > -1) {
            let g = self.wo_ref_pub(w, "wGc");
            let st = self.gc_state(g);
            let fh = self.font_height(&st, true);
            let v = self.wf_pub(w, "vSpace");
            // the limits compare with the rows laid out at the window's own width (`rowData`)
            let own = if wrap { self.wf_pub(w, "width") - (hm + hm) } else { 0.0 };
            let (_, _, laid) = self.large_text_extent(w, own.max(0.0));
            if min_lines > -1 && (laid as i32) < min_lines {
                *ph = min_lines as f32 * fh + (min_lines - 1) as f32 * v;
            }
            if max_lines > -1 && max_lines < laid as i32 {
                *ph = max_lines as f32 * fh + (max_lines - 1) as f32 * v;
            }
        }
        *pw += hm + hm;
        *ph += vm + vm;
        if !w_spec && *pw < self.wf_pub(w, "minWidth") {
            *pw = self.wf_pub(w, "minWidth");
        }
        true
    }
}

/// One row of a large text window's text (`XTextRowData`).
#[derive(Debug, Clone)]
pub(crate) struct TextRow {
    pub width: f32,
    pub start: usize,
    pub len: usize,
    pub state: crate::ui_gc::TextState,
}

/// `ClipListSubtract` (1003c1d0): each rectangle present when the call starts that `s` overlaps is replaced by what is
/// left of it: the pieces above, below, left and right of the overlap (the first non-empty one in its place, the
/// others appended); empty rectangles are dropped at the end. Rectangles are absolute `[x, y, w, h]`.
fn clip_list_subtract(list: &mut Vec<[f32; 4]>, s: [f32; 4]) {
    let n = list.len();
    for i in 0..n {
        let r = list[i];
        let (l, t) = (r[0].max(s[0]), r[1].max(s[1]));
        let (rr, b) = ((r[0] + r[2]).min(s[0] + s[2]), (r[1] + r[3]).min(s[1] + s[3]));
        let (iw, ih) = (rr - l, b - t);
        if iw <= 0.0 || ih <= 0.0 {
            continue;
        }
        let pieces = [
            [r[0], r[1], r[2], t - r[1]],
            [r[0], t + ih, r[2], r[1] + r[3] - (t + ih)],
            [r[0], t, l - r[0], ih],
            [l + iw, t, r[0] + r[2] - (l + iw), ih],
        ];
        list[i][3] = 0.0;
        let mut placed = false;
        for p in pieces {
            if p[2] > 0.0 && p[3] > 0.0 {
                if placed {
                    list.push(p);
                } else {
                    list[i] = p;
                    placed = true;
                }
            }
        }
    }
    list.retain(|r| r[2] > 0.0 && r[3] > 0.0);
}

#[cfg(test)]
mod clip_tests {
    use super::clip_list_subtract;

    #[test]
    fn a_hole_leaves_four_pieces_in_order() {
        let mut l = vec![[0.0, 0.0, 100.0, 100.0]];
        clip_list_subtract(&mut l, [10.0, 20.0, 30.0, 40.0]);
        assert_eq!(l, vec![[0.0, 0.0, 100.0, 20.0], [0.0, 60.0, 100.0, 40.0], [0.0, 20.0, 10.0, 40.0], [40.0, 20.0, 60.0, 40.0]]);
        clip_list_subtract(&mut l, [0.0, 0.0, 100.0, 100.0]);
        assert!(l.is_empty());
    }
}

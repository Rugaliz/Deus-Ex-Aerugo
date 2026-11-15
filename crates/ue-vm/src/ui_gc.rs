//! The graphics context (`GC`) of the window system: drawing state, text layout with the game's fonts, and the draw
//! primitives. Drawing does not touch the screen: every primitive appends a textured rectangle to `Vm::ui.draw`
//! (the draw list), which a renderer turns into pixels. Behaviour follows the original `XGC` code (see `docs/ui.md`).
//!
//! The state of a GC lives here (`UiState::gcs`), not in the GC object's script variables: only native code reads it.

use crate::{Value, Vm, VmError};
use std::rc::Rc;

type Res<T> = Result<T, VmError>;

/// One textured rectangle of the draw list. Positions and sizes are in screen units (UI units times the root multiplier);
/// `u, v, ul, vl` are in texels of `tex`.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawCmd {
    pub tex: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub u: f32,
    pub v: f32,
    pub ul: f32,
    pub vl: f32,
    pub color: [f32; 3],
    /// Poly flags of the original: 0x2 masked, 0x4 translucent, 0x40 modulated, 0x800 not smoothed.
    pub poly_flags: u32,
}

/// A 3D view the renderer draws into a rectangle of the screen, between the quads before it (`after` of them) and the
/// rest (a `ViewportWindow`). Rectangles are in screen pixels; `clip` is the part of `rect` that is visible.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewCmd {
    pub after: usize,
    /// Order among the frame's 3D commands (views and `DrawActor`s) that lie after the same quad.
    pub seq: usize,
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub pos: [f32; 3],
    /// Pitch, yaw, roll in rotator units.
    pub rot: [i32; 3],
    /// Horizontal field of view in degrees.
    pub fov: f32,
    pub clear_z: bool,
    /// An actor not to draw (the one the view looks out of), 0 = none.
    pub hide_actor: u32,
    /// Draw the first-person weapon.
    pub show_weapons: bool,
}

/// `GC.DrawActor` / `GC.ClearZ` (Extension.dll `XGC::DrawActor` 1002aa60, `XGC::ClearZ` 1002ae40): an actor drawn by
/// the main view's camera between the window system's quads (after `after` of them), or only a depth clear.
#[derive(Debug, Clone)]
pub struct ActorCmd {
    pub after: usize,
    pub seq: usize,
    /// The actor (0 for a bare `ClearZ`) and its pose with the `DrawActor` overrides applied.
    pub actor: u32,
    pub draw: Option<crate::animation::MeshDraw>,
    pub clear_z: bool,
}


/// Drawing state of a GC.
#[derive(Clone, Debug)]
pub struct GcState {
    pub canvas: bool,
    /// originX, originY (absolute position of the window), then the visible clip rectangle relative to the origin.
    pub clip: [f32; 6],
    pub style: u8,
    pub smoothed: bool,
    pub draw_enabled: bool,
    pub masked: bool,
    pub translucent: bool,
    pub modulated: bool,
    pub text_translucent: bool,
    pub poly_flags: u32,
    pub text_poly_flags: u32,
    pub tile_color: [u8; 4],
    pub text_color: [u8; 4],
    pub normal_font: u32,
    pub bold_font: u32,
    pub underline_tex: u32,
    pub underline_height: f32,
    pub baseline_offset: f32,
    pub v_spacing: f32,
    pub h_align: u8,
    pub v_align: u8,
    pub word_wrap: bool,
    pub parse_meta: bool,
    pub h_mul: i32,
    pub v_mul: i32,
}

impl Default for GcState {
    fn default() -> Self {
        GcState {
            canvas: false,
            clip: [0.0; 6],
            style: 1,
            smoothed: false,
            draw_enabled: true,
            masked: false,
            translucent: false,
            modulated: false,
            text_translucent: false,
            poly_flags: 0x900,
            text_poly_flags: 0x802,
            tile_color: [255, 255, 255, 255],
            text_color: [0, 255, 0, 255],
            normal_font: 0,
            bold_font: 0,
            underline_tex: 0,
            underline_height: 1.0,
            baseline_offset: 2.0,
            v_spacing: 1.0,
            h_align: 0,
            v_align: 0,
            word_wrap: true,
            parse_meta: true,
            h_mul: 1,
            v_mul: 1,
        }
    }
}

impl GcState {
    fn plane(c: [u8; 4]) -> [f32; 3] {
        [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
    }
    pub fn tile_plane(&self) -> [f32; 3] {
        Self::plane(self.tile_color)
    }
    pub fn text_plane(&self) -> [f32; 3] {
        Self::plane(self.text_color)
    }
    /// `GenerateStyle`: the style number from the flags.
    fn generate_style(&mut self) {
        self.style = if !self.draw_enabled {
            0
        } else if self.masked {
            2
        } else if self.translucent {
            3
        } else if self.modulated {
            4
        } else {
            1
        };
    }
    /// `GeneratePolyFlags`.
    fn generate_poly_flags(&mut self) {
        self.poly_flags = 0x100;
        if self.masked {
            self.poly_flags = 0x102;
        }
        if self.translucent {
            self.poly_flags |= 4;
        }
        if self.modulated {
            self.poly_flags |= 0x40;
        }
        if !self.smoothed {
            self.poly_flags |= 0x800;
        }
    }
    pub fn set_style(&mut self, s: u8) {
        self.style = s;
        if s == 0 {
            self.draw_enabled = false;
            return;
        }
        self.draw_enabled = true;
        let (m, t, d) = match s {
            2 => (true, false, false),
            3 => (false, true, false),
            4 => (false, false, true),
            _ => (false, false, false),
        };
        self.masked = m;
        self.translucent = t;
        self.modulated = d;
        self.generate_poly_flags();
    }
    pub fn enable_masking(&mut self, b: bool) {
        self.masked = b;
        self.generate_style();
        self.generate_poly_flags();
    }
    pub fn enable_translucency(&mut self, b: bool) {
        self.translucent = b;
        self.generate_style();
        self.generate_poly_flags();
    }
    pub fn enable_modulation(&mut self, b: bool) {
        self.modulated = b;
        self.generate_style();
        self.generate_poly_flags();
    }
    pub fn enable_smoothing(&mut self, b: bool) {
        self.smoothed = b;
        self.generate_poly_flags();
    }
    pub fn enable_translucent_text(&mut self, b: bool) {
        self.text_translucent = b;
        self.text_poly_flags = if b { 0x804 } else { 0x802 };
    }
    /// `Intersect`: shrinks the clip rectangle to the given one (relative to the origin).
    pub fn intersect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let [ox, oy, cx, cy, cw, ch] = self.clip;
        let (l, t) = (x + ox, y + oy);
        let (mut left, mut top) = (cx + ox, cy + oy);
        let (mut right, mut bottom) = (left + cw, top + ch);
        left = left.max(l);
        top = top.max(t);
        right = right.min(l + w);
        bottom = bottom.min(t + h);
        self.clip[2] = left - ox;
        self.clip[3] = top - oy;
        self.clip[4] = right - left;
        self.clip[5] = bottom - top;
    }
}

/// A font with its page textures resolved to resource ids.
#[derive(Debug)]
pub struct FontData {
    pages: Vec<(u32, Vec<ue_assets::font::Glyph>)>,
    per_page: usize,
}

/// A glyph: page texture and rectangle.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlyphRef {
    pub tex: u32,
    pub start_u: i32,
    pub start_v: i32,
    pub w: i32,
    pub h: i32,
}

impl FontData {
    pub(crate) fn glyph(&self, ch: u32) -> GlyphRef {
        let per = self.per_page.max(1);
        let (page, idx) = (ch as usize / per, ch as usize % per);
        match self.pages.get(page).and_then(|(t, g)| g.get(idx).map(|g| (*t, *g))) {
            Some((tex, g)) => GlyphRef { tex, start_u: g.start_u, start_v: g.start_v, w: g.u_size, h: g.v_size },
            None => GlyphRef::default(),
        }
    }
}

/// Text parsing state carried from line to line (the original's `XTextState`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextState {
    pub bold: bool,
    pub has_color: bool,
    /// The previous line ended with a newline character.
    pub newline: bool,
    /// The character just read is to be underlined.
    pub underline: bool,
    pub plane: [f32; 3],
}

pub(crate) struct Line {
    pub(crate) start: usize,
    pub(crate) len: usize,
    pub(crate) width: f32,
    pub(crate) next_pos: usize,
    pub(crate) next_state: TextState,
}

const PALETTE: [[f32; 3]; 8] = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]];

fn lower(c: u32) -> u32 {
    if (0x41..0x5b).contains(&c) {
        c + 0x20
    } else {
        c
    }
}

fn hex_digit(c: u32) -> Option<u8> {
    match c {
        0x30..=0x39 => Some((c - 0x30) as u8),
        0x61..=0x66 => Some((c - 0x61 + 10) as u8),
        0x41..=0x46 => Some((c - 0x41 + 10) as u8),
        _ => None,
    }
}

/// Reads one colour byte (two hex digits) at `pos`.
fn color_byte(s: &[u32], pos: &mut usize, end: usize) -> Option<u8> {
    if *pos >= end {
        return None;
    }
    let hi = hex_digit(s[*pos])?;
    *pos += 1;
    let mut v = hi << 4;
    if *pos < end {
        if let Some(lo) = hex_digit(s[*pos]) {
            v |= lo;
            *pos += 1;
        }
    }
    Some(v)
}

impl Vm {
    // ---- fonts -----------------------------------------------------------------------------------------------

    pub(crate) fn font_data(&mut self, font: u32) -> Option<Rc<FontData>> {
        if font == 0 {
            return None;
        }
        if let Some(f) = self.ui.fonts.get(&font) {
            return f.clone();
        }
        let path = self.reg.obj_path(ue_registry::ObjId(font)).to_string();
        self.reg.ensure_package_for_path(&path);
        let data = (|| {
            let (pi, ei) = self.reg.export_by_path(&path)?;
            let f = ue_assets::font::Font::load(&self.reg.packages[pi].pkg, ei).ok()?;
            let mut pages = Vec::new();
            for p in &f.pages {
                let tex = match self.reg.ref_path(pi, p.texture) {
                    Some(path) => self.reg.obj_id(&path).0,
                    None => 0,
                };
                pages.push((tex, p.glyphs.clone()));
            }
            Some(Rc::new(FontData { pages, per_page: f.characters_per_page as usize }))
        })();
        self.ui.fonts.insert(font, data.clone());
        data
    }

    fn glyph_of(&mut self, font: u32, ch: u32) -> GlyphRef {
        self.font_data(font).map(|f| f.glyph(ch)).unwrap_or_default()
    }

    // ---- text ------------------------------------------------------------------------------------------------

    /// `GetNextChar`: the next character, applying the `|b`, `|c`, `|p` and `|&` markup (when enabled). Returns 0 at the end.
    fn next_char(gc: &GcState, s: &[u32], pos: &mut usize, end: usize, st: &mut TextState) -> u32 {
        st.underline = false;
        if *pos >= end {
            return 0;
        }
        while gc.parse_meta && s[*pos] == '|' as u32 {
            *pos += 1;
            if *pos >= end {
                return 0;
            }
            let mut ch = lower(s[*pos]);
            let mut inverse = false;
            if ch == '!' as u32 {
                inverse = true;
                *pos += 1;
                if *pos >= end {
                    return 0;
                }
                ch = lower(s[*pos]);
            }
            match char::from_u32(ch).unwrap_or('\0') {
                'b' => {
                    *pos += 1;
                    st.bold = !inverse;
                }
                'c' => {
                    *pos += 1;
                    if !inverse {
                        let mut rgb = [0u8; 3];
                        if let Some(r) = color_byte(s, pos, end) {
                            rgb[0] = r;
                            if let Some(g) = color_byte(s, pos, end) {
                                rgb[1] = g;
                                if let Some(b) = color_byte(s, pos, end) {
                                    rgb[2] = b;
                                }
                            }
                        }
                        st.plane = [rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0];
                        st.has_color = true;
                    } else {
                        st.plane = gc.text_plane();
                        st.has_color = false;
                    }
                }
                'p' => {
                    *pos += 1;
                    if *pos < end {
                        if !inverse {
                            let d = s[*pos];
                            if (0x30..0x38).contains(&d) {
                                *pos += 1;
                                st.plane = PALETTE[(d - 0x30) as usize];
                                st.has_color = true;
                            }
                        } else {
                            st.plane = gc.text_plane();
                            st.has_color = false;
                        }
                    }
                }
                '&' => {
                    *pos += 1;
                    st.underline = true;
                }
                _ => {
                    // an unknown command: the character itself
                    let out = s[*pos];
                    *pos += 1;
                    return out;
                }
            }
            if *pos >= end {
                return 0;
            }
        }
        let c = s[*pos];
        *pos += 1;
        c
    }

    /// `ParseLine`: one line of text starting at `start`, wrapped at `max_width` (<= 0 = no limit).
    pub(crate) fn parse_line(&mut self, gc: &GcState, s: &[u32], start: usize, end: usize, state_in: &TextState, max_width: f32) -> Option<Line> {
        let max_w = if max_width <= 0.0 { 500000.0 } else { max_width };
        let mut st = state_in.clone();
        st.newline = false;
        let prior_newline = state_in.newline;
        let mut pos = start;
        let mut font = if st.bold { gc.bold_font } else { gc.normal_font };
        let (mut cur, mut cand_end, mut cand_w): (f32, Option<usize>, f32) = (0.0, None, 0.0);
        let (mut word_start, mut word_state): (Option<usize>, TextState) = (None, st.clone());
        let (mut in_space, mut wrapped, mut first) = (true, false, true);
        let next_pos;
        loop {
            let prev_state = st.clone();
            let prev_pos = pos;
            let c = Self::next_char(gc, s, &mut pos, end, &mut st);
            if c == 0 || c == 10 {
                let (e, w) = if wrapped { (cand_end.unwrap_or(prev_pos), cand_w) } else { (prev_pos, cur) };
                cand_end = Some(e);
                cand_w = w;
                if c == 10 {
                    st.newline = true;
                }
                next_pos = pos;
                break;
            }
            let is_space = (9..=13).contains(&c) || c == 32;
            if !is_space {
                if in_space {
                    if wrapped {
                        st = prev_state;
                        next_pos = prev_pos;
                        break;
                    }
                    in_space = false;
                    word_start = Some(prev_pos);
                    word_state = prev_state.clone();
                }
            } else if !in_space {
                in_space = true;
                cand_end = Some(prev_pos);
                cand_w = cur;
            }
            if st.bold != prev_state.bold {
                font = if st.bold { gc.bold_font } else { gc.normal_font };
            }
            if font != 0 {
                let g = self.glyph_of(font, c);
                if max_w < g.w as f32 + cur && !first {
                    match cand_end {
                        None => {
                            st = prev_state;
                            cand_end = Some(prev_pos);
                            cand_w = cur;
                            next_pos = prev_pos;
                            break;
                        }
                        Some(ce) if word_start.map(|w| ce < w).unwrap_or(false) => {
                            st = word_state;
                            next_pos = word_start.unwrap();
                            break;
                        }
                        _ => wrapped = true,
                    }
                }
                cur += g.w as f32;
            }
            first = false;
        }
        let end_pos = cand_end.unwrap_or(start);
        if next_pos <= start && !prior_newline {
            return None;
        }
        Some(Line { start, len: end_pos.saturating_sub(start), width: cand_w, next_pos, next_state: st })
    }

    fn wrap_width(gc: &GcState, w: f32) -> f32 {
        if gc.word_wrap {
            w
        } else {
            0.0
        }
    }

    /// `GetTextExtent`: width and height of the text when wrapped at `wrap_width` (0 = no wrapping). The height includes
    /// the line spacing after every line.
    pub(crate) fn text_extent(&mut self, gc: &GcState, wrap_width: f32, text: &str) -> (f32, f32) {
        let s: Vec<u32> = text.chars().map(|c| c as u32).collect();
        let end = s.len();
        let (mut max_w, mut total_h) = (0.0f32, 0.0f32);
        let mut state = TextState::default();
        let mut pos = 0usize;
        let mut font = gc.normal_font;
        let mut cur_bold = state.bold;
        while let Some(line) = self.parse_line(gc, &s, pos, end, &state, wrap_width) {
            state = line.next_state.clone();
            pos = line.next_pos;
            let mut p = line.start;
            let line_end = line.start + line.len;
            let mut line_h = 0.0f32;
            while p < line_end {
                let c = Self::next_char(gc, &s, &mut p, line_end, &mut state);
                if c == 0 {
                    break;
                }
                if cur_bold != state.bold {
                    cur_bold = state.bold;
                    font = if cur_bold { gc.bold_font } else { gc.normal_font };
                }
                if c != 10 && font != 0 {
                    let g = self.glyph_of(font, c);
                    line_h = line_h.max(g.h as f32);
                }
            }
            if line_h < 1.0 && font != 0 {
                line_h = self.glyph_of(font, 32).h as f32;
            }
            max_w = max_w.max(line.width);
            total_h += line_h + gc.v_spacing;
        }
        (max_w, total_h)
    }

    /// `GetFontHeight`: the taller of the two fonts' space glyphs (plus the line spacing).
    pub(crate) fn font_height(&mut self, gc: &GcState, include_space: bool) -> f32 {
        let mut h = 0.0f32;
        for f in [gc.normal_font, gc.bold_font] {
            if f != 0 {
                h = h.max(self.glyph_of(f, 32).h as f32);
            }
        }
        if include_space {
            h += gc.v_spacing;
        }
        h
    }

    // ---- primitives ------------------------------------------------------------------------------------------

    /// `ClipTile` + `DrawTile`: a texture rectangle (`x, y, w, h` relative to the GC origin) showing texels `(u, v, ul, vl)`,
    /// clipped to the GC's clip rectangle. `tile_x`/`tile_y` repeat the texture instead of scaling it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_tile(&mut self, gc: &GcState, tex: u32, x: f32, y: f32, w: f32, h: f32, u: f32, v: f32, ul: f32, vl: f32, tile_x: bool, tile_y: bool) {
        if !gc.draw_enabled || !gc.canvas || tex == 0 {
            return;
        }
        let (tw, th) = self.texture_size(tex);
        let [ox, oy, cx, cy, cw, ch] = gc.clip;
        let (mut x, mut y, mut w, mut h, mut u, mut v, mut ul, mut vl) = (x + ox, y + oy, w, h, u, v, ul, vl);
        let (left, top) = (cx + ox, cy + oy);
        if !tile_x {
            if w > 0.0 && x < left {
                let d = ((x - left) * ul) / w;
                ul += d;
                u -= d;
                w += x - left;
                x = left;
            }
            if w > 0.0 {
                let right = left + cw;
                if right < x + w {
                    ul += ((right - (x + w)) * ul) / w;
                    w = right - x;
                }
            }
            if ul > 0.0 && u < 0.0 {
                let d = (u * w) / ul;
                w += d;
                x -= d;
                ul += u;
                u = 0.0;
            }
            if ul > 0.0 && tw < u + ul {
                w += ((tw - (u + ul)) * w) / ul;
                ul = tw - u;
            }
        } else {
            if x < left {
                u -= x - left;
                w += x - left;
                x = left;
            }
            let right = left + cw;
            if x + w > right {
                w = right - x;
            }
            ul = w;
        }
        if !tile_y {
            if h > 0.0 && y < top {
                let d = ((y - top) * vl) / h;
                vl += d;
                v -= d;
                h += y - top;
                y = top;
            }
            if h > 0.0 {
                let bottom = top + ch;
                if bottom < y + h {
                    vl += ((bottom - (y + h)) * vl) / h;
                    h = bottom - y;
                }
            }
            if vl > 0.0 && v < 0.0 {
                let d = (v * h) / vl;
                h += d;
                y -= d;
                vl += v;
                v = 0.0;
            }
            if vl > 0.0 && th < v + vl {
                h += ((th - (v + vl)) * h) / vl;
                vl = th - v;
            }
        } else {
            if y < top {
                v -= y - top;
                h += y - top;
                y = top;
            }
            let bottom = top + ch;
            if y + h > bottom {
                h = bottom - y;
            }
            vl = h;
        }
        if w > 0.0 && h > 0.0 && ul > 0.0 && vl > 0.0 {
            // the original snaps each value to a whole pixel/texel before the canvas: trunc(v + 0.1)
            let n = |v: f32| (v as f64 + 0.1).trunc() as i32 as f32;
            let (hm, vm) = (gc.h_mul as f32, gc.v_mul as f32);
            let (sw, sh) = (n(w) * hm, n(h) * vm);
            if sw > 0.0 && sh > 0.0 {
                // `UCanvas::DrawTile` draws the texture's current animation frame (`Texture.Get(CurrentTime)`), masked
                // when that frame is `bMasked` whatever the GC's own flags
                let tex = self.texture_get(tex, self.ui.viewport_time);
                let poly_flags = gc.poly_flags | if self.texture_masked(tex) { 2 } else { 0 };
                self.ui.draw.push(DrawCmd { tex, x: n(x) * hm, y: n(y) * vm, w: sw, h: sh, u: n(u), v: n(v), ul: n(ul), vl: n(vl), color: gc.tile_plane(), poly_flags });
            }
        }
    }

    pub(crate) fn gc_draw_icon_pattern(&mut self, gc: &GcState, x: f32, y: f32, w: f32, h: f32, u: f32, v: f32, ul: f32, vl: f32, tex: u32) {
        if tex != 0 && gc.draw_enabled {
            self.draw_tile(gc, tex, x, y, w, h, u, v, ul, vl, ul == 0.0, vl == 0.0);
        }
    }

    pub(crate) fn gc_draw_pattern(&mut self, gc: &GcState, x: f32, y: f32, w: f32, h: f32, org_x: f32, org_y: f32, tex: u32) {
        if tex != 0 && gc.draw_enabled {
            self.draw_tile(gc, tex, x, y, w, h, org_x, org_y, 0.0, 0.0, true, true);
        }
    }

    pub(crate) fn gc_draw_stretched_icon(&mut self, gc: &GcState, x: f32, y: f32, w: f32, h: f32, tex: u32) {
        if tex != 0 && gc.draw_enabled {
            let (tw, th) = self.texture_size(tex);
            self.draw_tile(gc, tex, x, y, w, h, 0.0, 0.0, tw, th, false, false);
        }
    }

    pub(crate) fn gc_draw_box(&mut self, gc: &GcState, x: f32, y: f32, w: f32, h: f32, org_x: f32, org_y: f32, t: f32, tex: u32) {
        if tex == 0 || !gc.draw_enabled || t <= 0.0 {
            return;
        }
        if w > t + t && h > t + t {
            self.gc_draw_pattern(gc, x, y, w, t, org_x, org_y, tex);
            self.gc_draw_pattern(gc, x, y + h - t, w, t, org_x, org_y + h - t, tex);
            self.gc_draw_pattern(gc, x, y + t, t, h - t - t, org_x, org_y + t, tex);
            self.gc_draw_pattern(gc, x + w - t, y + t, t, h - t - t, org_x + w - t, org_y + t, tex);
        } else {
            self.gc_draw_pattern(gc, x, y, w, h, org_x, org_y, tex);
        }
    }

    /// `DrawBorders`: nine textures (corners, edges, centre) around a rectangle; `margins` are left, right, top, bottom
    /// (0 = from the textures). `tex` order: top-left, top-right, bottom-left, bottom-right, left, right, top, bottom, centre.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn gc_draw_borders(&mut self, gc: &GcState, x: f32, y: f32, w: f32, h: f32, margins: [f32; 4], tex: [u32; 9], stretch_h: bool, stretch_v: bool) {
        if !gc.draw_enabled || w <= 0.0 || h <= 0.0 {
            return;
        }
        let sz = |vm: &mut Vm, t: u32| vm.texture_size(t);
        let [tl, tr, bl, br, l, r, t, b, c] = tex;
        let (mut left, mut right, mut top, mut bottom) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        if tl != 0 {
            let s = sz(self, tl);
            if s.0 > 0.0 {
                left = s.0;
            }
            if s.1 > 0.0 {
                top = s.1;
            }
        }
        if tr != 0 {
            let s = sz(self, tr);
            if s.0 > 0.0 {
                right = s.0;
            }
            top = top.max(s.1);
        }
        if bl != 0 {
            let s = sz(self, bl);
            left = left.max(s.0);
            if s.1 > 0.0 {
                bottom = s.1;
            }
        }
        if br != 0 {
            let s = sz(self, br);
            right = right.max(s.0);
            bottom = bottom.max(s.1);
        }
        if l != 0 {
            left = left.max(sz(self, l).0);
        }
        if r != 0 {
            right = right.max(sz(self, r).0);
        }
        if t != 0 {
            top = top.max(sz(self, t).1);
        }
        if b != 0 {
            bottom = bottom.max(sz(self, b).1);
        }
        if margins[0] > 0.0 {
            left = margins[0];
        }
        if margins[1] > 0.0 {
            right = margins[1];
        }
        if margins[2] > 0.0 {
            top = margins[2];
        }
        if margins[3] > 0.0 {
            bottom = margins[3];
        }
        let mut mid_w = w - (right + left);
        let mut mid_h = h - (bottom + top);
        if mid_w < 0.0 {
            let k = w / (right + left);
            mid_w = 0.0;
            left *= k;
            right *= k;
        }
        if mid_h < 0.0 {
            let k = h / (bottom + top);
            mid_h = 0.0;
            top *= k;
            bottom *= k;
        }
        let (x1, x2) = (x + left, x + left + mid_w);
        let (y1, y2) = (y + top, y + top + mid_h);
        let ts = |vm: &mut Vm, t: u32| vm.texture_size(t);
        if tl != 0 {
            let s = ts(self, tl);
            self.gc_draw_icon_pattern(gc, x, y, left, top, s.0 - left, s.1 - top, left, top, tl);
        }
        if tr != 0 {
            let s = ts(self, tr);
            self.gc_draw_icon_pattern(gc, x2, y, right, top, 0.0, s.1 - top, right, top, tr);
        }
        if bl != 0 {
            let s = ts(self, bl);
            self.gc_draw_icon_pattern(gc, x, y2, left, bottom, s.0 - left, 0.0, left, bottom, bl);
        }
        if br != 0 {
            self.gc_draw_icon_pattern(gc, x2, y2, right, bottom, 0.0, 0.0, right, bottom, br);
        }
        if l != 0 {
            let s = ts(self, l);
            self.gc_draw_icon_pattern(gc, x, y1, left, mid_h, s.0 - left, 0.0, left, if stretch_v { s.1 } else { 0.0 }, l);
        }
        if r != 0 {
            let s = ts(self, r);
            self.gc_draw_icon_pattern(gc, x2, y1, right, mid_h, 0.0, 0.0, right, if stretch_v { s.1 } else { 0.0 }, r);
        }
        if t != 0 {
            let s = ts(self, t);
            self.gc_draw_icon_pattern(gc, x1, y, mid_w, top, 0.0, s.1 - top, if stretch_h { s.0 } else { 0.0 }, top, t);
        }
        if b != 0 {
            let s = ts(self, b);
            self.gc_draw_icon_pattern(gc, x1, y2, mid_w, bottom, 0.0, 0.0, if stretch_h { s.0 } else { 0.0 }, bottom, b);
        }
        if c != 0 {
            let s = ts(self, c);
            self.gc_draw_icon_pattern(gc, x1, y1, mid_w, mid_h, 0.0, 0.0, if stretch_h { s.0 } else { 0.0 }, if stretch_v { s.1 } else { 0.0 }, c);
        }
    }

    /// `DrawChar`: one glyph at `(x, y)` (relative to the GC origin), clipped; returns the advance and the glyph height.
    fn gc_draw_char(&mut self, gc: &GcState, font: u32, ch: u32, plane: [f32; 3], underline: bool, x: f32, y: f32) -> (f32, f32) {
        let g = self.glyph_of(font, ch);
        let [ox, oy, cx, cy, cw, chh] = gc.clip;
        let (mut gx, mut gy) = (x + ox, y + oy);
        let (mut w, mut h) = (g.w as f32, g.h as f32);
        let (mut u, mut v) = (g.start_u as f32, g.start_v as f32);
        let (left, top) = (cx + ox, cy + oy);
        if gx < left {
            u -= gx - left;
            w += gx - left;
            gx = left;
        }
        if gy < top {
            v -= gy - top;
            h += gy - top;
            gy = top;
        }
        if left + cw < w + gx {
            w = left + cw - gx;
        }
        if top + chh < h + gy {
            h = top + chh - gy;
        }
        // each value snapped to a whole pixel/texel as the original's `DrawChar`: trunc(v + 0.1)
        let n = |v: f32| (v as f64 + 0.1).trunc() as i32 as f32;
        if w > 0.0 && h > 0.0 && gc.draw_enabled && gc.canvas {
            let (hm, vm) = (gc.h_mul as f32, gc.v_mul as f32);
            let (sw, sh) = (n(w), n(h));
            if sw > 0.0 && sh > 0.0 {
                self.ui.draw.push(DrawCmd { tex: g.tex, x: n(gx) * hm, y: n(gy) * vm, w: sw * hm, h: sh * vm, u: n(u), v: n(v), ul: sw, vl: sh, color: plane, poly_flags: gc.text_poly_flags });
            }
        }
        if underline && gc.underline_tex != 0 && gc.underline_height > 0.0 && gc.draw_enabled && gc.canvas {
            let (hm, vm) = (gc.h_mul as f32, gc.v_mul as f32);
            let uy = g.h as f32 - gc.baseline_offset + oy + y;
            let (mut lx, mut ly, mut lw, mut lh) = (x + ox, uy, (g.w - 1) as f32, gc.underline_height);
            // the texel window moves with a cut at the left or top
            let (mut lu, mut lv) = (0.0f32, 0.0f32);
            if lx < left {
                lu -= lx - left;
                lw += lx - left;
                lx = left;
            }
            if ly < top {
                lv -= ly - top;
                lh += ly - top;
                ly = top;
            }
            if left + cw < lw + lx {
                lw = left + cw - lx;
            }
            if top + chh < lh + ly {
                lh = top + chh - ly;
            }
            if lw > 0.0 && lh > 0.0 && n(lw) > 0.0 && n(lh) > 0.0 {
                self.ui.draw.push(DrawCmd { tex: gc.underline_tex, x: n(lx) * hm, y: n(ly) * vm, w: n(lw) * hm, h: n(lh) * vm, u: n(lu), v: n(lv), ul: n(lw), vl: n(lh), color: plane, poly_flags: gc.text_poly_flags });
            }
        }
        (g.w as f32, g.h as f32)
    }

    /// `DrawText`: the text inside the rectangle `(x, y, w, h)` with the GC's alignments, wrapping and markup.
    pub(crate) fn gc_draw_text(&mut self, gc_in: &GcState, x: f32, y: f32, w: f32, h: f32, text: &str) {
        if !gc_in.draw_enabled || !gc_in.canvas {
            return;
        }
        let mut gc = gc_in.clone();
        gc.intersect(x, y, w, h);
        let s: Vec<u32> = text.chars().map(|c| c as u32).collect();
        let end = s.len();
        let wrap = Self::wrap_width(&gc, w).trunc();
        let mut y = y;
        if gc.v_align != 0 {
            let (_, th) = self.text_extent(&gc, wrap, text);
            if gc.v_align == 1 {
                y += (((h - th) as i32) / 2) as f32;
            } else if gc.v_align == 2 {
                y = (y + h) - th;
            }
        }
        let mut state = TextState { plane: gc.text_plane(), ..Default::default() };
        let mut pos = 0usize;
        let mut font = gc.normal_font;
        let mut cur_bold = false;
        while let Some(line) = self.parse_line(&gc, &s, pos, end, &state, wrap) {
            let mut lx = match gc.h_align {
                1 => (((w - line.width) as i32) / 2) as f32 + x,
                2 => (x + w) - line.width,
                _ => x,
            };
            let line_end = line.start + line.len;
            let mut p = line.start;
            let mut line_h = 0.0f32;
            while p < line_end {
                let c = Self::next_char(&gc, &s, &mut p, line_end, &mut state);
                if c == 0 {
                    break;
                }
                if cur_bold != state.bold {
                    cur_bold = state.bold;
                    font = if cur_bold { gc.bold_font } else { gc.normal_font };
                }
                if c != 10 && font != 0 {
                    let plane = if state.has_color { state.plane } else { gc.text_plane() };
                    let (adv, gh) = self.gc_draw_char(&gc, font, c, plane, state.underline, lx, y);
                    lx += adv;
                    line_h = line_h.max(gh);
                }
            }
            if line_h < 1.0 && font != 0 {
                line_h = self.glyph_of(font, 32).h as f32;
            }
            y += line_h + gc.v_spacing;
            // the next line starts from the state this one ended in
            state = line.next_state;
            pos = line.next_pos;
        }
    }

    /// `DrawTextLine`: one already broken row of text from its start state, no alignment or line advance (the large
    /// text window's rows).
    #[allow(clippy::too_many_arguments)]
    /// `inverse`: (xs, xe, plane) draws the characters whose pen x lies in [xs, xe) in that colour instead of the GC's
    /// text colour (an edit window's selection, drawn character by character).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn gc_draw_text_line(&mut self, gc: &GcState, x: f32, y: f32, s: &[u32], start: usize, len: usize, state_in: &TextState, inverse: Option<(f32, f32, [f32; 3])>) {
        if !gc.draw_enabled || !gc.canvas {
            return;
        }
        let mut state = state_in.clone();
        let mut cur_bold = state.bold;
        let mut font = if cur_bold { gc.bold_font } else { gc.normal_font };
        let end = (start + len).min(s.len());
        let mut p = start;
        let mut lx = x;
        while p < end {
            let c = Self::next_char(gc, s, &mut p, end, &mut state);
            if c == 0 {
                break;
            }
            if cur_bold != state.bold {
                cur_bold = state.bold;
                font = if cur_bold { gc.bold_font } else { gc.normal_font };
            }
            if c != 10 && font != 0 {
                let plane = match inverse {
                    _ if state.has_color => state.plane,
                    Some((xs, xe, p)) if lx >= xs && lx < xe => p,
                    _ => gc.text_plane(),
                };
                let (adv, _) = self.gc_draw_char(gc, font, c, plane, state.underline, lx, y);
                lx += adv;
            }
        }
    }

    // ---- GC objects ------------------------------------------------------------------------------------------

    /// Creates a GC object with default state.
    pub(crate) fn new_gc(&mut self) -> Res<u32> {
        let class = self.reg.class_by_name("GC").ok_or_else(|| VmError::Runtime("no GC class".into()))?;
        let o = self.new_object(class)?;
        // the texture of accelerator underlines is a default property of the class (nothing in the native code sets it)
        let underline = self.wo_ref_pub(o, "underlineTexture");
        self.ui.gcs.insert(o, GcState { underline_tex: underline, ..GcState::default() });
        Ok(o)
    }

    pub(crate) fn gc_state(&self, gc: u32) -> GcState {
        self.ui.gcs.get(&gc).cloned().unwrap_or_default()
    }

    pub(crate) fn gc_mut(&mut self, gc: u32) -> &mut GcState {
        self.ui.gcs.entry(gc).or_default()
    }

    /// `PushGC`: saves the state; returns the depth to pass to `PopGC`.
    pub(crate) fn gc_push(&mut self, gc: u32) -> i32 {
        let st = self.gc_state(gc);
        let stack = self.ui.gc_stacks.entry(gc).or_default();
        stack.push(st);
        stack.len() as i32
    }

    /// `PopGC(depth)`: restores the state saved by the push that returned `depth`.
    pub(crate) fn gc_pop(&mut self, gc: u32, depth: Option<i32>) {
        let stack = self.ui.gc_stacks.entry(gc).or_default();
        let target = depth.map(|d| (d - 1).max(0) as usize).unwrap_or(stack.len().saturating_sub(1));
        let mut restored = None;
        while stack.len() > target {
            restored = stack.pop();
        }
        if let Some(st) = restored {
            self.ui.gcs.insert(gc, st);
        }
    }

    /// `CopyGC`: copies the drawing state (not the stack).
    pub(crate) fn gc_copy(&mut self, dst: u32, src: u32) {
        let st = self.gc_state(src);
        self.ui.gcs.insert(dst, st);
    }

    pub(crate) fn gc_text_value(v: &Value) -> String {
        match v {
            Value::Str(s) => s.clone(),
            _ => String::new(),
        }
    }
}

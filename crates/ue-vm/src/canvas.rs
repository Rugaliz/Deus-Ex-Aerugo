//! The text and tile natives of `Canvas` (Engine `UCanvas`): what the player's and the console's `PostRender` draw
//! over the frame after the window system (`UGameEngine::Draw`). They draw into `Vm::canvas_frame` (the scene frame,
//! `Canvas.FramePtr`) and collect screen-pixel draw commands in `Vm::canvas_list`. Spec: `docs/canvas.md`.

use crate::ui_gc::{DrawCmd, FontData};
use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

/// The scene frame the canvas draws into (`FSceneNode`): its top-left corner on the screen and its size in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CanvasFrame {
    pub xb: i32,
    pub yb: i32,
    pub x: i32,
    pub y: i32,
}

/// MSVC `_ftol` (truncation).
fn ftol(v: f32) -> i32 {
    crate::route::ftol(v as f64)
}

/// The poly flags of a canvas `Style` (`STY_None` 1 = invisible, `STY_Translucent` 4, `STY_Modulated` 0x40, else 0;
/// `STY_Masked` adds nothing of its own).
fn style_flags(style: u8) -> u32 {
    match style {
        0 => 1,
        3 => 4,
        4 => 0x40,
        _ => 0,
    }
}

fn chars(v: Option<&Value>) -> Vec<u32> {
    match v {
        Some(Value::Str(s)) => s.chars().map(|c| c as u32).collect(),
        _ => Vec::new(),
    }
}

impl Vm {
    fn canvas_obj(&self, c: u32, name: &str) -> u32 {
        match self.get_prop(c, name, 0) {
            Ok(Value::Obj(o)) => o,
            _ => 0,
        }
    }

    fn canvas_style(&self, c: u32) -> u8 {
        match self.get_prop(c, "Style", 0) {
            Ok(Value::Byte(b)) => b,
            _ => 0,
        }
    }

    /// `DrawColor.Plane()`: red, green, blue over 255.
    fn canvas_color(&self, c: u32) -> [f32; 3] {
        let k = 1.0f32 / 255.0;
        match self.get_prop(c, "DrawColor", 0) {
            Ok(Value::Struct(_, b)) if b.len() >= 3 => [b[0] as f32 * k, b[1] as f32 * k, b[2] as f32 * k],
            _ => [0.0; 3],
        }
    }

    /// `UCanvas::DrawTile` (103019fb): a texture rectangle at `(x, y)` in the frame, clipped to it (the texels with
    /// it), drawn with the texture's current animation frame, masked when that frame is `bMasked`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn canvas_tile(&mut self, tex: u32, x: f32, y: f32, xl: f32, yl: f32, u: f32, v: f32, ul: f32, vl: f32, color: [f32; 3], flags: u32) {
        let f = self.canvas_frame;
        let (fx, fy) = (f.x as f32, f.y as f32);
        let (mut x, mut y, mut xl, mut yl, mut u, mut v, mut ul, mut vl) = (x, y, xl, yl, u, v, ul, vl);
        if tex == 0 || !(xl > 0.0 && yl > 0.0 && x + xl > 0.0 && y + yl > 0.0 && x < fx && y < fy) {
            return;
        }
        if x < 0.0 {
            let d = (x * ul) / xl;
            u -= d;
            ul += d;
            xl += x;
            x = 0.0;
        }
        if y < 0.0 {
            let d = (y * vl) / yl;
            v -= d;
            vl += d;
            yl += y;
            y = 0.0;
        }
        if fx - x < xl {
            ul += ((fx - x - xl) * ul) / xl;
            xl = fx - x;
        }
        if fy - y < yl {
            vl += ((fy - y - yl) * vl) / yl;
            yl = fy - y;
        }
        let tex = self.texture_get(tex, self.ui.viewport_time);
        let poly_flags = flags | if self.texture_masked(tex) { 2 } else { 0 };
        self.canvas_list.push(DrawCmd { tex, x: x + f.xb as f32, y: y + f.yb as f32, w: xl, h: yl, u, v, ul, vl, color, poly_flags });
    }

    /// One glyph through the render device (Engine 10377170): nothing when invisible; clipped to the frame in whole
    /// pixels, the drawn size being the clipped texel size.
    #[allow(clippy::too_many_arguments)]
    fn canvas_glyph(&mut self, flags: u32, tex: u32, x: i32, y: i32, xl: i32, yl: i32, u: i32, v: i32, ul: i32, vl: i32, color: [f32; 3]) {
        let f = self.canvas_frame;
        if flags & 1 != 0 || !(x.wrapping_add(xl) > 0 && y.wrapping_add(yl) > 0 && x < f.x && y < f.y && xl > 0 && yl > 0) {
            return;
        }
        let (mut x, mut y, mut xl, mut yl, mut u, mut v, mut ul, mut vl) = (x, y, xl, yl, u, v, ul, vl);
        if x < 0 {
            let d = x.wrapping_mul(ul) / xl;
            ul += d;
            u -= d;
            xl += x;
            x = 0;
        }
        if y < 0 {
            let d = y.wrapping_mul(vl) / yl;
            v -= d;
            vl += d;
            yl += y;
            y = 0;
        }
        if f.x - x < xl {
            ul += (f.x - x - xl).wrapping_mul(ul) / xl;
        }
        if f.y - y < yl {
            vl += (f.y - y - yl).wrapping_mul(vl) / yl;
        }
        let (ul, vl) = (ul as f32, vl as f32);
        self.canvas_list.push(DrawCmd { tex, x: (x + f.xb) as f32, y: (y + f.yb) as f32, w: ul, h: vl, u: u as f32, v: v as f32, ul, vl, color, poly_flags: flags });
    }

    /// `DrawString` (Engine 10376d20): the glyphs of `text` from `(draw_x, draw_y)` plus the canvas origin, each
    /// advanced by its width plus `SpaceX`; returns the width. `clip` clips them to `ClipX`/`ClipY` (before the
    /// origin); `hotkey` draws `&x` as `x` with an underscore over it.
    #[allow(clippy::too_many_arguments)]
    fn canvas_draw_string(&mut self, c: u32, flags: u32, font: &FontData, draw_x: i32, draw_y: i32, text: &[u32], color: [f32; 3], clip: bool, hotkey: bool) -> i32 {
        let (org_x, org_y, clip_x, clip_y, space_x) = (self.fp(c, "OrgX"), self.fp(c, "OrgY"), self.fp(c, "ClipX"), self.fp(c, "ClipY"), self.fp(c, "SpaceX"));
        let (mut line_x, mut underline, mut uwidth) = (0i32, false, 0i32);
        let at = |i: usize| text.get(i).copied().unwrap_or(0);
        let mut i = 0;
        while at(i) != 0 {
            let mut next_underline = false;
            let mut ch = at(i);
            if hotkey {
                if underline {
                    ch = '_' as u32;
                }
                if at(i) == '&' as u32 {
                    let n = at(i + 1);
                    if n == 0 {
                        break;
                    }
                    if n != '&' as u32 {
                        ch = n;
                        next_underline = true;
                    }
                }
            }
            let g = font.glyph(ch);
            if g.tex != 0 {
                let w0 = if underline { g.w.min(uwidth) } else { g.w };
                let (mut x, mut y, mut w, mut h, mut u, mut v) = (draw_x.wrapping_add(line_x), draw_y, w0, g.h, g.start_u, g.start_v);
                let mut draw = true;
                if clip {
                    draw = x.wrapping_add(w) > 0 && x as f32 <= clip_x && h.wrapping_add(y) > 0 && y as f32 <= clip_y;
                    if draw {
                        if x < 0 {
                            u -= x;
                            w += x;
                            x = 0;
                        }
                        if y < 0 {
                            v -= y;
                            h += y;
                            y = 0;
                        }
                        if clip_x < (x + w) as f32 {
                            w = ftol(clip_x - x as f32);
                        }
                        if clip_y < (h + y) as f32 {
                            h = ftol(clip_y - y as f32);
                        }
                    }
                }
                if draw {
                    self.canvas_glyph(flags, g.tex, ftol(x as f32 + org_x), ftol(y as f32 + org_y), w, h, u, v, w, h, color);
                }
                let adv = if underline { uwidth } else { w0 };
                if next_underline {
                    uwidth = g.w;
                } else {
                    line_x = line_x.wrapping_add(ftol(adv as f32 + space_x));
                }
                underline = next_underline;
            }
            i += 1;
        }
        line_x
    }

    /// `UCanvas::WrappedPrint` (103010af): `text` broken into lines at spaces to fit `ClipX` from `CurX`, each drawn
    /// (unless `STY_None`, or off the frame vertically) at `CurX` (or centred in `ClipX`) and `CurY`, which moves down
    /// a line each time (`CurX` back to 0). A newline ends the text. Returns the widest line and the total height.
    fn canvas_wrapped_print(&mut self, c: u32, style: u8, font: u32, center: bool, text: &[u32]) -> (i32, i32) {
        let (clip_x, clip_y) = (self.fp(c, "ClipX"), self.fp(c, "ClipY"));
        let Some(fd) = self.font_data(font) else { return (0, 0) };
        if !(clip_x >= 0.0 && clip_y >= 0.0) {
            return (0, 0);
        }
        let color = self.canvas_color(c);
        let flags = style_flags(style) | 0x1000802;
        let (space_x, space_y) = (self.fp(c, "SpaceX"), self.fp(c, "SpaceY"));
        let at = |i: usize| text.get(i).copied().unwrap_or(0);
        let (mut xl, mut yl) = (0i32, 0i32);
        let mut pos = 0usize;
        loop {
            let mut test_xl = ftol(self.fp(c, "CurX"));
            let (mut clean_xl, mut test_yl, mut clean_yl, mut clean_end, mut got_word) = (0i32, 0i32, 0i32, 0usize, false);
            let mut i = 0usize;
            loop {
                let ch = at(pos + i);
                if ch == 0 || ch == 10 {
                    break;
                }
                let g = fd.glyph(ch);
                test_xl = test_xl.wrapping_add(ftol(g.w as f32 + space_x));
                test_yl = test_yl.max(ftol(space_y).wrapping_add(g.h));
                if test_xl as f32 > clip_x {
                    break;
                }
                i += 1;
                let n = at(pos + i);
                let word_break = n == ' ' as u32 || n == 10 || n == 0;
                if word_break || !got_word {
                    clean_end = i;
                    clean_xl = test_xl;
                    clean_yl = test_yl;
                    got_word = got_word || word_break;
                }
            }
            if clean_end == 0 {
                return (xl, yl);
            }
            let top = self.fp(c, "OrgY") + self.fp(c, "CurY");
            if style != 0 && top < self.canvas_frame.y as f32 && clean_yl as f32 + top > 0.0 {
                let cur_x = self.fp(c, "CurX");
                let x = ftol(if center { (clip_x - clean_xl as f32) * 0.5 + cur_x } else { cur_x });
                let y = ftol(self.fp(c, "CurY"));
                let line = text[pos..pos + clean_end].to_vec();
                let w = self.canvas_draw_string(c, flags, &fd, x, y, &line, color, false, false);
                self.set_fp(c, "CurX", x.wrapping_add(w) as f32);
            }
            self.set_fp(c, "CurX", 0.0);
            let cy = self.fp(c, "CurY");
            self.set_fp(c, "CurY", clean_yl as f32 + cy);
            yl = yl.wrapping_add(clean_yl);
            xl = xl.max(clean_xl);
            pos += clean_end;
            while at(pos) == ' ' as u32 {
                pos += 1;
            }
            if at(pos) == 0 {
                return (xl, yl);
            }
        }
    }

    /// The canvas natives; `None` when `name` is not one of them or `this` is not a canvas.
    pub(crate) fn native_canvas(&mut self, this: u32, name: &str, a: &mut [Value]) -> Option<Res<Value>> {
        if !matches!(name, "DrawTile" | "DrawTileClipped" | "DrawText" | "DrawTextClipped" | "StrLen" | "TextSize") || !self.is_class(this, "Canvas") {
            return None;
        }
        let c = this;
        let fl = |a: &[Value], i: usize| a.get(i).and_then(Value::as_f32).unwrap_or(0.0);
        match name {
            // `execDrawTile` (10302b8a): at the pen plus the origin, unless `STY_None`; the pen moves right by the width
            // plus `SpaceX`, `CurYL` keeps the tallest
            "DrawTile" | "DrawTileClipped" => {
                let tex = match a.first() {
                    Some(Value::Obj(t)) => *t,
                    _ => 0,
                };
                if tex == 0 {
                    return Some(Ok(Value::Void));
                }
                let (mut xl, mut yl, mut u, mut v, mut ul, mut vl) = (fl(a, 1), fl(a, 2), fl(a, 3), fl(a, 4), fl(a, 5), fl(a, 6));
                if name == "DrawTileClipped" {
                    // `execDrawTileClipped` (10301ab9): clipped to the pen's clip rectangle first (moving a pen that
                    // is above or left of it onto it); nothing at all for an empty size
                    if !(xl > 0.0 && yl > 0.0) {
                        return Some(Ok(Value::Void));
                    }
                    let (cx, cy) = (self.fp(c, "CurX"), self.fp(c, "CurY"));
                    if cx < 0.0 {
                        let d = (ul * cx) / xl;
                        u -= d;
                        ul += d;
                        xl += cx;
                        self.set_fp(c, "CurX", 0.0);
                    }
                    if cy < 0.0 {
                        let d = (vl * cy) / yl;
                        v -= d;
                        vl += d;
                        yl += cy;
                        self.set_fp(c, "CurY", 0.0);
                    }
                    let r = self.fp(c, "ClipX") - self.fp(c, "CurX");
                    if r < xl {
                        ul += ((r - xl) * ul) / xl;
                        xl = r;
                    }
                    let b = self.fp(c, "ClipY") - self.fp(c, "CurY");
                    if b < yl {
                        vl += ((b - yl) * vl) / yl;
                        yl = b;
                    }
                }
                let style = self.canvas_style(c);
                if style != 0 {
                    let flags = 0x100 | style_flags(style) & !1 | if self.bp(c, "bNoSmooth") { 0x800 } else { 0 };
                    let (x, y) = (self.fp(c, "CurX") + self.fp(c, "OrgX"), self.fp(c, "CurY") + self.fp(c, "OrgY"));
                    let color = self.canvas_color(c);
                    self.canvas_tile(tex, x, y, xl, yl, u, v, ul, vl, color, flags);
                }
                let cx = self.fp(c, "CurX");
                let sx = self.fp(c, "SpaceX");
                self.set_fp(c, "CurX", sx + cx + xl);
                let cyl = self.fp(c, "CurYL");
                self.set_fp(c, "CurYL", if cyl < yl { yl } else { cyl });
            }
            // `execDrawText` (10301e33): wrapped at the pen unless `STY_None`; the pen moves right by the widest line
            // (after `WrappedPrint` moved it down), `CR` starts a new line below the tallest
            "DrawText" => {
                let font = self.canvas_obj(c, "Font");
                if font == 0 {
                    return Some(Ok(Value::Void));
                }
                let text = chars(a.first());
                let cr = a.get(1).and_then(Value::as_bool).unwrap_or(false);
                let style = self.canvas_style(c);
                let (xl, yl) = if style != 0 {
                    let center = self.bp(c, "bCenter");
                    self.canvas_wrapped_print(c, style, font, center, &text)
                } else {
                    (0, 0)
                };
                let cx = self.fp(c, "CurX");
                self.set_fp(c, "CurX", xl as f32 + cx);
                let cyl = self.fp(c, "CurYL");
                let tall = if yl as f32 <= cyl { cyl } else { yl as f32 };
                self.set_fp(c, "CurYL", tall);
                if cr {
                    self.set_fp(c, "CurX", 0.0);
                    self.set_fp(c, "CurYL", 0.0);
                    let cy = self.fp(c, "CurY");
                    self.set_fp(c, "CurY", tall + cy);
                }
            }
            // `execDrawTextClipped` (103022fc): one line at the pen, clipped to `ClipX`/`ClipY`, the pen unmoved.
            // (Outside the English version the big and large fonts become the medium one; the port runs `INT`.)
            "DrawTextClipped" => {
                let font = self.canvas_obj(c, "Font");
                if font == 0 {
                    return Some(Ok(Value::Void));
                }
                let Some(fd) = self.font_data(font) else { return Some(Ok(Value::Void)) };
                let text = chars(a.first());
                let hotkey = a.get(1).and_then(Value::as_bool).unwrap_or(false);
                let flags = style_flags(self.canvas_style(c)) | 0x1000802;
                let color = self.canvas_color(c);
                let (x, y) = (ftol(self.fp(c, "CurX")), ftol(self.fp(c, "CurY")));
                self.canvas_draw_string(c, flags, &fd, x, y, &text, color, true, hotkey);
            }
            // `execStrLen` (10301f64): the size `DrawText` would give from a pen at 0, 0 (`WrappedStrLenf`, nothing
            // drawn); the pen comes back truncated to whole pixels
            "StrLen" => {
                let text = chars(a.first());
                let (sx, sy) = (ftol(self.fp(c, "CurX")), ftol(self.fp(c, "CurY")));
                self.set_fp(c, "CurX", 0.0);
                self.set_fp(c, "CurY", 0.0);
                let font = self.canvas_obj(c, "Font");
                let (xl, yl) = self.canvas_wrapped_print(c, 0, font, false, &text);
                self.set_fp(c, "CurY", sy as f32);
                self.set_fp(c, "CurX", sx as f32);
                if a.len() > 1 {
                    a[1] = Value::Float(xl as f32);
                }
                if a.len() > 2 {
                    a[2] = Value::Float(yl as f32);
                }
            }
            // `execTextSize` (10301b22): the glyph widths added up (no spacing, no wrapping) and the tallest glyph
            "TextSize" => {
                let font = self.canvas_obj(c, "Font");
                let Some(fd) = self.font_data(font) else { return Some(Ok(Value::Void)) };
                let (mut w, mut h) = (0i32, 0i32);
                for ch in chars(a.first()) {
                    let g = fd.glyph(ch);
                    w = w.wrapping_add(g.w);
                    h = h.max(g.h);
                }
                if a.len() > 1 {
                    a[1] = Value::Float(w as f32);
                }
                if a.len() > 2 {
                    a[2] = Value::Float(h as f32);
                }
            }
            _ => return None,
        }
        Some(Ok(Value::Void))
    }
}

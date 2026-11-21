//! `ScaleWindow`: a slider / scroll bar. A scale texture, a thumb (optionally spanning a range of ticks, as the scroll
//! bars of a scroll area do) and tick marks; the thumb position is a tick (`currentPos`) mapped to a value range.
//! Ported from the `XScaleWindow` functions of Extension.dll (the arithmetic hidden behind `ftol()` calls was read from
//! the disassembly). Not done: the scale sounds (windows play no sounds yet).

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

/// The C runtime's `ftol`: truncation toward zero.
fn ftol(v: f32) -> f32 {
    v.trunc()
}


impl Vm {
    pub(crate) fn scale_init(&mut self, w: u32) {
        let _ = self.set_prop(w, "orientation", 0, Value::Byte(0));
        for n in ["scaleTexture", "thumbTexture", "tickTexture", "preCapTexture", "postCapTexture", "borderPattern", "setSound", "clickSound", "dragSound"] {
            self.set_o_pub(w, n, 0);
        }
        for (n, v) in [("bRepeatScaleTexture", false), ("bRepeatThumbTexture", false), ("bDrawEndTicks", true), ("bStretchScale", false), ("bSpanThumb", false), ("bDraggingThumb", false)] {
            self.set_b_pub(w, n, v);
        }
        let white = self.color_value_pub([255, 255, 255, 255]);
        for n in ["scaleBorderColor", "thumbBorderColor", "scaleColor", "thumbColor", "tickColor"] {
            let _ = self.set_prop(w, n, 0, white.clone());
        }
        for n in ["scaleStyle", "thumbStyle", "tickStyle"] {
            let _ = self.set_prop(w, n, 0, Value::Byte(2));
        }
        for n in [
            "scaleBorderSize", "thumbBorderSize", "scaleWidth", "scaleHeight", "thumbWidth", "thumbHeight", "tickWidth", "tickHeight", "preCapWidth", "preCapHeight", "postCapWidth", "postCapHeight", "startOffset",
            "endOffset", "marginWidth", "marginHeight", "fromValue", "scaleX", "scaleY", "scaleW", "scaleH", "thumbX", "thumbY", "thumbW", "thumbH", "tickX", "tickY", "tickW", "tickH", "preCapXOff", "preCapYOff",
            "preCapW", "preCapH", "postCapXOff", "postCapYOff", "postCapW", "postCapH", "absStartScale", "absEndScale", "mousePos", "remainingTime",
        ] {
            self.set_f_pub(w, n, 0.0);
        }
        self.set_f_pub(w, "toValue", 1.0);
        self.set_f_pub(w, "initialDelay", 0.5);
        self.set_f_pub(w, "repeatRate", 0.1);
        for (n, v) in [("numPositions", 10), ("currentPos", 0), ("spanRange", 1), ("thumbStep", 1), ("initialPos", -1)] {
            self.set_i_pub(w, n, v);
        }
        let _ = self.set_prop(w, "valueFmt", 0, Value::Str("%1.2f".into()));
        let _ = self.set_prop(w, "repeatDir", 0, Value::Byte(0));
        self.ui.scale_enums.remove(&w);
    }

    fn sf(&self, w: u32, n: &'static str) -> f32 {
        self.wf_pub(w, n)
    }

    /// Width and height the texture takes, plus the border on both sides (`ComputeTextureSize`).
    fn scale_texture_size(&mut self, tex: u32, w: f32, h: f32, border: f32) -> (f32, f32) {
        let tsize = if tex != 0 && (w <= 0.0 || h <= 0.0) { self.texture_size(tex) } else { (0.0, 0.0) };
        let ow = if w > 0.0 { w } else { tsize.0 };
        let oh = if h > 0.0 { h } else { tsize.1 };
        (ow + border * 2.0, oh + border * 2.0)
    }

    /// Pixels the thumb can travel along the scale, and the number of tick steps for that travel (clamped for pixels).
    fn scale_travel(&self, w: u32) -> (f32, i32) {
        let span = self.wb_pub(w, "bSpanThumb");
        let vertical = self.wi_pub(w, "orientation") == 1;
        let num = self.wi_pub(w, "numPositions");
        let mut range = self.sf(w, "absEndScale") - self.sf(w, "absStartScale");
        let steps;
        if span {
            steps = num - self.wi_pub(w, "spanRange");
            range = range + 1.0 - self.sf(w, if vertical { "thumbH" } else { "thumbW" });
        } else {
            steps = num - 1;
        }
        (range, steps)
    }

    /// `TickToPixel`: where the start of the thumb is for tick `tick`.
    fn scale_tick_to_pixel(&self, w: u32, tick: i32) -> f32 {
        let (range, steps) = self.scale_travel(w);
        ftol(tick as f32 * range / steps.max(1) as f32 + self.sf(w, "absStartScale") + 0.5)
    }

    /// `PixelToTick`.
    fn scale_pixel_to_tick(&self, w: u32, pixel: f32) -> i32 {
        let (range, steps) = self.scale_travel(w);
        let range = range.max(1.0);
        ftol((pixel - self.sf(w, "absStartScale")) * steps as f32 / range + 0.5) as i32
    }

    fn scale_tick_to_value(&self, w: u32, tick: i32) -> f32 {
        let denom = if self.wb_pub(w, "bSpanThumb") { self.wi_pub(w, "numPositions") } else { self.wi_pub(w, "numPositions") - 1 } as f32;
        let (from, to) = (self.sf(w, "fromValue"), self.sf(w, "toValue"));
        if denom > 0.0 {
            (to - from) * tick as f32 / denom + from
        } else {
            (to - from) * 0.5 + from
        }
    }

    fn scale_value_to_tick(&self, w: u32, v: f32) -> i32 {
        let denom = if self.wb_pub(w, "bSpanThumb") { self.wi_pub(w, "numPositions") } else { self.wi_pub(w, "numPositions") - 1 } as f32;
        let (from, to) = (self.sf(w, "fromValue"), self.sf(w, "toValue"));
        let t = if to != from { (v - from) * denom / (to - from) } else { 0.0 };
        (if t < 0.0 { ftol(t - 0.5) } else { ftol(t + 0.5) }) as i32
    }

    /// `ComputeThumbConfig`: the size of the thumb with its caps and its place for the current tick.
    fn scale_compute_thumb(&mut self, w: u32) {
        let vertical = self.wi_pub(w, "orientation") == 1;
        let tex = self.wo_ref_pub(w, "thumbTexture");
        let border = self.sf(w, "thumbBorderSize");
        let (mut tw, mut th) = self.scale_texture_size(tex, self.sf(w, "thumbWidth"), self.sf(w, "thumbHeight"), border);
        let (pre, post) = (self.wo_ref_pub(w, "preCapTexture"), self.wo_ref_pub(w, "postCapTexture"));
        let (mut pw, mut ph) = self.scale_texture_size(pre, self.sf(w, "preCapWidth"), self.sf(w, "preCapHeight"), 0.0);
        let (mut qw, mut qh) = self.scale_texture_size(post, self.sf(w, "postCapWidth"), self.sf(w, "postCapHeight"), 0.0);
        let (mut pxo, mut pyo, mut qxo, mut qyo) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        if vertical {
            th += ph + qh;
            pw = tw - border * 2.0;
            qw = pw;
        } else {
            tw += pw + qw;
            ph = th - border * 2.0;
            qh = ph;
        }
        let (width, height) = (self.sf(w, "width"), self.sf(w, "height"));
        let mut tx = ftol((width - tw) * 0.5);
        let mut ty = ftol((height - th) * 0.5);
        let pos = self.wi_pub(w, "currentPos");
        if self.wb_pub(w, "bSpanThumb") {
            let (start, end) = (self.sf(w, "absStartScale"), self.sf(w, "absEndScale"));
            let size = (end + 1.0 - start) * self.wi_pub(w, "spanRange") as f32 / self.wi_pub(w, "numPositions").max(1) as f32 + 0.5;
            let limit = end - start + 1.0;
            let (t, p, q, qo) = if vertical { (&mut th, &mut ph, &mut qh, &mut qyo) } else { (&mut tw, &mut pw, &mut qw, &mut qxo) };
            *t = ftol(size).max(6.0).min(limit);
            let excess = size + *p + *q - *t;
            if excess > 0.0 {
                let half = ftol(excess * 0.5);
                *p -= excess - half;
                *q -= half;
                *qo += half;
            }
            let px = self.scale_tick_to_pixel_with(w, pos, vertical, tw, th);
            if vertical {
                ty = px;
            } else {
                tx = px;
            }
        } else {
            let px = self.scale_tick_to_pixel(w, pos);
            if vertical {
                ty = ftol(px - th * 0.5 + 0.5);
            } else {
                tx = ftol(px - tw * 0.5 + 0.5);
            }
        }
        let _ = (&mut pxo, &mut pyo);
        for (n, v) in [
            ("thumbW", tw), ("thumbH", th), ("thumbX", tx), ("thumbY", ty), ("preCapW", pw), ("preCapH", ph), ("postCapW", qw), ("postCapH", qh), ("preCapXOff", pxo), ("preCapYOff", pyo), ("postCapXOff", qxo),
            ("postCapYOff", qyo),
        ] {
            self.set_f_pub(w, n, v);
        }
    }

    /// `TickToPixel` while the thumb size is still being computed (the size is not stored yet).
    fn scale_tick_to_pixel_with(&mut self, w: u32, tick: i32, vertical: bool, tw: f32, th: f32) -> f32 {
        self.set_f_pub(w, if vertical { "thumbH" } else { "thumbW" }, if vertical { th } else { tw });
        self.scale_tick_to_pixel(w, tick)
    }

    /// Delivers `event` to the scale and then to its ancestors until one handles it.
    fn scale_notify(&mut self, w: u32, event: &str, args: Vec<Value>) {
        let mut cur = w;
        while cur != 0 {
            let (r, _) = self.ui_event(cur, event, args.clone());
            if r.as_bool().unwrap_or(false) {
                break;
            }
            cur = self.wo_pub(cur, "winParent");
        }
    }

    /// `ChangeScalePosition`.
    fn scale_changed_position(&mut self, w: u32, final_: bool) {
        let pos = self.wi_pub(w, "currentPos");
        if !self.wb_pub(w, "bSpanThumb") {
            let v = self.scale_tick_to_value(w, pos);
            self.scale_notify(w, "ScalePositionChanged", vec![Value::Obj(w), Value::Int(pos), Value::Float(v), Value::Bool(final_)]);
        } else {
            let to = pos + self.wi_pub(w, "spanRange");
            let (vf, vt) = (self.scale_tick_to_value(w, pos), self.scale_tick_to_value(w, to));
            self.scale_notify(w, "ScaleRangeChanged", vec![Value::Obj(w), Value::Int(pos), Value::Int(to), Value::Float(vf), Value::Float(vt), Value::Bool(final_)]);
        }
    }

    /// `ChangeScaleAttributes`: only reported while the scale and all its ancestors are visible.
    fn scale_changed_attributes(&mut self, w: u32) {
        if self.ui_visible_recursive(w) {
            let args = vec![Value::Obj(w), Value::Int(self.wi_pub(w, "currentPos")), Value::Int(self.wi_pub(w, "spanRange")), Value::Int(self.wi_pub(w, "numPositions"))];
            self.scale_notify(w, "ScaleAttributesChanged", args);
        }
    }

    /// `ChangeThumbPosition`.
    fn scale_change_thumb(&mut self, w: u32, pos: i32, attrs: bool, final_: bool) {
        let num = self.wi_pub(w, "numPositions");
        let mut pos = if self.wb_pub(w, "bSpanThumb") { pos.min(num - self.wi_pub(w, "spanRange")) } else { pos.min(num - 1) };
        pos = pos.max(0);
        let mut attrs = attrs;
        if self.wi_pub(w, "currentPos") != pos {
            attrs = true;
            self.set_i_pub(w, "currentPos", pos);
            self.scale_compute_thumb(w);
            self.scale_changed_position(w, final_);
        }
        if attrs {
            self.scale_changed_attributes(w);
            self.scale_compute_thumb(w);
        }
    }

    /// `GetValueString`: the name of the tick when it has one, else the value in the value format.
    pub(crate) fn scale_value_string(&self, w: u32) -> String {
        let pos = self.wi_pub(w, "currentPos").max(0) as usize;
        if let Some(s) = self.ui.scale_enums.get(&w).and_then(|v| v.get(pos)) {
            if !s.is_empty() {
                return s.clone();
            }
        }
        let fmt = match self.get_prop(w, "valueFmt", 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        };
        // `appSprintf(valueFmt, value)`: the float goes as a double (`crt::sprintf_double`, probe `scale.format`)
        crate::crt::sprintf_double(&fmt, self.scale_tick_to_value(w, self.wi_pub(w, "currentPos")) as f64)
    }

    fn scale_move_thumb(&mut self, w: u32, how: i32) {
        let cur = self.wi_pub(w, "currentPos");
        let page = if self.wb_pub(w, "bSpanThumb") { self.wi_pub(w, "spanRange") } else { 4 };
        let step = self.wi_pub(w, "thumbStep");
        let target = match how {
            0 => 0,
            1 => self.wi_pub(w, "numPositions"),
            2 => cur - 1,
            3 => cur + 1,
            4 => cur - step,
            5 => cur + step,
            6 => cur - page,
            7 => cur + page,
            _ => 0,
        };
        self.scale_change_thumb(w, target, false, true);
    }

    fn scale_cursor(&self, w: u32) -> (f32, f32) {
        let root = self.ui_root_of_pub(w);
        if root == 0 {
            return (0.0, 0.0);
        }
        let g = self.wo_ref_pub(w, "wGc");
        let o = self.gc_state(g).clip;
        (self.wf_pub(root, "mouseX") - o[0], self.wf_pub(root, "mouseY") - o[1])
    }

    pub(crate) fn scale_event(&mut self, w: u32, name: &str, args: &mut Vec<Value>) -> Option<Value> {
        let vertical = self.wi_pub(w, "orientation") == 1;
        match name {
            "ParentRequestedPreferredSize" => {
                let (mut rw, mut rh);
                let (t, tw, th, b) = (self.wo_ref_pub(w, "scaleTexture"), self.sf(w, "scaleWidth"), self.sf(w, "scaleHeight"), self.sf(w, "scaleBorderSize"));
                (rw, rh) = self.scale_texture_size(t, tw, th, b);
                let (t, tw, th, b) = (self.wo_ref_pub(w, "thumbTexture"), self.sf(w, "thumbWidth"), self.sf(w, "thumbHeight"), self.sf(w, "thumbBorderSize"));
                let (a, c) = self.scale_texture_size(t, tw, th, b);
                rw = rw.max(a);
                rh = rh.max(c);
                let (t, tw, th) = (self.wo_ref_pub(w, "tickTexture"), self.sf(w, "tickWidth"), self.sf(w, "tickHeight"));
                let (a, c) = self.scale_texture_size(t, tw, th, 0.0);
                rw = rw.max(a) + self.sf(w, "marginWidth") * 2.0;
                rh = rh.max(c) + self.sf(w, "marginHeight") * 2.0;
                args[1] = Value::Float(rw);
                args[3] = Value::Float(rh);
                Some(Value::Void)
            }
            "ConfigurationChanged" => {
                let (t, tw, th, b) = (self.wo_ref_pub(w, "scaleTexture"), self.sf(w, "scaleWidth"), self.sf(w, "scaleHeight"), self.sf(w, "scaleBorderSize"));
                let (mut sw, mut sh) = self.scale_texture_size(t, tw, th, b);
                let (width, height) = (self.sf(w, "width"), self.sf(w, "height"));
                if self.wb_pub(w, "bStretchScale") {
                    if vertical {
                        sh = height - self.sf(w, "marginHeight") * 2.0;
                    } else {
                        sw = width - self.sf(w, "marginWidth") * 2.0;
                    }
                }
                let sx = ftol((width - sw) * 0.5);
                let sy = ftol((height - sh) * 0.5);
                let (mut start, mut end) = if vertical { (sy + self.sf(w, "startOffset"), sy + sh) } else { (sx + self.sf(w, "startOffset"), sx + sw) };
                end -= self.sf(w, "endOffset") + 1.0;
                start += b;
                end -= b;
                if end < start {
                    let mid = start + (end - start) * 0.5;
                    start = mid;
                    end = mid;
                }
                for (n, v) in [("scaleW", sw), ("scaleH", sh), ("scaleX", sx), ("scaleY", sy), ("absStartScale", start), ("absEndScale", end)] {
                    self.set_f_pub(w, n, v);
                }
                self.scale_compute_thumb(w);
                let (t, tw, th) = (self.wo_ref_pub(w, "tickTexture"), self.sf(w, "tickWidth"), self.sf(w, "tickHeight"));
                let (kw, kh) = self.scale_texture_size(t, tw, th, 0.0);
                for (n, v) in [("tickW", kw), ("tickH", kh), ("tickX", ftol((width - kw) * 0.5)), ("tickY", ftol((height - kh) * 0.5))] {
                    self.set_f_pub(w, n, v);
                }
                Some(Value::Void)
            }
            "MouseButtonPressed" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                if iv(args, 2) != 1 {
                    return Some(r);
                }
                let (x, y) = (fl(args, 0, 0.0), fl(args, 1, 0.0));
                let (tx, ty, tw, th) = (self.sf(w, "thumbX"), self.sf(w, "thumbY"), self.sf(w, "thumbW"), self.sf(w, "thumbH"));
                let on_thumb = x >= tx && x < tx + tw && y >= ty && y < ty + th;
                let cur = self.wi_pub(w, "currentPos");
                if on_thumb {
                    self.set_b_pub(w, "bDraggingThumb", true);
                    self.set_i_pub(w, "initialPos", cur);
                    self.set_f_pub(w, "mousePos", if vertical { y - ty } else { x - tx });
                    let s = self.wo_pub(w, "clickSound");
                    self.scale_play_sound(w, s, -1.0, 1.0);
                } else if !self.wb_pub(w, "bSpanThumb") {
                    self.set_i_pub(w, "initialPos", cur);
                    self.set_b_pub(w, "bDraggingThumb", true);
                    let tick = self.scale_pixel_to_tick(w, if vertical { y } else { x });
                    self.scale_change_thumb(w, tick, false, false);
                    let s = self.wo_pub(w, "setSound");
                    self.scale_play_sound(w, s, -1.0, 1.0);
                } else {
                    let before = if vertical { y < ty } else { x < tx };
                    let _ = self.set_prop(w, "repeatDir", 0, Value::Byte(if before { 2 } else { 1 }));
                    self.scale_move_thumb(w, if before { 6 } else { 7 });
                    let s = self.wo_pub(w, "setSound");
                    self.scale_play_sound(w, s, -1.0, 1.0);
                    let mut t = self.wf_pub(w, "initialDelay");
                    if t <= 0.0 {
                        t = self.wf_pub(w, "repeatRate");
                    }
                    self.set_f_pub(w, "remainingTime", t);
                    if t <= 0.0 {
                        let _ = self.set_prop(w, "repeatDir", 0, Value::Byte(0));
                    }
                }
                Some(Value::Bool(true))
            }
            "MouseMoved" => {
                self.ui_script_event(w, name, args.clone());
                if self.wb_pub(w, "bDraggingThumb") {
                    let (x, y) = (fl(args, 0, 0.0), fl(args, 1, 0.0));
                    let mut p = if vertical { y } else { x };
                    if self.wb_pub(w, "bSpanThumb") {
                        p -= self.sf(w, "mousePos");
                    }
                    let before = self.wi_pub(w, "currentPos");
                    let tick = self.scale_pixel_to_tick(w, p);
                    self.scale_change_thumb(w, tick, false, false);
                    if self.wi_pub(w, "currentPos") != before {
                        let s = self.wo_pub(w, "dragSound");
                        self.scale_play_sound(w, s, -1.0, 1.0);
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
                if self.wb_pub(w, "bDraggingThumb") && self.wi_pub(w, "currentPos") != self.wi_pub(w, "initialPos") {
                    self.scale_changed_position(w, true);
                    let s = self.wo_pub(w, "setSound");
                    self.scale_play_sound(w, s, -1.0, 1.0);
                }
                let _ = self.set_prop(w, "repeatDir", 0, Value::Byte(0));
                self.set_b_pub(w, "bDraggingThumb", false);
                Some(Value::Bool(true))
            }
            "Tick" => {
                let dir = self.wi_pub(w, "repeatDir");
                if dir == 0 {
                    return Some(Value::Void);
                }
                let left = self.sf(w, "remainingTime") - fl(args, 0, 0.0);
                self.set_f_pub(w, "remainingTime", left);
                if left >= 0.0 {
                    return Some(Value::Void);
                }
                let (x, y) = self.scale_cursor(w);
                if x >= 0.0 && x < self.sf(w, "width") && y >= 0.0 && y < self.sf(w, "height") {
                    let (tx, ty, tw, th) = (self.sf(w, "thumbX"), self.sf(w, "thumbY"), self.sf(w, "thumbW"), self.sf(w, "thumbH"));
                    // the original compares with the thumb's height on both axes
                    let (p, start, size) = if vertical { (y, ty, th) } else { (x, tx, th) };
                    let _ = tw;
                    let before = self.wi_pub(w, "currentPos");
                    if p < start && dir == 2 {
                        self.scale_move_thumb(w, 6);
                    }
                    if p >= start + size && dir == 1 {
                        self.scale_move_thumb(w, 7);
                    }
                    if self.wi_pub(w, "currentPos") != before {
                        let s = self.wo_pub(w, "setSound");
                        self.scale_play_sound(w, s, -1.0, 1.0);
                    }
                }
                let mut next = self.sf(w, "repeatRate") + self.sf(w, "remainingTime");
                if next < 0.0 {
                    next = 0.05;
                }
                self.set_f_pub(w, "remainingTime", next);
                Some(Value::Void)
            }
            _ => None,
        }
    }

    /// `XScaleWindow::PlayScaleSound` (Extension.dll 1003e9c0): a window sound at the thumb's centre.
    pub(crate) fn scale_play_sound(&mut self, w: u32, sound: u32, volume: f32, pitch: f32) {
        let x = self.sf(w, "thumbW") * 0.5 + self.sf(w, "thumbX");
        let y = self.sf(w, "thumbH") * 0.5 + self.sf(w, "thumbY");
        self.ui_play_sound_at(w, sound, volume, pitch, x, y);
    }

    /// `DrawScaleTexture`: border, caps and a (repeated or stretched) texture inside a rectangle.
    #[allow(clippy::too_many_arguments)]
    fn scale_draw_texture(&mut self, w: u32, g: u32, tex: u32, rect: [f32; 4], repeat: bool, style: u8, color: [u8; 4], border: f32, border_color: [u8; 4], pre: u32, post: u32, pre_geo: [f32; 4], post_geo: [f32; 4]) {
        let vertical = self.wi_pub(w, "orientation") == 1;
        let mut st = self.gc_state(g);
        st.set_style(style);
        let [mut x, mut y, mut cw, mut ch] = rect;
        if border > 0.0 {
            let pat = self.wo_ref_pub(w, "borderPattern");
            if pat != 0 {
                st.tile_color = border_color;
                self.gc_draw_box(&st, x, y, cw, ch, 0.0, 0.0, border, pat);
            }
            x += border;
            y += border;
            cw -= border * 2.0;
            ch -= border * 2.0;
        }
        st.tile_color = color;
        let (pxo, pyo, pw, ph) = (pre_geo[0], pre_geo[1], pre_geo[2], pre_geo[3]);
        let (qxo, qyo, qw, qh) = (post_geo[0], post_geo[1], post_geo[2], post_geo[3]);
        if pre != 0 && pw > 0.0 && ph > 0.0 && cw > 0.0 && ch > 0.0 {
            if vertical {
                self.gc_draw_icon_pattern(&st, x, y, pw, ph, pxo, pyo, 0.0, ph, pre);
                y += ph;
                ch -= ph;
            } else {
                self.gc_draw_icon_pattern(&st, x, y, pw, ph, pxo, pyo, pw, 0.0, pre);
                x += pw;
                cw -= pw;
            }
        }
        let y0 = y;
        if post != 0 && qw > 0.0 && qh > 0.0 && cw > 0.0 && ch > 0.0 {
            if vertical {
                self.gc_draw_icon_pattern(&st, x, y + ch - qh, qw, qh, qxo, qyo, 0.0, qh, post);
                ch -= qh;
            } else {
                self.gc_draw_icon_pattern(&st, x + cw - qw, y, qw, qh, qxo, qyo, qw, 0.0, post);
                cw -= qw;
            }
        }
        if tex != 0 && cw > 0.0 && ch > 0.0 {
            let (ul, vl) = if vertical { (0.0, if repeat { 0.0 } else { ch }) } else { (if repeat { 0.0 } else { cw }, 0.0) };
            self.gc_draw_icon_pattern(&st, x, y0, cw, ch, 0.0, 0.0, ul, vl, tex);
        }
    }

    pub(crate) fn scale_draw(&mut self, w: u32, g: u32) {
        let vertical = self.wi_pub(w, "orientation") == 1;
        let obj = |vm: &Vm, n: &'static str| vm.wo_ref_pub(w, n);
        let col = |vm: &Vm, n: &str| vm.color_bytes(w, n);
        let st = |vm: &Vm, n: &'static str| vm.wi_pub(w, n) as u8;
        let scale_rect = [self.sf(w, "scaleX"), self.sf(w, "scaleY"), self.sf(w, "scaleW"), self.sf(w, "scaleH")];
        let (t, rep, sty, c, b, bc) = (obj(self, "scaleTexture"), self.wb_pub(w, "bRepeatScaleTexture"), st(self, "scaleStyle"), col(self, "scaleColor"), self.sf(w, "scaleBorderSize"), col(self, "scaleBorderColor"));
        self.scale_draw_texture(w, g, t, scale_rect, rep, sty, c, b, bc, 0, 0, [0.0; 4], [0.0; 4]);
        let tick = obj(self, "tickTexture");
        if tick != 0 {
            let mut count = self.wi_pub(w, "numPositions");
            if self.wb_pub(w, "bSpanThumb") {
                count += 1;
            }
            let mut first = 0;
            if !self.wb_pub(w, "bDrawEndTicks") {
                count -= 1;
                first = 1;
            }
            let (tw, th, tx, ty) = (self.sf(w, "tickW"), self.sf(w, "tickH"), self.sf(w, "tickX"), self.sf(w, "tickY"));
            let (sty, c) = (st(self, "tickStyle"), col(self, "tickColor"));
            for i in first..count {
                let p = self.scale_tick_to_pixel(w, i);
                let rect = if vertical { [tx, p - ftol(th * 0.5), tw, th] } else { [p - ftol(tw * 0.5), ty, tw, th] };
                self.scale_draw_texture(w, g, tick, rect, false, sty, c, 0.0, [0, 0, 0, 0], 0, 0, [0.0; 4], [0.0; 4]);
            }
        }
        let rect = [self.sf(w, "thumbX"), self.sf(w, "thumbY"), self.sf(w, "thumbW"), self.sf(w, "thumbH")];
        let pre_geo = [self.sf(w, "preCapXOff"), self.sf(w, "preCapYOff"), self.sf(w, "preCapW"), self.sf(w, "preCapH")];
        let post_geo = [self.sf(w, "postCapXOff"), self.sf(w, "postCapYOff"), self.sf(w, "postCapW"), self.sf(w, "postCapH")];
        let (t, rep, sty, c, b, bc) = (obj(self, "thumbTexture"), self.wb_pub(w, "bRepeatThumbTexture"), st(self, "thumbStyle"), col(self, "thumbColor"), self.sf(w, "thumbBorderSize"), col(self, "thumbBorderColor"));
        let (pre, post) = (obj(self, "preCapTexture"), obj(self, "postCapTexture"));
        self.scale_draw_texture(w, g, t, rect, rep, sty, c, b, bc, pre, post, pre_geo, post_geo);
    }

    /// The script natives of `ScaleWindow`.
    pub(crate) fn native_scale(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        let nn = |v: f32| v.max(0.0);
        let mut reconfigure = true;
        let ret = match name {
            "SetScaleOrientation" => {
                let _ = self.set_prop(this, "orientation", 0, Value::Byte(iv(a, 0) as u8));
                Value::Void
            }
            "SetScaleTexture" => {
                self.set_o_pub(this, "scaleTexture", ob(a, 0));
                self.set_f_pub(this, "scaleWidth", nn(fl(a, 1, 0.0)));
                self.set_f_pub(this, "scaleHeight", nn(fl(a, 2, 0.0)));
                self.set_f_pub(this, "startOffset", fl(a, 3, 0.0));
                self.set_f_pub(this, "endOffset", fl(a, 4, 0.0));
                Value::Void
            }
            "SetThumbTexture" => {
                self.set_o_pub(this, "thumbTexture", ob(a, 0));
                self.set_f_pub(this, "thumbWidth", nn(fl(a, 1, 0.0)));
                self.set_f_pub(this, "thumbHeight", nn(fl(a, 2, 0.0)));
                Value::Void
            }
            "SetTickTexture" => {
                self.set_o_pub(this, "tickTexture", ob(a, 0));
                self.set_b_pub(this, "bDrawEndTicks", bl(a, 1, true));
                self.set_f_pub(this, "tickWidth", nn(fl(a, 2, 0.0)));
                self.set_f_pub(this, "tickHeight", nn(fl(a, 3, 0.0)));
                Value::Void
            }
            "SetThumbCaps" => {
                self.set_o_pub(this, "preCapTexture", ob(a, 0));
                self.set_o_pub(this, "postCapTexture", ob(a, 1));
                for (i, n) in ["preCapWidth", "preCapHeight", "postCapWidth", "postCapHeight"].into_iter().enumerate() {
                    self.set_f_pub(this, n, nn(fl(a, 2 + i, 0.0)));
                }
                Value::Void
            }
            "EnableStretchedScale" => {
                let on = bl(a, 0, true);
                if self.wb_pub(this, "bStretchScale") == on {
                    return Some(Ok(Value::Void));
                }
                self.set_b_pub(this, "bStretchScale", on);
                self.set_b_pub(this, "bRepeatScaleTexture", on);
                Value::Void
            }
            "SetBorderPattern" => {
                self.set_o_pub(this, "borderPattern", ob(a, 0));
                reconfigure = false;
                Value::Void
            }
            "SetScaleBorder" | "SetThumbBorder" => {
                let (size, color) = if name == "SetScaleBorder" { ("scaleBorderSize", "scaleBorderColor") } else { ("thumbBorderSize", "thumbBorderColor") };
                let c = color_arg(a, 1);
                let cv = self.color_value_pub(c);
                let _ = self.set_prop(this, color, 0, cv);
                let s = fl(a, 0, 0.0);
                if self.sf(this, size) == s {
                    return Some(Ok(Value::Void));
                }
                self.set_f_pub(this, size, s);
                Value::Void
            }
            "SetScaleStyle" | "SetThumbStyle" | "SetTickStyle" => {
                let n = match name {
                    "SetScaleStyle" => "scaleStyle",
                    "SetThumbStyle" => "thumbStyle",
                    _ => "tickStyle",
                };
                let _ = self.set_prop(this, n, 0, Value::Byte(iv(a, 0) as u8));
                reconfigure = false;
                Value::Void
            }
            "SetScaleColor" | "SetThumbColor" | "SetTickColor" => {
                let n = match name {
                    "SetScaleColor" => "scaleColor",
                    "SetThumbColor" => "thumbColor",
                    _ => "tickColor",
                };
                let cv = self.color_value_pub(color_arg(a, 0));
                let _ = self.set_prop(this, n, 0, cv);
                reconfigure = false;
                Value::Void
            }
            "SetScaleMargins" => {
                let (mw, mh) = (nn(fl(a, 0, 0.0)), nn(fl(a, 1, 0.0)));
                if self.sf(this, "marginWidth") == mw && self.sf(this, "marginHeight") == mh {
                    return Some(Ok(Value::Void));
                }
                self.set_f_pub(this, "marginWidth", mw);
                self.set_f_pub(this, "marginHeight", mh);
                Value::Void
            }
            "SetNumTicks" => {
                let n = iv(a, 0).max(1);
                if self.wi_pub(this, "numPositions") != n {
                    self.set_i_pub(this, "numPositions", n);
                    let cur = self.wi_pub(this, "currentPos");
                    self.scale_change_thumb(this, cur, true, true);
                }
                reconfigure = false;
                Value::Void
            }
            "GetNumTicks" => {
                reconfigure = false;
                Value::Int(self.wi_pub(this, "numPositions"))
            }
            "SetThumbSpan" => {
                let r = if matches!(a.first(), None | Some(Value::Void)) { 1 } else { iv(a, 0) };
                let on = r > 0;
                if self.wi_pub(this, "spanRange") != r || self.wb_pub(this, "bSpanThumb") != on {
                    self.set_b_pub(this, "bSpanThumb", on);
                    self.set_b_pub(this, "bRepeatThumbTexture", on);
                    if on {
                        self.set_i_pub(this, "spanRange", r);
                    }
                    let cur = self.wi_pub(this, "currentPos");
                    self.scale_change_thumb(this, cur, true, true);
                }
                reconfigure = false;
                Value::Void
            }
            "GetThumbSpan" => {
                reconfigure = false;
                Value::Int(self.wi_pub(this, "spanRange"))
            }
            "SetTickPosition" => {
                self.scale_change_thumb(this, iv(a, 0), false, true);
                reconfigure = false;
                Value::Void
            }
            "GetTickPosition" => {
                reconfigure = false;
                Value::Int(self.wi_pub(this, "currentPos"))
            }
            "SetValueRange" => {
                let (f, t) = (fl(a, 0, 0.0), fl(a, 1, 0.0));
                if self.sf(this, "fromValue") != f || self.sf(this, "toValue") != t {
                    self.set_f_pub(this, "fromValue", f);
                    self.set_f_pub(this, "toValue", t);
                    self.scale_changed_position(this, true);
                }
                reconfigure = false;
                Value::Void
            }
            "SetValue" => {
                let t = self.scale_value_to_tick(this, fl(a, 0, 0.0));
                self.scale_change_thumb(this, t, false, true);
                reconfigure = false;
                Value::Void
            }
            "GetValue" => {
                reconfigure = false;
                Value::Float(self.scale_tick_to_value(this, self.wi_pub(this, "currentPos")))
            }
            "GetValues" => {
                let cur = self.wi_pub(this, "currentPos");
                let v1 = self.scale_tick_to_value(this, cur);
                let v2 = if self.wb_pub(this, "bSpanThumb") { self.scale_tick_to_value(this, cur + self.wi_pub(this, "spanRange")) } else { self.sf(this, "fromValue") };
                a[0] = Value::Float(v1);
                a[1] = Value::Float(v2);
                reconfigure = false;
                Value::Void
            }
            "SetValueFormat" => {
                let f = match a.first() {
                    Some(Value::Str(s)) => s.clone(),
                    _ => String::new(),
                };
                let _ = self.set_prop(this, "valueFmt", 0, Value::Str(f));
                self.scale_changed_position(this, true);
                reconfigure = false;
                Value::Void
            }
            "GetValueString" => {
                reconfigure = false;
                Value::Str(self.scale_value_string(this))
            }
            "SetEnumeration" => {
                let tick = iv(a, 0);
                if (0..512).contains(&tick) {
                    let s = match a.get(1) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => String::new(),
                    };
                    let v = self.ui.scale_enums.entry(this).or_default();
                    if v.len() <= tick as usize {
                        v.resize(tick as usize + 1, String::new());
                    }
                    v[tick as usize] = s;
                    if tick == self.wi_pub(this, "currentPos") {
                        self.scale_changed_position(this, true);
                    }
                }
                reconfigure = false;
                Value::Void
            }
            "ClearAllEnumerations" => {
                self.ui.scale_enums.remove(&this);
                self.scale_changed_position(this, true);
                reconfigure = false;
                Value::Void
            }
            "MoveThumb" => {
                self.scale_move_thumb(this, iv(a, 0));
                reconfigure = false;
                Value::Void
            }
            "SetThumbStep" => {
                self.set_i_pub(this, "thumbStep", iv(a, 0).max(1));
                reconfigure = false;
                Value::Void
            }
            "SetScaleSounds" => {
                self.set_o_pub(this, "setSound", ob(a, 0));
                self.set_o_pub(this, "clickSound", ob(a, 1));
                self.set_o_pub(this, "dragSound", ob(a, 2));
                reconfigure = false;
                Value::Void
            }
            "PlayScaleSound" => {
                reconfigure = false;
                self.scale_play_sound(this, ob(a, 0), fl(a, 1, -1.0), fl(a, 2, 1.0));
                Value::Void
            }
            _ => return None,
        };
        if reconfigure {
            self.ui_ask_parent_for_reconfigure(this);
        }
        Some(Ok(ret))
    }
}

impl Vm {
    /// `SetRanges`: thumb span and number of positions at once (the scroll area's way to follow its clip).
    pub(crate) fn scale_set_ranges(&mut self, w: u32, span: i32, num: i32) {
        let span = span.max(1);
        if self.wi_pub(w, "spanRange") != span || self.wi_pub(w, "numPositions") != num {
            self.set_i_pub(w, "spanRange", span);
            self.set_i_pub(w, "numPositions", num);
            let cur = self.wi_pub(w, "currentPos");
            self.scale_change_thumb(w, cur, true, true);
        }
    }

    pub(crate) fn scale_move_thumb_pub(&mut self, w: u32, how: i32) {
        self.scale_move_thumb(w, how);
    }
}

//! `ScaleManagerWindow`: lays out a scale (slider) with its two step buttons and a value text, in a row or a column.
//! Ported from the `XScaleManagerWindow` functions of Extension.dll. The scale itself (`ScaleWindow`) is in a later
//! slice: until then the manager lays out whatever children it has and forwards nothing to a scale.

use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

fn fl(a: &[Value], i: usize, d: f32) -> f32 {
    match a.get(i) {
        None | Some(Value::Void) => d,
        Some(v) => v.as_f32().unwrap_or(d),
    }
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

impl Vm {
    pub(crate) fn scale_manager_init(&mut self, w: u32) {
        for n in ["decButton", "incButton", "valueField", "scale"] {
            self.set_o_pub(w, n, 0);
        }
        let _ = self.set_prop(w, "orientation", 0, Value::Byte(0));
        self.set_b_pub(w, "bStretchScaleField", false);
        self.set_b_pub(w, "bStretchValueField", true);
        for n in ["marginWidth", "marginHeight", "spacing"] {
            self.set_f_pub(w, n, 0.0);
        }
        let _ = self.set_prop(w, "childHAlign", 0, Value::Byte(1));
        let _ = self.set_prop(w, "childVAlign", 0, Value::Byte(1));
    }

    /// `ChangeValueField`: the value text shows the scale's value string.
    fn scale_manager_change_value_field(&mut self, w: u32) {
        let (field, scale) = (self.wo_pub(w, "valueField"), self.wo_pub(w, "scale"));
        if field != 0 && scale != 0 {
            let text = self.scale_value_string(scale);
            let _ = self.native_widgets_pub(field, "SetText", &mut vec![Value::Str(text)]);
        }
    }

    /// `IsDescendant`: `c` is `w` or lies below it.
    fn is_descendant(&self, w: u32, c: u32) -> bool {
        let mut cur = c;
        while cur != 0 {
            if cur == w {
                return true;
            }
            cur = self.wo_pub(cur, "winParent");
        }
        false
    }

    /// Visible children, bottom of the stacking order first.
    fn visible_children_bottom_up(&self, w: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut c = self.wo_pub(w, "firstChild");
        while c != 0 {
            if self.wb_pub(c, "bIsVisible") {
                out.push(c);
            }
            c = self.wo_pub(c, "nextSibling");
        }
        out
    }

    /// Grows the bounding box (`bw`, `bh`) by a child of `cw` x `ch` after `gap` units of spacing.
    fn scale_mgr_bounds(&self, w: u32, bw: &mut f32, bh: &mut f32, cw: f32, ch: f32, gap: f32) {
        if self.wi_pub(w, "orientation") == 1 {
            *bh += ch + gap;
            if *bw < cw {
                *bw = cw;
            }
        } else {
            *bw += cw + gap;
            if *bh < ch {
                *bh = ch;
            }
        }
    }

    /// `ComputeChildConfig`: a child's place along the main axis (`pos` = end of the previous child), aligned on the
    /// other axis.
    fn scale_mgr_place(&mut self, w: u32, c: u32, pos: &mut f32, gap: f32) {
        let (pw, ph) = self.ui_query_preferred_size(c, false, 0.0, false, 0.0);
        self.set_f_pub(c, "holdWidth", pw);
        self.set_f_pub(c, "holdHeight", ph);
        let (mw, mh) = (self.wf_pub(w, "marginWidth"), self.wf_pub(w, "marginHeight"));
        let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
        if self.wi_pub(w, "orientation") == 1 {
            let y = gap + *pos;
            let (x, cw) = match self.wi_pub(w, "childHAlign") {
                0 => (mw, pw),
                1 => ((width - pw) * 0.5, pw),
                2 => ((width - pw) - mw, pw),
                _ => (mw, width - (mw + mw)),
            };
            self.set_f_pub(c, "holdX", x);
            self.set_f_pub(c, "holdY", y);
            self.set_f_pub(c, "holdWidth", cw);
            *pos = y + ph;
        } else {
            let x = gap + *pos;
            let (y, ch) = match self.wi_pub(w, "childVAlign") {
                0 => (mh, ph),
                1 => ((height - ph) * 0.5, ph),
                2 => ((height - ph) - mh, ph),
                _ => (mh, height - (mh + mh)),
            };
            self.set_f_pub(c, "holdX", x);
            self.set_f_pub(c, "holdY", y);
            self.set_f_pub(c, "holdHeight", ch);
            *pos = pw + x;
        }
    }

    pub(crate) fn scale_manager_event(&mut self, w: u32, name: &str, args: &mut Vec<Value>) -> Option<Value> {
        match name {
            "ParentRequestedPreferredSize" => {
                let (mut bw, mut bh) = (0.0f32, 0.0f32);
                let mut gap = 0.0;
                for c in self.visible_children_bottom_up(w) {
                    let (cw, ch) = self.ui_query_preferred_size(c, false, 0.0, false, 0.0);
                    self.scale_mgr_bounds(w, &mut bw, &mut bh, cw, ch, gap);
                    gap = self.wf_pub(w, "spacing");
                }
                bw += self.wf_pub(w, "marginWidth") * 2.0;
                bh += self.wf_pub(w, "marginHeight") * 2.0;
                args[1] = Value::Float(bw);
                args[3] = Value::Float(bh);
                Some(Value::Void)
            }
            "ChildRequestedVisibilityChange" => {
                let child = ob(args, 0);
                self.ui_set_child_visibility(child, bl(args, 1, false));
                self.ui_ask_parent_for_reconfigure(w);
                Some(Value::Void)
            }
            "ConfigurationChanged" => {
                let vertical = self.wi_pub(w, "orientation") == 1;
                let (mw, mh) = (self.wf_pub(w, "marginWidth"), self.wf_pub(w, "marginHeight"));
                let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
                let mut gap = if vertical { mh } else { mw };
                let end = if vertical { height - mh } else { width - mw };
                let mut pos = 0.0f32;
                let kids = self.visible_children_bottom_up(w);
                for &c in &kids {
                    self.scale_mgr_place(w, c, &mut pos, gap);
                    gap = self.wf_pub(w, "spacing");
                }
                let (scale, value) = (self.wo_pub(w, "scale"), self.wo_pub(w, "valueField"));
                let stretch_scale = scale != 0 && self.wo_pub(scale, "winParent") == w && self.wb_pub(scale, "bIsVisible") && self.wb_pub(w, "bStretchScaleField");
                let stretch_value = value != 0 && self.wo_pub(value, "winParent") == w && self.wb_pub(value, "bIsVisible") && self.wb_pub(w, "bStretchValueField");
                if pos != end && (stretch_scale || stretch_value) {
                    let extra = end - pos;
                    let (to_scale, to_value) = match (stretch_scale, stretch_value) {
                        (true, false) => (extra, 0.0),
                        (false, _) => (0.0, extra),
                        (true, true) => (extra * 0.5, extra - extra * 0.5),
                    };
                    let mut off = 0.0f32;
                    for &c in &kids {
                        let amount = if c == scale { to_scale } else if c == value { to_value } else { 0.0 };
                        if vertical {
                            let (y, h) = (self.wf_pub(c, "holdY"), self.wf_pub(c, "holdHeight"));
                            self.set_f_pub(c, "holdY", off + y);
                            self.set_f_pub(c, "holdHeight", amount + h);
                        } else {
                            let (x, cw) = (self.wf_pub(c, "holdX"), self.wf_pub(c, "holdWidth"));
                            self.set_f_pub(c, "holdX", x + off);
                            self.set_f_pub(c, "holdWidth", amount + cw);
                        }
                        off += amount;
                    }
                }
                for c in kids {
                    let (x, y, cw, ch) = (self.wf_pub(c, "holdX"), self.wf_pub(c, "holdY"), self.wf_pub(c, "holdWidth"), self.wf_pub(c, "holdHeight"));
                    self.ui_configure_child(c, x, y, cw, ch);
                }
                Some(Value::Void)
            }
            "ScalePositionChanged" | "ScaleAttributesChanged" | "ButtonActivated" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                let scale = self.wo_pub(w, "scale");
                let who = ob(args, 0);
                match name {
                    "ScalePositionChanged" => {
                        if who == scale && scale != 0 {
                            self.scale_manager_change_value_field(w);
                        }
                        Some(r)
                    }
                    "ScaleAttributesChanged" => {
                        if who == scale && scale != 0 {
                            let (pos, span, num) = (args.get(1).and_then(Value::as_i32).unwrap_or(0), args.get(2).and_then(Value::as_i32).unwrap_or(0), args.get(3).and_then(Value::as_i32).unwrap_or(0));
                            let (dec, inc) = (self.wo_pub(w, "decButton"), self.wo_pub(w, "incButton"));
                            if dec != 0 {
                                self.ui_set_sensitivity_pub(dec, pos > 0);
                            }
                            if inc != 0 {
                                self.ui_set_sensitivity_pub(inc, pos + span < num);
                            }
                        }
                        Some(r)
                    }
                    _ => {
                        if scale != 0 {
                            let how = if who == self.wo_pub(w, "incButton") { 5 } else if who == self.wo_pub(w, "decButton") { 4 } else { -1 };
                            if how >= 0 {
                                self.scale_move_thumb_pub(scale, how);
                                return Some(Value::Bool(true));
                            }
                        }
                        Some(r)
                    }
                }
            }
            "ChildRemoved" => {
                // the script event first, then the manager forgets the child
                let (_, out) = self.ui_script_event(w, "ChildRemoved", args.clone());
                *args = out;
                let child = ob(args, 0);
                for n in ["decButton", "incButton", "scale", "valueField"] {
                    if self.wo_pub(w, n) == child {
                        self.set_o_pub(w, n, 0);
                        break;
                    }
                }
                Some(Value::Void)
            }
            _ => None,
        }
    }

    /// The script natives of `ScaleManagerWindow`.
    pub(crate) fn native_scale_manager(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        match name {
            "SetScaleButtons" => {
                let (dec, inc) = (ob(a, 0), ob(a, 1));
                let dec = if self.is_descendant(this, dec) { dec } else { 0 };
                let inc = if self.is_descendant(this, inc) { inc } else { 0 };
                self.set_o_pub(this, "incButton", inc);
                self.set_o_pub(this, "decButton", dec);
                return Some(Ok(Value::Void));
            }
            "SetScale" | "SetValueField" => {
                let prop = if name == "SetScale" { "scale" } else { "valueField" };
                let c = ob(a, 0);
                let c = if self.is_descendant(this, c) { c } else { 0 };
                if self.wo_pub(this, prop) != c {
                    self.set_o_pub(this, prop, c);
                    self.ui_ask_parent_for_reconfigure(this);
                    self.scale_manager_change_value_field(this);
                }
                return Some(Ok(Value::Void));
            }
            "SetManagerOrientation" => {
                let _ = self.set_prop(this, "orientation", 0, Value::Byte(a.first().and_then(Value::as_i32).unwrap_or(0) as u8));
            }
            "StretchScaleField" => self.set_b_pub(this, "bStretchScaleField", bl(a, 0, true)),
            "StretchValueField" => self.set_b_pub(this, "bStretchValueField", bl(a, 0, true)),
            "SetManagerMargins" => {
                let (mw, mh) = (fl(a, 0, 0.0), fl(a, 1, 0.0));
                if mw != self.wf_pub(this, "marginWidth") || mh != self.wf_pub(this, "marginHeight") {
                    self.set_f_pub(this, "marginWidth", mw);
                    self.set_f_pub(this, "marginHeight", mh);
                } else {
                    return Some(Ok(Value::Void));
                }
            }
            "SetMarginSpacing" => {
                let s = fl(a, 0, 0.0);
                if s == self.wf_pub(this, "spacing") {
                    return Some(Ok(Value::Void));
                }
                self.set_f_pub(this, "spacing", s);
            }
            "SetManagerAlignments" => {
                let (h, v) = (a.first().and_then(Value::as_i32).unwrap_or(0) as u8, a.get(1).and_then(Value::as_i32).unwrap_or(0) as u8);
                let _ = self.set_prop(this, "childHAlign", 0, Value::Byte(h));
                let _ = self.set_prop(this, "childVAlign", 0, Value::Byte(v));
            }
            _ => return None,
        }
        self.ui_ask_parent_for_reconfigure(this);
        Some(Ok(Value::Void))
    }
}

//! `ClipWindow`: a window that shows part of its one visible child, which may be bigger than the clip. The child's
//! position is kept in "units" (the child's granularity, e.g. one text row or one list row) as `childH`/`childV`;
//! scroll bars (`ScrollAreaWindow`) and lists read and move it. Ported from the `XClipWindow` functions of
//! Extension.dll.

use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

fn iv(a: &[Value], i: usize) -> i32 {
    a.get(i).and_then(Value::as_i32).unwrap_or(0)
}
/// Optional bool arguments of the clip window natives default to true.
fn bl(a: &[Value], i: usize) -> bool {
    match a.get(i) {
        Some(Value::Void) | None => true,
        Some(v) => v.as_bool().unwrap_or(true),
    }
}

impl Vm {
    pub(crate) fn clip_init(&mut self, w: u32) {
        for (n, v) in [("prefHUnits", -1), ("prefVUnits", -1), ("childH", 0), ("childV", 0), ("areaHSize", 0), ("areaVSize", 0), ("childHSize", 0), ("childVSize", 0)] {
            self.set_i_pub(w, n, v);
        }
        self.set_f_pub(w, "hMult", 1.0);
        self.set_f_pub(w, "vMult", 1.0);
        for (n, v) in [("bForceChildWidth", false), ("bForceChildHeight", false), ("bSnapToUnits", true), ("bFillWindow", true)] {
            self.set_b_pub(w, n, v);
        }
    }

    /// `GetChild`: the top visible child.
    pub(crate) fn clip_child(&self, w: u32) -> u32 {
        let mut c = self.wo_pub(w, "lastChild");
        while c != 0 && !self.wb_pub(c, "bIsVisible") {
            c = self.wo_pub(c, "prevSibling");
        }
        c
    }

    /// The granularity of the child when the clip snaps to units, else (1, 1).
    fn clip_child_units(&mut self, w: u32, child: u32) -> (f32, f32) {
        if self.wb_pub(w, "bSnapToUnits") && child != 0 {
            self.ui_query_granularity(child)
        } else {
            (1.0, 1.0)
        }
    }

    fn clip_clamp(&self, w: u32, x: &mut f32, y: &mut f32, cw: f32, ch: f32) {
        *x = x.max(self.wf_pub(w, "width") - cw);
        if *x > 0.0 {
            *x = 0.0;
        }
        *y = y.max(self.wf_pub(w, "height") - ch);
        if *y > 0.0 {
            *y = 0.0;
        }
    }

    /// `GetChildPreferredSize`: the child's wanted place and size into its `hold*` fields.
    fn clip_child_preferred(&mut self, w: u32, child: u32, w_spec: bool, wv: f32, h_spec: bool, hv: f32) {
        if child == 0 {
            return;
        }
        let force_w = w_spec && self.wb_pub(w, "bForceChildWidth");
        let force_h = h_spec && self.wb_pub(w, "bForceChildHeight");
        let (ch, cv) = (self.wi_pub(w, "childH") as f32, self.wi_pub(w, "childV") as f32);
        let (nw, nh);
        if force_w {
            self.set_f_pub(child, "holdX", 0.0);
            if !force_h {
                self.set_f_pub(child, "holdY", cv);
                nw = wv;
                nh = self.ui_query_preferred_size(child, true, wv, false, 0.0).1;
            } else {
                self.set_f_pub(child, "holdY", 0.0);
                nw = wv;
                nh = hv;
            }
        } else {
            self.set_f_pub(child, "holdX", ch);
            if !force_h {
                self.set_f_pub(child, "holdY", cv);
                let (pw, ph) = self.ui_query_preferred_size(child, false, 0.0, false, 0.0);
                nw = pw;
                nh = ph;
            } else {
                self.set_f_pub(child, "holdY", 0.0);
                nw = self.ui_query_preferred_size(child, false, 0.0, true, hv).0;
                nh = hv;
            }
        }
        self.set_f_pub(child, "holdWidth", nw);
        self.set_f_pub(child, "holdHeight", nh);
    }

    /// `GetClipPreferredSize`: with a child, the unit size set by `SetUnitSize` (when the caller gave no size) or the
    /// child's own preferred size; `None` without a child.
    pub(crate) fn clip_preferred(&mut self, w: u32, child: u32, w_spec: bool, wv: f32, h_spec: bool, hv: f32) -> Option<(f32, f32)> {
        if child == 0 {
            // `GetClipPreferredSize` (1000bfa0) without a child: the tab group's own answer (0 x 0 for a clip,
            // which sizes itself to its children), else the values as they came
            return self.tab_group_preferred(w, w_spec, wv, h_spec, hv);
        }
        let (mut w_spec, mut h_spec, mut wv, mut hv) = (w_spec, h_spec, wv, hv);
        let (ux, uy) = self.clip_child_units(w, child);
        let (ph, pv) = (self.wi_pub(w, "prefHUnits"), self.wi_pub(w, "prefVUnits"));
        if ph > -1 && !w_spec {
            w_spec = true;
            wv = ph as f32 * ux;
        }
        if pv > -1 && !h_spec {
            h_spec = true;
            hv = pv as f32 * uy;
        }
        self.clip_child_preferred(w, child, w_spec, wv, h_spec, hv);
        if !w_spec {
            wv = self.wf_pub(child, "holdWidth");
        }
        if !h_spec {
            hv = self.wf_pub(child, "holdHeight");
        }
        Some((wv, hv))
    }

    /// `QueryClipPreferredSize`: the size the clip wants given what the caller specified and whether the child is wider
    /// or taller than that (a scroll area then needs a scroll bar). Returns (width, height, child wider, child taller).
    pub(crate) fn clip_query_preferred(&mut self, w: u32, w_spec: bool, wv: f32, h_spec: bool, hv: f32) -> (f32, f32, bool, bool) {
        let (mut w_spec, mut h_spec, mut wv, mut hv) = (w_spec, h_spec, wv, hv);
        if !w_spec && self.wf_pub(w, "hardcodedWidth") >= 0.0 {
            wv = self.wf_pub(w, "hardcodedWidth");
            w_spec = true;
        }
        if !h_spec && self.wf_pub(w, "hardcodedHeight") >= 0.0 {
            hv = self.wf_pub(w, "hardcodedHeight");
            h_spec = true;
        }
        let (mut pw, mut ph) = (if w_spec { wv } else { 10.0 }, if h_spec { hv } else { 10.0 });
        let child = self.clip_child(w);
        if let Some((a, b)) = self.clip_preferred(w, child, w_spec, pw, h_spec, ph) {
            pw = a;
            ph = b;
        }
        let (mut wide, mut tall) = (false, false);
        if child != 0 {
            wide = pw < self.wf_pub(child, "holdWidth");
            tall = ph < self.wf_pub(child, "holdHeight");
        }
        if w_spec {
            pw = wv;
        }
        if h_spec {
            ph = hv;
        }
        (pw, ph, wide, tall)
    }

    /// Delivers `event` to the clip and then to its ancestors until one handles it.
    fn clip_notify(&mut self, w: u32, event: &str, args: Vec<Value>) {
        let mut cur = w;
        while cur != 0 {
            let (r, _) = self.ui_event(cur, event, args.clone());
            if r.as_bool().unwrap_or(false) {
                break;
            }
            cur = self.wo_pub(cur, "winParent");
        }
    }

    /// `ReconfigureChild`: puts the child (preferred size `cw` x `ch`, top-left unit `px`, `py`) in the clip and
    /// reports a changed unit geometry.
    fn clip_reconfigure_child(&mut self, w: u32, child: u32, px: i32, py: i32, cw: f32, ch: f32) {
        let (hm, vm) = self.clip_child_units(w, child);
        self.set_f_pub(w, "hMult", hm);
        self.set_f_pub(w, "vMult", vm);
        let mut x = -hm * px as f32;
        let mut y = -vm * py as f32;
        let n_child_h = (cw / hm).ceil() as i32;
        let n_child_v = (ch / vm).ceil() as i32;
        let (mut cw, mut ch) = (cw, ch);
        let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
        if self.wb_pub(w, "bFillWindow") {
            cw = cw.max(width);
            ch = ch.max(height);
        }
        let hidden_h = ((cw - width) / hm).ceil() as i32;
        let hidden_v = ((ch - height) / vm).ceil() as i32;
        self.clip_clamp(w, &mut x, &mut y, cw, ch);
        let (area_h, area_v) = (n_child_h - hidden_h, n_child_v - hidden_v);
        let changed = self.wi_pub(w, "childHSize") != n_child_h
            || self.wi_pub(w, "childVSize") != n_child_v
            || self.wi_pub(w, "areaHSize") != area_h
            || self.wi_pub(w, "areaVSize") != area_v;
        self.ui_configure_child(child, x, y, cw, ch);
        self.set_i_pub(w, "childHSize", n_child_h);
        self.set_i_pub(w, "childVSize", n_child_v);
        self.set_i_pub(w, "areaHSize", area_h);
        self.set_i_pub(w, "areaVSize", area_v);
        if changed {
            self.clip_notify(w, "ClipAttributesChanged", vec![Value::Obj(w), Value::Int(area_h), Value::Int(area_v), Value::Int(n_child_h), Value::Int(n_child_v)]);
        }
    }

    pub(crate) fn clip_set_child_position(&mut self, w: u32, h: i32, v: i32) {
        if self.wi_pub(w, "childH") == h && self.wi_pub(w, "childV") == v {
            return;
        }
        self.set_i_pub(w, "childH", h);
        self.set_i_pub(w, "childV", v);
        let child = self.clip_child(w);
        if child == 0 {
            return;
        }
        let (hm, vm) = (self.wf_pub(w, "hMult"), self.wf_pub(w, "vMult"));
        let mut x = if self.wb_pub(w, "bForceChildWidth") { 0.0 } else { (-hm * h as f32).trunc() };
        let mut y = if self.wb_pub(w, "bForceChildHeight") { 0.0 } else { (-vm * v as f32).trunc() };
        let (cw, ch) = (self.wf_pub(child, "width"), self.wf_pub(child, "height"));
        self.clip_clamp(w, &mut x, &mut y, cw, ch);
        self.ui_lock_pub(w);
        self.ui_configure_child(child, x, y, cw, ch);
        self.ui_unlock_pub(w);
        self.clip_notify(w, "ClipPositionChanged", vec![Value::Obj(w), Value::Int(h), Value::Int(v)]);
    }

    /// The native overrides of the clip's events; `None` = not one of them.
    pub(crate) fn clip_event(&mut self, w: u32, name: &str, args: &mut Vec<Value>) -> Option<Value> {
        match name {
            "ParentRequestedPreferredSize" => {
                let child = self.clip_child(w);
                let (w_spec, h_spec) = (bl(args, 0), bl(args, 2));
                let (wv, hv) = (args.get(1).and_then(Value::as_f32).unwrap_or(0.0), args.get(3).and_then(Value::as_f32).unwrap_or(0.0));
                let (nw, nh) = self.clip_preferred(w, child, w_spec, wv, h_spec, hv)?;
                args[1] = Value::Float(nw);
                args[3] = Value::Float(nh);
                Some(Value::Void)
            }
            "ConfigurationChanged" => {
                let child = self.clip_child(w);
                let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
                if child != 0 {
                    self.clip_child_preferred(w, child, true, width, true, height);
                    let (x, y) = (self.wf_pub(child, "holdX") as i32, self.wf_pub(child, "holdY") as i32);
                    let (cw, ch) = (self.wf_pub(child, "holdWidth"), self.wf_pub(child, "holdHeight"));
                    self.clip_reconfigure_child(w, child, x, y, cw, ch);
                }
                // every other visible child is collapsed
                let mut c = self.wo_pub(w, "lastChild");
                while c != 0 {
                    let next = self.wo_pub(c, "prevSibling");
                    if c != child && self.wb_pub(c, "bIsVisible") {
                        self.ui_configure_child(c, 0.0, 0.0, 0.0, 0.0);
                    }
                    c = next;
                }
                Some(Value::Void)
            }
            "ChildRequestedVisibilityChange" => {
                let child = match args.first() {
                    Some(Value::Obj(c)) => *c,
                    _ => 0,
                };
                self.ui_set_child_visibility(child, bl(args, 1));
                self.ui_ask_parent_for_reconfigure(w);
                Some(Value::Void)
            }
            "ChildRequestedShowArea" => {
                let child = match args.first() {
                    Some(Value::Obj(c)) => *c,
                    _ => 0,
                };
                let f = |i: usize| args.get(i).and_then(Value::as_f32).unwrap_or(0.0);
                let (sx, sy, sw, sh) = (f(1), f(2), f(3), f(4));
                let (hm, vm) = self.clip_child_units(w, child);
                self.set_f_pub(w, "hMult", hm);
                self.set_f_pub(w, "vMult", vm);
                if hm <= 0.0 || vm <= 0.0 {
                    return Some(Value::Void);
                }
                let (x0, y0) = (sx + 0.005, sy + 0.005);
                let (ww, hh) = (sw - 0.01, sh - 0.01);
                let left = (x0 / hm).floor() as i32;
                let wd = ((x0 + ww) / hm).ceil() as i32 - left;
                let top = (y0 / vm).floor() as i32;
                let ht = ((y0 + hh) / vm).ceil() as i32 - top;
                let (mut ch, mut cv) = (self.wi_pub(w, "childH"), self.wi_pub(w, "childV"));
                let (dh, dv) = (left - ch, top - cv);
                ch += clip_scroll_delta(self.wi_pub(w, "areaHSize"), wd, dh);
                cv += clip_scroll_delta(self.wi_pub(w, "areaVSize"), ht, dv);
                self.clip_set_child_position(w, ch, cv);
                Some(Value::Void)
            }
            _ => None,
        }
    }

    /// The script natives of `ClipWindow`.
    pub(crate) fn native_clip(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        let out = match name {
            "SetChildPosition" => {
                self.clip_set_child_position(this, iv(a, 0), iv(a, 1));
                Value::Void
            }
            "GetChildPosition" => {
                a[0] = Value::Int(self.wi_pub(this, "childH"));
                a[1] = Value::Int(self.wi_pub(this, "childV"));
                Value::Void
            }
            "SetUnitSize" | "SetUnitWidth" | "SetUnitHeight" => {
                let (mut h, mut v) = (self.wi_pub(this, "prefHUnits"), self.wi_pub(this, "prefVUnits"));
                match name {
                    "SetUnitSize" => {
                        h = iv(a, 0).max(1);
                        v = iv(a, 1).max(1);
                    }
                    "SetUnitWidth" => h = iv(a, 0).max(1),
                    _ => v = iv(a, 0).max(1),
                }
                if h != self.wi_pub(this, "prefHUnits") || v != self.wi_pub(this, "prefVUnits") {
                    self.set_i_pub(this, "prefHUnits", h);
                    self.set_i_pub(this, "prefVUnits", v);
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Value::Void
            }
            "ResetUnitSize" | "ResetUnitWidth" | "ResetUnitHeight" => {
                let (mut h, mut v) = (self.wi_pub(this, "prefHUnits"), self.wi_pub(this, "prefVUnits"));
                let before = (h, v);
                match name {
                    "ResetUnitSize" => {
                        h = -1;
                        v = -1;
                    }
                    "ResetUnitWidth" => h = -1,
                    // (1000b910) the original tests the vertical units but clears the horizontal ones
                    _ => {
                        if v > -1 {
                            h = -1;
                        }
                    }
                }
                if (h, v) != before {
                    self.set_i_pub(this, "prefHUnits", h);
                    self.set_i_pub(this, "prefVUnits", v);
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Value::Void
            }
            "GetUnitSize" => {
                for (i, n) in ["areaHSize", "areaVSize", "childHSize", "childVSize"].into_iter().enumerate() {
                    a[i] = Value::Int(self.wi_pub(this, n));
                }
                Value::Void
            }
            "ForceChildSize" => {
                let (fw, fh) = (bl(a, 0), bl(a, 1));
                if self.wb_pub(this, "bForceChildWidth") != fw || self.wb_pub(this, "bForceChildHeight") != fh {
                    self.set_b_pub(this, "bForceChildWidth", fw);
                    self.set_b_pub(this, "bForceChildHeight", fh);
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Value::Void
            }
            "EnableSnapToUnits" => {
                let s = bl(a, 0);
                if self.wb_pub(this, "bSnapToUnits") != s {
                    self.set_b_pub(this, "bSnapToUnits", s);
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Value::Void
            }
            "GetChild" => Value::Obj(self.clip_child(this)),
            _ => return None,
        };
        Some(Ok(out))
    }
}

/// How far the clip's first unit has to move so that a wanted span (`span` units, starting `rel` units after the
/// current first unit) fits into an area of `area` units.
fn clip_scroll_delta(area: i32, span: i32, rel: i32) -> i32 {
    if area < span {
        if rel < area - span {
            (span - area) + rel
        } else if rel > 0 {
            rel
        } else {
            0
        }
    } else if area - span < rel {
        (span - area) + rel
    } else if rel < 0 {
        rel
    } else {
        0
    }
}

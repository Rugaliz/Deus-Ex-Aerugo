//! `ScrollAreaWindow`: a clip window with an optional horizontal and vertical scroll bar (a scale manager holding two
//! arrow buttons and a span scale each). Ported from the `XScrollAreaWindow` functions of Extension.dll.

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

const KEY_WHEEL_UP: i32 = 0xec;
const KEY_WHEEL_DOWN: i32 = 0xed;

impl Vm {
    fn class_ref(&mut self, name: &str) -> u32 {
        let Some(c) = self.reg.class_by_name(name) else { return 0 };
        let path = self.reg.class(c).path.clone();
        self.reg.obj_id(&path).0
    }

    fn new_child(&mut self, class: &str, parent: u32, show: bool) -> u32 {
        let cls = self.class_ref(class);
        self.ui_create_window(cls, parent, show).unwrap_or(0)
    }

    /// `XScrollAreaWindow::Init`: builds the scroll bars and the clip window.
    pub(crate) fn scroll_area_init(&mut self, w: u32) {
        self.set_f_pub(w, "marginWidth", 3.0);
        self.set_f_pub(w, "marginHeight", 3.0);
        self.set_f_pub(w, "scrollbarDistance", 3.0);
        self.set_b_pub(w, "bHideScrollbars", true);
        self.set_b_pub(w, "bHLastShow", false);
        self.set_b_pub(w, "bVLastShow", false);
        let hmgr = self.new_child("ScaleManagerWindow", w, false);
        let vmgr = self.new_child("ScaleManagerWindow", w, false);
        let clip = self.new_child("ClipWindow", w, true);
        let left = self.new_child("ButtonWindow", hmgr, true);
        let hscale = self.new_child("ScaleWindow", hmgr, true);
        let right = self.new_child("ButtonWindow", hmgr, true);
        let up = self.new_child("ButtonWindow", vmgr, true);
        let vscale = self.new_child("ScaleWindow", vmgr, true);
        let down = self.new_child("ButtonWindow", vmgr, true);
        for (n, v) in [("hScaleMgr", hmgr), ("vScaleMgr", vmgr), ("clipWindow", clip), ("leftButton", left), ("hScale", hscale), ("rightButton", right), ("upButton", up), ("vScale", vscale), ("downButton", down)] {
            self.set_o_pub(w, n, v);
        }
        for (mgr, vertical, a, b, scale) in [(hmgr, 0u8, left, right, hscale), (vmgr, 1, up, down, vscale)] {
            let _ = self.set_prop(mgr, "orientation", 0, Value::Byte(vertical));
            self.ui_ask_parent_for_reconfigure(mgr);
            let _ = self.native_scale_manager(mgr, "SetScaleButtons", &mut vec![Value::Obj(a), Value::Obj(b)]);
            let _ = self.native_scale_manager(mgr, "SetScale", &mut vec![Value::Obj(scale)]);
            self.set_b_pub(mgr, "bStretchScaleField", true);
            self.ui_ask_parent_for_reconfigure(mgr);
        }
        for (scale, vertical) in [(hscale, 0), (vscale, 1)] {
            let _ = self.native_scale(scale, "SetScaleOrientation", &mut vec![Value::Byte(vertical)]);
            self.ui_set_selectability(scale, false);
            let _ = self.native_scale(scale, "SetNumTicks", &mut vec![Value::Int(1)]);
            let _ = self.native_scale(scale, "SetThumbSpan", &mut vec![Value::Int(1)]);
            let _ = self.native_scale(scale, "EnableStretchedScale", &mut vec![Value::Bool(true)]);
        }
        for b in [left, right, up, down] {
            self.ui_set_selectability(b, false);
            let _ = self.native_widgets_pub(b, "EnableAutoRepeat", &mut vec![Value::Bool(true), Value::Float(0.5), Value::Float(0.1)]);
        }
        self.scroll_area_enable(w, false, true);
        self.ui_set_selectability(w, false);
    }

    fn scroll_area_enable(&mut self, w: u32, h: bool, v: bool) {
        let (hm, vm, clip) = (self.wo_pub(w, "hScaleMgr"), self.wo_pub(w, "vScaleMgr"), self.wo_pub(w, "clipWindow"));
        if hm != 0 {
            self.ui_set_visibility(hm, h);
        }
        if vm != 0 {
            self.ui_set_visibility(vm, v);
        }
        if clip != 0 {
            let _ = self.native_clip(clip, "ForceChildSize", &mut vec![Value::Bool(!h), Value::Bool(!v)]);
        }
    }

    fn visible_or_none(&self, w: u32) -> u32 {
        if w != 0 && self.wb_pub(w, "bIsVisible") {
            w
        } else {
            0
        }
    }

    /// `ComputeChildSizes`: places the clip and the visible scroll bars into their `hold*` fields and returns the size the
    /// area wants. Scroll bars that hide themselves appear when the child does not fit (a few passes, since a bar takes
    /// room that may make the other one necessary).
    fn scroll_area_sizes(&mut self, w: u32, w_spec: bool, wv: f32, h_spec: bool, hv: f32) -> (f32, f32) {
        let hm = self.visible_or_none(self.wo_pub(w, "hScaleMgr"));
        let vm = self.visible_or_none(self.wo_pub(w, "vScaleMgr"));
        let clip = self.visible_or_none(self.wo_pub(w, "clipWindow"));
        if clip == 0 {
            for m in [hm, vm] {
                if m != 0 {
                    for n in ["holdX", "holdY", "holdWidth", "holdHeight"] {
                        self.set_f_pub(m, n, 0.0);
                    }
                }
            }
            return (10.0, 10.0);
        }
        let dist = self.wf_pub(w, "scrollbarDistance");
        let (mut bar_h, mut bar_v) = (0.0f32, 0.0f32);
        let (mut hbar_h, mut vbar_w) = (0.0f32, 0.0f32);
        let mut show_h = hm != 0;
        let mut show_v = vm != 0;
        if hm != 0 {
            hbar_h = self.ui_query_preferred_size(hm, false, 0.0, false, 0.0).1;
            bar_h = hbar_h + dist;
        }
        if vm != 0 {
            vbar_w = self.ui_query_preferred_size(vm, false, 0.0, false, 0.0).0;
            bar_v = vbar_w + dist;
        }
        let (mut auto_h, mut auto_v) = (false, false);
        if self.wb_pub(w, "bHideScrollbars") {
            if hm != 0 {
                show_h = self.wb_pub(w, "bHLastShow");
                auto_h = true;
            }
            if vm != 0 {
                auto_v = true;
                show_v = self.wb_pub(w, "bVLastShow");
            }
        }
        let (mw, mh) = (self.wf_pub(w, "marginWidth"), self.wf_pub(w, "marginHeight"));
        let avail_w = if w_spec { wv - mw * 2.0 } else { 0.0 };
        let avail_h = if h_spec { hv - mh * 2.0 } else { 0.0 };
        let mut max_pass = 1;
        if auto_h {
            max_pass = 2;
        }
        if auto_v {
            max_pass *= 2;
        }
        let (mut cw, mut ch);
        let mut pass = 0;
        loop {
            pass += 1;
            if pass > max_pass {
                if auto_h {
                    show_h = true;
                }
                if auto_v {
                    show_v = true;
                }
            }
            let sub_h = if show_h && h_spec { bar_h } else { 0.0 };
            let sub_w = if show_v && w_spec { bar_v } else { 0.0 };
            let (a, b, wide, tall) = self.clip_query_preferred(clip, w_spec, avail_w - sub_w, h_spec, avail_h - sub_h);
            cw = a;
            ch = b;
            if pass > max_pass {
                break;
            }
            if auto_h && wide && !show_h {
                show_h = true;
            } else if auto_v && tall && !show_v {
                show_v = true;
            } else if auto_h && !wide && show_h {
                show_h = false;
            } else if auto_v && !tall && show_v {
                show_v = false;
            } else {
                break;
            }
        }
        self.set_b_pub(w, "bHLastShow", show_h);
        self.set_b_pub(w, "bVLastShow", show_v);
        for (n, v) in [("holdX", mw), ("holdY", mh), ("holdWidth", cw), ("holdHeight", ch)] {
            self.set_f_pub(clip, n, v);
        }
        if hm != 0 {
            self.set_f_pub(hm, "holdX", mw);
            self.set_f_pub(hm, "holdY", dist + ch + mh);
            self.set_f_pub(hm, "holdWidth", cw);
            self.set_f_pub(hm, "holdHeight", if show_h { hbar_h } else { 0.0 });
        }
        if vm != 0 {
            self.set_f_pub(vm, "holdX", dist + cw + mw);
            self.set_f_pub(vm, "holdY", mh);
            self.set_f_pub(vm, "holdHeight", ch);
            self.set_f_pub(vm, "holdWidth", if show_v { vbar_w } else { 0.0 });
        }
        let mut out_w = mw * 2.0 + cw;
        if show_v {
            out_w += bar_v;
        }
        let mut out_h = mh * 2.0 + ch;
        if show_h {
            out_h += bar_h;
        }
        (out_w, out_h)
    }

    pub(crate) fn scroll_area_event(&mut self, w: u32, name: &str, args: &mut Vec<Value>) -> Option<Value> {
        match name {
            "ParentRequestedPreferredSize" => {
                let (rw, rh) = self.scroll_area_sizes(w, bl(args, 0, false), fl(args, 1, 0.0), bl(args, 2, false), fl(args, 3, 0.0));
                args[1] = Value::Float(rw);
                args[3] = Value::Float(rh);
                Some(Value::Void)
            }
            "ConfigurationChanged" => {
                let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
                self.scroll_area_sizes(w, true, width, true, height);
                for n in ["hScaleMgr", "vScaleMgr", "clipWindow"] {
                    let c = self.visible_or_none(self.wo_pub(w, n));
                    if c != 0 {
                        let (x, y, cw, ch) = (self.wf_pub(c, "holdX"), self.wf_pub(c, "holdY"), self.wf_pub(c, "holdWidth"), self.wf_pub(c, "holdHeight"));
                        self.ui_configure_child(c, x, y, cw, ch);
                    }
                }
                Some(Value::Void)
            }
            "ChildRequestedVisibilityChange" => {
                self.ui_set_child_visibility(ob(args, 0), bl(args, 1, false));
                self.ui_ask_parent_for_reconfigure(w);
                Some(Value::Void)
            }
            "DescendantRemoved" => {
                let gone = ob(args, 0);
                for n in ["hScaleMgr", "vScaleMgr", "hScale", "vScale", "leftButton", "rightButton", "upButton", "downButton", "clipWindow"] {
                    if self.wo_pub(w, n) == gone {
                        self.set_o_pub(w, n, 0);
                    }
                }
                Some(Value::Void)
            }
            "ScaleRangeChanged" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                let clip = self.wo_pub(w, "clipWindow");
                if clip == 0 {
                    return Some(r);
                }
                let who = ob(args, 0);
                let from = args.get(1).and_then(Value::as_i32).unwrap_or(0);
                let (mut col, mut row) = (self.wi_pub(clip, "childH"), self.wi_pub(clip, "childV"));
                let mut handled = false;
                if who == self.wo_pub(w, "hScale") {
                    col = from;
                    handled = true;
                } else if who == self.wo_pub(w, "vScale") {
                    row = from;
                    handled = true;
                }
                self.clip_set_child_position(clip, col, row);
                Some(if handled { Value::Bool(true) } else { r })
            }
            "ClipAttributesChanged" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                if ob(args, 0) != self.wo_pub(w, "clipWindow") {
                    return Some(r);
                }
                let g = |i: usize| args.get(i).and_then(Value::as_i32).unwrap_or(0);
                let (area_h, area_v, child_h, child_v) = (g(1), g(2), g(3).max(1), g(4).max(1));
                let (hs, vs) = (self.wo_pub(w, "hScale"), self.wo_pub(w, "vScale"));
                if hs != 0 {
                    self.scale_set_ranges(hs, area_h, child_h);
                }
                if vs != 0 {
                    self.scale_set_ranges(vs, area_v, child_v);
                }
                Some(Value::Bool(true))
            }
            "ClipPositionChanged" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                if ob(args, 0) != self.wo_pub(w, "clipWindow") {
                    return Some(r);
                }
                let (col, row) = (args.get(1).and_then(Value::as_i32).unwrap_or(0), args.get(2).and_then(Value::as_i32).unwrap_or(0));
                let (hs, vs) = (self.wo_pub(w, "hScale"), self.wo_pub(w, "vScale"));
                if hs != 0 {
                    let _ = self.native_scale(hs, "SetTickPosition", &mut vec![Value::Int(col)]);
                }
                if vs != 0 {
                    let _ = self.native_scale(vs, "SetTickPosition", &mut vec![Value::Int(row)]);
                }
                Some(Value::Bool(true))
            }
            "MouseButtonPressed" => {
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                let key = args.get(2).and_then(Value::as_i32).unwrap_or(0);
                if key != KEY_WHEEL_UP && key != KEY_WHEEL_DOWN {
                    return Some(r);
                }
                let vs = self.wo_pub(w, "vScale");
                if vs != 0 && self.ui_visible_recursive(vs) {
                    let _ = self.native_scale(vs, "MoveThumb", &mut vec![Value::Byte(if key == KEY_WHEEL_UP { 4 } else { 5 })]);
                }
                Some(Value::Bool(true))
            }
            _ => None,
        }
    }

    /// The script natives of `ScrollAreaWindow`.
    pub(crate) fn native_scroll_area(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        match name {
            "EnableScrolling" => self.scroll_area_enable(this, bl(a, 0, true), bl(a, 1, true)),
            "SetScrollbarDistance" => {
                let d = fl(a, 0, 0.0);
                if d != self.wf_pub(this, "scrollbarDistance") {
                    self.set_f_pub(this, "scrollbarDistance", d);
                    self.ui_ask_parent_for_reconfigure(this);
                }
            }
            "SetAreaMargins" => {
                let (mw, mh) = (fl(a, 0, 0.0), fl(a, 1, 0.0));
                if mw != self.wf_pub(this, "marginWidth") || mh != self.wf_pub(this, "marginHeight") {
                    self.set_f_pub(this, "marginWidth", mw);
                    self.set_f_pub(this, "marginHeight", mh);
                    self.ui_ask_parent_for_reconfigure(this);
                }
            }
            "AutoHideScrollbars" => {
                let hide = bl(a, 0, true);
                if self.wb_pub(this, "bHideScrollbars") != hide {
                    self.set_b_pub(this, "bHideScrollbars", hide);
                    self.ui_ask_parent_for_reconfigure(this);
                }
            }
            _ => return None,
        }
        Some(Ok(Value::Void))
    }
}

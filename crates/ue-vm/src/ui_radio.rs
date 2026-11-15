//! `RadioBoxWindow`: a tab group whose toggle-button descendants form a group where (normally) exactly one is on.
//! Ported from the `XRadioBoxWindow` functions of Extension.dll. The original keeps an array of its toggles; here the
//! group is found from the tree (the toggles whose nearest radio box above them is this window), which is the same set.

use crate::ui_widgets::Kind;
use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

fn ob(a: &[Value], i: usize) -> u32 {
    match a.get(i) {
        Some(Value::Obj(r)) => *r,
        _ => 0,
    }
}

impl Vm {
    pub(crate) fn radio_init(&mut self, w: u32) {
        self.set_o_pub(w, "currentSelection", 0);
        self.set_b_pub(w, "bOneCheck", true);
    }

    /// The nearest radio box at or above `w`.
    fn radio_box_of(&self, w: u32) -> u32 {
        let mut p = w;
        while p != 0 {
            if self.ui_kinds(p).contains(&Kind::Radio) {
                return p;
            }
            p = self.wo_pub(p, "winParent");
        }
        0
    }

    /// Is `t` one of the toggles of radio box `w`?
    fn radio_has(&self, w: u32, t: u32) -> bool {
        self.ui_kinds(t).contains(&Kind::Toggle) && self.radio_box_of(t) == w
    }

    pub(crate) fn radio_event(&mut self, w: u32, name: &str, args: &mut Vec<Value>) -> Option<Value> {
        match name {
            "ConfigurationChanged" => {
                let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
                let mut c = self.visible_child(w, true);
                while c != 0 {
                    self.ui_configure_child(c, 0.0, 0.0, width, height);
                    c = self.next_visible(c, false);
                }
                Some(Value::Void)
            }
            "ParentRequestedPreferredSize" => {
                let (w_spec, h_spec) = (args.first().and_then(Value::as_bool).unwrap_or(false), args.get(2).and_then(Value::as_bool).unwrap_or(false));
                let (in_w, in_h) = (args.get(1).and_then(Value::as_f32).unwrap_or(0.0), args.get(3).and_then(Value::as_f32).unwrap_or(0.0));
                let (mut max_w, mut max_h) = (0.0f32, 0.0f32);
                let mut c = self.visible_child(w, true);
                while c != 0 {
                    let (cw, ch) = self.ui_query_preferred_size(c, w_spec, in_w, h_spec, in_h);
                    max_w = max_w.max(cw);
                    max_h = max_h.max(ch);
                    c = self.next_visible(c, false);
                }
                args[1] = Value::Float(max_w);
                args[3] = Value::Float(max_h);
                Some(Value::Void)
            }
            "ChildRequestedVisibilityChange" => {
                let vis = args.get(1).and_then(Value::as_bool).unwrap_or(false);
                self.ui_set_child_visibility(ob(args, 0), vis);
                if !vis {
                    self.ui_ask_parent_for_reconfigure(w);
                }
                Some(Value::Void)
            }
            "ToggleChanged" => {
                let (t, on) = (ob(args, 0), args.get(1).and_then(Value::as_bool).unwrap_or(false));
                if self.ui_kinds(t).contains(&Kind::Toggle) {
                    let cur = self.wo_pub(w, "currentSelection");
                    if on {
                        if self.radio_has(w, t) {
                            if cur == t {
                                return Some(Value::Bool(true));
                            }
                            self.set_o_pub(w, "currentSelection", t);
                            if cur != 0 {
                                self.toggle_set(cur, false);
                            }
                        }
                    } else if self.wb_pub(w, "bOneCheck") {
                        // the one that is on cannot be switched off by clicking it
                        if cur == t {
                            self.toggle_set(t, true);
                        }
                        return Some(Value::Bool(true));
                    } else if cur == t {
                        self.set_o_pub(w, "currentSelection", 0);
                    } else {
                        return Some(Value::Bool(true));
                    }
                }
                let (r, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                Some(r)
            }
            "DescendantRemoved" => {
                let gone = ob(args, 0);
                if self.wo_pub(w, "currentSelection") == gone {
                    self.set_o_pub(w, "currentSelection", 0);
                }
                let (_, out) = self.ui_script_event(w, name, args.clone());
                *args = out;
                Some(Value::Void)
            }
            _ => None,
        }
    }

    pub(crate) fn native_radio(&mut self, this: u32, name: &str, _a: &mut Vec<Value>) -> Option<Res<Value>> {
        match name {
            "GetEnabledToggle" => Some(Ok(Value::Obj(self.wo_pub(this, "currentSelection")))),
            _ => None,
        }
    }
}

//! The script-callable natives of the window classes (`Window`, `RootWindow`, `ModalWindow`). See `ui.rs`.

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

/// `XWindow::ConvertScriptString`: the script's `|n` becomes a newline (at most 2047 characters are kept).
pub(crate) fn convert_script_string(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if out.chars().count() >= 0x7ff {
            break;
        }
        if c == '|' {
            match it.peek() {
                Some('n') => {
                    it.next();
                    out.push('\n');
                }
                Some(&o) => {
                    out.push('|');
                    out.push(o);
                    it.next();
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Vm {
    /// Dispatches the window natives; `None` when `name` is not one of them for the class of `this`.
    pub(crate) fn native_ui(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        use Value::*;
        if let Some(r) = self.native_gc(this, name, a) {
            return Some(r);
        }
        if !self.is_class(this, "Window") {
            return None;
        }
        if let Some(r) = self.native_tile(this, name, a) {
            return Some(r);
        }
        if self.ui_kinds(this).contains(&crate::ui_widgets::Kind::Clip) {
            if let Some(r) = self.native_clip(this, name, a) {
                return Some(r);
            }
        }
        if self.ui_kinds(this).contains(&crate::ui_widgets::Kind::Edit) {
            if let Some(r) = self.native_edit(this, name, a) {
                return Some(r);
            }
        }
        if self.ui_kinds(this).contains(&crate::ui_widgets::Kind::Viewport) {
            if let Some(r) = self.native_viewport(this, name, a) {
                return Some(r);
            }
        }
        if self.ui_kinds(this).contains(&crate::ui_widgets::Kind::Radio) {
            if let Some(r) = self.native_radio(this, name, a) {
                return Some(r);
            }
        }
        if self.ui_kinds(this).contains(&crate::ui_widgets::Kind::Computer) {
            if let Some(r) = self.native_computer(this, name, a) {
                return Some(r);
            }
        }
        if self.ui_kinds(this).contains(&crate::ui_widgets::Kind::List) {
            if let Some(r) = self.native_list(this, name, a) {
                return Some(r);
            }
        }
        if self.ui_kinds(this).contains(&crate::ui_widgets::Kind::ScrollArea) {
            if let Some(r) = self.native_scroll_area(this, name, a) {
                return Some(r);
            }
        }
        if self.ui_kinds(this).contains(&crate::ui_widgets::Kind::Scale) {
            if let Some(r) = self.native_scale(this, name, a) {
                return Some(r);
            }
        }
        if self.ui_kinds(this).contains(&crate::ui_widgets::Kind::ScaleManager) {
            if let Some(r) = self.native_scale_manager(this, name, a) {
                return Some(r);
            }
        }
        if let Some(r) = self.native_widgets(this, name, a) {
            return Some(r);
        }
        let v = match name {
            "Destroy" => {
                self.ui_safe_destroy(this);
                Void
            }
            "NewChild" => {
                let show = bl(a, 1, true);
                return Some(self.ui_create_window(ob(a, 0), this, show).map(Obj));
            }
            "Raise" | "Lower" => {
                let top = name == "Raise";
                let parent = self.wo_pub(this, "winParent");
                let edge = self.wo_pub(this, if top { "nextSibling" } else { "prevSibling" });
                if parent != 0 && edge != 0 {
                    self.ui_add_child_pub(parent, this, top);
                    self.ui_ask_parent_for_reconfigure(this);
                    // the stacking order decides the current modal: the focus may have to move
                    self.ui_check_focus(this);
                    self.ui_check_grabbed_window(this);
                }
                Void
            }
            "Show" => {
                self.ui_set_visibility(this, bl(a, 0, true));
                Void
            }
            "Hide" => {
                self.ui_set_visibility(this, false);
                Void
            }
            "IsVisible" => {
                // `bRecurse` defaults to true in the exec wrapper
                if bl(a, 0, true) {
                    Bool(self.ui_visible_recursive(this))
                } else {
                    Bool(self.wb_pub(this, "bIsVisible"))
                }
            }
            "SetChildVisibility" => {
                self.ui_set_child_visibility(this, bl(a, 0, false));
                Void
            }
            "SetSensitivity" => {
                self.ui_set_sensitivity_pub(this, bl(a, 0, true));
                Void
            }
            "EnableWindow" => {
                self.ui_set_sensitivity_pub(this, bl(a, 0, true));
                Void
            }
            "DisableWindow" => {
                self.ui_set_sensitivity_pub(this, false);
                Void
            }
            "IsSensitive" => {
                if bl(a, 0, true) {
                    Bool(self.ui_sensitive_recursive_pub(this))
                } else {
                    Bool(self.wb_pub(this, "bIsSensitive"))
                }
            }
            "SetSelectability" => {
                self.set_b_pub(this, "bIsSelectable", bl(a, 0, false));
                Void
            }
            "GetRootWindow" => Obj(self.ui_root_of_pub(this)),
            "GetParent" => Obj(self.wo_pub(this, "winParent")),
            "GetModalWindow" => {
                let mut cur = this;
                while cur != 0 && self.wi_pub(cur, "windowType") < 2 {
                    cur = self.wo_pub(cur, "winParent");
                }
                Obj(cur)
            }
            "GetTabGroupWindow" => {
                let mut cur = this;
                while cur != 0 && self.wi_pub(cur, "windowType") == 0 {
                    cur = self.wo_pub(cur, "winParent");
                }
                Obj(cur)
            }
            "GetPlayerPawn" => {
                let root = self.ui_root_of_pub(this);
                Obj(if root != 0 { self.wo_pub(root, "parentPawn") } else { 0 })
            }
            "GetTopChild" | "GetBottomChild" => {
                let only_visible = bl(a, 0, true);
                let (mut c, step) = if name == "GetTopChild" { (self.wo_pub(this, "lastChild"), "prevSibling") } else { (self.wo_pub(this, "firstChild"), "nextSibling") };
                while c != 0 && only_visible && !self.wb_pub(c, "bIsVisible") {
                    c = self.wo_pub(c, step);
                }
                Obj(c)
            }
            "GetHigherSibling" | "GetLowerSibling" => {
                let only_visible = bl(a, 0, true);
                let step = if name == "GetHigherSibling" { "nextSibling" } else { "prevSibling" };
                let mut c = self.wo_pub(this, step);
                while c != 0 && only_visible && !self.wb_pub(c, "bIsVisible") {
                    c = self.wo_pub(c, step);
                }
                Obj(c)
            }
            "DestroyAllChildren" => {
                let mut c = self.wo_pub(this, "firstChild");
                while c != 0 {
                    let next = self.wo_pub(c, "nextSibling");
                    self.ui_safe_destroy(c);
                    c = next;
                }
                Void
            }
            // ---- geometry
            "SetConfiguration" => {
                let (x, y) = (fl(a, 0, 0.0), fl(a, 1, 0.0));
                let (w, h) = (fl(a, 2, 0.0).max(0.0), fl(a, 3, 0.0).max(0.0));
                let same = self.wi_pub(this, "winHAlign") == 0 && self.wi_pub(this, "winVAlign") == 0 && x == self.wf_pub(this, "hMargin0") && y == self.wf_pub(this, "vMargin0") && w == self.wf_pub(this, "hardcodedWidth") && h == self.wf_pub(this, "hardcodedHeight");
                if !same {
                    self.set_align(this, 0, 0);
                    self.set_f_pub(this, "hMargin0", x);
                    self.set_f_pub(this, "vMargin0", y);
                    self.set_f_pub(this, "hardcodedWidth", w);
                    self.set_f_pub(this, "hardcodedHeight", h);
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "SetPos" => {
                let (x, y) = (fl(a, 0, 0.0), fl(a, 1, 0.0));
                let same = self.wi_pub(this, "winHAlign") == 0 && self.wi_pub(this, "winVAlign") == 0 && x == self.wf_pub(this, "hMargin0") && y == self.wf_pub(this, "vMargin0");
                if !same {
                    self.set_align(this, 0, 0);
                    self.set_f_pub(this, "hMargin0", x);
                    self.set_f_pub(this, "vMargin0", y);
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "SetSize" => {
                let (w, h) = (fl(a, 0, 0.0).max(0.0), fl(a, 1, 0.0).max(0.0));
                if w != self.wf_pub(this, "hardcodedWidth") || h != self.wf_pub(this, "hardcodedHeight") {
                    self.set_f_pub(this, "hardcodedWidth", w);
                    self.set_f_pub(this, "hardcodedHeight", h);
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "SetWidth" | "SetHeight" => {
                let key = if name == "SetWidth" { "hardcodedWidth" } else { "hardcodedHeight" };
                let v = fl(a, 0, 0.0).max(0.0);
                if v != self.wf_pub(this, key) {
                    self.set_f_pub(this, key, v);
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "ResetSize" | "ResetWidth" | "ResetHeight" => {
                let keys: &[&str] = match name {
                    "ResetSize" => &["hardcodedWidth", "hardcodedHeight"],
                    "ResetWidth" => &["hardcodedWidth"],
                    _ => &["hardcodedHeight"],
                };
                if keys.iter().any(|k| self.wf_pub(this, k) >= 0.0) {
                    for k in keys {
                        self.set_f_pub(this, k, -1.0);
                    }
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "SetWindowAlignments" => {
                let (ha, va) = (a.first().and_then(Value::as_i32).unwrap_or(0), a.get(1).and_then(Value::as_i32).unwrap_or(0));
                let (hm0, vm0, hm1, vm1) = (fl(a, 2, 0.0), fl(a, 3, 0.0), fl(a, 4, 0.0), fl(a, 5, 0.0));
                let same = ha == self.wi_pub(this, "winHAlign") && va == self.wi_pub(this, "winVAlign") && hm0 == self.wf_pub(this, "hMargin0") && hm1 == self.wf_pub(this, "hMargin1") && vm0 == self.wf_pub(this, "vMargin0") && vm1 == self.wf_pub(this, "vMargin1");
                if !same {
                    self.set_align(this, ha, va);
                    for (k, v) in [("hMargin0", hm0), ("vMargin0", vm0), ("hMargin1", hm1), ("vMargin1", vm1)] {
                        self.set_f_pub(this, k, v);
                    }
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "AskParentForReconfigure" => {
                self.ui_ask_parent_for_reconfigure(this);
                Void
            }
            "ConfigureChild" => {
                self.ui_configure_child(this, fl(a, 0, 0.0), fl(a, 1, 0.0), fl(a, 2, 0.0), fl(a, 3, 0.0));
                Void
            }
            "ResizeChild" => {
                self.ui_resize_child(this);
                Void
            }
            "QueryPreferredWidth" => {
                let h = fl(a, 0, 0.0);
                Float(self.ui_query_preferred_size(this, false, 0.0, true, h).0)
            }
            "QueryPreferredHeight" => {
                let w = fl(a, 0, 0.0);
                Float(self.ui_query_preferred_size(this, true, w, false, 0.0).1)
            }
            "QueryPreferredSize" => {
                let (w, h) = self.ui_query_preferred_size(this, false, 0.0, false, 0.0);
                a[0] = Float(w);
                a[1] = Float(h);
                Void
            }
            "QueryGranularity" => {
                let (h, v) = self.ui_query_granularity(this);
                a[0] = Float(h);
                a[1] = Float(v);
                Void
            }
            // ---- look
            "SetBackground" => {
                let t = ob(a, 0);
                if t != self.wo_ref(this, "background") {
                    self.set_o_pub(this, "background", t);
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "SetBackgroundStyle" => {
                let st = a.first().and_then(Value::as_i32).unwrap_or(0);
                let _ = self.set_prop(this, "backgroundStyle", 0, Byte(st as u8));
                let g = self.wo_ref_pub(this, "wGc");
                self.gc_mut(g).set_style(st as u8);
                Void
            }
            "SetBackgroundSmoothing" => {
                self.set_b_pub(this, "bSmoothBackground", bl(a, 0, false));
                let g = self.wo_ref_pub(this, "wGc");
                self.gc_mut(g).enable_smoothing(bl(a, 0, false));
                Void
            }
            "SetBackgroundStretching" => {
                self.set_b_pub(this, "bStretchBackground", bl(a, 0, false));
                Void
            }
            "SetTextColor" | "SetTileColor" => {
                if let Some(Struct(_, b)) = a.first() {
                    if b.len() >= 4 {
                        let rgba = [b[0], b[1], b[2], b[3]];
                        let (c, p) = if name == "SetTextColor" { ("textColor", "textPlane") } else { ("tileColor", "tilePlane") };
                        self.ui_set_color_pub(this, c, p, rgba);
                        let g = self.wo_ref_pub(this, "wGc");
                        if name == "SetTextColor" {
                            self.gc_mut(g).text_color = rgba;
                        } else {
                            self.gc_mut(g).tile_color = rgba;
                        }
                    }
                }
                Void
            }
            "SetFont" | "SetFonts" | "SetNormalFont" | "SetBoldFont" => {
                let (n, b) = (ob(a, 0), if name == "SetFonts" { ob(a, 1) } else { ob(a, 0) });
                match name {
                    "SetBoldFont" => self.set_o_pub(this, "boldFont", ob(a, 0)),
                    "SetNormalFont" => self.set_o_pub(this, "normalFont", ob(a, 0)),
                    _ => {
                        self.set_o_pub(this, "normalFont", n);
                        self.set_o_pub(this, "boldFont", b);
                    }
                }
                self.ui_sync_gc_fonts(this);
                self.ui_ask_parent_for_reconfigure(this);
                Void
            }
            "EnableSpecialText" => {
                let b = bl(a, 0, true);
                if self.wb_pub(this, "bSpecialText") != b {
                    self.set_b_pub(this, "bSpecialText", b);
                    let g = self.wo_ref_pub(this, "wGc");
                    self.gc_mut(g).parse_meta = b;
                    self.ui_ask_parent_for_reconfigure(this);
                }
                Void
            }
            "EnableTranslucentText" => {
                let b = bl(a, 0, true);
                self.set_b_pub(this, "bTextTranslucent", b);
                let g = self.wo_ref_pub(this, "wGc");
                self.gc_mut(g).enable_translucent_text(b);
                Void
            }
            "SetBaselineData" => {
                let g = self.wo_ref_pub(this, "wGc");
                let st = self.gc_mut(g);
                st.baseline_offset = fl(a, 0, 2.0);
                st.underline_height = fl(a, 1, 1.0);
                Void
            }
            "PlaySound" => {
                // PlaySound(sound, optional Volume = -1, optional Pitch = 1, posX, posY); the original leaves an omitted
                // point uninitialised (stack leftovers): the window's centre here
                let (cx, cy) = (self.wf_pub(this, "width") * 0.5, self.wf_pub(this, "height") * 0.5);
                let given = |a: &Vec<Value>, i: usize| !matches!(a.get(i), None | Some(Void));
                let x = if given(a, 3) { fl(a, 3, cx) } else { cx };
                let y = if given(a, 4) { fl(a, 4, cy) } else { cy };
                self.ui_play_sound_at(this, ob(a, 0), fl(a, 1, -1.0), fl(a, 2, 1.0), x, y);
                Void
            }
            "SetSoundVolume" => {
                self.set_f_pub(this, "soundVolume", fl(a, 0, 0.0));
                Void
            }
            "SetFocusSounds" => {
                self.set_o_pub(this, "focusSound", ob(a, 0));
                self.set_o_pub(this, "unfocusSound", ob(a, 1));
                Void
            }
            "SetVisibilitySounds" => {
                self.set_o_pub(this, "visibleSound", ob(a, 0));
                self.set_o_pub(this, "invisibleSound", ob(a, 1));
                Void
            }
            "SetAcceleratorText" => {
                let t = match a.first() {
                    Some(Str(s)) => Some(s.clone()),
                    _ => None,
                };
                self.ui_set_accelerator_text(this, t.as_deref());
                Void
            }
            // `XWindow::ChangeStyle` (10050310): `StyleChanged` for the root and every window under it, top child first
            "ChangeStyle" => {
                let root = self.ui_root_of_pub(this);
                if root != 0 {
                    self.ui_change_style_tree(root);
                }
                Void
            }
            // `XWindow::AddActorRef`/`RemoveActorRef`: the player's list of actors windows refer to
            "AddActorRef" | "RemoveActorRef" => {
                let pp = self.window_player(this);
                if name == "AddActorRef" {
                    self.add_actor_ref(pp, ob(a, 0));
                } else {
                    self.remove_actor_ref(pp, ob(a, 0));
                }
                Void
            }
            "CarriageReturn" => Str("\n".into()),
            "ConvertScriptString" => Str(convert_script_string(match a.first() {
                Some(Str(s)) => s,
                _ => "",
            })),
            "SetClientObject" => {
                self.set_o_pub(this, "clientObject", ob(a, 0));
                Void
            }
            "GetClientObject" => Obj(self.wo_pub(this, "clientObject")),
            "SetFocusWindow" => {
                let t = ob(a, 0);
                Bool(self.ui_set_focus_window(this, t))
            }
            "GetFocusWindow" => {
                let root = self.ui_root_of_pub(this);
                Obj(if root != 0 { self.wo_pub(root, "focusWindow") } else { 0 })
            }
            "IsFocusWindow" => {
                let root = self.ui_root_of_pub(this);
                Bool(root != 0 && self.wo_pub(root, "focusWindow") == this)
            }
            "MoveFocusLeft" => Obj(self.ui_move_focus(this, 0)),
            "MoveFocusRight" => Obj(self.ui_move_focus(this, 1)),
            "MoveFocusUp" => Obj(self.ui_move_focus(this, 2)),
            "MoveFocusDown" => Obj(self.ui_move_focus(this, 3)),
            "MoveTabGroupNext" => Obj(self.ui_move_tab_group(this, 3)),
            "MoveTabGroupPrev" => Obj(self.ui_move_tab_group(this, 2)),
            "GrabMouse" => {
                self.ui_grab_mouse(this);
                Void
            }
            "UngrabMouse" => {
                self.ui_ungrab_mouse(this);
                Void
            }
            "IsKeyDown" => {
                let root = self.ui_root_of_pub(this);
                // keys outside the map are never down
                let key = a.first().and_then(Value::as_i32).unwrap_or(0);
                Bool(root != 0 && (0..255).contains(&key) && matches!(self.get_prop(root, "keyDownMap", key as u32), Ok(Byte(b)) if b != 0))
            }
            "GetCursorPos" => {
                let root = self.ui_root_of_pub(this);
                let (mx, my) = if root != 0 { (self.wf_pub(root, "mouseX"), self.wf_pub(root, "mouseY")) } else { (0.0, 0.0) };
                let g = self.wo_ref_pub(this, "wGc");
                let o = self.gc_state(g).clip;
                a[0] = Float(mx - o[0]);
                a[1] = Float(my - o[1]);
                Void
            }
            "SetCursorPos" => {
                let root = self.ui_root_of_pub(this);
                if root != 0 {
                    let g = self.wo_ref_pub(this, "wGc");
                    let o = self.gc_state(g).clip;
                    // `SetRootCursorPos` (10039d80): moved, clamped and processed at once (enter/leave, `MouseMoved`),
                    // whatever `bMouseMoveLocked` says
                    self.set_f_pub(root, "mouseX", fl(a, 0, 0.0) + o[0]);
                    self.set_f_pub(root, "mouseY", fl(a, 1, 0.0) + o[1]);
                    self.set_b_pub(root, "bMouseMoved", true);
                    self.ui_clamp_mouse(root);
                    self.ui_process_mouse_move(root);
                }
                Void
            }
            "IsPointInWindow" => Bool(self.ui_is_point_in_window(this, fl(a, 0, 0.0), fl(a, 1, 0.0))),
            "FindWindow" => {
                let root = self.ui_root_of_pub(this);
                let (x, y) = (fl(a, 0, 0.0), fl(a, 1, 0.0));
                let mut found = (0u32, 0.0f32, 0.0f32);
                if root != 0 {
                    let g = self.wo_ref_pub(this, "wGc");
                    let o = self.gc_state(g).clip;
                    let rg = self.wo_ref_pub(root, "wGc");
                    let ro = self.gc_state(rg).clip;
                    let (rx, ry) = (x + o[0] - ro[0], y + o[1] - ro[1]);
                    // the topmost modal window (or the root) is searched
                    let mut m = self.wo_pub(root, "lastChild");
                    let mut modal = root;
                    while m != 0 {
                        if self.wi_pub(m, "windowType") > 1 {
                            modal = m;
                            break;
                        }
                        m = self.wo_pub(m, "prevSibling");
                    }
                    let mg = self.wo_ref_pub(modal, "wGc");
                    let mo = self.gc_state(mg).clip;
                    if let Some(h) = self.ui_find_window_by_point(modal, rx + ro[0] - mo[0], ry + ro[1] - mo[1]) {
                        found = h;
                    }
                }
                a[2] = Float(found.1);
                a[3] = Float(found.2);
                Obj(found.0)
            }
            "ConvertCoordinates" => {
                let (from, to) = (ob(a, 0), ob(a, 3));
                let (fg, tg) = (self.wo_ref_pub(from, "wGc"), self.wo_ref_pub(to, "wGc"));
                let (fo, to_) = (self.gc_state(fg).clip, self.gc_state(tg).clip);
                a[4] = Float(fl(a, 1, 0.0) + fo[0] - to_[0]);
                a[5] = Float(fl(a, 2, 0.0) + fo[1] - to_[1]);
                Void
            }
            "AskParentToShowArea" => {
                self.ui_ask_parent_to_show_area(this, fl(a, 0, 0.0), fl(a, 1, 0.0), fl(a, 2, 0.0), fl(a, 3, 0.0));
                Void
            }
            "AddTimer" => {
                let function = match a.get(3) {
                    Some(Name(n)) => *n,
                    _ => 0,
                };
                Int(self.ui_add_timer(this, fl(a, 0, 0.0), bl(a, 1, false), a.get(2).and_then(Value::as_i32).unwrap_or(0), function))
            }
            "RemoveTimer" => {
                self.ui_remove_timer(this, a.first().and_then(Value::as_i32).unwrap_or(0));
                Void
            }
            "GetTickOffset" => Float(0.0),
            // `XWindow::IsActorValid` (1004ff20): in the player's list of referred actors
            "IsActorValid" => {
                let pp = self.window_player(this);
                Bool(self.is_actor_ref_valid(pp, ob(a, 0)))
            }
            "SetDefaultCursor" => {
                self.set_o_pub(this, "defaultCursor", ob(a, 0));
                self.set_o_pub(this, "defaultCursorShadow", ob(a, 1));
                self.set_f_pub(this, "defaultHotX", fl(a, 2, -1.0));
                self.set_f_pub(this, "defaultHotY", fl(a, 3, -1.0));
                if let Some(Struct(_, b)) = a.get(4) {
                    if b.len() >= 4 {
                        let _ = self.set_prop(this, "defaultCursorColor", 0, a[4].clone());
                    }
                }
                Void
            }
            // ---- root and modal windows
            "SetMouseFocusMode" => {
                let _ = self.set_prop(this, "focusMode", 0, Byte(a.first().and_then(Value::as_i32).unwrap_or(0) as u8));
                Void
            }
            "IsCurrentModal" => {
                let root = self.ui_root_of_pub(this);
                Bool(root != 0 && self.ui_current_modal(root) == this)
            }
            "SetDefaultEditCursor" => {
                self.set_o_pub(this, "defaultEditCursor", ob(a, 0));
                Void
            }
            "SetDefaultMovementCursors" => {
                for (i, k) in ["defaultMoveCursor", "defaultHorizontalMoveCursor", "defaultVerticalMoveCursor", "defaultTopLeftMoveCursor", "defaultTopRightMoveCursor"].iter().enumerate() {
                    self.set_o_pub(this, k, ob(a, i));
                }
                Void
            }
            "EnableRendering" => {
                self.set_b_pub(this, "bRender", bl(a, 0, true));
                Void
            }
            "IsRenderingEnabled" => Bool(self.wb_pub(this, "bRender")),
            "SetRenderViewport" => {
                self.set_b_pub(this, "bClipRender", true);
                for (i, k) in ["renderX", "renderY", "renderWidth", "renderHeight"].iter().enumerate() {
                    self.set_f_pub(this, k, fl(a, i, 0.0));
                }
                Void
            }
            "ResetRenderViewport" => {
                self.set_b_pub(this, "bClipRender", false);
                Void
            }
            "SetRawBackground" => {
                // `execSetRawBackground` (1003cbe0): the colour defaults to white (its alpha byte is not written)
                self.set_o_pub(this, "rawBackground", ob(a, 0));
                match a.get(1) {
                    Some(c @ Struct(..)) => {
                        let _ = self.set_prop(this, "rawColor", 0, c.clone());
                    }
                    _ => {
                        if let Ok(Struct(id, mut b)) = self.get_prop(this, "rawColor", 0) {
                            for k in 0..3.min(b.len()) {
                                b[k] = 255;
                            }
                            let _ = self.set_prop(this, "rawColor", 0, Struct(id, b));
                        }
                    }
                }
                Void
            }
            "SetRawBackgroundSize" => {
                self.set_f_pub(this, "rawBackgroundWidth", fl(a, 0, 0.0));
                self.set_f_pub(this, "rawBackgroundHeight", fl(a, 1, 0.0));
                Void
            }
            "StretchRawBackground" => {
                self.set_b_pub(this, "bStretchRawBackground", bl(a, 0, true));
                Void
            }
            "EnablePositionalSound" => {
                self.set_b_pub(this, "bPositionalSound", bl(a, 0, true));
                Void
            }
            "IsPositionalSoundEnabled" => Bool(self.wb_pub(this, "bPositionalSound")),
            "LockMouse" => {
                self.set_b_pub(this, "bMouseMoveLocked", bl(a, 0, true));
                self.set_b_pub(this, "bMouseButtonLocked", bl(a, 1, true));
                Void
            }
            "ShowCursor" => {
                self.set_b_pub(this, "bCursorVisible", bl(a, 0, true));
                Void
            }
            "SetSnapshotSize" => {
                self.set_snapshot_size(this, fl(a, 0, 0.0), fl(a, 1, 0.0));
                Void
            }
            // `execGenerateSnapshot` (1003cf30): `bFilter` goes to a parameter the C++ never reads; always the greyscale
            // picture of the last frame (filled by the host before the next one, see `snapshot.rs`)
            "GenerateSnapshot" => Obj(self.request_snapshot(this, None)),
            "ConvertVectorToCoordinates" => {
                let p = match a.first() {
                    Some(Vector(v)) => *v,
                    _ => [0.0; 3],
                };
                let (ok, x, y) = self.ui_convert_vector(this, p);
                a[1] = Float(x);
                a[2] = Float(y);
                Bool(ok)
            }
            "InitRootWindow" if self.is_class(this, "PlayerPawnExt") => return None,
            _ => return None,
        };
        Some(Ok(v))
    }

    fn set_align(&mut self, w: u32, h: i32, v: i32) {
        let _ = self.set_prop(w, "winHAlign", 0, Value::Byte(h as u8));
        let _ = self.set_prop(w, "winVAlign", 0, Value::Byte(v as u8));
    }
}

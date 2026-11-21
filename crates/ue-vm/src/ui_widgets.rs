//! The native widget classes of the window system: what their C++ overrides do (initial state, layout, visibility rules)
//! and their script-callable setters. A script class derived from a native widget (`HUDObjectBelt extends TileWindow`)
//! gets the native behaviour; the script events of the derived class run where the original native code runs them.

use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    LargeText,
    TabGroup,
    Modal,
    Root,
    Tile,
    Text,
    TextLog,
    Button,
    Toggle,
    Checkbox,
    Border,
    Clip,
    ScaleManager,
    Scale,
    ScrollArea,
    List,
    Edit,
    Computer,
    Radio,
    Viewport,
}

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
fn iv(a: &[Value], i: usize) -> i32 {
    a.get(i).and_then(Value::as_i32).unwrap_or(0)
}

impl Vm {
    /// The native widget classes `w` derives from, base first.
    pub(crate) fn ui_kinds(&self, w: u32) -> Vec<Kind> {
        let mut out = Vec::new();
        let mut cur = self.class_of(w);
        while let Some(c) = cur {
            let kind = match self.reg.names.str(self.reg.class(c).name) {
                "TabGroupWindow" => Some(Kind::TabGroup),
                "ModalWindow" => Some(Kind::Modal),
                "RootWindow" => Some(Kind::Root),
                "TileWindow" => Some(Kind::Tile),
                "TextWindow" => Some(Kind::Text),
                "LargeTextWindow" => Some(Kind::LargeText),
                "TextLogWindow" => Some(Kind::TextLog),
                "ButtonWindow" => Some(Kind::Button),
                "ToggleWindow" => Some(Kind::Toggle),
                "CheckboxWindow" => Some(Kind::Checkbox),
                "BorderWindow" => Some(Kind::Border),
                "ClipWindow" => Some(Kind::Clip),
                "ScaleManagerWindow" => Some(Kind::ScaleManager),
                "ScaleWindow" => Some(Kind::Scale),
                "ScrollAreaWindow" => Some(Kind::ScrollArea),
                "ListWindow" => Some(Kind::List),
                "EditWindow" => Some(Kind::Edit),
                "ComputerWindow" => Some(Kind::Computer),
                "RadioBoxWindow" => Some(Kind::Radio),
                "ViewportWindow" => Some(Kind::Viewport),
                _ => None,
            };
            if let Some(k) = kind {
                out.insert(0, k);
            }
            cur = self.reg.class(c).super_id;
        }
        out
    }

    /// `XTabGroupWindow::ParentRequestedPreferredSize` (Extension 10045700): with `bSizeParentToChildren` or
    /// `bSizeChildrenToParent`, the largest preferred size of the visible children (with their margins unless the
    /// children follow the parent), 0 x 0 without children; otherwise `None` (`XWindow`'s, the script event's).
    pub(crate) fn tab_group_preferred(&mut self, w: u32, w_spec: bool, in_w: f32, h_spec: bool, in_h: f32) -> Option<(f32, f32)> {
        if !self.wb_pub(w, "bSizeParentToChildren") && !self.wb_pub(w, "bSizeChildrenToParent") {
            return None;
        }
        let (mut max_w, mut max_h) = (0.0f32, 0.0f32);
        let to_parent = self.wb_pub(w, "bSizeChildrenToParent");
        let mut c = self.visible_child(w, true);
        while c != 0 {
            let (mut cw, mut ch) = self.ui_query_preferred_size(c, w_spec, in_w, h_spec, in_h);
            if !to_parent {
                cw += self.wf_pub(c, "hMargin0");
                ch += self.wf_pub(c, "vMargin0");
                if self.wi_pub(c, "winHAlign") == 3 {
                    cw += self.wf_pub(c, "hMargin1");
                }
                if self.wi_pub(c, "winVAlign") == 3 {
                    ch += self.wf_pub(c, "vMargin1");
                }
            }
            max_w = max_w.max(cw);
            max_h = max_h.max(ch);
            c = self.next_visible(c, false);
        }
        Some((max_w, max_h))
    }

    /// The initial state the native constructors give (after `XWindow::Init`).
    pub(crate) fn ui_native_init(&mut self, w: u32, parent: u32) {
        for k in self.ui_kinds(w) {
            match k {
                Kind::TabGroup => {
                    self.set_i_pub(w, "windowType", 1);
                    self.set_i_pub(w, "tabGroupIndex", -1);
                    self.set_f_pub(w, "firstAbsX", 0.0);
                    self.set_f_pub(w, "firstAbsY", 0.0);
                    self.set_b_pub(w, "bSizeParentToChildren", true);
                    self.set_b_pub(w, "bSizeChildrenToParent", false);
                }
                Kind::Modal => {
                    if parent == 0 || self.wi_pub(parent, "windowType") == 3 {
                        self.set_i_pub(w, "windowType", 2);
                    } else {
                        self.log.push("a ModalWindow must be a child of the root window".into());
                    }
                    let _ = self.set_prop(w, "focusMode", 0, Value::Byte(2));
                    self.set_o_pub(w, "preferredFocus", 0);
                    self.set_b_pub(w, "bDirtyAccelerators", true);
                }
                Kind::Root => {}
                Kind::Text => {
                    for (n, v) in [("hMargin", 3.0), ("vMargin", 3.0), ("minWidth", 0.0)] {
                        self.set_f_pub(w, n, v);
                    }
                    let _ = self.set_prop(w, "hAlign", 0, Value::Byte(1));
                    let _ = self.set_prop(w, "vAlign", 0, Value::Byte(1));
                    self.set_b_pub(w, "bWordWrap", true);
                    self.set_i_pub(w, "minLines", -1);
                    self.set_i_pub(w, "maxLines", -1);
                    let _ = self.set_prop(w, "text", 0, Value::Str(String::new()));
                    self.set_b_pub(w, "bTextIsAccelerator", true);
                }
                Kind::TextLog => {
                    let _ = self.set_prop(w, "hAlign", 0, Value::Byte(0));
                    let _ = self.set_prop(w, "vAlign", 0, Value::Byte(0));
                    self.set_f_pub(w, "textTimeout", 5.0);
                    self.set_b_pub(w, "bTooTall", false);
                    self.ui.logs.remove(&w);
                }
                Kind::Button => {
                    for n in ["bButtonPressed", "bAutoRepeat", "bMousePressed"] {
                        self.set_b_pub(w, n, false);
                    }
                    for (n, v) in [("activateDelay", 0.3), ("initialDelay", 0.5), ("repeatRate", 0.1), ("repeatTime", 0.0)] {
                        self.set_f_pub(w, n, v);
                    }
                    self.set_i_pub(w, "activateTimer", 0);
                    for n in ["pressSound", "clickSound", "curTexture"] {
                        self.set_o_pub(w, n, 0);
                    }
                    self.ui.buttons.insert(w, ButtonInfo::default());
                    let white = self.color_value_pub([255, 255, 255, 255]);
                    for n in ["curTextColor", "curTileColor"] {
                        let _ = self.set_prop(w, n, 0, white.clone());
                    }
                    self.set_b_pub(w, "bIsSelectable", true);
                }
                Kind::Toggle => {
                    self.set_o_pub(w, "enableSound", 0);
                    self.set_o_pub(w, "disableSound", 0);
                }
                Kind::Checkbox => {
                    self.set_f_pub(w, "textureWidth", 0.0);
                    self.set_f_pub(w, "textureHeight", 0.0);
                    self.set_b_pub(w, "bRightSide", false);
                    self.set_f_pub(w, "checkboxSpacing", 3.0);
                    let _ = self.set_prop(w, "checkboxStyle", 0, Value::Byte(2));
                    let white = self.color_value_pub([255, 255, 255, 255]);
                    let _ = self.set_prop(w, "checkboxColor", 0, white);
                }
                Kind::Clip => self.clip_init(w),
                Kind::ScaleManager => self.scale_manager_init(w),
                Kind::Scale => self.scale_init(w),
                Kind::ScrollArea => self.scroll_area_init(w),
                Kind::List => self.list_init(w),
                Kind::Edit => self.edit_init(w),
                // `XLargeTextWindow::Init`: one unit between lines
                Kind::LargeText => self.set_f_pub(w, "vSpace", 1.0),
                Kind::Computer => self.computer_init(w),
                Kind::Radio => self.radio_init(w),
                Kind::Viewport => self.viewport_init(w, parent),
                Kind::Border => {
                    for n in ["borderLeft", "borderTopLeft", "borderTop", "borderTopRight", "borderRight", "borderBottomRight", "borderBottom", "borderBottomLeft", "center", "moveCursor", "hMoveCursor", "vMoveCursor", "tlMoveCursor", "trMoveCursor"] {
                        self.set_o_pub(w, n, 0);
                    }
                    for n in ["bSmoothBorder", "bResizeable", "bStretchBorder", "bLeftDrag", "bRightDrag", "bUpDrag", "bDownDrag"] {
                        self.set_b_pub(w, n, false);
                    }
                    self.set_b_pub(w, "bMarginsFromBorder", true);
                    for n in ["leftMargin", "rightMargin", "topMargin", "bottomMargin", "childLeftMargin", "childRightMargin", "childTopMargin", "childBottomMargin", "lastMouseX", "lastMouseY", "dragX", "dragY", "dragWidth", "dragHeight"] {
                        self.set_f_pub(w, n, 0.0);
                    }
                    let _ = self.set_prop(w, "borderStyle", 0, Value::Byte(1));
                    self.border_compute_margins(w);
                }
                Kind::Tile => {
                    for (n, v) in [("hMargin", 3.0), ("vMargin", 3.0), ("minorSpacing", 1.0), ("majorSpacing", 1.0)] {
                        self.set_f_pub(w, n, v);
                    }
                    for (n, v) in [("bWrap", true), ("bEqualHeight", true), ("bEqualWidth", false), ("bFillParent", true)] {
                        self.set_b_pub(w, n, v);
                    }
                    for (n, v) in [("orientation", 0u8), ("hDirection", 0), ("vDirection", 0), ("hChildAlign", 3), ("vChildAlign", 3)] {
                        let _ = self.set_prop(w, n, 0, Value::Byte(v));
                    }
                }
            }
        }
    }

    /// A point of the window in root coordinates (`ConvertCoordinates` to the root), from the origins of the last
    /// `ClipTree`.
    fn border_to_root(&self, w: u32, x: f32, y: f32) -> (f32, f32) {
        let root = self.ui_root_of_pub(w);
        let (wg, rg) = (self.wo_ref_pub(w, "wGc"), if root != 0 { self.wo_ref_pub(root, "wGc") } else { 0 });
        let o = if wg != 0 { self.gc_state(wg).clip } else { [0.0; 6] };
        let r = if rg != 0 { self.gc_state(rg).clip } else { [0.0; 6] };
        (x + o[0] - r[0], y + o[1] - r[1])
    }

    /// Native work that precedes the script event of the same name (the script then sees the changed arguments).
    pub(crate) fn ui_native_pre_event(&mut self, w: u32, name: &str, args: &mut Vec<Value>) {
        if name == "ParentRequestedGranularity" && self.ui_kinds(w).contains(&Kind::Text) {
            // `XLargeTextWindow::ParentRequestedGranularity`: a line and the space after it
            let h = if self.ui_kinds(w).contains(&Kind::LargeText) { self.large_line_height(w) + self.wf_pub(w, "vSpace") } else { self.text_granularity(w) };
            args[0] = Value::Float(1.0);
            args[1] = Value::Float(h);
        }
    }

    /// The native override of a window event, if the window's native classes have one. `args` are updated for out
    /// parameters. `None` = no override: the script event runs.
    pub(crate) fn ui_native_event(&mut self, w: u32, name: &str, args: &mut Vec<Value>) -> Option<Value> {
        let kinds = self.ui_kinds(w);
        // A child asking for a new size must not place itself in windows that lay out their children natively: their
        // native answer is "not handled" (the script default, which resizes the child alone at its unconstrained preferred
        // size, never runs), so the request climbs on and the window itself is reconfigured.
        if name == "ChildRequestedReconfiguration"
            && kinds.last().is_some_and(|k| {
                matches!(k, Kind::Border | Kind::TabGroup | Kind::Modal | Kind::Root | Kind::Clip | Kind::Tile | Kind::Radio | Kind::ScaleManager | Kind::ScrollArea)
            })
        {
            return Some(Value::Bool(false));
        }
        // the most derived native class with an override wins
        for k in kinds.iter().rev() {
            if *k == Kind::LargeText && name == "ParentRequestedPreferredSize" {
                let (mut pw, mut ph) = (fl(args, 1, 0.0), fl(args, 3, 0.0));
                if self.large_text_preferred_size(w, bl(args, 0, false), &mut pw, bl(args, 2, false), &mut ph) {
                    args[1] = Value::Float(pw);
                    args[3] = Value::Float(ph);
                    return Some(Value::Void);
                }
            }
            if *k == Kind::TextLog {
                match name {
                    "Tick" => {
                        let dt = fl(args, 0, 0.0);
                        self.text_log_tick(w, dt);
                    }
                    // the native part, then the script's event as the original's `ProcessScript`
                    "ConfigurationChanged" => self.text_log_configured(w),
                    _ => {}
                }
            }
            if *k == Kind::Clip {
                if let Some(v) = self.clip_event(w, name, args) {
                    return Some(v);
                }
            }
            if *k == Kind::Edit {
                if let Some(v) = self.edit_event(w, name, args) {
                    return Some(v);
                }
            }
            if *k == Kind::Radio {
                if let Some(v) = self.radio_event(w, name, args) {
                    return Some(v);
                }
            }
            if *k == Kind::Computer {
                if let Some(v) = self.computer_event(w, name, args) {
                    return Some(v);
                }
            }
            if *k == Kind::List {
                if let Some(v) = self.list_event(w, name, args) {
                    return Some(v);
                }
            }
            if *k == Kind::ScrollArea {
                if let Some(v) = self.scroll_area_event(w, name, args) {
                    return Some(v);
                }
            }
            if *k == Kind::Scale {
                if let Some(v) = self.scale_event(w, name, args) {
                    return Some(v);
                }
            }
            if *k == Kind::ScaleManager {
                if let Some(v) = self.scale_manager_event(w, name, args) {
                    return Some(v);
                }
            }
            match (k, name) {
                (Kind::Tile, "ParentRequestedPreferredSize") => {
                    let (mut pw, mut ph) = (fl(args, 1, 0.0), fl(args, 3, 0.0));
                    self.tile_compute_child_sizes(w, bl(args, 0, false), &mut pw, bl(args, 2, false), &mut ph);
                    args[1] = Value::Float(pw);
                    args[3] = Value::Float(ph);
                    return Some(Value::Void);
                }
                (Kind::Tile, "ConfigurationChanged") => {
                    let (mut pw, mut ph) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
                    self.tile_compute_child_sizes(w, true, &mut pw, true, &mut ph);
                    let mut c = self.visible_child(w, false);
                    while c != 0 {
                        let (x, y, cw, ch) = (self.wf_pub(c, "holdX"), self.wf_pub(c, "holdY"), self.wf_pub(c, "holdWidth"), self.wf_pub(c, "holdHeight"));
                        self.ui_configure_child(c, x, y, cw, ch);
                        c = self.next_visible(c, true);
                    }
                    return Some(Value::Void);
                }
                (Kind::Tile, "ChildRequestedVisibilityChange") => {
                    let child = match args.first() {
                        Some(Value::Obj(c)) => *c,
                        _ => 0,
                    };
                    self.ui_set_child_visibility(child, bl(args, 1, false));
                    self.ui_ask_parent_for_reconfigure(w);
                    return Some(Value::Void);
                }
                (Kind::Tile, "ParentRequestedGranularity") => {
                    let (mut gh, mut gv) = (1.0, 1.0);
                    let top = self.visible_child(w, true);
                    if top != 0 {
                        let vertical = self.wi_pub(w, "orientation") == 1;
                        if self.wb_pub(w, "bEqualWidth") {
                            gh = (if vertical { self.wf_pub(w, "majorSpacing") } else { self.wf_pub(w, "minorSpacing") }) + self.wf_pub(top, "width");
                        }
                        if self.wb_pub(w, "bEqualHeight") {
                            gv = (if vertical { self.wf_pub(w, "minorSpacing") } else { self.wf_pub(w, "majorSpacing") }) + self.wf_pub(top, "height");
                        }
                    }
                    args[0] = Value::Float(gh);
                    args[1] = Value::Float(gv);
                    return Some(Value::Void);
                }
                (Kind::TabGroup | Kind::Modal | Kind::Root, "ConfigurationChanged") if *k == Kind::TabGroup => {
                    if !self.wb_pub(w, "bSizeChildrenToParent") {
                        return None; // the script event
                    }
                    let (pw, ph) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
                    let mut c = self.visible_child(w, true);
                    while c != 0 {
                        self.ui_configure_child(c, 0.0, 0.0, pw, ph);
                        c = self.next_visible(c, false);
                    }
                    return Some(Value::Void);
                }
                (Kind::TabGroup, "ParentRequestedPreferredSize") => {
                    let (w_spec, h_spec) = (bl(args, 0, false), bl(args, 2, false));
                    let (in_w, in_h) = (fl(args, 1, 0.0), fl(args, 3, 0.0));
                    let (max_w, max_h) = self.tab_group_preferred(w, w_spec, in_w, h_spec, in_h)?;
                    args[1] = Value::Float(max_w);
                    args[3] = Value::Float(max_h);
                    return Some(Value::Void);
                }
                // `XToggleWindow::MouseButtonPressed` (10049a80): takes the left button, else the script's answer
                (Kind::Toggle, "MouseButtonPressed") => {
                    let (r, out) = self.ui_script_event(w, "MouseButtonPressed", args.clone());
                    *args = out;
                    return Some(if iv(args, 2) == 1 { Value::Bool(true) } else { r });
                }
                // `XToggleWindow::MouseButtonReleased` (10049b30): a left release inside plays the sound of the state
                // before the flip (`enableSound` when it was on), then flips
                (Kind::Toggle, "MouseButtonReleased") => {
                    let (r, out) = self.ui_script_event(w, "MouseButtonReleased", args.clone());
                    *args = out;
                    if iv(args, 2) == 1 {
                        if self.ui_is_point_in_window(w, fl(args, 0, 0.0), fl(args, 1, 0.0)) {
                            let was = self.wb_pub(w, "bButtonPressed");
                            let s = self.wo_pub(w, if was { "enableSound" } else { "disableSound" });
                            self.ui_play_sound(w, s, -1.0, 1.0);
                            self.toggle_set(w, !was);
                        }
                        return Some(Value::Bool(true));
                    }
                    return Some(r);
                }
                (Kind::Button, "MouseButtonPressed") => {
                    let (r, out) = self.ui_script_event(w, "MouseButtonPressed", args.clone());
                    *args = out;
                    let key = iv(args, 2);
                    if key == 1 || (key == 2 && self.wb_pub(w, "bEnableRightMouseClick")) {
                        self.set_b_pub(w, "bButtonPressed", true);
                        self.set_b_pub(w, "bMousePressed", true);
                        self.button_change_appearance(w);
                        if !self.wb_pub(w, "bAutoRepeat") {
                            let s = self.wo_pub(w, "pressSound");
                            self.ui_play_sound(w, s, -1.0, 1.0);
                        } else {
                            let s = self.wo_pub(w, "clickSound");
                            self.ui_play_sound(w, s, -1.0, 1.0);
                            let d = self.wf_pub(w, "initialDelay");
                            self.set_f_pub(w, "repeatTime", d);
                            self.set_i_pub(w, "lastInputKey", key);
                            self.button_activate(w, key);
                        }
                        return Some(Value::Bool(true));
                    }
                    return Some(r);
                }
                (Kind::Button, "MouseButtonReleased") => {
                    let (r, out) = self.ui_script_event(w, "MouseButtonReleased", args.clone());
                    *args = out;
                    let key = iv(args, 2);
                    if key == 1 || (key == 2 && self.wb_pub(w, "bEnableRightMouseClick")) {
                        self.set_f_pub(w, "repeatTime", 0.0);
                        self.set_b_pub(w, "bButtonPressed", false);
                        self.set_b_pub(w, "bMousePressed", false);
                        self.set_i_pub(w, "lastInputKey", 0);
                        self.button_change_appearance(w);
                        if self.ui_is_point_in_window(w, fl(args, 0, 0.0), fl(args, 1, 0.0)) && !self.wb_pub(w, "bAutoRepeat") {
                            let s = self.wo_pub(w, "clickSound");
                            self.ui_play_sound(w, s, -1.0, 1.0);
                            self.button_activate(w, key);
                        }
                        return Some(Value::Bool(true));
                    }
                    return Some(r);
                }
                (Kind::Button, "MouseMoved") => {
                    let (_, out) = self.ui_script_event(w, "MouseMoved", args.clone());
                    *args = out;
                    if self.wb_pub(w, "bMousePressed") {
                        let inside = self.wb_pub(w, "bIsVisible") && self.ui_is_point_in_window(w, fl(args, 0, 0.0), fl(args, 1, 0.0));
                        self.set_b_pub(w, "bButtonPressed", inside);
                        self.button_change_appearance(w);
                    }
                    return Some(Value::Void);
                }
                (Kind::Button, "SensitivityChanged") => {
                    let (_, out) = self.ui_script_event(w, "SensitivityChanged", args.clone());
                    *args = out;
                    if !bl(args, 0, true) && self.wb_pub(w, "bMousePressed") {
                        self.set_b_pub(w, "bButtonPressed", false);
                        self.set_b_pub(w, "bMousePressed", false);
                    }
                    self.button_change_appearance(w);
                    return Some(Value::Void);
                }
                (Kind::Button, "FocusEnteredWindow" | "FocusLeftWindow") => {
                    self.ui_script_event(w, name, args.clone());
                    self.button_change_appearance(w);
                    return Some(Value::Void);
                }
                (Kind::Button, "Tick") => {
                    // auto-repeat: while the mouse stays pressed inside the button it activates again every repeatRate
                    if self.wb_pub(w, "bMousePressed") && self.wb_pub(w, "bAutoRepeat") {
                        let mut left = self.wf_pub(w, "repeatTime") - fl(args, 0, 0.0);
                        self.set_f_pub(w, "repeatTime", left);
                        if left < 0.0 {
                            left += self.wf_pub(w, "repeatRate");
                            if left < 0.0 {
                                left = self.wf_pub(w, "repeatRate");
                            }
                            self.set_f_pub(w, "repeatTime", left);
                            let root = self.ui_root_of_pub(w);
                            if root != 0 {
                                let g = self.wo_ref_pub(w, "wGc");
                                let o = self.gc_state(g).clip;
                                let (x, y) = (self.wf_pub(root, "mouseX") - o[0], self.wf_pub(root, "mouseY") - o[1]);
                                if self.ui_is_point_in_window(w, x, y) {
                                    let key = self.wi_pub(w, "lastInputKey");
                                    let s = self.wo_pub(w, "clickSound");
                                    self.ui_play_sound(w, s, -1.0, 1.0);
                                    self.button_activate(w, key);
                                }
                            }
                        }
                    }
                    return Some(Value::Void);
                }
                (Kind::Button, "Timer") => {
                    let (_, out) = self.ui_script_event(w, "Timer", args.clone());
                    *args = out;
                    if iv(args, 0) == self.wi_pub(w, "activateTimer") {
                        self.set_b_pub(w, "bButtonPressed", false);
                        self.button_change_appearance(w);
                        let t = self.wi_pub(w, "activateTimer");
                        self.ui_remove_timer(w, t);
                        self.set_i_pub(w, "activateTimer", 0);
                    }
                    return Some(Value::Void);
                }
                (Kind::Modal, "KeyPressed") => {
                    return Some(self.ui_modal_key_pressed(w, args));
                }
                (Kind::Modal, "VisibilityChanged") => {
                    // the script first; a modal window that shows takes the keyboard and mouse for the window system
                    let vis = bl(args, 0, false);
                    let (_, out) = self.ui_script_event(w, "VisibilityChanged", args.clone());
                    *args = out;
                    if self.wo_pub(w, "winParent") != 0 {
                        let root = self.ui_root_of_pub(w);
                        if root != 0 {
                            self.ui_grab_events(root, vis);
                        }
                    }
                    return Some(Value::Void);
                }
                (Kind::Checkbox, "ParentRequestedPreferredSize") => {
                    let (mut pw, mut ph) = (fl(args, 1, 0.0), fl(args, 3, 0.0));
                    if !self.checkbox_preferred_size(w, bl(args, 0, false), &mut pw, bl(args, 2, false), &mut ph) {
                        return None;
                    }
                    args[1] = Value::Float(pw);
                    args[3] = Value::Float(ph);
                    return Some(Value::Void);
                }
                (Kind::Text, "ParentRequestedPreferredSize") => {
                    let (mut pw, mut ph) = (fl(args, 1, 0.0), fl(args, 3, 0.0));
                    if !self.text_preferred_size(w, bl(args, 0, false), &mut pw, bl(args, 2, false), &mut ph) {
                        return None;
                    }
                    args[1] = Value::Float(pw);
                    args[3] = Value::Float(ph);
                    return Some(Value::Void);
                }
                // `XBorderWindow::MouseButtonPressed` (10001e00), resizeable windows only: the left button picks the
                // edges under the cursor (inside the border margins) to drag, the right button drags all four (a move);
                // the drag starts from the current geometry and the cursor in root coordinates, the window comes to the
                // front
                (Kind::Border, "MouseButtonPressed") => {
                    let (r, out) = self.ui_script_event(w, name, args.clone());
                    *args = out;
                    let key = iv(args, 2);
                    if !matches!(key, 1 | 2) || !self.wb_pub(w, "bResizeable") {
                        return Some(r);
                    }
                    let (x, y) = (fl(args, 0, 0.0), fl(args, 1, 0.0));
                    let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
                    let flags = if key == 1 {
                        [x < self.wf_pub(w, "leftMargin"), width - self.wf_pub(w, "rightMargin") <= x, y < self.wf_pub(w, "topMargin"), height - self.wf_pub(w, "bottomMargin") <= y]
                    } else {
                        [true; 4]
                    };
                    for (n, v) in ["bLeftDrag", "bRightDrag", "bUpDrag", "bDownDrag"].into_iter().zip(flags) {
                        self.set_b_pub(w, n, v);
                    }
                    let (wx, wy) = (self.wf_pub(w, "x"), self.wf_pub(w, "y"));
                    for (n, v) in [("DragX", wx), ("DragY", wy), ("dragWidth", width), ("dragHeight", height)] {
                        self.set_f_pub(w, n, v);
                    }
                    let (rx, ry) = self.border_to_root(w, x, y);
                    self.set_f_pub(w, "lastMouseX", rx);
                    self.set_f_pub(w, "lastMouseY", ry);
                    let _ = self.native_ui(w, "Raise", &mut Vec::new());
                    return Some(Value::Bool(true));
                }
                // `XBorderWindow::MouseMoved` (100020e0): the cursor's motion in root coordinates moves the dragged edges
                // and the window takes the new geometry (`SetConfiguration`)
                (Kind::Border, "MouseMoved") => {
                    let (_, out) = self.ui_script_event(w, name, args.clone());
                    *args = out;
                    let flags = ["bLeftDrag", "bRightDrag", "bUpDrag", "bDownDrag"].map(|n| self.wb_pub(w, n));
                    if flags.iter().any(|&f| f) {
                        let (rx, ry) = self.border_to_root(w, fl(args, 0, 0.0), fl(args, 1, 0.0));
                        let (dx, dy) = (rx - self.wf_pub(w, "lastMouseX"), ry - self.wf_pub(w, "lastMouseY"));
                        self.set_f_pub(w, "lastMouseX", rx);
                        self.set_f_pub(w, "lastMouseY", ry);
                        let [mut gx, mut gy, mut gw, mut gh] = ["DragX", "DragY", "dragWidth", "dragHeight"].map(|n| self.wf_pub(w, n));
                        if flags[0] {
                            gw -= dx;
                            gx += dx;
                        }
                        if flags[1] {
                            gw += dx;
                        }
                        if flags[2] {
                            gh -= dy;
                            gy += dy;
                        }
                        if flags[3] {
                            gh += dy;
                        }
                        for (n, v) in [("DragX", gx), ("DragY", gy), ("dragWidth", gw), ("dragHeight", gh)] {
                            self.set_f_pub(w, n, v);
                        }
                        let _ = self.native_ui(w, "SetConfiguration", &mut vec![Value::Float(gx), Value::Float(gy), Value::Float(gw), Value::Float(gh)]);
                    }
                    return Some(Value::Void);
                }
                // `XBorderWindow::MouseButtonReleased` (10001ff0): either button ends the drag; taken when resizeable
                (Kind::Border, "MouseButtonReleased") => {
                    let (r, out) = self.ui_script_event(w, name, args.clone());
                    *args = out;
                    if !matches!(iv(args, 2), 1 | 2) {
                        return Some(r);
                    }
                    for n in ["bLeftDrag", "bRightDrag", "bUpDrag", "bDownDrag"] {
                        self.set_b_pub(w, n, false);
                    }
                    return Some(if self.wb_pub(w, "bResizeable") { Value::Bool(true) } else { r });
                }
                (Kind::Border, "ParentRequestedPreferredSize") => {
                    let top = self.visible_child(w, true);
                    if top == 0 {
                        return None;
                    }
                    let (w_spec, h_spec) = (bl(args, 0, false), bl(args, 2, false));
                    let g = |vm: &Vm, n: &'static str| vm.wf_pub(w, n);
                    let (cl, cr, ct, cb) = (g(self, "childLeftMargin"), g(self, "childRightMargin"), g(self, "childTopMargin"), g(self, "childBottomMargin"));
                    let (ml, mr, mt, mb) = (g(self, "leftMargin"), g(self, "rightMargin"), g(self, "topMargin"), g(self, "bottomMargin"));
                    let from_border = self.wb_pub(w, "bMarginsFromBorder");
                    let (mut pw, mut ph) = (fl(args, 1, 0.0) - (cl + cr), fl(args, 3, 0.0) - (ct + cb));
                    if from_border {
                        pw -= mr + ml;
                        ph -= mb + mt;
                    }
                    let (mut rw, mut rh) = self.ui_query_preferred_size(top, w_spec, pw, h_spec, ph);
                    rw += cl + cr;
                    rh += ct + cb;
                    if from_border {
                        rw += mr + ml;
                        rh += mb + mt;
                    }
                    rw = rw.max(mr + ml);
                    rh = rh.max(mb + mt);
                    args[1] = Value::Float(rw);
                    args[3] = Value::Float(rh);
                    return Some(Value::Void);
                }
                (Kind::Border, "ConfigurationChanged") => {
                    let top = self.visible_child(w, true);
                    if top != 0 {
                        let g = |vm: &Vm, n: &'static str| vm.wf_pub(w, n);
                        let (mut x, mut y) = (g(self, "childLeftMargin"), g(self, "childTopMargin"));
                        let mut cw = g(self, "width") - (g(self, "childRightMargin") + g(self, "childLeftMargin"));
                        let mut ch = g(self, "height") - (g(self, "childBottomMargin") + g(self, "childTopMargin"));
                        if self.wb_pub(w, "bMarginsFromBorder") {
                            x += g(self, "leftMargin");
                            y += g(self, "topMargin");
                            cw -= g(self, "rightMargin") + g(self, "leftMargin");
                            ch -= g(self, "bottomMargin") + g(self, "topMargin");
                        }
                        self.ui_configure_child(top, x, y, cw, ch);
                    }
                    return Some(Value::Void);
                }
                (Kind::TabGroup, "ChildRequestedVisibilityChange") => {
                    let child = match args.first() {
                        Some(Value::Obj(c)) => *c,
                        _ => 0,
                    };
                    let vis = bl(args, 1, false);
                    self.ui_set_child_visibility(child, vis);
                    if !vis {
                        self.ui_ask_parent_for_reconfigure(w);
                    }
                    return Some(Value::Void);
                }
                _ => {}
            }
        }
        None
    }

    /// `ComputeWindowBorders`: the margins the border textures take on each side.
    pub(crate) fn border_compute_margins(&mut self, w: u32) {
        let (mut l, mut t, mut r, mut b) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        if self.wi_pub(w, "borderStyle") != 0 {
            let size = |vm: &mut Vm, n: &'static str| {
                let tex = vm.wo_ref(w, n);
                vm.texture_size(tex)
            };
            let (tl, tr, br, bl) = (size(self, "borderTopLeft"), size(self, "borderTopRight"), size(self, "borderBottomRight"), size(self, "borderBottomLeft"));
            let (top, right, bottom, left) = (size(self, "borderTop"), size(self, "borderRight"), size(self, "borderBottom"), size(self, "borderLeft"));
            if tl.0 > 0.0 {
                l = tl.0;
            }
            if tl.1 > 0.0 {
                t = tl.1;
            }
            if tr.0 > 0.0 {
                r = tr.0;
            }
            t = t.max(tr.1);
            r = r.max(br.0);
            if br.1 > 0.0 {
                b = br.1;
            }
            l = l.max(bl.0);
            b = b.max(bl.1);
            t = t.max(top.1);
            r = r.max(right.0);
            b = b.max(bottom.1);
            l = l.max(left.0);
        }
        self.set_f_pub(w, "leftMargin", l);
        self.set_f_pub(w, "topMargin", t);
        self.set_f_pub(w, "rightMargin", r);
        self.set_f_pub(w, "bottomMargin", b);
    }

    /// `GrabKeyboardEvents`/`GrabMouseEvents` and their `Ungrab` counterparts on the root.
    pub(crate) fn ui_grab_events(&mut self, root: u32, grab: bool) {
        for n in ["handleKeyboardRef", "handleMouseRef"] {
            let v = self.wi_pub(root, n);
            if grab {
                if v < 1 {
                    // the first grab counts every key of its kind as up (`GrabKeyboardEvents` 1003a110: the keys,
                    // `GrabMouseEvents` 1003a1d0: the mouse buttons and axes)
                    let mouse = n == "handleMouseRef";
                    for k in 0..255u32 {
                        if crate::ui_input::is_mouse_button(k as i32) == mouse {
                            let _ = self.set_prop(root, "keyDownMap", k, Value::Byte(0));
                        }
                    }
                }
                self.set_i_pub(root, n, v + 1);
            } else if v > 0 {
                self.set_i_pub(root, n, v - 1);
            }
        }
    }

    /// First visible child from the top (`from_top`) or from the bottom of the stacking order.
    pub(crate) fn visible_child(&self, w: u32, from_top: bool) -> u32 {
        let (mut c, step) = if from_top { (self.wo_pub(w, "lastChild"), "prevSibling") } else { (self.wo_pub(w, "firstChild"), "nextSibling") };
        while c != 0 && !self.wb_pub(c, "bIsVisible") {
            c = self.wo_pub(c, step);
        }
        c
    }

    /// The next visible sibling above (`higher`) or below `c`.
    pub(crate) fn next_visible(&self, c: u32, higher: bool) -> u32 {
        let step = if higher { "nextSibling" } else { "prevSibling" };
        let mut n = self.wo_pub(c, step);
        while n != 0 && !self.wb_pub(n, "bIsVisible") {
            n = self.wo_pub(n, step);
        }
        n
    }

    /// `XTileWindow::ComputeChildSizes`: sizes and (when both dimensions are given) places the visible children into
    /// their `holdX/holdY/holdWidth/holdHeight`, or, when a dimension is not given, computes it into `pw`/`ph`.
    /// Children are stacked in rows along the major axis; rows wrap when the width/height runs out (if wrapping is on).
    fn tile_compute_child_sizes(&mut self, w: u32, w_spec: bool, pw: &mut f32, h_spec: bool, ph: &mut f32) {
        let vertical = self.wi_pub(w, "orientation") == 1;
        let (hm, vm) = (self.wf_pub(w, "hMargin"), self.wf_pub(w, "vMargin"));
        let (minor, major) = (self.wf_pub(w, "minorSpacing"), self.wf_pub(w, "majorSpacing"));
        let (wrap, fill) = (self.wb_pub(w, "bWrap"), self.wb_pub(w, "bFillParent"));
        let (eq_w, eq_h) = (self.wb_pub(w, "bEqualWidth"), self.wb_pub(w, "bEqualHeight"));
        // a fixed size across the rows when not wrapping and filling the parent
        let (mut fix_w, mut fix_h, mut fixed) = (false, false, 0.0f32);
        if !wrap && fill {
            if vertical {
                if w_spec {
                    fix_w = true;
                    fixed = (*pw - (hm + hm)).max(0.0);
                }
            } else if h_spec {
                fix_h = true;
                fixed = (*ph - (vm + vm)).max(0.0);
            }
        }
        let (mut max_w, mut max_h) = (0.0f32, 0.0f32);
        let mut c = self.wo_pub(w, "lastChild");
        while c != 0 {
            if !self.wb_pub(c, "bIsVisible") {
                for n in ["holdWidth", "holdHeight", "holdX", "holdY"] {
                    self.set_f_pub(c, n, 0.0);
                }
            } else {
                let (cw, ch) = if fix_w {
                    let h = self.ui_query_preferred_size(c, true, fixed, false, 0.0).1;
                    (fixed, h)
                } else if fix_h {
                    let wd = self.ui_query_preferred_size(c, false, 0.0, true, fixed).0;
                    (wd, fixed)
                } else {
                    self.ui_query_preferred_size(c, false, 0.0, false, 0.0)
                };
                self.set_f_pub(c, "holdWidth", cw);
                self.set_f_pub(c, "holdHeight", ch);
                if eq_w && cw > max_w {
                    max_w = cw;
                }
                if eq_h && ch > max_h {
                    max_h = ch;
                }
            }
            c = self.wo_pub(c, "prevSibling");
        }
        if eq_w || eq_h {
            let mut c = self.visible_child(w, false);
            while c != 0 {
                if eq_w {
                    self.set_f_pub(c, "holdWidth", max_w);
                }
                if eq_h {
                    self.set_f_pub(c, "holdHeight", max_h);
                }
                c = self.next_visible(c, true);
            }
        }
        // rows: (cross size, length along the major axis, number of children)
        let mut limit = 1.0e6f32;
        if wrap {
            if w_spec && !vertical {
                limit = *pw - (hm + hm);
            }
            if h_spec && vertical {
                limit = *ph - (vm + vm);
            }
        }
        let mut rows: Vec<(i32, i32, i32)> = Vec::new();
        let (mut cross_max, mut acc, mut count) = (fixed as i32, 0i32, 0i32);
        let mut c = self.visible_child(w, false);
        while c != 0 {
            let (hw, hh) = (self.wf_pub(c, "holdWidth"), self.wf_pub(c, "holdHeight"));
            let (cross, len) = if vertical { (hw, hh) } else { (hh, hw) };
            if count > 0 && acc as f32 + len > limit {
                rows.push((cross_max, (acc as f32 - minor) as i32, count));
                cross_max = fixed as i32;
                acc = 0;
                count = 0;
            }
            acc = (acc as f32 + minor + len) as i32;
            if (cross_max as f32) < cross {
                cross_max = cross as i32;
            }
            count += 1;
            c = self.next_visible(c, true);
        }
        if count > 0 {
            rows.push((cross_max, (acc as f32 - minor) as i32, count));
        }
        if !w_spec || !h_spec {
            let mut across = 0i32;
            if vertical {
                let mut longest = 0.0f32;
                for (i, r) in rows.iter().enumerate() {
                    longest = longest.max(r.1 as f32);
                    across = (r.0 as f32 + across as f32) as i32;
                    if i > 0 {
                        across = (across as f32 + major) as i32;
                    }
                }
                let along = longest;
                if !w_spec {
                    *pw = hm + hm + across as f32;
                }
                if !h_spec {
                    *ph = vm + vm + along;
                }
            } else {
                let mut total = 0.0f32;
                for (i, r) in rows.iter().enumerate() {
                    if r.1 > across {
                        across = r.1;
                    }
                    total += r.0 as f32;
                    if i > 0 {
                        total += major;
                    }
                }
                if !w_spec {
                    *pw = hm + hm + across as f32;
                }
                if !h_spec {
                    *ph = vm + vm + total;
                }
            }
            return;
        }
        // both sizes given: place the children
        let (h_dir, v_dir) = (self.wi_pub(w, "hDirection") == 1, self.wi_pub(w, "vDirection") == 1);
        let (h_al, v_al) = (self.wi_pub(w, "hChildAlign"), self.wi_pub(w, "vChildAlign"));
        let (mut x, mut y) = (hm, vm);
        let mut row = 0usize;
        let mut c = self.visible_child(w, false);
        while c != 0 && row < rows.len() {
            let (hw, hh) = (self.wf_pub(c, "holdWidth"), self.wf_pub(c, "holdHeight"));
            let cross = rows[row].0 as f32;
            if vertical {
                self.set_f_pub(c, "holdY", if v_dir { *ph - (y + hh) } else { y });
                let mut hx = if h_dir { *pw - (cross + x) } else { x };
                match h_al {
                    1 => hx += (((cross - hw) as i32) / 2) as f32,
                    2 => hx += cross - hw,
                    3 => self.set_f_pub(c, "holdWidth", cross),
                    _ => {}
                }
                self.set_f_pub(c, "holdX", hx);
                y += minor + hh;
                rows[row].2 -= 1;
                if rows[row].2 < 1 {
                    y = vm;
                    x = rows[row].0 as f32 + major + x;
                    row += 1;
                }
            } else {
                self.set_f_pub(c, "holdX", if h_dir { *pw - (x + hw) } else { x });
                let mut hy = if v_dir { *ph - (cross + y) } else { y };
                match v_al {
                    1 => hy += (((cross - hh) as i32) / 2) as f32,
                    2 => hy += cross - hh,
                    3 => self.set_f_pub(c, "holdHeight", cross),
                    _ => {}
                }
                self.set_f_pub(c, "holdY", hy);
                x += minor + hw;
                rows[row].2 -= 1;
                if rows[row].2 < 1 {
                    x = hm;
                    y = rows[row].0 as f32 + major + y;
                    row += 1;
                }
            }
            c = self.next_visible(c, true);
        }
    }

    /// The setters of `TileWindow`.
    pub(crate) fn native_tile(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        if !self.ui_kinds(this).contains(&Kind::Tile) {
            return None;
        }
        let byte = |vm: &mut Vm, n: &str, v: i32| {
            let _ = vm.set_prop(this, n, 0, Value::Byte(v as u8));
        };
        match name {
            "SetMargins" => {
                self.set_f_pub(this, "hMargin", fl(a, 0, 0.0));
                self.set_f_pub(this, "vMargin", fl(a, 1, 0.0));
            }
            "SetOrientation" => byte(self, "orientation", iv(a, 0)),
            "SetDirections" => {
                byte(self, "hDirection", iv(a, 0));
                byte(self, "vDirection", iv(a, 1));
            }
            "SetOrder" => {
                // (orientation, hDirection, vDirection, wrap) for Right, Left, Down, Up, then the "...Then..." orders which wrap
                let (o, h, v, wrap) = match iv(a, 0) {
                    0 => (0, 0, 0, false),
                    1 => (0, 1, 0, false),
                    2 => (1, 0, 0, false),
                    3 => (1, 0, 1, false),
                    5 => (0, 0, 1, true),
                    6 => (0, 1, 0, true),
                    7 => (0, 1, 1, true),
                    8 => (1, 0, 0, true),
                    9 => (1, 1, 0, true),
                    10 => (1, 0, 1, true),
                    11 => (1, 1, 1, true),
                    _ => (0, 0, 0, true),
                };
                byte(self, "orientation", o);
                byte(self, "hDirection", h);
                byte(self, "vDirection", v);
                self.set_b_pub(this, "bWrap", wrap);
            }
            "SetMinorSpacing" => self.set_f_pub(this, "minorSpacing", fl(a, 0, 0.0)),
            "SetMajorSpacing" => self.set_f_pub(this, "majorSpacing", fl(a, 0, 0.0)),
            "SetChildAlignments" => {
                byte(self, "hChildAlign", iv(a, 0));
                byte(self, "vChildAlign", iv(a, 1));
            }
            "EnableWrapping" => self.set_b_pub(this, "bWrap", bl(a, 0, false)),
            "FillParent" => self.set_b_pub(this, "bFillParent", bl(a, 0, false)),
            "MakeWidthsEqual" => self.set_b_pub(this, "bEqualWidth", bl(a, 0, false)),
            "MakeHeightsEqual" => self.set_b_pub(this, "bEqualHeight", bl(a, 0, false)),
            _ => return None,
        }
        self.ui_ask_parent_for_reconfigure(this);
        Some(Ok(Value::Void))
    }
}

/// Per-state appearance of a button: [normal, pressed, focus, focus+pressed, insensitive, insensitive+pressed].
#[derive(Clone, Debug)]
pub struct ButtonInfo {
    pub texture: [u32; 6],
    pub tile: [[u8; 4]; 6],
    pub text: [[u8; 4]; 6],
}

impl Default for ButtonInfo {
    fn default() -> Self {
        ButtonInfo { texture: [0; 6], tile: [[255; 4]; 6], text: [[0, 255, 0, 255]; 6] }
    }
}

pub(crate) fn color_arg(a: &[Value], i: usize) -> [u8; 4] {
    match a.get(i) {
        Some(Value::Struct(_, b)) if b.len() >= 4 => [b[0], b[1], b[2], b[3]],
        _ => [0, 0, 0, 0],
    }
}

fn obj_arg(a: &[Value], i: usize) -> u32 {
    match a.get(i) {
        Some(Value::Obj(r)) => *r,
        _ => 0,
    }
}

impl Vm {
    /// `ActivateButton`: tells the button's window and then its ancestors, until one handles `ButtonActivated`.
    pub(crate) fn button_activate(&mut self, w: u32, key: i32) {
        let ev = if key == 2 { "ButtonActivatedRight" } else { "ButtonActivated" };
        let mut cur = w;
        while cur != 0 {
            let (r, _) = self.ui_event(cur, ev, vec![Value::Obj(w)]);
            if r.as_bool().unwrap_or(false) {
                break;
            }
            cur = self.wo_pub(cur, "winParent");
        }
    }

    /// Re-picks the texture and colours a button shows for its state (`ChangeButtonAppearance`).
    pub(crate) fn button_change_appearance(&mut self, w: u32) {
        let mut idx = 0usize;
        if !self.ui_sensitive_pub(w) {
            idx = 4;
        } else if self.ui_focus_window_is(w) {
            idx = 2;
        }
        let pressed = self.wb_pub(w, "bButtonPressed");
        if pressed {
            idx += 1;
        }
        let Some(info) = self.ui.buttons.get(&w).cloned() else { return };
        let mut tex = info.texture[idx];
        if tex == 0 && (!pressed || info.texture[1] == 0) {
            tex = info.texture[0];
        } else if tex == 0 {
            tex = info.texture[1];
        }
        self.set_o_pub(w, "curTexture", tex);
        let (tile, text) = (self.color_value_pub(info.tile[idx]), self.color_value_pub(info.text[idx]));
        let _ = self.set_prop(w, "curTileColor", 0, tile);
        let _ = self.set_prop(w, "curTextColor", 0, text);
    }

    /// Setters and text access of `TextWindow`, `TextLogWindow`, `ButtonWindow`, `ToggleWindow`, `CheckboxWindow` and
    /// `BorderWindow`. (Their drawing and size queries are not implemented yet.)
    pub(crate) fn native_widgets(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        let kinds = self.ui_kinds(this);
        let has = |k: Kind| kinds.contains(&k);
        let mut reconfigure = false;
        let ret = match name {
            // `XLargeTextWindow::SetVerticalSpacing`: below 0 is 0; a change asks for a new layout
            "SetVerticalSpacing" if has(Kind::LargeText) => {
                let v = fl(a, 0, 0.0).max(0.0);
                if self.wf_pub(this, "vSpace") != v {
                    self.set_f_pub(this, "vSpace", v);
                    reconfigure = true;
                }
                Value::Void
            }
            // ---- TextWindow
            "SetText" if has(Kind::Text) => {
                let new = match a.first() {
                    Some(Value::Str(s)) => crate::ui_natives::convert_script_string(s),
                    _ => String::new(),
                };
                let old = match self.get_prop(this, "text", 0) {
                    Ok(Value::Str(s)) => s,
                    _ => String::new(),
                };
                if !old.eq_ignore_ascii_case(&new) {
                    if self.wb_pub(this, "bTextIsAccelerator") {
                        self.ui_set_accelerator_text(this, Some(&new));
                    }
                    let _ = self.set_prop(this, "text", 0, Value::Str(new));
                    reconfigure = true;
                }
                Value::Void
            }
            "AppendText" if has(Kind::Text) => {
                let add = match a.first() {
                    Some(Value::Str(s)) => crate::ui_natives::convert_script_string(s),
                    _ => String::new(),
                };
                let mut old = match self.get_prop(this, "text", 0) {
                    Ok(Value::Str(s)) => s,
                    _ => String::new(),
                };
                old.push_str(&add);
                if self.wb_pub(this, "bTextIsAccelerator") {
                    self.ui_set_accelerator_text(this, Some(&old));
                }
                let _ = self.set_prop(this, "text", 0, Value::Str(old));
                reconfigure = true;
                Value::Void
            }
            "GetText" if has(Kind::Text) => self.get_prop(this, "text", 0).unwrap_or(Value::Str(String::new())),
            "GetTextLength" if has(Kind::Text) => Value::Int(match self.get_prop(this, "text", 0) {
                Ok(Value::Str(s)) => s.chars().count() as i32,
                _ => 0,
            }),
            "GetTextPart" if has(Kind::Text) => {
                let text = match self.get_prop(this, "text", 0) {
                    Ok(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let (start, count) = (iv(a, 0).max(0) as usize, iv(a, 1).max(0) as usize);
                let part: String = text.chars().skip(start).take(count).collect();
                let n = part.chars().count() as i32;
                a[2] = Value::Str(part);
                Value::Int(n)
            }
            "SetWordWrap" if has(Kind::Text) => {
                let b = bl(a, 0, false);
                if self.wb_pub(this, "bWordWrap") != b {
                    self.set_b_pub(this, "bWordWrap", b);
                    reconfigure = true;
                }
                Value::Void
            }
            "SetTextAlignments" if has(Kind::Text) => {
                let _ = self.set_prop(this, "hAlign", 0, Value::Byte(iv(a, 0) as u8));
                let _ = self.set_prop(this, "vAlign", 0, Value::Byte(iv(a, 1) as u8));
                Value::Void
            }
            "SetTextMargins" if has(Kind::Text) => {
                let (h, v) = (fl(a, 0, 0.0), fl(a, 1, 0.0));
                if self.wf_pub(this, "hMargin") != h || self.wf_pub(this, "vMargin") != v {
                    self.set_f_pub(this, "hMargin", h);
                    self.set_f_pub(this, "vMargin", v);
                    reconfigure = true;
                }
                Value::Void
            }
            "SetLines" | "SetMinLines" | "SetMaxLines" | "ResetLines" if has(Kind::Text) => {
                let (omin, omax) = (self.wi_pub(this, "minLines"), self.wi_pub(this, "maxLines"));
                let (mut nmin, mut nmax) = (omin, omax);
                match name {
                    "SetLines" => {
                        nmin = iv(a, 0).max(0);
                        nmax = iv(a, 1).max(0);
                        if nmax < nmin {
                            nmin = nmax;
                        }
                    }
                    "SetMinLines" => {
                        nmin = iv(a, 0).max(0).min(omax);
                    }
                    "SetMaxLines" => {
                        nmax = iv(a, 0).max(0).max(omin);
                    }
                    _ => {
                        nmin = -1;
                        nmax = -1;
                    }
                }
                if (nmin, nmax) != (omin, omax) {
                    self.set_i_pub(this, "minLines", nmin);
                    self.set_i_pub(this, "maxLines", nmax);
                    reconfigure = true;
                }
                Value::Void
            }
            "SetMinWidth" | "ResetMinWidth" if has(Kind::Text) => {
                let v = if name == "SetMinWidth" { fl(a, 0, 0.0).max(0.0) } else { 0.0 };
                if self.wf_pub(this, "minWidth") != v {
                    self.set_f_pub(this, "minWidth", v);
                    reconfigure = true;
                }
                Value::Void
            }
            "EnableTextAsAccelerator" if has(Kind::Text) => {
                let on = bl(a, 0, true);
                if self.wb_pub(this, "bTextIsAccelerator") != on {
                    self.set_b_pub(this, "bTextIsAccelerator", on);
                    let text = if on {
                        match self.get_prop(this, "text", 0) {
                            Ok(Value::Str(s)) => Some(s),
                            _ => Some(String::new()),
                        }
                    } else {
                        None
                    };
                    self.ui_set_accelerator_text(this, text.as_deref());
                }
                Value::Void
            }
            // ---- TextLogWindow
            "SetTextTimeout" if has(Kind::TextLog) => {
                self.set_f_pub(this, "textTimeout", fl(a, 0, 0.0));
                Value::Void
            }
            "AddLog" if has(Kind::TextLog) => {
                let text = match a.first() {
                    Some(Value::Str(s)) => s.clone(),
                    _ => String::new(),
                };
                self.text_log_add(this, text, color_arg(a, 1));
                Value::Void
            }
            "ClearLog" if has(Kind::TextLog) => {
                self.text_log_clear(this);
                Value::Void
            }
            "PauseLog" if has(Kind::TextLog) => {
                self.set_b_pub(this, "bPaused", bl(a, 0, false));
                Value::Void
            }
            // ---- ButtonWindow, ToggleWindow
            "SetButtonTextures" if has(Kind::Button) => {
                let info = self.ui.buttons.entry(this).or_default();
                for i in 0..6 {
                    info.texture[i] = obj_arg(a, i);
                }
                self.button_change_appearance(this);
                Value::Void
            }
            "SetButtonColors" | "SetTextColors" if has(Kind::Button) => {
                let info = self.ui.buttons.entry(this).or_default();
                for i in 0..6 {
                    if name == "SetButtonColors" {
                        info.tile[i] = color_arg(a, i);
                    } else {
                        info.text[i] = color_arg(a, i);
                    }
                }
                self.button_change_appearance(this);
                Value::Void
            }
            "EnableAutoRepeat" if has(Kind::Button) => {
                self.set_b_pub(this, "bAutoRepeat", bl(a, 0, true));
                self.set_f_pub(this, "initialDelay", fl(a, 1, 0.5));
                self.set_f_pub(this, "repeatRate", fl(a, 2, 0.1));
                Value::Void
            }
            "EnableRightMouseClick" if has(Kind::Button) => {
                self.set_b_pub(this, "bEnableRightMouseClick", bl(a, 0, true));
                Value::Void
            }
            "SetActivateDelay" if has(Kind::Button) => {
                self.set_f_pub(this, "activateDelay", fl(a, 0, 0.3));
                Value::Void
            }
            "SetButtonSounds" if has(Kind::Button) => {
                self.set_o_pub(this, "pressSound", obj_arg(a, 0));
                self.set_o_pub(this, "clickSound", obj_arg(a, 1));
                Value::Void
            }
            "ActivateButton" if has(Kind::Button) => {
                self.button_activate(this, iv(a, 0));
                Value::Void
            }
            "PressButton" if has(Kind::Button) => {
                if has(Kind::Toggle) {
                    // `XToggleWindow::PressButton` (10049c30): flip, then the sound of the new state
                    let now = !self.wb_pub(this, "bButtonPressed");
                    self.toggle_set(this, now);
                    let on = self.wb_pub(this, "bButtonPressed");
                    let s = self.wo_pub(this, if on { "enableSound" } else { "disableSound" });
                    self.ui_play_sound(this, s, -1.0, 1.0);
                } else {
                    self.set_b_pub(this, "bButtonPressed", true);
                    self.button_change_appearance(this);
                    let old = self.wi_pub(this, "activateTimer");
                    if old != 0 {
                        self.ui_remove_timer(this, old);
                    }
                    let delay = self.wf_pub(this, "activateDelay");
                    let id = self.ui_add_timer(this, delay, false, 0, 0);
                    self.set_i_pub(this, "activateTimer", id);
                    let s = self.wo_pub(this, "clickSound");
                    self.ui_play_sound(this, s, -1.0, 1.0);
                    let key = a.first().and_then(Value::as_i32).unwrap_or(1);
                    self.set_i_pub(this, "lastInputKey", key);
                    self.button_activate(this, key);
                }
                Value::Void
            }
            "SetToggle" if has(Kind::Toggle) => {
                self.toggle_set(this, bl(a, 0, false));
                Value::Void
            }
            "GetToggle" if has(Kind::Toggle) => Value::Bool(self.wb_pub(this, "bButtonPressed")),
            "ChangeToggle" if has(Kind::Toggle) => {
                self.toggle_changed(this);
                Value::Void
            }
            "SetToggleSounds" if has(Kind::Toggle) => {
                self.set_o_pub(this, "enableSound", obj_arg(a, 0));
                self.set_o_pub(this, "disableSound", obj_arg(a, 1));
                Value::Void
            }
            // ---- CheckboxWindow
            "SetCheckboxTextures" if has(Kind::Checkbox) => {
                self.set_o_pub(this, "toggleOff", obj_arg(a, 0));
                self.set_o_pub(this, "toggleOn", obj_arg(a, 1));
                self.set_f_pub(this, "textureWidth", fl(a, 2, 0.0));
                self.set_f_pub(this, "textureHeight", fl(a, 3, 0.0));
                reconfigure = true;
                Value::Void
            }
            "SetCheckboxSpacing" if has(Kind::Checkbox) => {
                self.set_f_pub(this, "checkboxSpacing", fl(a, 0, 0.0));
                reconfigure = true;
                Value::Void
            }
            "ShowCheckboxOnRightSide" if has(Kind::Checkbox) => {
                self.set_b_pub(this, "bRightSide", bl(a, 0, true));
                reconfigure = true;
                Value::Void
            }
            "SetCheckboxStyle" if has(Kind::Checkbox) => {
                let _ = self.set_prop(this, "checkboxStyle", 0, Value::Byte(iv(a, 0) as u8));
                Value::Void
            }
            "SetCheckboxColor" if has(Kind::Checkbox) => {
                if let Some(c) = a.first() {
                    let _ = self.set_prop(this, "checkboxColor", 0, c.clone());
                }
                Value::Void
            }
            // ---- BorderWindow
            "SetBorders" if has(Kind::Border) => {
                for (i, n) in ["borderTopLeft", "borderTopRight", "borderBottomLeft", "borderBottomRight", "borderLeft", "borderRight", "borderTop", "borderBottom", "center"].iter().enumerate() {
                    self.set_o_pub(this, n, obj_arg(a, i));
                }
                self.border_compute_margins(this);
                reconfigure = true;
                Value::Void
            }
            "SetBorderMargins" if has(Kind::Border) => {
                for (i, n) in ["childLeftMargin", "childRightMargin", "childTopMargin", "childBottomMargin"].iter().enumerate() {
                    self.set_f_pub(this, n, fl(a, i, 0.0));
                }
                reconfigure = true;
                Value::Void
            }
            "BaseMarginsFromBorder" if has(Kind::Border) => {
                self.set_b_pub(this, "bMarginsFromBorder", bl(a, 0, true));
                reconfigure = true;
                Value::Void
            }
            "EnableResizing" if has(Kind::Border) => {
                self.set_b_pub(this, "bResizeable", bl(a, 0, true));
                Value::Void
            }
            "SetMoveCursors" if has(Kind::Border) => {
                for (i, n) in ["moveCursor", "hMoveCursor", "vMoveCursor", "tlMoveCursor", "trMoveCursor"].iter().enumerate() {
                    self.set_o_pub(this, n, obj_arg(a, i));
                }
                Value::Void
            }
            _ => return None,
        };
        if reconfigure {
            self.ui_ask_parent_for_reconfigure(this);
        }
        Some(Ok(ret))
    }

    pub(crate) fn toggle_set(&mut self, w: u32, on: bool) {
        if self.wb_pub(w, "bButtonPressed") != on {
            self.set_b_pub(w, "bButtonPressed", on);
            self.button_change_appearance(w);
            self.toggle_changed(w);
        }
    }

    /// `ChangeToggle`: tells the window and then its ancestors until one handles `ToggleChanged`.
    fn toggle_changed(&mut self, w: u32) {
        let state = self.wb_pub(w, "bButtonPressed");
        let mut cur = w;
        while cur != 0 {
            let (r, _) = self.ui_event(cur, "ToggleChanged", vec![Value::Obj(w), Value::Bool(state)]);
            if r.as_bool().unwrap_or(false) {
                break;
            }
            cur = self.wo_pub(cur, "winParent");
        }
    }
}

impl Vm {
    pub(crate) fn native_widgets_pub(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        self.native_widgets(this, name, a)
    }
}

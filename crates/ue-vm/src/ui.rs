//! The window system of the `Extension` package (the game's whole user interface runs on it): the window tree, layout,
//! visibility, destruction and the events scripts receive. A window is an ordinary VM object of a `Window` class whose
//! script variables (`x`, `width`, `winParent`, `firstChild`, `bIsVisible`, ...) are the window's state; the natives work
//! on those variables directly, as the original native code worked on the same memory.
//!
//! Behaviour follows the original `XWindow`/`XRootWindow` code (see `docs/ui.md`). Not here yet: drawing, the graphics
//! context, mouse and keyboard routing, focus and tab groups.

use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

const HALIGN_FULL: i32 = 3;
const WINDOW_ROOT: i32 = 3;

/// The camera the world is drawn with.
#[derive(Clone, Copy, Debug)]
pub struct ViewInfo {
    pub pos: [f32; 3],
    pub rot: [i32; 3],
    /// Horizontal field of view in degrees.
    pub fov_deg: f32,
    /// Size of the 3D view in screen pixels.
    pub width: f32,
    pub height: f32,
}

/// Window system state that is not part of any object.
#[derive(Debug, Default)]
pub struct UiState {
    /// Root windows, newest first (the original's global list).
    pub roots: Vec<u32>,
    /// Per-state appearance of button windows (the original keeps it in a native array).
    pub buttons: std::collections::HashMap<u32, crate::ui_widgets::ButtonInfo>,
    /// Entries of text log windows: text, colour, age in seconds.
    pub logs: std::collections::HashMap<u32, Vec<(String, [u8; 4], f32)>>,
    /// Names of the tick positions of scale windows (`SetEnumeration`).
    pub scale_enums: std::collections::HashMap<u32, Vec<String>>,
    /// Rows and columns of list windows.
    pub lists: std::collections::HashMap<u32, crate::ui_list::ListData>,
    /// Undo history and caret state of edit windows, and the text last copied in any of them.
    pub edits: std::collections::HashMap<u32, crate::ui_edit::EditData>,
    pub clipboard: String,
    /// Event queue and display buffer of computer windows.
    pub computers: std::collections::HashMap<u32, crate::ui_computer::ComputerData>,
    /// `GameDirectory` objects: type (0 maps, 1 save games), filter, the listing.
    pub game_dirs: std::collections::HashMap<u32, crate::save_dir::GameDir>,
    /// The main view's master frame of the current frame (`XRootWindow::rootFrame`, the canvas's frame), for projecting
    /// world positions into window coordinates.
    pub view: Option<ViewInfo>,
    /// Each viewport window's last master frame (`XViewportWindow::viewportFrame`), made when it was last drawn.
    pub viewport_frames: std::collections::HashMap<u32, ViewInfo>,
    /// Graphics contexts by GC object, their saved states, the fonts loaded so far and the draw list of the frame.
    pub gcs: ue_registry::FxHashMap<u32, crate::ui_gc::GcState>,
    pub gc_stacks: ue_registry::FxHashMap<u32, Vec<crate::ui_gc::GcState>>,
    pub fonts: ue_registry::FxHashMap<u32, Option<std::rc::Rc<crate::ui_gc::FontData>>>,
    pub draw: Vec<crate::ui_gc::DrawCmd>,
    /// 3D views of the frame (`ViewportWindow`s) and whether the caller can draw them.
    pub views: Vec<crate::ui_gc::ViewCmd>,
    /// `GC.DrawActor`/`ClearZ` commands of the frame (only recorded while `wants_views`).
    pub actor_cmds: Vec<crate::ui_gc::ActorCmd>,
    /// Counter that orders views and actor commands.
    pub cmd_seq: usize,
    pub wants_views: bool,
    /// `Viewport.CurrentTime` of the frame being drawn: `UCanvas::DrawTile` shows a texture's animation frame at it.
    pub viewport_time: f64,
    /// Timers by window and the id counter.
    pub timers: std::collections::HashMap<u32, Vec<crate::ui_input::Timer>>,
    pub next_timer: i32,
    /// Texture sizes by resource id.
    pub tex_sizes: ue_registry::FxHashMap<u32, (f32, f32)>,
    /// `ui_kinds` per window class.
    pub(crate) kinds_cache: std::cell::RefCell<ue_registry::FxHashMap<ue_registry::ClassId, Vec<crate::ui_widgets::Kind>>>,
    /// Nesting depth of `ConfigureChild` calls (clipping is recomputed only at the outermost level).
    configure_depth: u32,
}

impl Vm {
    // ---- property access ----------------------------------------------------------------------------------

    fn wf(&self, o: u32, n: &'static str) -> f32 {
        self.get_prop_static(o, n, 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0)
    }
    fn wi(&self, o: u32, n: &'static str) -> i32 {
        self.get_prop_static(o, n, 0).ok().and_then(|v| v.as_i32()).unwrap_or(0)
    }
    fn wb(&self, o: u32, n: &'static str) -> bool {
        self.get_prop_static(o, n, 0).ok().and_then(|v| v.as_bool()).unwrap_or(false)
    }
    /// An object reference property; destroyed objects read as none.
    fn wo(&self, o: u32, n: &'static str) -> u32 {
        match self.get_prop_static(o, n, 0) {
            Ok(Value::Obj(r)) if self.is_live(r) => r,
            _ => 0,
        }
    }
    fn set_f(&mut self, o: u32, n: &'static str, v: f32) {
        let _ = self.set_prop_static(o, n, 0, Value::Float(v));
    }
    fn set_b(&mut self, o: u32, n: &'static str, v: bool) {
        let _ = self.set_prop_static(o, n, 0, Value::Bool(v));
    }
    fn set_i(&mut self, o: u32, n: &'static str, v: i32) {
        let _ = self.set_prop_static(o, n, 0, Value::Int(v));
    }
    fn set_o(&mut self, o: u32, n: &'static str, v: u32) {
        let _ = self.set_prop_static(o, n, 0, Value::Obj(v));
    }

    /// Delivers a script event to a window; errors in the event are logged and swallowed, as the engine keeps running.
    /// Returns the event's result and the arguments after the call (out parameters).
    pub(crate) fn ui_event(&mut self, w: u32, name: &str, args: Vec<Value>) -> (Value, Vec<Value>) {
        let mut args = args;
        if let Some(v) = self.ui_native_event(w, name, &mut args) {
            return (v, args);
        }
        self.ui_native_pre_event(w, name, &mut args);
        self.ui_script_event(w, name, args)
    }

    /// Delivers the script version of an event, without the native override.
    pub(crate) fn ui_script_event(&mut self, w: u32, name: &str, args: Vec<Value>) -> (Value, Vec<Value>) {
        let mut args = args;
        let Some(id) = self.reg.names.get(name) else { return (Value::Void, args) };
        let Some(loc) = self.resolve_function(w, id) else { return (Value::Void, args) };
        match self.call_function(w, loc, &mut args) {
            Ok(v) => (v, args),
            Err(e) => {
                let class = self.class_of(w).map(|c| self.reg.class(c).path.clone()).unwrap_or_default();
                self.log.push(format!("window event {class}.{name} failed: {e}"));
                (Value::Void, args)
            }
        }
    }

    // crate-visible accessors for the natives (same module family)
    pub(crate) fn ui_lock_pub(&mut self, w: u32) { self.ui_lock(w) }
    pub(crate) fn ui_unlock_pub(&mut self, w: u32) { self.ui_unlock(w) }
    pub(crate) fn wo_ref_pub(&self, o: u32, n: &'static str) -> u32 { self.wo_ref(o, n) }
    pub(crate) fn wf_pub(&self, o: u32, n: &'static str) -> f32 { self.wf(o, n) }
    pub(crate) fn wi_pub(&self, o: u32, n: &'static str) -> i32 { self.wi(o, n) }
    pub(crate) fn wb_pub(&self, o: u32, n: &'static str) -> bool { self.wb(o, n) }
    pub(crate) fn wo_pub(&self, o: u32, n: &'static str) -> u32 { self.wo(o, n) }
    pub(crate) fn set_f_pub(&mut self, o: u32, n: &'static str, v: f32) { self.set_f(o, n, v) }
    pub(crate) fn set_b_pub(&mut self, o: u32, n: &'static str, v: bool) { self.set_b(o, n, v) }
    pub(crate) fn set_i_pub(&mut self, o: u32, n: &'static str, v: i32) { self.set_i(o, n, v) }
    pub(crate) fn set_o_pub(&mut self, o: u32, n: &'static str, v: u32) { self.set_o(o, n, v) }
    pub(crate) fn ui_add_child_pub(&mut self, p: u32, c: u32, top: bool) { self.ui_add_child(p, c, top) }
    pub(crate) fn ui_set_sensitivity_pub(&mut self, w: u32, s: bool) { self.ui_set_sensitivity(w, s) }
    pub(crate) fn ui_sensitive_recursive_pub(&self, w: u32) -> bool { self.ui_sensitive_recursive(w) }
    pub(crate) fn ui_root_of_pub(&self, w: u32) -> u32 { self.ui_root_of(w) }
    /// An object reference property as stored (also resources such as textures, which are path ids, not instances).
    pub(crate) fn wo_ref(&self, o: u32, n: &'static str) -> u32 {
        match self.get_prop_static(o, n, 0) {
            Ok(Value::Obj(r)) => r,
            _ => 0,
        }
    }

    /// Size in texels of a texture resource (0, 0 for none or unknown). Cached.
    pub fn texture_size(&mut self, tex: u32) -> (f32, f32) {
        if tex == 0 {
            return (0.0, 0.0);
        }
        if let Some(t) = self.runtime_textures.get(&tex) {
            return (t.usize as f32, t.vsize as f32);
        }
        if let Some(&s) = self.ui.tex_sizes.get(&tex) {
            return s;
        }
        let path = self.reg.obj_path(ue_registry::ObjId(tex)).to_string();
        self.reg.ensure_package_for_path(&path);
        let size = (|| {
            let (pi, ei) = self.reg.export_by_path(&path)?;
            let class = self.reg.class_of_export(pi, ei)?;
            let (inst, _) = self.instantiate_export(class, pi, ei).ok()?;
            let off = |vm: &Vm, n: &str| vm.reg.find_prop(class, n).map(|p| p.offset as usize);
            let rd = |o: usize| i32::from_le_bytes(inst.data[o..o + 4].try_into().unwrap()) as f32;
            Some((rd(off(self, "USize")?), rd(off(self, "VSize")?)))
        })()
        .unwrap_or((0.0, 0.0));
        self.ui.tex_sizes.insert(tex, size);
        size
    }

    pub(crate) fn color_value_pub(&self, rgba: [u8; 4]) -> Value { self.color_value(rgba) }
    pub(crate) fn ui_sensitive_pub(&self, w: u32) -> bool { self.ui_sensitive_recursive(w) }
    /// Does the window hold the keyboard focus of its root? (focus is not routed yet: only a root's `focusWindow` counts)
    pub(crate) fn ui_focus_window_is(&self, w: u32) -> bool {
        let root = self.ui_root_of(w);
        root != 0 && self.wo(root, "focusWindow") == w
    }
    pub(crate) fn ui_set_color_pub(&mut self, w: u32, c: &str, p: &str, rgba: [u8; 4]) { self.ui_set_color(w, c, p, rgba) }

    // ---- tree --------------------------------------------------------------------------------------------

    /// `AddChild`: puts `child` last (drawn on top) or first among the children of `parent`.
    fn ui_add_child(&mut self, parent: u32, child: u32, top: bool) {
        if child == 0 {
            return;
        }
        let old = self.wo(child, "winParent");
        if old != 0 {
            self.ui_remove_child(old, child);
        }
        if !top {
            let first = self.wo(parent, "firstChild");
            self.set_o(child, "prevSibling", 0);
            self.set_o(child, "nextSibling", first);
            if first == 0 {
                self.set_o(parent, "lastChild", child);
            } else {
                self.set_o(first, "prevSibling", child);
            }
            self.set_o(parent, "firstChild", child);
        } else {
            let last = self.wo(parent, "lastChild");
            self.set_o(child, "nextSibling", 0);
            self.set_o(child, "prevSibling", last);
            if last == 0 {
                self.set_o(parent, "firstChild", child);
            } else {
                self.set_o(last, "nextSibling", child);
            }
            self.set_o(parent, "lastChild", child);
        }
        self.set_o(child, "winParent", parent);
    }

    fn ui_remove_child(&mut self, parent: u32, child: u32) {
        if child == 0 {
            return;
        }
        let (prev, next) = (self.wo(child, "prevSibling"), self.wo(child, "nextSibling"));
        if prev == 0 {
            self.set_o(parent, "firstChild", next);
        } else {
            self.set_o(prev, "nextSibling", next);
        }
        if next == 0 {
            self.set_o(parent, "lastChild", prev);
        } else {
            self.set_o(next, "prevSibling", prev);
        }
        self.set_o(child, "nextSibling", 0);
        self.set_o(child, "prevSibling", 0);
        self.set_o(child, "winParent", 0);
    }

    /// Is the window and every ancestor visible?
    pub(crate) fn ui_visible_recursive(&self, w: u32) -> bool {
        let mut cur = w;
        while cur != 0 {
            if !self.wb(cur, "bIsVisible") {
                return false;
            }
            cur = self.wo(cur, "winParent");
        }
        true
    }

    fn ui_sensitive_recursive(&self, w: u32) -> bool {
        let mut cur = w;
        while cur != 0 {
            if !self.wb(cur, "bIsSensitive") {
                return false;
            }
            cur = self.wo(cur, "winParent");
        }
        true
    }

    fn ui_root_of(&self, w: u32) -> u32 {
        let mut top = w;
        while top != 0 {
            let p = self.wo(top, "winParent");
            if p == 0 {
                break;
            }
            top = p;
        }
        if top != 0 && self.wi(top, "windowType") >= WINDOW_ROOT {
            top
        } else {
            0
        }
    }

    // ---- creation and destruction ---------------------------------------------------------------------------

    /// `XWindow::Init`: the state a window starts with, then linking it under `parent`.
    fn ui_init(&mut self, w: u32, parent: u32) {
        let fl = |vm: &mut Vm, n: &'static str, v: f32| vm.set_f(w, n, v);
        let bl = |vm: &mut Vm, n: &'static str, v: bool| vm.set_b(w, n, v);
        self.set_i(w, "windowType", 0);
        self.set_o(w, "parentOwner", parent);
        for (n, v) in [("bIsVisible", false), ("bBeingDestroyed", false), ("bIsSelectable", false), ("bIsSensitive", true), ("bDrawRawBackground", true), ("bStretchBackground", false), ("bSmoothBackground", false), ("bSpecialText", true), ("bNeedsReconfigure", true), ("bLastHeightSpecified", true), ("bLastWidthSpecified", true), ("bConfigured", false), ("bNeedsQuery", true), ("bTickEnabled", false)] {
            bl(self, n, v);
        }
        self.set_i(w, "lockCount", 0);
        let _ = self.set_prop(w, "backgroundStyle", 0, Value::Byte(1));
        self.ui_set_color(w, "textColor", "textPlane", [0, 255, 0, 255]);
        self.ui_set_color(w, "tileColor", "tilePlane", [255, 255, 255, 255]);
        let _ = self.set_prop(w, "defaultCursorColor", 0, self.color_value([255, 255, 255, 255]));
        for (n, v) in [("textVSpacing", 1.0), ("x", 0.0), ("y", 0.0), ("width", 10.0), ("height", 10.0), ("hMargin0", 0.0), ("hMargin1", 0.0), ("vMargin0", 0.0), ("vMargin1", 0.0), ("hardcodedWidth", -1.0), ("hardcodedHeight", -1.0), ("defaultHotX", -1.0), ("defaultHotY", -1.0), ("lastSpecifiedWidth", 0.0), ("lastSpecifiedHeight", 0.0), ("lastQueryWidth", 0.0), ("lastQueryHeight", 0.0), ("soundVolume", 1.0)] {
            fl(self, n, v);
        }
        let _ = self.set_prop(w, "winHAlign", 0, Value::Byte(0));
        let _ = self.set_prop(w, "winVAlign", 0, Value::Byte(0));
        self.set_i(w, "maxClicks", 0);
        self.set_i(w, "rowMajorIndex", -1);
        self.set_i(w, "colMajorIndex", -1);
        for n in ["background", "defaultCursor", "focusSound", "unfocusSound", "visibleSound", "invisibleSound", "clientObject", "winParent", "firstChild", "lastChild", "prevSibling", "nextSibling"] {
            self.set_o(w, n, 0);
        }
        if parent != 0 {
            self.ui_add_child(parent, w, true);
            self.ui_event(parent, "ChildAdded", vec![Value::Obj(w)]);
            let mut anc = parent;
            while anc != 0 {
                self.ui_event(anc, "DescendantAdded", vec![Value::Obj(w)]);
                anc = self.wo(anc, "winParent");
            }
        }
        // fonts come from the nearest ancestor that has any
        let (mut normal, mut bold) = (0, 0);
        let mut anc = self.wo(w, "winParent");
        while anc != 0 {
            let (n, b) = (self.wo(anc, "normalFont"), self.wo(anc, "boldFont"));
            if n != 0 || b != 0 {
                normal = n;
                bold = b;
                break;
            }
            anc = self.wo(anc, "winParent");
        }
        self.set_o(w, "normalFont", normal);
        self.set_o(w, "boldFont", bold);
        self.ui_make_gcs(w, parent);
        self.ui_sync_gc_fonts(w);
        self.ui_native_init(w, parent);
    }

    fn color_value(&self, rgba: [u8; 4]) -> Value {
        let sid = self.reg.struct_id_by_path("Core.Object.Color").expect("Color struct");
        Value::Struct(sid, rgba.to_vec())
    }

    /// Stores a colour and the plane (0..1 floats) derived from it.
    fn ui_set_color(&mut self, w: u32, color: &str, plane: &str, rgba: [u8; 4]) {
        let _ = self.set_prop(w, color, 0, self.color_value(rgba));
        if let Some(sid) = self.reg.struct_id_by_path("Core.Object.Plane") {
            let k = 1.0 / 255.0;
            let mut b = Vec::with_capacity(16);
            for f in [rgba[0] as f32 * k, rgba[1] as f32 * k, rgba[2] as f32 * k, 0.0f32] {
                b.extend_from_slice(&f.to_le_bytes());
            }
            let _ = self.set_prop(w, plane, 0, Value::Struct(sid, b));
        }
    }

    /// The port's version line on the main menu, beside the original's (`MenuMain.ShowVersionInfo`: a white text window
    /// at the menu's bottom right): the same kind of window, made with the same calls, at the bottom left of the same row.
    fn ui_add_port_version(&mut self, menu: u32) {
        let Some(text) = self.port_version.clone() else { return };
        let Some(class) = self.reg.class_by_name("TextWindow") else { return };
        let path = self.reg.class(class).path.clone();
        let class_ref = Value::Obj(self.reg.obj_id(&path).0);
        let Ok(Value::Obj(t)) = self.call_by_name(menu, "NewChild", vec![class_ref]) else { return };
        if t == 0 {
            return;
        }
        // HALIGN_Left 0, VALIGN_Bottom 2
        let _ = self.call_by_name(t, "SetTextMargins", vec![Value::Float(0.0), Value::Float(0.0)]);
        let _ = self.call_by_name(t, "SetWindowAlignments", vec![Value::Byte(0), Value::Byte(2)]);
        let _ = self.call_by_name(t, "SetTextColorRGB", vec![Value::Int(255), Value::Int(255), Value::Int(255)]);
        let _ = self.call_by_name(t, "SetTextAlignments", vec![Value::Byte(0), Value::Byte(2)]);
        let _ = self.call_by_name(t, "SetText", vec![Value::Str(text)]);
    }

    /// `CreateNewWindow`: constructs a window of `class` as a child of `parent` and runs its `InitWindow`.
    pub(crate) fn ui_create_window(&mut self, class: u32, parent: u32, show: bool) -> Res<u32> {
        let Some(c) = self.class_of_ref(class) else { return Ok(0) };
        let w = self.new_object(c)?;
        self.ui_init(w, parent);
        self.ui_lock(w);
        self.ui_event(w, "InitWindow", vec![]);
        if self.port_version.is_some() && self.is_class(w, "MenuMain") {
            self.ui_add_port_version(w);
        }
        if show {
            self.ui_set_visibility(w, true);
        }
        self.ui_ask_parent_for_reconfigure(w);
        self.ui_unlock(w);
        Ok(if self.wb(w, "bBeingDestroyed") { 0 } else { w })
    }

    fn ui_lock(&mut self, w: u32) {
        let n = self.wi(w, "lockCount");
        self.set_i(w, "lockCount", n + 1);
    }

    fn ui_unlock(&mut self, w: u32) {
        let n = self.wi(w, "lockCount");
        if n > 0 {
            self.set_i(w, "lockCount", n - 1);
            if n == 1 && self.wb(w, "bBeingDestroyed") {
                self.ui_safe_destroy(w);
            }
        }
    }

    /// `PreDestroy`: hides the window, drops its children, runs `DestroyWindow` and unlinks it.
    fn ui_pre_destroy(&mut self, w: u32) {
        if self.wb(w, "bBeingDestroyed") {
            return;
        }
        if self.ui_visible_recursive(w) {
            self.ui_set_visibility(w, false);
        }
        self.ui_set_selectability(w, false);
        // children first
        let mut c = self.wo(w, "firstChild");
        while c != 0 {
            let next = self.wo(c, "nextSibling");
            self.ui_safe_destroy(c);
            c = next;
        }
        self.ui_event(w, "DestroyWindow", vec![]);
        self.ui_clean_up(w);
        self.set_b(w, "bBeingDestroyed", true);
    }

    fn ui_clean_up(&mut self, w: u32) {
        self.ui.lists.remove(&w);
        self.ui.edits.remove(&w);
        self.ui.scale_enums.remove(&w);
        self.ui.logs.remove(&w);
        let root = self.ui_root_of(w);
        let parent = self.wo(w, "winParent");
        if parent != 0 {
            self.ui_event(parent, "ChildRemoved", vec![Value::Obj(w)]);
            let mut anc = parent;
            while anc != 0 {
                self.ui_event(anc, "DescendantRemoved", vec![Value::Obj(w)]);
                // the native parts: the root forgets its last pressed window (1003b5c0), a modal its preferred
                // focus (10037400)
                if self.wi(anc, "windowType") >= WINDOW_ROOT && self.wo(anc, "lastButtonWindow") == w {
                    self.set_o(anc, "lastButtonWindow", 0);
                }
                if self.wi(anc, "windowType") > 1 && self.wo(anc, "preferredFocus") == w {
                    self.set_o(anc, "preferredFocus", 0);
                }
                anc = self.wo(anc, "winParent");
            }
        }
        if root != 0 {
            for n in ["lastMouseWindow", "grabbedWindow", "focusWindow"] {
                if self.wo(root, n) == w {
                    self.set_o(root, n, 0);
                }
            }
        }
        let parent = self.wo(w, "winParent");
        if parent != 0 {
            self.ui_remove_child(parent, w);
        }
    }

    /// `SafeDestroy`: destroys now unless the window is locked (then when the last lock goes).
    pub(crate) fn ui_safe_destroy(&mut self, w: u32) {
        self.ui_pre_destroy(w);
        if self.wi(w, "lockCount") < 1 {
            if let Some(i) = crate::ref_instance(w) {
                if let Some(o) = self.objects.get_mut(i) {
                    o.alive = false;
                }
            }
            self.ui.roots.retain(|&r| r != w);
        }
    }

    // ---- visibility ----------------------------------------------------------------------------------------

    pub(crate) fn ui_set_visibility(&mut self, w: u32, vis: bool) {
        if self.wb(w, "bIsVisible") == vis {
            return;
        }
        let parent = self.wo(w, "winParent");
        if parent == 0 {
            self.ui_set_child_visibility(w, vis);
        } else {
            self.ui_event(parent, "ChildRequestedVisibilityChange", vec![Value::Obj(w), Value::Bool(vis)]);
        }
    }

    pub(crate) fn ui_set_child_visibility(&mut self, w: u32, vis: bool) {
        if self.wb(w, "bIsVisible") == vis {
            return;
        }
        let before = self.ui_visible_recursive(w);
        self.set_b(w, "bIsVisible", vis);
        let after = self.ui_visible_recursive(w);
        if before != after {
            self.ui_check_focus(w);
            self.ui_check_grabbed_window(w);
            self.set_b(w, "bNeedsReconfigure", true);
            self.ui_recompute_init_tree(w, after);
            self.ui_invoke_visibility_change(w, after);
            self.ui_reconfigure_tree(w, false);
        }
    }

    /// Counts windows that still wait for `WindowReady` in their root.
    fn ui_recompute_init_tree(&mut self, w: u32, now_visible: bool) {
        let root = self.ui_root_of(w);
        if !self.wb(w, "bIsInitialized") && root != 0 {
            let n = self.wi(root, "initCount");
            self.set_i(root, "initCount", if now_visible { n + 1 } else { n - 1 });
        }
        let mut c = self.wo(w, "firstChild");
        while c != 0 {
            if self.wb(c, "bIsVisible") {
                self.ui_recompute_init_tree(c, now_visible);
            }
            c = self.wo(c, "nextSibling");
        }
    }

    /// `XWindow::PlaySound` (Extension.dll 1004ffb0/10050050) for a sound centred on the window.
    pub(crate) fn ui_play_sound(&mut self, w: u32, sound: u32, volume: f32, pitch: f32) {
        let (x, y) = (self.wf(w, "width") * 0.5, self.wf(w, "height") * 0.5);
        self.ui_play_sound_at(w, sound, volume, pitch, x, y);
    }

    /// `XWindow::PlaySound` with a point of the window: needs a root with a player pawn (volume -1 = the window's
    /// `soundVolume`). The sound plays one unit from the player's location (radius 100, volume times 100/99 so that the
    /// distance costs nothing), in the direction of the view yaw turned by the pan: when the window and all its parents
    /// are visible and the root has `bPositionalSound`, the point's x across the root maps to -1..1, i.e. up to a
    /// quarter turn left or right (`pan * 16384`); otherwise straight ahead. A hidden window still plays.
    pub(crate) fn ui_play_sound_at(&mut self, w: u32, sound: u32, volume: f32, pitch: f32, x: f32, _y: f32) {
        let root = self.ui_root_of(w);
        let pawn = if root != 0 { self.wo(root, "parentPawn") } else { 0 };
        if sound == 0 || root == 0 || pawn == 0 || !self.is_live(pawn) {
            return;
        }
        let volume = if volume == -1.0 { self.wf(w, "soundVolume") } else { volume };
        let pan = if self.ui_visible_recursive(w) && self.wb(root, "bPositionalSound") {
            // `ConvertCoordinates` to the root, as the native does
            let (wg, rg) = (self.wo_ref_pub(w, "wGc"), self.wo_ref_pub(root, "wGc"));
            let rx = x + self.gc_state(wg).clip[0] - self.gc_state(rg).clip[0];
            let width = self.wf(root, "width").max(1.0);
            ((rx + rx) / width - 1.0).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let (radius, dist) = (100.0f32, 1.0f32);
        let gain = if radius - dist > 0.0 { radius / (radius - dist) } else { 1.0 };
        let yaw = match self.get_prop(pawn, "ViewRotation", 0) {
            Ok(Value::Rotator(r)) => r[1],
            _ => 0,
        }
        .wrapping_add((pan * 16384.0) as i32);
        let a = (yaw & 0xffff) as f32 * std::f32::consts::TAU / 65536.0;
        let o = self.offsets();
        let l = self.vec_at(pawn, o.location);
        let at = [l[0] + a.cos() * dist, l[1] + a.sin() * dist, l[2]];
        // straight to the audio subsystem: id 0, no actor (engine-sounds.md 5)
        self.audio_play_sound(0, 0, sound, at, volume * gain, radius, pitch);
    }

    fn ui_invoke_visibility_change(&mut self, w: u32, vis: bool) {
        self.ui_event(w, "VisibilityChanged", vec![Value::Bool(vis)]);
        let s = self.wo(w, if vis { "visibleSound" } else { "invisibleSound" });
        self.ui_play_sound(w, s, -1.0, 1.0);
        let mut c = self.wo(w, "firstChild");
        while c != 0 {
            let next = self.wo(c, "nextSibling");
            if self.wb(c, "bIsVisible") {
                self.ui_invoke_visibility_change(c, vis);
            }
            c = next;
        }
    }

    fn ui_set_sensitivity(&mut self, w: u32, s: bool) {
        if self.wb(w, "bIsSensitive") == s {
            return;
        }
        let before = self.ui_sensitive_recursive(w);
        self.set_b(w, "bIsSensitive", s);
        let after = self.ui_sensitive_recursive(w);
        if before != after {
            self.ui_check_focus(w);
            self.ui_check_grabbed_window(w);
            self.ui_event(w, "SensitivityChanged", vec![Value::Bool(after)]);
        }
    }

    pub(crate) fn ui_set_selectability(&mut self, w: u32, s: bool) {
        if self.wb(w, "bIsSelectable") != s {
            self.set_b(w, "bIsSelectable", s);
            self.ui_check_focus(w);
        }
    }

    // ---- layout --------------------------------------------------------------------------------------------

    /// `AskParentForReconfigure`: this window's size or alignment changed; if it is on screen, the parent decides.
    pub(crate) fn ui_ask_parent_for_reconfigure(&mut self, w: u32) {
        self.set_b(w, "bNeedsReconfigure", true);
        self.set_b(w, "bNeedsQuery", true);
        if !self.ui_visible_recursive(w) {
            return;
        }
        let parent = self.wo(w, "winParent");
        if parent == 0 {
            self.set_b(w, "bNeedsReconfigure", false);
            self.ui_event(w, "ConfigurationChanged", vec![]);
        } else {
            let (handled, _) = self.ui_event(parent, "ChildRequestedReconfiguration", vec![Value::Obj(w)]);
            if !handled.as_bool().unwrap_or(false) {
                self.ui_ask_parent_for_reconfigure(parent);
            }
            if self.wb(w, "bNeedsReconfigure") {
                self.ui_resize_child(w);
            }
        }
    }

    fn ui_invalidate_tree(&mut self, w: u32) {
        self.set_b(w, "bNeedsReconfigure", true);
        let mut c = self.wo(w, "firstChild");
        while c != 0 {
            self.ui_invalidate_tree(c);
            c = self.wo(c, "nextSibling");
        }
    }

    fn ui_reconfigure_tree(&mut self, w: u32, invalidate: bool) {
        if invalidate {
            self.ui_invalidate_tree(w);
        }
        if !self.ui_visible_recursive(w) {
            return;
        }
        if self.wb(w, "bNeedsReconfigure") {
            self.ui_ask_parent_for_reconfigure(w);
        }
        let mut c = self.wo(w, "firstChild");
        while c != 0 {
            let next = self.wo(c, "nextSibling");
            self.ui_reconfigure_tree(c, false);
            c = next;
        }
    }

    /// `QueryPreferredSize`: asks the window (through `ParentRequestedPreferredSize`) for the size it wants. A specified
    /// width or height (by the caller or hard-coded) is kept. Results are cached until the window needs a new query.
    pub(crate) fn ui_query_preferred_size(&mut self, w: u32, w_spec: bool, w_in: f32, h_spec: bool, h_in: f32) -> (f32, f32) {
        let (mut w_spec, mut h_spec) = (w_spec, h_spec);
        let (mut pw, mut ph) = (w_in, h_in);
        if !w_spec {
            let hw = self.wf(w, "hardcodedWidth");
            if hw < 0.0 {
                pw = -1.0;
            } else {
                pw = hw;
                w_spec = true;
            }
        }
        if !h_spec {
            let hh = self.wf(w, "hardcodedHeight");
            if hh < 0.0 {
                ph = -1.0;
            } else {
                ph = hh;
                h_spec = true;
            }
        }
        if !w_spec || !h_spec {
            let cached = !self.wb(w, "bNeedsQuery")
                && self.wb(w, "bLastWidthSpecified") == w_spec
                && self.wb(w, "bLastHeightSpecified") == h_spec
                && self.wf(w, "lastSpecifiedWidth") == pw
                && self.wf(w, "lastSpecifiedHeight") == ph;
            if cached {
                pw = self.wf(w, "lastQueryWidth");
                ph = self.wf(w, "lastQueryHeight");
            } else {
                self.set_f(w, "lastSpecifiedHeight", ph);
                self.set_f(w, "lastSpecifiedWidth", pw);
                self.set_b(w, "bNeedsQuery", false);
                self.set_b(w, "bLastWidthSpecified", w_spec);
                self.set_b(w, "bLastHeightSpecified", h_spec);
                let (_, out) = self.ui_event(w, "ParentRequestedPreferredSize", vec![Value::Bool(w_spec), Value::Float(pw), Value::Bool(h_spec), Value::Float(ph)]);
                if !w_spec {
                    pw = out.get(1).and_then(Value::as_f32).unwrap_or(pw);
                }
                if !h_spec {
                    ph = out.get(3).and_then(Value::as_f32).unwrap_or(ph);
                }
                self.set_f(w, "lastQueryHeight", ph);
                self.set_f(w, "lastQueryWidth", pw);
            }
            if pw < 0.0 {
                let bg = self.wo_ref(w, "background");
                pw = if bg == 0 { self.wf(w, "width") } else { self.texture_size(bg).0 };
            }
            if ph < 0.0 {
                let bg = self.wo_ref(w, "background");
                ph = if bg == 0 { self.wf(w, "height") } else { self.texture_size(bg).1 };
            }
        }
        (pw, ph)
    }

    pub(crate) fn ui_query_granularity(&mut self, w: u32) -> (f32, f32) {
        let (_, out) = self.ui_event(w, "ParentRequestedGranularity", vec![Value::Float(1.0), Value::Float(1.0)]);
        let g = |i: usize| out.get(i).and_then(Value::as_f32).filter(|v| *v >= 1.0).unwrap_or(1.0);
        (g(0), g(1))
    }

    /// `ResizeChild`: computes the window's place from its alignment, margins and preferred size inside its parent.
    pub(crate) fn ui_resize_child(&mut self, w: u32) {
        let parent = self.wo(w, "winParent");
        let (pw, ph) = if parent != 0 { (self.wf(parent, "width"), self.wf(parent, "height")) } else { (0.0, 0.0) };
        let (h_align, v_align) = (self.wi(w, "winHAlign"), self.wi(w, "winVAlign"));
        let (hm0, hm1, vm0, vm1) = (self.wf(w, "hMargin0"), self.wf(w, "hMargin1"), self.wf(w, "vMargin0"), self.wf(w, "vMargin1"));
        let full_w = h_align == HALIGN_FULL;
        let full_h = v_align == 3;
        let w_in = if full_w { pw - (hm1 + hm0) } else { 0.0 };
        let h_in = if full_h { ph - (vm1 + vm0) } else { 0.0 };
        let (cw, ch) = self.ui_query_preferred_size(w, full_w, w_in, full_h, h_in);
        let x = match h_align {
            1 => ((pw - cw) / 2.0).trunc() + hm0,
            2 => (pw - cw) - hm0,
            _ => hm0,
        };
        let y = match v_align {
            1 => ((ph - ch) / 2.0).trunc() + vm0,
            2 => (ph - ch) - vm0,
            _ => vm0,
        };
        self.ui_configure_child(w, x, y, cw, ch);
    }

    /// `ConfigureChild`: the parent has decided the window's place; a visible window takes it and learns of a size change.
    pub(crate) fn ui_configure_child(&mut self, w: u32, x: f32, y: f32, width: f32, height: f32) {
        self.ui_lock(w);
        self.ui.configure_depth += 1;
        let was = self.wb(w, "bNeedsReconfigure");
        self.set_b(w, "bConfigured", true);
        if self.wb(w, "bIsVisible") {
            let changed = self.wf(w, "width") != width || self.wf(w, "height") != height || was;
            self.set_b(w, "bNeedsReconfigure", false);
            self.set_f(w, "x", x);
            self.set_f(w, "y", y);
            self.set_f(w, "width", width);
            self.set_f(w, "height", height);
            if changed {
                self.ui_change_configuration(w);
            }
            if self.ui.configure_depth == 1 || self.wi(w, "windowType") != 0 {
                self.ui_clip_tree(w);
            }
        }
        self.ui.configure_depth -= 1;
        self.ui_unlock(w);
    }

    /// `ChangeConfiguration`: the window's size changed; it lays out its children (`ConfigurationChanged`), then the
    /// children aligned to it are sized.
    fn ui_change_configuration(&mut self, w: u32) {
        let mut c = self.wo(w, "firstChild");
        while c != 0 {
            self.set_b(c, "bConfigured", true);
            if self.wb(c, "bIsVisible") && (self.wi(c, "winHAlign") != 0 || self.wi(c, "winVAlign") != 0) {
                self.set_b(c, "bConfigured", false);
            }
            c = self.wo(c, "nextSibling");
        }
        self.ui_event(w, "ConfigurationChanged", vec![]);
        let mut c = self.wo(w, "firstChild");
        while c != 0 {
            let next = self.wo(c, "nextSibling");
            if self.wb(c, "bIsVisible") && (self.wi(c, "winHAlign") != 0 || self.wi(c, "winVAlign") != 0) && !self.wb(c, "bConfigured") {
                self.ui_resize_child(c);
            }
            c = next;
        }
    }

    // ---- root window ---------------------------------------------------------------------------------------

    /// `APlayerPawnExt::ConstructRootWindow`: builds the root window of the class `Engine.Engine.Root` names, owned by
    /// the player, runs its `InitWindow` and shows it. Does nothing if the pawn already has one.
    pub(crate) fn ui_construct_root_window(&mut self, pawn: u32) -> Res<()> {
        if self.wo(pawn, "rootWindow") != 0 {
            return Ok(());
        }
        let Some(spec) = self.reg.config_value("System", "Engine.Engine", "Root") else {
            self.log.push("no root window class configured (Engine.Engine Root)".into());
            return Ok(());
        };
        let spec = ue_config::unquote(&spec);
        let Some(class) = self.reg.class_by_path(&spec) else {
            self.log.push(format!("root window class {spec} not found"));
            return Ok(());
        };
        let root = self.new_object(class)?;
        self.ui_init_root(root, pawn);
        self.ui_event(root, "InitWindow", vec![]);
        self.ui_set_visibility(root, true);
        Ok(())
    }

    /// `XRootWindow::DestroyAllWindows` (1003a330, from `XGameEngineExt::Destroy` when the engine shuts down): every
    /// root window `SafeDestroy`ed in turn until none is left (their trees' `DestroyWindow` events run).
    pub fn ui_destroy_all_windows(&mut self) {
        let mut guard = 0;
        while let Some(&root) = self.ui.roots.first() {
            self.ui_safe_destroy(root);
            guard += 1;
            if guard > 64 {
                self.ui.roots.clear();
            }
        }
    }

    /// `XGameEngineExt::Browse` (Extension 10026330): after a level or saved game is loaded, `InitRootWindow` on the
    /// first viewport's actor (no class test; nothing when it has a root already).
    pub fn ui_after_browse(&mut self, pawn: u32) {
        if pawn != 0 && self.is_live(pawn) {
            let _ = self.ui_construct_root_window(pawn);
        }
    }

    /// `XRootWindow::Init`.
    fn ui_init_root(&mut self, root: u32, pawn: u32) {
        self.ui_init(root, 0);
        self.set_i(root, "windowType", WINDOW_ROOT);
        for (n, v) in [("initCount", 0), ("handleKeyboardRef", 0), ("handleMouseRef", 0), ("clickCount", 0), ("lastButtonType", 0)] {
            self.set_i(root, n, v);
        }
        for (n, v) in [("mouseX", 0.0), ("mouseY", 0.0), ("prevMouseX", -1.0), ("prevMouseY", -1.0), ("firstButtonMouseX", -1.0), ("firstButtonMouseY", -1.0), ("renderX", 0.0), ("renderY", 0.0), ("renderWidth", 0.0), ("renderHeight", 0.0), ("frameTimer", 0.0), ("multiClickTimeout", 0.5), ("maxMouseDist", 10.0), ("lastButtonPress", 0.0)] {
            self.set_f(root, n, v);
        }
        for (n, v) in [("bMouseMoved", true), ("bCursorVisible", true), ("bMouseButtonLocked", false), ("bMouseMoveLocked", false), ("bPositionalSound", true), ("bRender", true), ("bClipRender", false), ("bTickEnabled", true), ("bShowStats", false), ("bShowFrames", false)] {
            self.set_b(root, n, v);
        }
        for n in ["grabbedWindow", "focusWindow", "lastMouseWindow", "lastButtonWindow"] {
            self.set_o(root, n, 0);
        }
        self.set_o(pawn, "rootWindow", root);
        self.set_o(root, "parentPawn", pawn);
        self.ui.roots.insert(0, root);
    }

    /// `XRootWindow::ResizeRoot`: the root takes the size of the screen divided by a whole multiplier, the largest one for
    /// which the result stays at least 640x480 (so the UI is not tiny on large screens: 1920x1080 is drawn as 960x540).
    pub fn ui_resize_root(&mut self, root: u32, screen_w: f32, screen_h: f32) {
        let m = ((screen_w / 640.0) as i32).max(1).min(((screen_h / 480.0) as i32).max(1));
        for n in ["x", "y", "hMargin0", "hMargin1", "vMargin0", "vMargin1"] {
            self.set_f(root, n, 0.0);
        }
        let _ = self.set_prop(root, "winHAlign", 0, Value::Byte(0));
        let _ = self.set_prop(root, "winVAlign", 0, Value::Byte(0));
        self.set_i(root, "hMultiplier", m);
        self.set_i(root, "vMultiplier", m);
        let (w, h) = (screen_w / m as f32, screen_h / m as f32);
        self.set_f(root, "hardcodedWidth", w);
        self.set_f(root, "hardcodedHeight", h);
        if self.wf(root, "width") != w || self.wf(root, "height") != h {
            self.ui_configure_child(root, 0.0, 0.0, w, h);
        }
        // `ResizeRoot` ends with `ClampMousePosition`
        self.ui_clamp_mouse(root);
    }

    /// Tells the player's root window the screen size (what the engine does when it renders: `ResizeRoot`) and lets the
    /// windows that are ready know it (`WindowReady`). Returns the root window (0 if the player has none).
    pub fn ui_set_screen(&mut self, pawn: u32, screen_w: f32, screen_h: f32) -> u32 {
        let root = self.wo(pawn, "rootWindow");
        if root != 0 {
            self.ui_resize_root(root, screen_w, screen_h);
            self.ui_initialize_windows(root);
        }
        root
    }

    /// The child of `parent` (any depth, first found) whose class is `class_name` or derives from it.
    pub fn ui_find_child(&self, parent: u32, class_name: &str) -> u32 {
        let Some(target) = self.reg.class_by_name(class_name) else { return 0 };
        let mut c = self.wo(parent, "firstChild");
        while c != 0 {
            if self.class_of(c).map(|k| self.is_a(k, target)).unwrap_or(false) {
                return c;
            }
            let deep = self.ui_find_child(c, class_name);
            if deep != 0 {
                return deep;
            }
            c = self.wo(c, "nextSibling");
        }
        0
    }

    /// Sets the camera used by `ConvertVectorToCoordinates` (call before painting each frame).
    pub fn ui_set_view(&mut self, view: Option<ViewInfo>) {
        self.ui.view = view;
    }

    /// The viewport time of the frame about to be drawn (animated UI textures follow it).
    pub fn ui_set_time(&mut self, time: f64) {
        self.ui.viewport_time = time;
    }

    /// `ConvertVectorToCoordinates` of window `w`: whether the world position `p` lies in front of the frame the window
    /// projects through, and where it lands in the window's coordinates.
    /// - The root window (`XRootWindow` 10039f10) projects through its `rootFrame`, the main view's master frame: the
    ///   view-space point (kept in extended precision until stored) behind or within one unit of the eye gives the
    ///   frame's centre truncated and false; otherwise `Proj.Z / Z` stored as a float, then
    ///   `ftol((x * scale + FX2) / hMultiplier + renderX)` and the same downwards.
    /// - A viewport window (`XViewportWindow` 1004afc0) projects through the frame it last drew, without the root's
    ///   multipliers and offsets.
    /// - Any other window (`XWindow` 1004faf0) asks its parent and subtracts its own position; without a parent: half
    ///   its size and false.
    pub fn ui_convert_vector(&mut self, w: u32, p: [f32; 3]) -> (bool, f32, f32) {
        let parent = self.wo(w, "winParent");
        if self.wi(w, "windowType") >= WINDOW_ROOT {
            let Some(v) = self.ui.view else { return (false, self.wf(w, "width") * 0.5, self.wf(w, "height") * 0.5) };
            let div = [self.wi(w, "hMultiplier") as f64, self.wi(w, "vMultiplier") as f64];
            let add = [self.wf(w, "renderX") as f64, self.wf(w, "renderY") as f64];
            return Self::frame_project(&v, p, div, add);
        }
        if self.is_class(w, "ViewportWindow") {
            if let Some(v) = self.ui.viewport_frames.get(&w).copied() {
                return Self::frame_project(&v, p, [1.0; 2], [0.0; 2]);
            }
        }
        if parent == 0 {
            return (false, self.wf(w, "width") * 0.5, self.wf(w, "height") * 0.5);
        }
        let (ok, x, y) = self.ui_convert_vector(parent, p);
        (ok, x - self.wf(w, "x"), y - self.wf(w, "y"))
    }

    /// The inline projection of the root and viewport windows' `ConvertVectorToCoordinates` through master frame `v`
    /// (`CreateMasterFrame` with the port's widescreen rule, as the view is drawn), dividing by `div` and adding `add`.
    fn frame_project(v: &ViewInfo, p: [f32; 3], div: [f64; 2], add: [f64; 2]) -> (bool, f32, f32) {
        let (w, h) = (v.width as i32, v.height as i32);
        let fov = ue_bsp::scene::hor_plus_fov(v.fov_deg, w as f32 / h.max(1) as f32);
        let f = ue_bsp::scene::SceneView::master(v.pos, v.rot, fov, w, h);
        let c = f.coords;
        let d = [p[0] as f64 - c.origin[0] as f64, p[1] as f64 - c.origin[1] as f64, p[2] as f64 - c.origin[2] as f64];
        let t = |a: [f32; 3]| ((d[0] * a[0] as f64 + d[1] * a[1] as f64) + d[2] * a[2] as f64) as f32;
        let (vx, vy, vz) = (t(c.x), t(c.y), t(c.z));
        // `_ftol` truncates toward zero
        let ftol = |x: f64| x as i32 as f32;
        if vz as f64 <= 1.0 {
            return (false, ftol(f.size.fx2 as f64), ftol(f.size.fy2 as f64));
        }
        let scale = (f.size.proj_z / vz) as f64;
        let x = ftol((vx as f64 * scale + f.size.fx2 as f64) / div[0] + add[0]);
        let y = ftol((vy as f64 * scale + f.size.fy2 as f64) / div[1] + add[1]);
        (true, x, y)
    }

    /// Absolute position of a window's top-left corner in root units.
    pub fn ui_window_origin(&self, w: u32) -> (f32, f32) {
        let g = self.wo_ref(w, "wGc");
        let c = self.gc_state(g).clip;
        (c[0], c[1])
    }

    /// Geometry of a window: x, y, width, height (relative to its parent).
    /// A float property of a window (0 when it has none).
    pub fn ui_float(&self, w: u32, name: &str) -> f32 {
        if w == 0 {
            0.0
        } else {
            self.get_prop(w, name, 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0)
        }
    }

    pub fn ui_geometry(&self, w: u32) -> [f32; 4] {
        [self.wf(w, "x"), self.wf(w, "y"), self.wf(w, "width"), self.wf(w, "height")]
    }

    /// `XRootWindow::InitializeWindows` (1003bc80) / `InvokeWindowReady`: every visible window that has not yet been
    /// told gets `WindowReady`, children before their parents. Only while the root counts such windows (`initCount`,
    /// raised and lowered by `RecomputeInitTree` when a subtree's visibility changes).
    pub fn ui_initialize_windows(&mut self, root: u32) {
        if self.wi(root, "initCount") > 0 {
            self.ui_invoke_window_ready(root);
        }
    }

    fn ui_invoke_window_ready(&mut self, w: u32) {
        let mut c = self.wo(w, "firstChild");
        while c != 0 {
            let next = self.wo(c, "nextSibling");
            if self.wb(c, "bIsVisible") {
                self.ui_invoke_window_ready(c);
            }
            c = next;
        }
        if !self.wb(w, "bIsInitialized") {
            self.ui_event(w, "WindowReady", vec![]);
            self.set_b(w, "bIsInitialized", true);
            let root = self.ui_root_of(w);
            if root != 0 {
                let n = self.wi(root, "initCount");
                self.set_i(root, "initCount", n - 1);
            }
        }
    }

    /// Ticks the player's window tree (windows that enabled `Tick`).
    pub fn ui_tick_player(&mut self, pawn: u32, dt: f32) {
        let root = self.wo(pawn, "rootWindow");
        if root != 0 {
            self.ui_tick(root, dt);
        }
    }

    /// `XRootWindow::RootTick`: ticks the whole tree (windows that enabled `Tick`, and their timers).
    pub fn ui_tick(&mut self, root: u32, dt: f32) {
        self.ui_lock(root);
        self.ui_tick_tree(root, dt);
        self.ui_unlock(root);
    }

    fn ui_tick_tree(&mut self, w: u32, dt: f32) {
        if w == 0 {
            return;
        }
        self.ui_lock(w);
        // siblings above first, then this window, then its children (as the original walks)
        let higher = self.wo(w, "nextSibling");
        self.ui_tick_tree(higher, dt);
        if !self.wb(w, "bBeingDestroyed") {
            if self.wi(w, "windowType") >= WINDOW_ROOT {
                self.ui_root_focus_tick(w);
                let t = (self.wf(w, "lastButtonPress") - dt).max(0.0);
                self.set_f(w, "lastButtonPress", t);
            }
            // the native Tick of the window's class always runs, the script's only when it asked for ticks
            let mut targs = vec![Value::Float(dt)];
            let _ = self.ui_native_event(w, "Tick", &mut targs);
            if !self.wb(w, "bBeingDestroyed") && self.wb(w, "bTickEnabled") {
                self.ui_script_event(w, "Tick", vec![Value::Float(dt)]);
            }
            if !self.wb(w, "bBeingDestroyed") {
                self.ui_invoke_timers(w, dt);
            }
            if !self.wb(w, "bBeingDestroyed") {
                let first = self.wo(w, "firstChild");
                self.ui_tick_tree(first, dt);
            }
        }
        self.ui_unlock(w);
    }
}

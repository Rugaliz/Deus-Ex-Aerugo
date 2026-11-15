//! Input routing of the window system: mouse and keyboard to windows, focus, mouse grabs, the cursor, timers
//! (`XRootWindow::Process`/`HandleButtons`/`ProcessMouseMove`, `XWindow::SetFocusWindow`, `AddTimer`, ...).
//! Not here yet: accelerator keys.

use crate::{Value, Vm};

const IK_LEFT_MOUSE: i32 = 1;
const IK_RIGHT_MOUSE: i32 = 2;
const IK_MIDDLE_MOUSE: i32 = 4;
const IK_MOUSE_X: i32 = 0xE4;
const IK_MOUSE_WHEEL_UP: i32 = 0xEC;
const IK_ALT: i32 = 0x12;

/// A window timer.
#[derive(Clone, Debug)]
pub struct Timer {
    pub id: i32,
    pub period: f32,
    pub remaining: f32,
    pub function: u32,
    pub looping: bool,
    pub client_data: i32,
    pub removed: bool,
}

pub(crate) fn is_mouse_button(key: i32) -> bool {
    matches!(key, IK_MOUSE_X | 0xE5 | 0xE6 | 0xE7 | IK_LEFT_MOUSE | IK_RIGHT_MOUSE | IK_MIDDLE_MOUSE | IK_MOUSE_WHEEL_UP | 0xED)
}

impl Vm {
    // ---- focus ---------------------------------------------------------------------------------------------

    pub(crate) fn ui_change_style_tree(&mut self, w: u32) {
        self.ui_event(w, "StyleChanged", vec![]);
        let mut c = self.wo_pub(w, "lastChild");
        while c != 0 {
            let next = self.wo_pub(c, "prevSibling");
            self.ui_change_style_tree(c);
            c = next;
        }
    }

    /// `XModalWindow::IsCurrentModal` (10036e20): the topmost visible modal child of the root, else the root.
    pub(crate) fn ui_current_modal(&self, root: u32) -> u32 {
        let mut c = self.wo_pub(root, "lastChild");
        while c != 0 {
            if self.wb_pub(c, "bIsVisible") && self.wi_pub(c, "windowType") > 1 {
                return c;
            }
            c = self.wo_pub(c, "prevSibling");
        }
        root
    }

    /// `XWindow::SetAcceleratorText` (1004f310): the character after `|&` becomes the window's accelerator key (none
    /// without one; codes above 254 count as none).
    pub(crate) fn ui_set_accelerator_text(&mut self, w: u32, text: Option<&str>) {
        let mut key = 0u32;
        if let Some(t) = text {
            let u: Vec<u16> = t.encode_utf16().collect();
            let mut i = 0;
            while i < u.len() {
                if u[i] == b'|' as u16 && u.get(i + 1) == Some(&(b'&' as u16)) {
                    key = u.get(i + 2).copied().unwrap_or(0) as u32;
                    break;
                }
                i += if u[i] == b'|' as u16 { 2 } else { 1 };
            }
        }
        if key > 0xfe {
            key = 0;
        }
        self.set_i_pub(w, "acceleratorKey", key as i32);
    }

    /// `XModalWindow::SetAcceleratorWindows` (10036a50): the window owning each accelerator key under a modal window.
    /// A window counts when its key is set, it is visible, sensitive and selectable along its chain; the first one
    /// found takes both the upper- and lower-case slot. Children are visited from the bottom up (visible ones only),
    /// not into nested modal windows and not when insensitive.
    fn ui_accelerator_window(&self, modal: u32, key: u16) -> u32 {
        fn chain(vm: &Vm, w: u32, prop: &str) -> bool {
            let mut c = w;
            while c != 0 {
                if !vm.wb_pub(c, prop) {
                    return false;
                }
                c = vm.wo_pub(c, "winParent");
            }
            true
        }
        fn visit(vm: &Vm, w: u32, key: u16, found: &mut u32) {
            if *found != 0 {
                return;
            }
            let k = vm.wi_pub(w, "acceleratorKey") as u32;
            if k != 0 && k < 0xff && chain(vm, w, "bIsVisible") && chain(vm, w, "bIsSensitive") && vm.wb_pub(w, "bIsSelectable") {
                let up = if (0x61..0x7b).contains(&k) { k - 0x20 } else { k };
                let low = if (0x41..0x5b).contains(&up) { up + 0x20 } else { up };
                if key as u32 == up || key as u32 == low {
                    *found = w;
                    return;
                }
            }
            let mut c = vm.wo_pub(w, "firstChild");
            while c != 0 {
                if vm.wb_pub(c, "bIsVisible") && vm.wi_pub(c, "windowType") < 2 && chain(vm, c, "bIsSensitive") {
                    visit(vm, c, key, found);
                }
                c = vm.wo_pub(c, "nextSibling");
            }
        }
        if key == 0 || key >= 0xff {
            return 0;
        }
        let mut found = 0;
        visit(self, modal, key, &mut found);
        found
    }

    /// `XModalWindow::KeyPressed` (10037510): the script first; a key it leaves goes, with Alt held, to the window of
    /// this modal whose accelerator it is (`AcceleratorKeyPressed`).
    pub(crate) fn ui_modal_key_pressed(&mut self, w: u32, args: &mut Vec<Value>) -> Value {
        let (r, out) = self.ui_script_event(w, "KeyPressed", args.clone());
        *args = out;
        if r.as_bool().unwrap_or(false) {
            return r;
        }
        let root = self.ui_root_of_pub(w);
        if root == 0 || self.ui_current_modal(root) != w || !self.ui_key_down(root, 0x12) {
            return r;
        }
        let ch = match args.first() {
            Some(Value::Str(s)) => s.encode_utf16().next().unwrap_or(0),
            _ => 0,
        };
        let target = self.ui_accelerator_window(w, ch);
        if target == 0 {
            return r;
        }
        let key = args.first().cloned().unwrap_or(Value::Str(String::new()));
        self.ui_event(target, "AcceleratorKeyPressed", vec![key]).0
    }

    /// `IsTraversable`: can the window take the focus (selectable, and visible and sensitive along its chain; with
    /// `check_modal`, in the topmost modal window)?
    pub(crate) fn ui_is_traversable(&self, w: u32, check_modal: bool) -> bool {
        if !self.wb_pub(w, "bIsSelectable") {
            return false;
        }
        let mut cur = w;
        let mut first_modal = 0;
        while cur != 0 {
            if !self.wb_pub(cur, "bIsVisible") || !self.wb_pub(cur, "bIsSensitive") {
                return false;
            }
            if check_modal {
                if first_modal == 0 && self.wi_pub(cur, "windowType") > 1 {
                    first_modal = cur;
                }
                if self.wi_pub(cur, "windowType") == 3 && self.ui_current_modal(cur) != first_modal {
                    return false;
                }
            }
            cur = self.wo_pub(cur, "winParent");
        }
        true
    }

    // ---- keyboard focus navigation (Extension.dll `XWindow::MoveFocus`, `MoveTabGroup`, `XTabGroupWindow` tables) ----

    /// `GetTabGroupWindow`: the window itself or its nearest ancestor that is a tab group (any non-plain window type).
    fn ui_tab_group_of(&self, w: u32) -> u32 {
        let mut c = w;
        while c != 0 && self.wi_pub(c, "windowType") == 0 {
            c = self.wo_pub(c, "winParent");
        }
        c
    }

    /// `GetModalWindow`: the window itself or its nearest ancestor that is modal (or the root).
    fn ui_modal_of(&self, w: u32) -> u32 {
        let mut c = w;
        while c != 0 && self.wi_pub(c, "windowType") < 2 {
            c = self.wo_pub(c, "winParent");
        }
        c
    }

    fn ui_subtree(&self, w: u32, out: &mut Vec<u32>) {
        out.push(w);
        let mut c = self.wo_pub(w, "firstChild");
        while c != 0 {
            self.ui_subtree(c, out);
            c = self.wo_pub(c, "nextSibling");
        }
    }

    /// The clip rectangle (`winClipRect`: origin x/y, clip x/y, width, height) of a window.
    fn ui_clip_rect(&self, w: u32) -> [f32; 6] {
        let g = self.wo_ref_pub(w, "wGc");
        if g == 0 {
            return [0.0; 6];
        }
        self.gc_state(g).clip
    }

    /// A tab group's window tables (`rowMajorWindowList` / `colMajorWindowList`): its selectable windows (those whose
    /// tab group it is), sorted by absolute origin, row-major by y then x, column-major by x then y (ties by identity).
    fn ui_tab_group_tables(&self, g: u32) -> (Vec<u32>, Vec<u32>) {
        self.ui_tab_group_tables_with(g, 0)
    }

    /// The tables with `keep` still in them (a window being made unselectable leaves its tables only after
    /// `CheckFocusWindow`, which starts the focus move from its place).
    fn ui_tab_group_tables_with(&self, g: u32, keep: u32) -> (Vec<u32>, Vec<u32>) {
        let mut all = Vec::new();
        self.ui_subtree(g, &mut all);
        let mut rows: Vec<(f32, f32, u32)> = all
            .into_iter()
            .filter(|&w| (w == keep || self.wb_pub(w, "bIsSelectable")) && self.ui_tab_group_of(w) == g)
            .map(|w| {
                let c = self.ui_clip_rect(w);
                (c[0], c[1], w)
            })
            .collect();
        let mut cols = rows.clone();
        let cmp = |a: f32, b: f32| a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
        rows.sort_by(|a, b| cmp(a.1, b.1).then(cmp(a.0, b.0)).then(a.2.cmp(&b.2)));
        cols.sort_by(|a, b| cmp(a.0, b.0).then(cmp(a.1, b.1)).then(a.2.cmp(&b.2)));
        (rows.into_iter().map(|r| r.2).collect(), cols.into_iter().map(|c| c.2).collect())
    }

    /// `ComputeTabGroupLocation`: where the first visible, traversable window of the group (row order) starts.
    fn ui_tab_group_location(&self, g: u32) -> (f32, f32) {
        for w in self.ui_tab_group_tables(g).0 {
            let c = self.ui_clip_rect(w);
            if c[4] > 0.0 && c[5] > 0.0 && self.ui_is_traversable(w, false) {
                return (c[0] + c[2], c[1] + c[3]);
            }
        }
        (0.0, 0.0)
    }

    /// A modal window's tab groups (`tabGroupWindowList`): the tab groups (itself included) whose modal window it is,
    /// sorted by their location, y then x.
    fn ui_modal_tab_groups(&self, m: u32) -> Vec<u32> {
        let mut all = Vec::new();
        self.ui_subtree(m, &mut all);
        let cmp = |a: f32, b: f32| a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
        let mut groups: Vec<(f32, f32, u32)> = all
            .into_iter()
            .filter(|&t| self.wi_pub(t, "windowType") >= 1 && self.ui_modal_of(t) == m)
            .map(|t| {
                let (x, y) = self.ui_tab_group_location(t);
                (x, y, t)
            })
            .collect();
        groups.sort_by(|a, b| cmp(a.1, b.1).then(cmp(a.0, b.0)).then(a.2.cmp(&b.2)));
        groups.into_iter().map(|g| g.2).collect()
    }

    /// `XWindow::MoveFocus(dir)` (dir: 0 left, 1 right, 2 up, 3 down): the next traversable window of the focused
    /// window's tab group in that direction (row order for left/right, column order for up/down; tab groups always
    /// wrap); none there, or no focus: `MoveTabGroup`. Returns the focus window.
    pub(crate) fn ui_move_focus(&mut self, w: u32, dir: i32) -> u32 {
        let root = self.ui_root_of_pub(w);
        if root == 0 {
            return 0;
        }
        let focus = self.wo_pub(root, "focusWindow");
        if focus == 0 {
            self.ui_move_tab_group(root, dir);
        } else {
            let g = self.ui_tab_group_of(focus);
            if g != 0 {
                let (rows, cols) = self.ui_tab_group_tables_with(g, focus);
                let list = if dir == 0 || dir == 1 { rows } else { cols };
                let back = dir == 0 || dir == 2;
                if let Some(start) = list.iter().position(|&x| x == focus) {
                    let n = list.len() as i32;
                    let mut i = start as i32;
                    let mut found = false;
                    for _ in 0..n {
                        i = if back { (i - 1).rem_euclid(n) } else { (i + 1) % n };
                        let cand = list[i as usize];
                        if self.ui_is_traversable(cand, true) {
                            self.ui_set_focus_window(root, cand);
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        self.ui_move_tab_group(root, dir);
                    }
                }
            }
        }
        self.wo_pub(root, "focusWindow")
    }

    /// `XWindow::MoveTabGroup(dir)` (2 previous, 3 next; 0 and 2 go backwards): the first traversable window (row
    /// order) of the next tab group of the topmost modal window that has one, wrapping around.
    pub(crate) fn ui_move_tab_group(&mut self, w: u32, dir: i32) -> u32 {
        let root = self.ui_root_of_pub(w);
        if root == 0 {
            return 0;
        }
        let mut modal = root;
        let mut c = self.wo_pub(root, "lastChild");
        while c != 0 {
            if self.wb_pub(c, "bIsVisible") && self.wi_pub(c, "windowType") > 1 {
                modal = c;
                break;
            }
            c = self.wo_pub(c, "prevSibling");
        }
        let groups = self.ui_modal_tab_groups(modal);
        let back = dir == 2 || dir == 0;
        let n = groups.len() as i32;
        let focus = self.wo_pub(root, "focusWindow");
        let fg = if focus != 0 && self.ui_modal_of(focus) == modal { self.ui_tab_group_of(focus) } else { 0 };
        let start = match groups.iter().position(|&g| g == fg) {
            Some(i) if fg != 0 => {
                if back {
                    (i as i32 - 1).rem_euclid(n)
                } else {
                    (i as i32 + 1) % n
                }
            }
            _ => {
                if n < 1 {
                    return focus;
                }
                0
            }
        };
        let mut k = start;
        loop {
            let g = groups[k as usize];
            if let Some(&target) = self.ui_tab_group_tables(g).0.iter().find(|&&x| self.ui_is_traversable(x, false)) {
                self.ui_set_focus_window(root, target);
                break;
            }
            k = if back { (k - 1).rem_euclid(n) } else { (k + 1) % n };
            if k == start {
                break;
            }
        }
        self.wo_pub(root, "focusWindow")
    }

    /// `SetFocusWindow`: the nearest selectable window at or above `target` takes the keyboard focus.
    pub(crate) fn ui_set_focus_window(&mut self, w: u32, target: u32) -> bool {
        let root = self.ui_root_of_pub(w);
        let mut target = target;
        if target != 0 {
            let mut c = target;
            while c != 0 {
                if self.wb_pub(c, "bIsSelectable") {
                    target = c;
                    break;
                }
                c = self.wo_pub(c, "winParent");
            }
        }
        if root == 0 {
            return false;
        }
        if self.wo_pub(root, "focusWindow") != target {
            if target != 0 {
                if !self.ui_is_traversable(target, true) {
                    return false;
                }
                let mut m = target;
                while m != 0 && self.wi_pub(m, "windowType") < 2 {
                    m = self.wo_pub(m, "winParent");
                }
                if m != 0 {
                    self.set_o_pub(m, "preferredFocus", target);
                }
            }
            let old = self.wo_pub(root, "focusWindow");
            self.set_o_pub(root, "focusWindow", target);
            if old != 0 {
                self.ui_event(old, "FocusLeftWindow", vec![]);
                let s = self.wo_pub(old, "unfocusSound");
                self.ui_play_sound(old, s, -1.0, 1.0);
            }
            let mut c = old;
            while c != 0 {
                self.ui_event(c, "FocusLeftDescendant", vec![Value::Obj(old)]);
                c = self.wo_pub(c, "winParent");
            }
            let new = self.wo_pub(root, "focusWindow");
            if new != 0 {
                let (nw, nh) = (self.wf_pub(new, "width"), self.wf_pub(new, "height"));
                self.ui_ask_parent_to_show_area(new, 0.0, 0.0, nw, nh);
                let s = self.wo_pub(new, "focusSound");
                self.ui_play_sound(new, s, -1.0, 1.0);
                self.ui_event(new, "FocusEnteredWindow", vec![]);
            }
            let mut c = new;
            while c != 0 {
                self.ui_event(c, "FocusEnteredDescendant", vec![Value::Obj(new)]);
                c = self.wo_pub(c, "winParent");
            }
        }
        true
    }

    /// `CheckFocusWindow`: a window that can no longer hold the focus gives it to the modal window's preferred window, or drops it.
    pub(crate) fn ui_check_focus(&mut self, w: u32) {
        let root = self.ui_root_of_pub(w);
        if root == 0 {
            return;
        }
        let cur = self.wo_pub(root, "focusWindow");
        if cur == 0 || self.ui_is_traversable(cur, true) {
            return;
        }
        let top = self.ui_current_modal(root);
        let pref = self.wo_pub(top, "preferredFocus");
        if pref != 0 && self.ui_is_traversable(pref, true) {
            self.ui_set_focus_window(w, pref);
        }
        // still there: the next window to the right (wrapping, then the next tab group); nothing else: no focus
        if self.wo_pub(root, "focusWindow") == cur && self.ui_move_focus(root, 1) == cur {
            self.ui_set_focus_window(root, 0);
        }
    }

    /// `CheckGrabbedWindow` (10050c90): a mouse grab ends when its window is no longer visible or sensitive along its
    /// chain.
    pub(crate) fn ui_check_grabbed_window(&mut self, w: u32) {
        let root = self.ui_root_of_pub(w);
        if root == 0 {
            return;
        }
        let g = self.wo_pub(root, "grabbedWindow");
        if g != 0 && (!self.ui_visible_recursive(g) || !self.ui_sensitive_recursive_pub(g)) {
            self.set_o_pub(root, "grabbedWindow", 0);
        }
    }

    /// The root's own tick (`XRootWindow::Tick`, 1003a540): with nothing focused, the current modal (unless its focus
    /// follows the mouse) focuses its preferred window, else the first window of its next tab group.
    pub(crate) fn ui_root_focus_tick(&mut self, root: u32) {
        if self.wo_pub(root, "focusWindow") != 0 {
            return;
        }
        let top = self.ui_current_modal(root);
        let mode = match self.get_prop(top, "focusMode", 0) {
            Ok(Value::Byte(b)) => b as i32,
            Ok(Value::Int(i)) => i,
            _ => 0,
        };
        if mode >= 3 {
            return;
        }
        let pref = self.wo_pub(top, "preferredFocus");
        if pref != 0 && self.ui_is_traversable(pref, true) {
            self.ui_set_focus_window(root, pref);
        }
        if self.wo_pub(root, "focusWindow") == 0 {
            self.ui_move_tab_group(root, 3);
        }
    }

    /// `AskParentToShowArea`: a rectangle of a visible window should become visible; scroll areas react (`ChildRequestedShowArea`).
    pub(crate) fn ui_ask_parent_to_show_area(&mut self, w: u32, x: f32, y: f32, width: f32, height: f32) {
        let mut cur = w;
        let (mut x, mut y) = (x, y);
        while cur != 0 && self.ui_visible_recursive(cur) {
            let parent = self.wo_pub(cur, "winParent");
            if parent == 0 {
                break;
            }
            self.ui_event(parent, "ChildRequestedShowArea", vec![Value::Obj(cur), Value::Float(x), Value::Float(y), Value::Float(width), Value::Float(height)]);
            x += self.wf_pub(cur, "x");
            y += self.wf_pub(cur, "y");
            cur = parent;
        }
    }

    // ---- hit testing ---------------------------------------------------------------------------------------

    pub(crate) fn ui_is_point_in_window(&self, w: u32, x: f32, y: f32) -> bool {
        x >= 0.0 && x < self.wf_pub(w, "width") && y >= 0.0 && y < self.wf_pub(w, "height")
    }

    /// `FindWindowByPoint`: the topmost visible, sensitive window (not a tab group/modal child) containing the point,
    /// with the point relative to it.
    pub(crate) fn ui_find_window_by_point(&self, w: u32, x: f32, y: f32) -> Option<(u32, f32, f32)> {
        if !(self.wb_pub(w, "bIsVisible") && self.wb_pub(w, "bIsSensitive") && self.ui_is_point_in_window(w, x, y)) {
            return None;
        }
        let mut c = self.wo_pub(w, "lastChild");
        while c != 0 {
            if self.wi_pub(c, "windowType") < 2 {
                if let Some(hit) = self.ui_find_window_by_point(c, x - self.wf_pub(c, "x"), y - self.wf_pub(c, "y")) {
                    return Some(hit);
                }
            }
            c = self.wo_pub(c, "prevSibling");
        }
        Some((w, x, y))
    }

    fn ui_origin(&mut self, w: u32) -> (f32, f32) {
        let g = self.wo_ref_pub(w, "wGc");
        let c = self.gc_state(g).clip;
        (c[0], c[1])
    }

    /// `GetMouseWindow`: the window under the cursor (the grabbed one if any), with the cursor relative to it.
    pub(crate) fn ui_mouse_window(&mut self, root: u32, modal: u32) -> (u32, f32, f32) {
        let modal = if modal == 0 { self.ui_current_modal(root) } else { modal };
        let (mx, my) = (self.wf_pub(root, "mouseX"), self.wf_pub(root, "mouseY"));
        let grabbed = self.wo_pub(root, "grabbedWindow");
        let (mox, moy) = self.ui_origin(modal);
        let hit = if grabbed != 0 { Some(grabbed) } else { self.ui_find_window_by_point(modal, mx - mox, my - moy).map(|h| h.0) };
        let found = hit.unwrap_or(modal);
        let (ox, oy) = self.ui_origin(found);
        (found, mx - ox, my - oy)
    }

    // ---- mouse ----------------------------------------------------------------------------------------------

    pub(crate) fn ui_clamp_mouse(&mut self, root: u32) {
        let (x, y, w, h) = (self.wf_pub(root, "x"), self.wf_pub(root, "y"), self.wf_pub(root, "width"), self.wf_pub(root, "height"));
        let mx = self.wf_pub(root, "mouseX");
        let my = self.wf_pub(root, "mouseY");
        let max_x = (w + x) - 1.0;
        let max_y = (h + y) - 1.0;
        self.set_f_pub(root, "mouseX", if mx < x { x } else if mx >= max_x { max_x } else { mx });
        self.set_f_pub(root, "mouseY", if my < y { y } else if my >= max_y { max_y } else { my });
    }

    /// `HandleMouse`: the cursor moved to `(x, y)` (root units).
    fn ui_handle_mouse(&mut self, root: u32, x: f32, y: f32) -> bool {
        if !self.wb_pub(root, "bMouseMoveLocked") {
            self.set_f_pub(root, "mouseX", x);
            self.set_f_pub(root, "mouseY", y);
            self.set_b_pub(root, "bMouseMoved", true);
            self.ui_clamp_mouse(root);
            self.ui_process_mouse_move(root);
        }
        true
    }

    pub(crate) fn ui_process_mouse_move(&mut self, root: u32) {
        if !self.wb_pub(root, "bMouseMoved") {
            return;
        }
        let modal = self.ui_current_modal(root);
        let (win, wx, wy) = self.ui_mouse_window(root, modal);
        let last = self.wo_pub(root, "lastMouseWindow");
        let focus_mode = self.wi_pub(modal, "focusMode");
        if last != win {
            if last != 0 {
                self.ui_event(last, "MouseLeftWindow", vec![]);
                if focus_mode > 2 {
                    self.ui_set_focus_window(root, 0);
                }
            }
            if win != 0 {
                self.ui_event(win, "MouseEnteredWindow", vec![]);
                if focus_mode > 1 {
                    self.ui_set_focus_window(root, win);
                }
            }
            self.set_o_pub(root, "lastMouseWindow", win);
        }
        let (mx, my) = (self.wf_pub(root, "mouseX"), self.wf_pub(root, "mouseY"));
        if self.wf_pub(root, "prevMouseX") != mx || self.wf_pub(root, "prevMouseY") != my {
            if win != 0 {
                self.ui_event(win, "MouseMoved", vec![Value::Float(wx), Value::Float(wy)]);
            }
            self.set_f_pub(root, "prevMouseX", mx);
            self.set_f_pub(root, "prevMouseY", my);
        }
        self.set_b_pub(root, "bMouseMoved", false);
    }

    /// `HandleButtons`: a mouse button (or wheel step) goes down or up; windows from the one under the cursor upwards
    /// get `RawMouseButtonPressed`, then `MouseButtonPressed`/`MouseButtonReleased`, until one handles it.
    fn ui_handle_buttons(&mut self, root: u32, key: i32, action: i32) -> bool {
        if self.wb_pub(root, "bMouseButtonLocked") {
            return true;
        }
        let modal = self.ui_current_modal(root);
        let (mut win, _, _) = self.ui_mouse_window(root, modal);
        if win != 0 {
            if action == 1 {
                self.ui_grab_mouse(win);
                if self.wi_pub(modal, "focusMode") != 0 {
                    self.ui_set_focus_window(root, win);
                }
            } else if action == 3 {
                self.ui_ungrab_mouse(win);
            }
        }
        if action == 1 || action == 3 {
            // multi-click counting: a press near the last one, soon after, on the same window and button continues the count
            let (mx, my) = (self.wf_pub(root, "mouseX"), self.wf_pub(root, "mouseY"));
            let remaining = self.wf_pub(root, "lastButtonPress");
            let (dx, dy) = ((mx - self.wf_pub(root, "firstButtonMouseX")).abs(), (my - self.wf_pub(root, "firstButtonMouseY")).abs());
            let maxd = self.wf_pub(root, "maxMouseDist");
            let continues = remaining > 0.0 && win == self.wo_pub(root, "lastButtonWindow") && dx <= maxd && dy <= maxd && key == self.wi_pub(root, "lastButtonType");
            if !continues {
                self.set_f_pub(root, "lastButtonPress", 0.0);
                self.set_i_pub(root, "lastButtonType", key);
                self.set_o_pub(root, "lastButtonWindow", win);
                self.set_i_pub(root, "clickCount", 0);
            }
            if action == 1 {
                if continues {
                    let n = self.wi_pub(root, "clickCount");
                    self.set_i_pub(root, "clickCount", n + 1);
                }
                let t = self.wf_pub(root, "multiClickTimeout");
                self.set_f_pub(root, "lastButtonPress", t);
                if self.wi_pub(root, "clickCount") < 1 {
                    self.set_f_pub(root, "firstButtonMouseX", mx);
                    self.set_f_pub(root, "firstButtonMouseY", my);
                }
            }
        }
        let count = self.wi_pub(root, "clickCount");
        let (mx, my) = (self.wf_pub(root, "mouseX"), self.wf_pub(root, "mouseY"));
        while win != 0 {
            // the count each window sees wraps at its `maxClicks` (`HandleButtons`, 1003bba5)
            let max = self.wi_pub(win, "maxClicks");
            let clicks = if max > 0 { count % max + 1 } else { count + 1 };
            let parent = self.wo_pub(win, "winParent");
            let (ox, oy) = self.ui_origin(win);
            let (px, py) = (mx - ox, my - oy);
            let (r, _) = self.ui_event(win, "RawMouseButtonPressed", vec![Value::Float(px), Value::Float(py), Value::Byte(key as u8), Value::Byte(action as u8)]);
            if r.as_bool().unwrap_or(false) {
                return true;
            }
            let ev = if action == 1 {
                Some("MouseButtonPressed")
            } else if action == 3 {
                Some("MouseButtonReleased")
            } else {
                None
            };
            if let Some(ev) = ev {
                let (r, _) = self.ui_event(win, ev, vec![Value::Float(px), Value::Float(py), Value::Byte(key as u8), Value::Int(clicks)]);
                if r.as_bool().unwrap_or(false) {
                    return true;
                }
            }
            win = parent;
        }
        false
    }

    pub(crate) fn ui_grab_mouse(&mut self, w: u32) {
        let root = self.ui_root_of_pub(w);
        if root != 0 {
            let cur = self.wo_pub(root, "grabbedWindow");
            if cur != w {
                if cur != 0 {
                    self.ui_event(cur, "MouseGrabbedByOther", vec![Value::Obj(w)]);
                }
                self.set_o_pub(root, "grabbedWindow", w);
            }
        }
    }

    pub(crate) fn ui_ungrab_mouse(&mut self, w: u32) {
        let root = self.ui_root_of_pub(w);
        if root != 0 && self.wo_pub(root, "grabbedWindow") == w {
            self.set_o_pub(root, "grabbedWindow", 0);
        }
    }

    // ---- keyboard --------------------------------------------------------------------------------------------

    fn ui_key_down(&self, root: u32, key: i32) -> bool {
        (0..255).contains(&key) && matches!(self.get_prop(root, "keyDownMap", key as u32), Ok(Value::Byte(b)) if b != 0)
    }

    fn ui_set_key_down(&mut self, root: u32, key: i32, down: bool) {
        if (0..255).contains(&key) {
            let _ = self.set_prop(root, "keyDownMap", key as u32, Value::Byte(down as u8));
        }
    }

    /// `HandleVirtualKeyboard`: a key press/release goes to the focused window (or the top modal) and its ancestors.
    fn ui_handle_virtual_key(&mut self, root: u32, key: i32, action: i32, repeat: bool) -> bool {
        let focus = self.wo_pub(root, "focusWindow");
        let mut win = if focus != 0 { focus } else { self.ui_current_modal(root) };
        while win != 0 {
            if !repeat {
                let (r, _) = self.ui_event(win, "RawKeyPressed", vec![Value::Byte(key as u8), Value::Byte(action as u8), Value::Bool(false)]);
                if r.as_bool().unwrap_or(false) {
                    return true;
                }
            }
            if action == 1 {
                let (r, _) = self.ui_event(win, "VirtualKeyPressed", vec![Value::Byte(key as u8), Value::Bool(repeat)]);
                if r.as_bool().unwrap_or(false) {
                    return true;
                }
            }
            win = self.wo_pub(win, "winParent");
        }
        false
    }

    /// `HandleKeyboard`: a typed character goes to the focused window and its ancestors as `KeyPressed`.
    fn ui_handle_char(&mut self, root: u32, ch: char) -> bool {
        let focus = self.wo_pub(root, "focusWindow");
        let mut win = if focus != 0 { focus } else { self.ui_current_modal(root) };
        let alt = self.ui_key_down(root, IK_ALT);
        while win != 0 {
            if self.wi_pub(win, "windowType") > 1 || !alt {
                let (r, _) = self.ui_event(win, "KeyPressed", vec![Value::Str(ch.to_string())]);
                if r.as_bool().unwrap_or(false) {
                    return true;
                }
            }
            win = self.wo_pub(win, "winParent");
        }
        false
    }

    // ---- entry points for the host ----------------------------------------------------------------------------

    /// The player's root window, if it exists.
    fn ui_player_root(&self, pawn: u32) -> u32 {
        self.wo_pub(pawn, "rootWindow")
    }

    /// The player's root window (0 if none).
    pub fn ui_root(&self, pawn: u32) -> u32 {
        self.ui_player_root(pawn)
    }

    /// Does the window system want the mouse (a modal window is open)?
    pub fn ui_wants_mouse(&self, pawn: u32) -> bool {
        let root = self.ui_player_root(pawn);
        root != 0 && self.wi_pub(root, "handleMouseRef") > 0
    }

    /// `XRootWindow::PreRender` (1003a760): the scene frame the world is drawn into on a `w` x `h` screen, as
    /// (x, y, width, height) in pixels. Empty (0 x 0) when the root window has rendering off (`EnableRendering(False)`:
    /// menus over a snapshot); with a render viewport (`SetRenderViewport`, root units) the rectangle
    /// `trunc(mult * render + origin)` for both corners, clipped to the screen. Without a root window, the screen.
    pub fn ui_render_rect(&self, pawn: u32, w: i32, h: i32) -> [i32; 4] {
        let root = self.ui_player_root(pawn);
        if root == 0 {
            return [0, 0, w, h];
        }
        if !self.wb_pub(root, "bRender") {
            return [0, 0, 0, 0];
        }
        if !self.wb_pub(root, "bClipRender") {
            return [0, 0, w, h];
        }
        let (hm, vm) = (self.wi_pub(root, "hMultiplier") as f32, self.wi_pub(root, "vMultiplier") as f32);
        let (rx, ry, rw, rh) = (self.wf_pub(root, "renderX"), self.wf_pub(root, "renderY"), self.wf_pub(root, "renderWidth"), self.wf_pub(root, "renderHeight"));
        // the frame's origin is 0, 0 for the main view; `_ftol` truncates
        let x1 = (hm * rx) as i32;
        let y1 = (vm * ry) as i32;
        let x2 = (hm * rw + x1 as f32) as i32;
        let y2 = (vm * rh + y1 as f32) as i32;
        let (x1, y1, x2, y2) = (x1.max(0), y1.max(0), x2.min(w), y2.min(h));
        [x1, y1, (x2 - x1).max(0), (y2 - y1).max(0)]
    }

    pub fn ui_wants_keyboard(&self, pawn: u32) -> bool {
        let root = self.ui_player_root(pawn);
        root != 0 && self.wi_pub(root, "handleKeyboardRef") > 0
    }

    /// `XRootWindow::Process`: a key or button goes down (`action` 1) or up (3). Returns true when the window system took
    /// it (the game must not act on it).
    pub fn ui_process_key(&mut self, pawn: u32, key: i32, action: i32) -> bool {
        let root = self.ui_player_root(pawn);
        if root == 0 {
            return false;
        }
        let (mut was_down, mut was_up) = (false, false);
        if action == 1 {
            was_down = self.ui_key_down(root, key);
            self.ui_set_key_down(root, key, true);
        } else if action == 3 {
            was_up = !self.ui_key_down(root, key);
            self.ui_set_key_down(root, key, false);
        }
        if !is_mouse_button(key) {
            if self.wi_pub(root, "handleKeyboardRef") == 0 {
                return false;
            }
            if !was_up {
                self.ui_handle_virtual_key(root, key, action, was_down);
            }
        } else {
            if self.wi_pub(root, "handleMouseRef") == 0 {
                return false;
            }
            let _ = self.set_prop(pawn, "bFire", 0, Value::Byte(0));
            let _ = self.set_prop(pawn, "bAltFire", 0, Value::Byte(0));
            if !was_up {
                self.ui_handle_buttons(root, key, action);
            }
        }
        true
    }

    /// `XRootWindow::Key`: a typed character.
    pub fn ui_process_char(&mut self, pawn: u32, ch: char) -> bool {
        let root = self.ui_player_root(pawn);
        if root == 0 || self.wi_pub(root, "handleKeyboardRef") == 0 {
            return false;
        }
        self.ui_handle_char(root, ch);
        true
    }

    /// `XRootWindow::SetMouseDelta`: relative mouse movement in root units (fast movements are doubled).
    pub fn ui_mouse_delta(&mut self, pawn: u32, dx: f32, dy: f32) -> bool {
        let root = self.ui_player_root(pawn);
        if root == 0 || self.wi_pub(root, "handleMouseRef") == 0 {
            return false;
        }
        let (mut dx, mut dy) = (dx, dy);
        if dx.abs() > 2.0 || dy.abs() > 2.0 {
            dx *= 2.0;
            dy *= 2.0;
        }
        let (mx, my) = (self.wf_pub(root, "mouseX"), self.wf_pub(root, "mouseY"));
        self.ui_handle_mouse(root, mx + dx, my + dy)
    }

    /// `XRootWindow::SetMousePosition`: absolute cursor position in root units.
    pub fn ui_mouse_position(&mut self, pawn: u32, x: f32, y: f32) -> bool {
        let root = self.ui_player_root(pawn);
        if root == 0 || self.wi_pub(root, "handleMouseRef") == 0 {
            return false;
        }
        self.ui_handle_mouse(root, x.trunc(), y.trunc())
    }

    // ---- cursor ----------------------------------------------------------------------------------------------

    /// `QueryCursor`: the cursor texture for the window under the mouse (its own request, or the nearest ancestor's default)
    /// with hot spot, tint and shadow texture.
    fn ui_query_cursor(&mut self, win: u32, x: f32, y: f32) -> Option<(u32, f32, f32, [u8; 4], u32)> {
        let (mut cur, mut x, mut y) = (win, x, y);
        while cur != 0 {
            let white = self.color_value_pub([255, 255, 255, 255]);
            let (r, out) = self.ui_event(cur, "CursorRequested", vec![Value::Obj(win), Value::Float(x), Value::Float(y), Value::Float(-1.0), Value::Float(-1.0), white, Value::Obj(0)]);
            let mut tex = match r {
                Value::Obj(t) => t,
                _ => 0,
            };
            let (mut hx, mut hy) = (out.get(3).and_then(Value::as_f32).unwrap_or(-1.0), out.get(4).and_then(Value::as_f32).unwrap_or(-1.0));
            let mut color = match out.get(5) {
                Some(Value::Struct(_, b)) if b.len() >= 4 => [b[0], b[1], b[2], b[3]],
                _ => [255; 4],
            };
            let mut shadow = match out.get(6) {
                Some(Value::Obj(s)) => *s,
                _ => 0,
            };
            if tex == 0 {
                let d = self.wo_ref_pub(cur, "defaultCursor");
                if d != 0 {
                    tex = d;
                    shadow = self.wo_ref_pub(cur, "defaultCursorShadow");
                    hx = self.wf_pub(cur, "defaultHotX");
                    hy = self.wf_pub(cur, "defaultHotY");
                    if let Ok(Value::Struct(_, b)) = self.get_prop(cur, "defaultCursorColor", 0) {
                        if b.len() >= 4 {
                            color = [b[0], b[1], b[2], b[3]];
                        }
                    }
                }
            }
            if tex != 0 {
                let (tw, th) = self.texture_size(tex);
                if hx < 0.0 || tw <= hx {
                    hx = ((tw as i32 >> 1) - 1) as f32;
                }
                if hy < 0.0 || th <= hy {
                    hy = ((th as i32 >> 1) - 1) as f32;
                }
                return Some((tex, hx, hy, color, shadow));
            }
            x += self.wf_pub(cur, "x");
            y += self.wf_pub(cur, "y");
            cur = self.wo_pub(cur, "winParent");
        }
        None
    }

    /// Draws the mouse cursor (after the window tree), when the window system has the mouse.
    pub(crate) fn ui_draw_cursor(&mut self, root: u32, hm: i32, vm: i32, under: (u32, f32, f32)) {
        if self.wi_pub(root, "handleMouseRef") == 0 || !self.wb_pub(root, "bCursorVisible") {
            return;
        }
        let (win, wx, wy) = under;
        let Some((tex, hx, hy, color, shadow)) = self.ui_query_cursor(win, wx, wy) else { return };
        let g = self.wo_ref_pub(root, "wGc");
        let depth = self.gc_push(g);
        let (rw, rh) = (self.wf_pub(root, "width"), self.wf_pub(root, "height"));
        {
            let st = self.gc_mut(g);
            st.canvas = true;
            st.draw_enabled = true;
            st.h_mul = hm;
            st.v_mul = vm;
            st.clip = [0.0, 0.0, 0.0, 0.0, rw, rh];
        }
        let (px, py) = (self.wf_pub(root, "mouseX") - hx, self.wf_pub(root, "mouseY") - hy);
        if shadow != 0 {
            self.gc_mut(g).set_style(4);
            let st = self.gc_state(g);
            let (tw, th) = self.texture_size(shadow);
            self.gc_draw_stretched_icon(&st, px, py, tw, th, shadow);
        }
        {
            let st = self.gc_mut(g);
            st.set_style(2);
            st.tile_color = color;
        }
        let st = self.gc_state(g);
        let (tw, th) = self.texture_size(tex);
        self.gc_draw_stretched_icon(&st, px, py, tw, th, tex);
        self.gc_pop(g, Some(depth));
    }

    // ---- timers ----------------------------------------------------------------------------------------------

    pub(crate) fn ui_add_timer(&mut self, w: u32, timeout: f32, looping: bool, client_data: i32, function: u32) -> i32 {
        if self.wb_pub(w, "bBeingDestroyed") {
            return 0;
        }
        self.ui.next_timer += 1;
        let id = self.ui.next_timer;
        // (Extension 1004fc20) at least 1e-6; linked at the head of the window's list, so `InvokeTimers` visits the
        // newest timer first; the time left starts at the tick offset (0 here: windows tick by the frame's step,
        // where the original measures the wall clock since the last window tick) plus the period
        let period = if timeout < 1.0e-6 { 1.0e-6 } else { timeout };
        self.ui.timers.entry(w).or_default().insert(0, Timer { id, period, remaining: period, function, looping, client_data, removed: false });
        id
    }

    pub(crate) fn ui_remove_timer(&mut self, w: u32, id: i32) {
        if let Some(list) = self.ui.timers.get_mut(&w) {
            list.retain(|t| t.id != id);
        }
    }

    /// `InvokeTimers`: counts down the window's timers and calls the ones that ran out (the named function, or `Timer`)
    /// with the timer id, the number of periods that passed and the client data.
    pub(crate) fn ui_invoke_timers(&mut self, w: u32, dt: f32) {
        if self.wb_pub(w, "bBeingDestroyed") {
            return;
        }
        let ids: Vec<i32> = self.ui.timers.get(&w).map(|l| l.iter().map(|t| t.id).collect()).unwrap_or_default();
        for id in ids {
            let Some(t) = self.ui.timers.get(&w).and_then(|l| l.iter().find(|t| t.id == id)).cloned() else { continue };
            if t.removed {
                continue;
            }
            let remaining = t.remaining - dt;
            if remaining >= 0.0 {
                if let Some(x) = self.ui.timers.get_mut(&w).and_then(|l| l.iter_mut().find(|x| x.id == id)) {
                    x.remaining = remaining;
                }
                continue;
            }
            let mut invocations = 1;
            let mut new_remaining = remaining;
            if t.looping {
                invocations = (-remaining / t.period) as i32 + 1;
                new_remaining = invocations as f32 * t.period + remaining;
            }
            if let Some(list) = self.ui.timers.get_mut(&w) {
                if t.looping {
                    if let Some(x) = list.iter_mut().find(|x| x.id == id) {
                        x.remaining = new_remaining;
                    }
                } else {
                    list.retain(|x| x.id != id);
                }
            }
            let name = if t.function != 0 { self.reg.names.str(t.function).to_string() } else { "Timer".to_string() };
            self.ui_event(w, &name, vec![Value::Int(id), Value::Int(invocations), Value::Int(t.client_data)]);
            if self.wb_pub(w, "bBeingDestroyed") {
                break;
            }
        }
    }
}

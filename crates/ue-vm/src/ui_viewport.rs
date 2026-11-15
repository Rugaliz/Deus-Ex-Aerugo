//! `ViewportWindow`: a window that shows the 3D world from another point of view (the drone and zoom views of the
//! augmentations, security cameras). Ported from the `XViewportWindow` functions of Extension.dll. The window works out the
//! camera each time it is drawn and hands a `ViewCmd` to the renderer, which draws the world into the window's rectangle
//! between the quads painted before and after it; a disabled window (or one without a renderer) tiles its default
//! texture instead.

use crate::ui_gc::{GcState, ViewCmd};
use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

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
fn vec3(a: &[Value], i: usize) -> [f32; 3] {
    match a.get(i) {
        Some(Value::Vector(v)) => *v,
        _ => [0.0; 3],
    }
}
fn rot3(a: &[Value], i: usize) -> [i32; 3] {
    match a.get(i) {
        Some(Value::Rotator(r)) => *r,
        _ => [0; 3],
    }
}

/// `FVector::Rotation`: the rotator that points along `d` (no roll).
fn rotator_of(d: [f32; 3]) -> [i32; 3] {
    crate::ai_move::vec_to_rot(d)
}

/// The forward axis of a rotator.
fn forward(r: [i32; 3]) -> [f32; 3] {
    let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
    let (sp, cp) = ang(r[0]).sin_cos();
    let (sy, cy) = ang(r[1]).sin_cos();
    [cp * cy, cp * sy, sp]
}

impl Vm {
    pub(crate) fn viewport_init(&mut self, w: u32, parent: u32) {
        let _ = parent;
        self.set_b_pub(w, "bDrawRawBackground", false);
        for (n, v) in [("bEnableViewport", true), ("bWatchEyeHeight", true), ("bUseEyeHeight", true), ("bUseViewRotation", true), ("bShowWeapons", false), ("bShowActor", false), ("bClearZ", true), ("bOriginActorDestroyed", false)] {
            self.set_b_pub(w, n, v);
        }
        let black = self.color_value_pub([0, 0, 0, 255]);
        let _ = self.set_prop(w, "defaultColor", 0, black);
        self.set_o_pub(w, "defaultTexture", 0);
        for n in ["Location", "relLocation", "lastLocation"] {
            let _ = self.set_prop(w, n, 0, Value::Vector([0.0; 3]));
        }
        for n in ["Rotation", "relRotation", "lastRotation"] {
            let _ = self.set_prop(w, n, 0, Value::Rotator([0; 3]));
        }
        self.set_f_pub(w, "fov", 90.0);
        self.set_o_pub(w, "originActor", 0);
        self.set_o_pub(w, "watchActor", 0);
        // the window starts by looking through the player's eyes
        let pawn = self.viewport_player(w);
        self.viewport_set_actor(w, pawn, true, true);
    }

    fn viewport_player(&self, w: u32) -> u32 {
        let root = self.ui_root_of_pub(w);
        if root != 0 {
            self.wo_pub(root, "parentPawn")
        } else {
            0
        }
    }

    fn vp_vec(&self, o: u32, n: &str) -> [f32; 3] {
        match self.get_prop(o, n, 0) {
            Ok(Value::Vector(v)) => v,
            _ => [0.0; 3],
        }
    }

    fn vp_rot(&self, o: u32, n: &str) -> [i32; 3] {
        match self.get_prop(o, n, 0) {
            Ok(Value::Rotator(r)) => r,
            _ => [0; 3],
        }
    }

    fn viewport_set_actor(&mut self, w: u32, actor: u32, eye: bool, enable: bool) {
        self.set_b_pub(w, "bEnableViewport", enable);
        self.set_b_pub(w, "bUseEyeHeight", eye);
        for n in ["Location", "relLocation"] {
            let _ = self.set_prop(w, n, 0, Value::Vector([0.0; 3]));
        }
        // (1004a170) a new origin is referred to before the old one is let go
        let old = self.vp_ref(w, "originActor").0;
        if old != actor {
            let pp = self.window_player(w);
            self.add_actor_ref(pp, actor);
            self.remove_actor_ref(pp, old);
        }
        self.set_o_pub(w, "originActor", actor);
        let last = if actor != 0 { self.vp_vec(actor, "Location") } else { [0.0; 3] };
        let _ = self.set_prop(w, "lastLocation", 0, Value::Vector(last));
    }

    pub(crate) fn native_viewport(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        match name {
            "SetViewportActor" => self.viewport_set_actor(this, ob(a, 0), bl(a, 1, true), bl(a, 2, true)),
            "SetViewportLocation" => {
                self.set_b_pub(this, "bEnableViewport", bl(a, 1, true));
                let loc = vec3(a, 0);
                let _ = self.set_prop(this, "Location", 0, Value::Vector(loc));
                let _ = self.set_prop(this, "relLocation", 0, Value::Vector([0.0; 3]));
                let old = self.vp_ref(this, "originActor").0;
                if old != 0 {
                    let pp = self.window_player(this);
                    self.remove_actor_ref(pp, old);
                }
                self.set_o_pub(this, "originActor", 0);
                let _ = self.set_prop(this, "lastLocation", 0, Value::Vector(loc));
            }
            "SetWatchActor" => {
                let actor = ob(a, 0);
                self.set_b_pub(this, "bWatchEyeHeight", bl(a, 1, true));
                for n in ["Rotation", "relRotation"] {
                    let _ = self.set_prop(this, n, 0, Value::Rotator([0; 3]));
                }
                let old = self.vp_ref(this, "watchActor").0;
                if old != actor {
                    let pp = self.window_player(this);
                    self.add_actor_ref(pp, actor);
                    self.remove_actor_ref(pp, old);
                }
                self.set_o_pub(this, "watchActor", actor);
                let last = if actor != 0 {
                    let (t, p) = (self.vp_vec(actor, "Location"), self.vp_vec(this, "lastLocation"));
                    rotator_of([t[0] - p[0], t[1] - p[1], t[2] - p[2]])
                } else {
                    self.vp_rot(this, "Rotation")
                };
                let _ = self.set_prop(this, "lastRotation", 0, Value::Rotator(last));
            }
            "SetRotation" => {
                let r = rot3(a, 0);
                let _ = self.set_prop(this, "Rotation", 0, Value::Rotator(r));
                let _ = self.set_prop(this, "relRotation", 0, Value::Rotator([0; 3]));
                let old = self.vp_ref(this, "watchActor").0;
                if old != 0 {
                    let pp = self.window_player(this);
                    self.remove_actor_ref(pp, old);
                }
                self.set_o_pub(this, "watchActor", 0);
                let _ = self.set_prop(this, "lastRotation", 0, Value::Rotator(r));
            }
            "EnableViewport" => self.set_b_pub(this, "bEnableViewport", bl(a, 0, true)),
            "SetFOVAngle" => {
                let f = a.first().and_then(Value::as_f32).filter(|_| !matches!(a.first(), Some(Value::Void))).unwrap_or(90.0);
                let f = if f > 179.0 { 179.0 } else { f };
                self.set_f_pub(this, "fov", if f < 0.1 { 0.1 } else { f });
            }
            "ShowViewportActor" => self.set_b_pub(this, "bShowActor", bl(a, 0, true)),
            "ShowWeapons" => self.set_b_pub(this, "bShowWeapons", bl(a, 0, true)),
            "SetRelativeLocation" => {
                let _ = self.set_prop(this, "relLocation", 0, Value::Vector(vec3(a, 0)));
            }
            "SetRelativeRotation" => {
                let _ = self.set_prop(this, "relRotation", 0, Value::Rotator(rot3(a, 0)));
            }
            "SetDefaultTexture" => {
                self.set_o_pub(this, "defaultTexture", ob(a, 0));
                let c = match a.get(1) {
                    Some(v @ Value::Struct(..)) => v.clone(),
                    _ => self.color_value_pub([255, 255, 255, 0]),
                };
                let _ = self.set_prop(this, "defaultColor", 0, c);
            }
            "ClearZBuffer" => self.set_b_pub(this, "bClearZ", bl(a, 0, true)),
            _ => return None,
        }
        Some(Ok(Value::Void))
    }

    fn is_pawn(&self, o: u32) -> bool {
        let pawn = self.reg.class_by_name("Pawn");
        match (self.class_of(o), pawn) {
            (Some(c), Some(p)) => self.is_a(c, p),
            _ => false,
        }
    }

    /// A reference property that may point at a destroyed object: (reference, still alive).
    fn vp_ref(&self, w: u32, n: &str) -> (u32, bool) {
        match self.get_prop(w, n, 0) {
            Ok(Value::Obj(r)) if r != 0 => (r, self.is_live(r)),
            _ => (0, true),
        }
    }

    /// The camera the window looks through this frame: position and rotation.
    fn viewport_camera(&mut self, w: u32) -> ([f32; 3], [i32; 3]) {
        // actors that were destroyed since the last frame: stay where they were last seen
        let (origin, ok) = self.vp_ref(w, "originActor");
        if origin != 0 && !ok {
            self.set_o_pub(w, "originActor", 0);
            let last = self.vp_vec(w, "lastLocation");
            let _ = self.set_prop(w, "Location", 0, Value::Vector(last));
            let _ = self.set_prop(w, "relLocation", 0, Value::Vector([0.0; 3]));
            let (lr, rr) = (self.vp_rot(w, "lastRotation"), self.vp_rot(w, "relRotation"));
            let _ = self.set_prop(w, "Rotation", 0, Value::Rotator([lr[0] - rr[0], lr[1] - rr[1], lr[2] - rr[2]]));
            self.set_b_pub(w, "bOriginActorDestroyed", true);
        }
        let (watch, ok) = self.vp_ref(w, "watchActor");
        if watch != 0 && !ok {
            self.set_o_pub(w, "watchActor", 0);
            let lr = self.vp_rot(w, "lastRotation");
            let _ = self.set_prop(w, "Rotation", 0, Value::Rotator(lr));
            let _ = self.set_prop(w, "relRotation", 0, Value::Rotator([0; 3]));
        }
        let (origin, watch) = (self.wo_pub(w, "originActor"), self.wo_pub(w, "watchActor"));
        let origin_pawn = origin != 0 && self.is_pawn(origin);
        let (mut pos, mut rot);
        if origin == 0 {
            pos = self.vp_vec(w, "Location");
            rot = self.vp_rot(w, "Rotation");
        } else {
            pos = self.vp_vec(origin, "Location");
            if origin_pawn && self.wb_pub(w, "bUseEyeHeight") {
                pos[2] += self.get_prop(origin, "EyeHeight", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0);
            }
            rot = if origin_pawn && self.wb_pub(w, "bUseViewRotation") { self.vp_rot(origin, "ViewRotation") } else { self.vp_rot(origin, "Rotation") };
        }
        // the offset is given in the frame of the viewing direction
        let rel = self.vp_vec(w, "relLocation");
        let dist = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
        let rr = rotator_of(rel);
        let axis = forward([rr[0] + rot[0], rr[1] + rot[1], rr[2] + rot[2]]);
        for k in 0..3 {
            pos[k] += axis[k] * dist;
        }
        if watch != 0 {
            let mut t = self.vp_vec(watch, "Location");
            if self.wb_pub(w, "bWatchEyeHeight") && self.is_pawn(watch) {
                t[2] += self.get_prop(watch, "EyeHeight", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0);
            }
            rot = rotator_of([t[0] - pos[0], t[1] - pos[1], t[2] - pos[2]]);
        }
        let rel_rot = self.vp_rot(w, "relRotation");
        rot = [rot[0] + rel_rot[0], rot[1] + rel_rot[1], rot[2] + rel_rot[2]];
        // the script may adjust the view
        let args = vec![Value::Obj(origin), Value::Obj(watch), Value::Vector(pos), Value::Rotator(rot)];
        let (_, out) = self.ui_script_event(w, "CalcView", args);
        pos = vec3(&out, 2);
        rot = rot3(&out, 3);
        let _ = self.set_prop(w, "lastLocation", 0, Value::Vector(pos));
        let _ = self.set_prop(w, "lastRotation", 0, Value::Rotator(rot));
        (pos, rot)
    }

    /// `XViewportWindow::Draw`.
    pub(crate) fn viewport_draw(&mut self, w: u32, g: u32) {
        let mut drew = false;
        if self.wb_pub(w, "bEnableViewport") && self.ui.wants_views {
            let (pos, rot) = self.viewport_camera(w);
            let st: GcState = self.gc_state(g);
            let (ox, oy) = (st.clip[0], st.clip[1]);
            let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
            let (hm, vm) = (st.h_mul as f32, st.v_mul as f32);
            if width > 0.0 && height > 0.0 {
                let origin = self.wo_pub(w, "originActor");
                let player = self.viewport_player(w);
                let cmd = ViewCmd {
                    after: self.ui.draw.len(),
                    seq: {
                        self.ui.cmd_seq += 1;
                        self.ui.cmd_seq
                    },
                    rect: [ox * hm, oy * vm, width * hm, height * vm],
                    clip: [(ox + st.clip[2]) * hm, (oy + st.clip[3]) * vm, st.clip[4] * hm, st.clip[5] * vm],
                    pos,
                    rot,
                    fov: self.wf_pub(w, "fov"),
                    clear_z: self.wb_pub(w, "bClearZ"),
                    hide_actor: if origin != 0 && !self.wb_pub(w, "bShowActor") { origin } else { 0 },
                    show_weapons: self.wb_pub(w, "bShowWeapons") && origin == player && origin != 0,
                };
                // its master frame (`CreateMasterFrame`, then the window's size and field of view): what its
                // `ConvertVectorToCoordinates` projects through
                let frame = crate::ui::ViewInfo { pos, rot, fov_deg: cmd.fov, width: ((width * hm) as i32) as f32, height: ((height * vm) as i32) as f32 };
                self.ui.viewport_frames.insert(w, frame);
                self.ui.views.push(cmd);
                drew = true;
            }
        }
        if !drew {
            // nothing to show: the default texture, tiled
            let tex = self.wo_ref_pub(w, "defaultTexture");
            let color = self.color_bytes(w, "defaultColor");
            {
                let st = self.gc_mut(g);
                st.set_style(1);
                st.tile_color = color;
            }
            let st = self.gc_state(g);
            let (width, height) = (self.wf_pub(w, "width"), self.wf_pub(w, "height"));
            self.gc_draw_pattern(&st, 0.0, 0.0, width, height, 0.0, 0.0, tex);
        }
        self.ui_event(w, "DrawWindow", vec![Value::Obj(g)]);
    }
}

/// `APlayerPawnExt`'s list of actors windows refer to (`actorList[32]` of `ActorRef { Actor, refCount }`,
/// `actorCount`; Extension 10037f50 / 10038060 / 10038400). `IsActorValid` answers from it, not from the level.
impl Vm {
    fn actor_ref_at(&mut self, pp: u32, i: usize) -> (u32, i32) {
        match self.get_prop(pp, "actorList", i as u32) {
            Ok(Value::Struct(_, b)) if b.len() >= 8 => (u32::from_le_bytes([b[0], b[1], b[2], b[3]]), i32::from_le_bytes([b[4], b[5], b[6], b[7]])),
            _ => (0, 0),
        }
    }

    fn set_actor_ref_at(&mut self, pp: u32, i: usize, actor: u32, count: i32) {
        if let Ok(Value::Struct(id, mut b)) = self.get_prop(pp, "actorList", i as u32) {
            if b.len() >= 8 {
                b[..4].copy_from_slice(&actor.to_le_bytes());
                b[4..8].copy_from_slice(&count.to_le_bytes());
                let _ = self.set_prop(pp, "actorList", i as u32, Value::Struct(id, b));
            }
        }
    }

    fn actor_ref_count(&mut self, pp: u32) -> usize {
        self.get_prop(pp, "actorCount", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0).clamp(0, 32) as usize
    }

    /// `FindActor`: drops the entries of destroyed actors (compacting the list) and returns the index of `actor`.
    fn find_actor_ref(&mut self, pp: u32, actor: u32) -> i32 {
        let n = self.actor_ref_count(pp);
        let (mut kept, mut found) = (0usize, -1i32);
        for i in 0..n {
            let (mut a, c) = self.actor_ref_at(pp, i);
            if a != 0 && !self.is_live(a) {
                a = 0;
            }
            if a == actor {
                // the index as counted before this entry is moved down
                found = kept as i32;
            }
            if a != 0 {
                self.set_actor_ref_at(pp, kept, a, c);
                kept += 1;
            }
        }
        let _ = self.set_prop(pp, "actorCount", 0, Value::Int(kept as i32));
        found
    }

    pub(crate) fn add_actor_ref(&mut self, pp: u32, actor: u32) {
        if pp == 0 || actor == 0 {
            return;
        }
        let i = self.find_actor_ref(pp, actor);
        if i >= 0 {
            let (a, c) = self.actor_ref_at(pp, i as usize);
            self.set_actor_ref_at(pp, i as usize, a, c + 1);
            return;
        }
        let mut n = self.actor_ref_count(pp);
        if n > 31 {
            // full: the oldest goes
            for j in 1..n {
                let (a, c) = self.actor_ref_at(pp, j);
                self.set_actor_ref_at(pp, j - 1, a, c);
            }
            n -= 1;
        }
        self.set_actor_ref_at(pp, n, actor, 1);
        let _ = self.set_prop(pp, "actorCount", 0, Value::Int(n as i32 + 1));
    }

    pub(crate) fn remove_actor_ref(&mut self, pp: u32, actor: u32) {
        if pp == 0 || actor == 0 {
            return;
        }
        let i = self.find_actor_ref(pp, actor);
        if i < 0 {
            return;
        }
        let (a, c) = self.actor_ref_at(pp, i as usize);
        if c - 1 >= 1 {
            self.set_actor_ref_at(pp, i as usize, a, c - 1);
            return;
        }
        let n = self.actor_ref_count(pp);
        for j in i as usize..n.saturating_sub(1) {
            let (a, c) = self.actor_ref_at(pp, j + 1);
            self.set_actor_ref_at(pp, j, a, c);
        }
        let _ = self.set_prop(pp, "actorCount", 0, Value::Int(n as i32 - 1));
    }

    pub(crate) fn is_actor_ref_valid(&mut self, pp: u32, actor: u32) -> bool {
        pp != 0 && actor != 0 && self.find_actor_ref(pp, actor) >= 0
    }

    /// The window's player (`XWindow::GetPlayerPawn`: its root's pawn).
    pub(crate) fn window_player(&mut self, w: u32) -> u32 {
        let root = self.ui_root_of_pub(w);
        if root != 0 {
            self.wo_pub(root, "parentPawn")
        } else {
            0
        }
    }
}

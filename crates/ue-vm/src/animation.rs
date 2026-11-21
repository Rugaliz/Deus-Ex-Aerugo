//! Mesh animation state: `PlayAnim`, `LoopAnim`, `TweenAnim`, `FinishAnim`, `HasAnim`, `GetAnimGroup` and the per-frame
//! advance of `AnimFrame` (with `AnimEnd` and mesh notify events). Ports of the original behaviour. Frames are
//! fractions of the sequence: 0 is the first frame, `AnimLast` the last (1 - 1/frames); a negative `AnimFrame`
//! means "tweening" from the previous pose. Blended animations (`PlayBlendAnim`) are advanced after the main sequence.

use crate::{Vm, VmError};
use std::rc::Rc;

type Res<T> = Result<T, VmError>;

/// One animation sequence of a mesh, with name ids resolved to registry-wide names.
#[derive(Debug, Clone)]
pub struct SeqInfo {
    pub name: u32,
    pub group: u32,
    pub start_frame: i32,
    pub num_frames: i32,
    pub rate: f32,
    /// (time as a fraction of the sequence, global name id of the script function)
    pub notifys: Vec<(f32, u32)>,
}

impl Vm {
    /// Sequences of the mesh in the actor's `Mesh` property (empty when it has none).
    pub(crate) fn mesh_seqs(&mut self, a: u32) -> Option<Rc<Vec<SeqInfo>>> {
        let mesh_ref = self.op(a, "Mesh");
        if mesh_ref == 0 {
            return None;
        }
        if let Some(v) = self.mesh_anims.get(&mesh_ref) {
            return v.clone();
        }
        let path = self.reg.obj_path(ue_registry::ObjId(mesh_ref)).to_string();
        let seqs = (|| {
            let (pi, ei) = self.reg.export_by_path(&path)?;
            let pkg = &self.reg.packages[pi].pkg;
            let raw = match pkg.export_class_name(ei).as_str() {
                "LodMesh" => ue_assets::mesh::LodMesh::load(pkg, ei).ok()?.mesh.anim_seqs,
                "Mesh" => ue_assets::mesh::Mesh::load(pkg, ei).ok()?.anim_seqs,
                _ => return None,
            };
            Some(
                raw.iter()
                    .map(|s| SeqInfo {
                        name: self.reg.global_name(pi, s.name),
                        group: self.reg.global_name(pi, s.group),
                        start_frame: s.start_frame,
                        num_frames: s.num_frames.max(1),
                        rate: s.rate,
                        notifys: s.notifys.iter().map(|n| (n.time, self.reg.global_name(pi, n.function))).collect(),
                    })
                    .collect::<Vec<_>>(),
            )
        })()
        .map(Rc::new);
        self.mesh_anims.insert(mesh_ref, seqs.clone());
        seqs
    }

    fn seq_of(&mut self, a: u32, name: u32) -> Option<SeqInfo> {
        self.mesh_seqs(a)?.iter().find(|s| s.name == name).cloned()
    }

    /// `PlayAnim` / `LoopAnim` (`looping`): start a sequence at `rate`, tweening in over `tween` seconds (-1: the engine's
    /// default blend from the previous animation). `min_rate` is the speed-scaled minimum for looping walk cycles.
    pub fn native_play_anim(&mut self, a: u32, name: u32, rate: f32, tween: f32, min_rate: f32, looping: bool) {
        let Some(seq) = self.seq_of(a, name) else {
            let n = self.reg.names.str(name).to_string();
            let who = self.name_of(a);
            self.log.push(format!("{who}: sequence {n} not found"));
            return;
        };
        let o = self.offsets();
        let n = seq.num_frames as f32;
        let mut tween = tween;
        if looping && self.u32_at_prop(a, "AnimSequence") == name && self.bp(a, "bAnimLoop") {
            // already looping this sequence: only the speed changes, if it is still running
            let frame = self.fp(a, "AnimFrame");
            let running = if frame < 0.0 { self.fp(a, "TweenRate") != 0.0 } else { self.fp(a, "AnimRate") != 0.0 };
            if running {
                self.set_bp(a, "bAnimFinished", false);
                let r = rate * seq.rate / n;
                self.set_fp(a, "AnimRate", r);
                self.set_fp(a, "AnimMinRate", if min_rate == 0.0 { 0.0 } else { seq.rate / n * min_rate });
                self.set_fp(a, "OldAnimRate", r);
                return;
            }
        }
        if self.u32_at_prop(a, "AnimSequence") == 0 {
            tween = 0.0;
        }
        self.set_u32_prop(a, "AnimSequence", name);
        let anim_rate = rate * seq.rate / n;
        self.set_fp(a, "AnimRate", anim_rate);
        self.set_fp(a, "AnimLast", 1.0 - 1.0 / n);
        if looping {
            self.set_fp(a, "AnimMinRate", if min_rate == 0.0 { 0.0 } else { seq.rate / n * min_rate });
        }
        self.set_bp(a, "bAnimFinished", false);
        self.set_bp(a, "bAnimNotify", !seq.notifys.is_empty());
        self.set_bp(a, "bAnimLoop", looping);
        let old_rate = self.fp(a, "OldAnimRate");
        if self.fp(a, "AnimLast") == 0.0 {
            // a single pose: tween into it, nothing to play
            self.set_fp(a, "AnimMinRate", 0.0);
            self.set_bp(a, "bAnimFinished", false);
            self.set_bp(a, "bAnimNotify", false);
            self.set_fp(a, "OldAnimRate", 0.0);
            self.set_fp(a, "TweenRate", if tween <= 0.0 { 10.0 } else { 1.0 / tween });
            self.set_fp(a, "AnimRate", 0.0);
            self.set_fp(a, "AnimFrame", -1.0 / n);
        } else if tween <= 0.0 {
            if tween == -1.0 {
                self.set_fp(a, "AnimFrame", -1.0 / n);
                let tr = if old_rate > 0.0 {
                    old_rate
                } else if old_rate < 0.0 {
                    let v = self.vec_at(a, o.velocity);
                    let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    (speed * old_rate * -1.0).max(anim_rate * 0.5)
                } else {
                    1.0 / (n * 0.025)
                };
                self.set_fp(a, "TweenRate", tr);
            } else {
                // no tween: start just past the first frame (`LoopAnim` 0.0001, `PlayAnim` 0.001)
                self.set_fp(a, "TweenRate", 0.0);
                self.set_fp(a, "AnimFrame", if looping { 0.0001 } else { 0.001 });
            }
        } else {
            self.set_fp(a, "TweenRate", 1.0 / (n * tween));
            self.set_fp(a, "AnimFrame", -1.0 / n);
        }
        let r = self.fp(a, "AnimRate");
        self.set_fp(a, "OldAnimRate", r);
    }

    /// `TweenAnim(seq, time)`: blend to the first pose of a sequence without playing it.
    pub fn native_tween_anim(&mut self, a: u32, name: u32, time: f32) {
        let Some(seq) = self.seq_of(a, name) else {
            let n = self.reg.names.str(name).to_string();
            let who = self.name_of(a);
            self.log.push(format!("{who}: sequence {n} not found"));
            return;
        };
        let n = seq.num_frames as f32;
        self.set_u32_prop(a, "AnimSequence", name);
        self.set_bp(a, "bAnimFinished", false);
        self.set_bp(a, "bAnimNotify", false);
        self.set_bp(a, "bAnimLoop", false);
        for p in ["AnimLast", "AnimMinRate", "AnimRate", "OldAnimRate"] {
            self.set_fp(a, p, 0.0);
        }
        if time <= 0.0 {
            self.set_fp(a, "TweenRate", 0.0);
            self.set_fp(a, "AnimFrame", 0.0);
        } else {
            self.set_fp(a, "TweenRate", 1.0 / (n * time));
            self.set_fp(a, "AnimFrame", -1.0 / n);
        }
    }

    fn blend_f(&self, a: u32, prop: &str, slot: u32) -> f32 {
        self.get_prop(a, prop, slot).ok().and_then(|v| v.as_f32()).unwrap_or(0.0)
    }

    fn set_blend_f(&mut self, a: u32, prop: &str, slot: u32, v: f32) {
        let _ = self.set_prop(a, prop, slot, crate::Value::Float(v));
    }

    /// The blend channel `slot` of `a` with its sequence `name` found in the mesh; logs and gives `None` otherwise
    /// (a slot outside 0..3, no mesh, an unknown sequence: nothing changes).
    fn blend_target(&mut self, a: u32, name: u32, slot: i32) -> Option<(u32, SeqInfo)> {
        let who = self.name_of(a);
        if !(0..=3).contains(&slot) {
            self.log.push(format!("{who}: blend slot {slot} out of range"));
            return None;
        }
        if self.op(a, "Mesh") == 0 {
            self.log.push(format!("{who}: blend animation without a mesh"));
            return None;
        }
        match self.seq_of(a, name) {
            Some(s) => Some((slot as u32, s)),
            None => {
                let n = self.reg.names.str(name).to_string();
                self.log.push(format!("{who}: sequence {n} not found"));
                None
            }
        }
    }

    /// `SimBlendAnim[slot]` for replication: frame, rate and last times 10000, tween rate times 1000; `PlayBlendAnim`
    /// bumps W by 1 when nothing changed (so the change still replicates).
    fn set_sim_blend(&mut self, a: u32, slot: u32, bump_if_same: bool) {
        let v = [
            self.blend_f(a, "BlendAnimFrame", slot) * 10000.0,
            self.blend_f(a, "BlendAnimRate", slot) * 10000.0,
            self.blend_f(a, "BlendTweenRate", slot) * 1000.0,
            self.blend_f(a, "BlendAnimLast", slot) * 10000.0,
        ];
        if let Ok(crate::Value::Struct(id, old)) = self.get_prop(a, "SimBlendAnim", slot) {
            let prev: Vec<f32> = old.chunks(4).take(4).map(|b| f32::from_le_bytes(b.try_into().unwrap_or([0; 4]))).collect();
            let mut v = v;
            if bump_if_same && prev.len() == 4 && prev.iter().zip(v.iter()).all(|(p, n)| p == n) {
                v[3] += 1.0;
            }
            let mut bytes = old.clone();
            for (i, f) in v.iter().enumerate() {
                if bytes.len() >= i * 4 + 4 {
                    bytes[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
                }
            }
            let _ = self.set_prop(a, "SimBlendAnim", slot, crate::Value::Struct(id, bytes));
        }
    }

    /// `PlayBlendAnim(seq, rate = 1, tween = -1, slot = 0)` (`AActor::execPlayBlendAnim`, 103e0d80): plays `seq` on a
    /// blend channel; the same rules as `PlayAnim` for the rate, the last frame and the tween (no tween into a channel
    /// that had no sequence; a single pose tweens in at 10 per second or `1/tween`; -1 blends from the channel's old
    /// rate; another tween <= 0 starts just past the first frame), then `SimBlendAnim` and `OldBlendAnimRate`.
    pub fn native_play_blend_anim(&mut self, a: u32, name: u32, rate: f32, tween: f32, slot: i32) {
        let Some((s, seq)) = self.blend_target(a, name, slot) else { return };
        let n = seq.num_frames as f32;
        let mut tween = tween;
        if self.get_prop(a, "BlendAnimSequence", s).ok().and_then(|v| if let crate::Value::Name(x) = v { Some(x) } else { None }).unwrap_or(0) == 0 {
            tween = 0.0;
        }
        let _ = self.set_prop(a, "BlendAnimSequence", s, crate::Value::Name(name));
        let anim_rate = rate * seq.rate / n;
        self.set_blend_f(a, "BlendAnimRate", s, anim_rate);
        self.set_blend_f(a, "BlendAnimLast", s, 1.0 - 1.0 / n);
        if self.blend_f(a, "BlendAnimLast", s) == 0.0 {
            self.set_blend_f(a, "BlendAnimMinRate", s, 0.0);
            self.set_blend_f(a, "OldBlendAnimRate", s, 0.0);
            self.set_blend_f(a, "BlendTweenRate", s, if tween <= 0.0 { 10.0 } else { 1.0 / tween });
            self.set_blend_f(a, "BlendAnimFrame", s, -1.0 / n);
            self.set_blend_f(a, "BlendAnimRate", s, 0.0);
        } else if tween <= 0.0 {
            if tween == -1.0 {
                self.set_blend_f(a, "BlendAnimFrame", s, -1.0 / n);
                let old = self.blend_f(a, "OldBlendAnimRate", s);
                let tr = if old > 0.0 {
                    old
                } else if old < 0.0 {
                    let o = self.offsets();
                    let v = self.vec_at(a, o.velocity);
                    let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    (speed * old * -1.0).max(anim_rate * 0.5)
                } else {
                    1.0 / (n * 0.025)
                };
                self.set_blend_f(a, "BlendTweenRate", s, tr);
            } else {
                self.set_blend_f(a, "BlendTweenRate", s, 0.0);
                self.set_blend_f(a, "BlendAnimFrame", s, 0.001);
            }
        } else {
            self.set_blend_f(a, "BlendTweenRate", s, 1.0 / (n * tween));
            self.set_blend_f(a, "BlendAnimFrame", s, -1.0 / n);
        }
        self.set_sim_blend(a, s, true);
        let r = self.blend_f(a, "BlendAnimRate", s);
        self.set_blend_f(a, "OldBlendAnimRate", s, r);
    }

    /// `TweenBlendAnim(seq, time, slot = 0)` (`AActor::execTweenBlendAnim`, 103e1300): a blend channel tweens to the
    /// first pose of `seq` over `time` seconds (at once when `time` <= 0) and stays there.
    pub fn native_tween_blend_anim(&mut self, a: u32, name: u32, time: f32, slot: i32) {
        let Some((s, seq)) = self.blend_target(a, name, slot) else { return };
        let n = seq.num_frames as f32;
        let _ = self.set_prop(a, "BlendAnimSequence", s, crate::Value::Name(name));
        for p in ["BlendAnimLast", "BlendAnimMinRate", "BlendAnimRate", "OldBlendAnimRate"] {
            self.set_blend_f(a, p, s, 0.0);
        }
        if time <= 0.0 {
            self.set_blend_f(a, "BlendTweenRate", s, 0.0);
            self.set_blend_f(a, "BlendAnimFrame", s, 0.0);
        } else {
            self.set_blend_f(a, "BlendTweenRate", s, 1.0 / (n * time));
            self.set_blend_f(a, "BlendAnimFrame", s, -1.0 / n);
        }
        self.set_sim_blend(a, s, false);
    }

    /// `FinishAnim` (103037ba): a looping animation stops looping (`bAnimFinished` and `bAnimLoop` cleared, only then),
    /// and when a sequence is still running the state code waits until it ends.
    pub fn native_finish_anim(&mut self, a: u32) {
        if self.bp(a, "bAnimLoop") {
            self.set_bp(a, "bAnimFinished", false);
            self.set_bp(a, "bAnimLoop", false);
        }
        if self.u32_at_prop(a, "AnimSequence") == 0 {
            return;
        }
        let frame = self.fp(a, "AnimFrame");
        let running = if frame < 0.0 { self.fp(a, "TweenRate") != 0.0 } else { self.fp(a, "AnimRate") != 0.0 };
        if running && frame < self.fp(a, "AnimLast") {
            self.pending_latent = Some(crate::Latent::External("FinishAnim".into()));
        }
    }

    pub fn native_has_anim(&mut self, a: u32, name: u32) -> bool {
        self.seq_of(a, name).is_some()
    }

    pub fn native_anim_group(&mut self, a: u32, name: u32) -> u32 {
        self.seq_of(a, name).map(|s| s.group).unwrap_or(0)
    }

    pub(crate) fn u32_at_prop(&mut self, a: u32, name: &'static str) -> u32 {
        self.op(a, name)
    }

    pub(crate) fn set_u32_prop(&mut self, a: u32, name: &'static str, v: u32) {
        if let Some((o, _)) = self.prop_at(a, name) {
            self.set_u32_at(a, o, v);
        }
    }

    /// Advances an actor's animation by `dt` (the first thing `AActor::Tick` does): tweening, playing, looping,
    /// mesh notifies and `AnimEnd`.
    pub fn tick_animation(&mut self, a: u32, dt: f32) -> Res<()> {
        let o = self.offsets();
        let mut remaining = dt;
        // one counter for the main sequence and the blend channels (`AActor::Tick`): at most 4 passes
        let mut loops = 0;
        loop {
            let seq_name = self.u32_at_prop(a, "AnimSequence");
            if seq_name == 0 {
                return Ok(());
            }
            let frame = self.fp(a, "AnimFrame");
            let running = if frame < 0.0 { self.fp(a, "TweenRate") != 0.0 } else { self.fp(a, "AnimRate") != 0.0 };
            if !running {
                return Ok(());
            }
            if remaining <= 0.0 {
                return Ok(());
            }
            loops += 1;
            if loops > 4 {
                return Ok(());
            }
            let last = frame;
            if frame < 0.0 {
                // tweening toward the first frame
                let f = remaining * self.fp(a, "TweenRate") + frame;
                self.set_fp(a, "AnimFrame", f);
                if f < 0.0 {
                    return self.tick_blend_anims(a, remaining, loops);
                }
                self.set_fp(a, "AnimFrame", 0.0);
                remaining = f * remaining / (f - last);
                if self.fp(a, "AnimRate") == 0.0 {
                    self.set_bp(a, "bAnimFinished", true);
                    self.send_event(a, "AnimEnd", vec![])?;
                }
                continue;
            }
            let rate = self.fp(a, "AnimRate");
            let new = if rate < 0.0 {
                let v = self.vec_at(a, o.velocity);
                let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                frame + (-speed * rate).max(self.fp(a, "AnimMinRate")) * remaining
            } else {
                frame + remaining * rate
            };
            self.set_fp(a, "AnimFrame", new);
            if self.bp(a, "bAnimNotify") {
                // the earliest notify passed during this step
                let notify = self.seq_of_current(a).and_then(|s| s.notifys.iter().copied().filter(|&(t, _)| t > last && t <= new).min_by(|x, y| x.0.total_cmp(&y.0)));
                if let Some((t, func)) = notify {
                    remaining = (new - t) * remaining / (new - last);
                    self.set_fp(a, "AnimFrame", t);
                    let fname = self.reg.names.str(func).to_string();
                    if self.reg.names.get(&fname).is_some() {
                        self.send_event(a, &fname, vec![])?;
                    }
                    continue;
                }
            }
            let anim_last = self.fp(a, "AnimLast");
            if new < anim_last {
                return self.tick_blend_anims(a, remaining, loops);
            }
            if !self.bp(a, "bAnimLoop") {
                remaining = (new - anim_last) * remaining / (new - last);
                self.set_fp(a, "AnimFrame", anim_last);
                self.set_fp(a, "AnimRate", 0.0);
                self.set_bp(a, "bAnimFinished", true);
                self.send_event(a, "AnimEnd", vec![])?;
            } else {
                if new >= 1.0 {
                    remaining = (new - 1.0) * remaining / (new - last);
                    self.set_fp(a, "AnimFrame", 0.0);
                } else {
                    remaining = 0.0;
                }
                if last < anim_last {
                    if matches!(self.latent_of(a), Some(crate::Latent::External(ref n)) if n == "FinishAnim") {
                        self.set_bp(a, "bAnimFinished", true);
                    }
                    self.send_event(a, "AnimEnd", vec![])?;
                }
            }
        }
    }

    /// The blend channels (`BlendAnimSequence[4]`: lip sync, blinking, head turns), advanced after the main sequence
    /// while it is still playing, with the time it left over (`AActor::Tick`). The channels share that time in order
    /// (one that finishes passes on the rest) and the pass repeats while the shared counter allows: a channel that does
    /// not finish is advanced again, as in the engine.
    fn tick_blend_anims(&mut self, a: u32, mut remaining: f32, mut loops: i32) -> Res<()> {
        let o = self.offsets();
        loop {
            if self.u32_at_prop(a, "AnimSequence") == 0 {
                return Ok(());
            }
            let frame = self.fp(a, "AnimFrame");
            let running = if frame < 0.0 { self.fp(a, "TweenRate") != 0.0 } else { self.fp(a, "AnimRate") != 0.0 };
            if !running || remaining <= 0.0 {
                return Ok(());
            }
            loops += 1;
            if loops >= 5 {
                return Ok(());
            }
            let Some((seq_off, _)) = self.prop_at(a, "BlendAnimSequence") else { return Ok(()) };
            let field = |vm: &mut Vm, n: &'static str| vm.prop_at(a, n).map(|(off, _)| off);
            let (Some(f_off), Some(r_off), Some(l_off), Some(t_off), Some(m_off)) = (field(self, "BlendAnimFrame"), field(self, "BlendAnimRate"), field(self, "BlendAnimLast"), field(self, "BlendTweenRate"), field(self, "BlendAnimMinRate")) else { return Ok(()) };
            for i in 0..4usize {
                let k = 4 * i;
                if self.u32_at(a, seq_off + k) == 0 {
                    continue;
                }
                let last = self.f32_at(a, f_off + k);
                let end = self.f32_at(a, l_off + k);
                if last >= end {
                    continue;
                }
                if last < 0.0 {
                    let f = remaining * self.f32_at(a, t_off + k) + last;
                    self.set_u32_at(a, f_off + k, f.to_bits());
                    if f < 0.0 {
                        continue;
                    }
                    self.set_u32_at(a, f_off + k, 0f32.to_bits());
                    remaining = f * remaining / (f - last);
                    continue;
                }
                let rate = self.f32_at(a, r_off + k);
                let f = if rate < 0.0 {
                    let v = self.vec_at(a, o.velocity);
                    let speed = ((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) as f64).sqrt() as f32;
                    (-(speed * rate)).max(self.f32_at(a, m_off + k)) * remaining + last
                } else {
                    remaining * rate + last
                };
                self.set_u32_at(a, f_off + k, f.to_bits());
                if f < end {
                    continue;
                }
                self.set_u32_at(a, f_off + k, end.to_bits());
                self.set_u32_at(a, r_off + k, 0f32.to_bits());
                remaining = (f - end) * remaining / (f - last);
            }
        }
    }

    fn seq_of_current(&mut self, a: u32) -> Option<SeqInfo> {
        let name = self.u32_at_prop(a, "AnimSequence");
        self.seq_of(a, name)
    }

    fn latent_of(&self, a: u32) -> Option<crate::Latent> {
        crate::ref_instance(a).and_then(|i| self.objects.get(i)).and_then(|o| o.latent.clone())
    }
}

/// What the renderer needs to draw an actor's mesh.
#[derive(Debug, Clone)]
pub struct MeshDraw {
    /// Object path of the mesh.
    pub mesh: String,
    /// Absolute mesh frames to blend and the blend factor toward `frame_b`.
    pub frame_a: usize,
    pub frame_b: usize,
    pub blend: f32,
    pub location: [f32; 3],
    pub rotation: [i32; 3],
    /// `PrePivot`: `GetFrame` places the mesh at `Location + PrePivot`.
    pub pre_pivot: [f32; 3],
    pub scale: f32,
    /// Texture object paths per texture index (`MultiSkins[i]`, else `Skin` for index 0).
    pub skins: Vec<Option<String>>,
    /// `Style` of the actor (0 none, 1 normal, 2 masked, 3 translucent, 4 modulated), `ScaleGlow` and `bUnlit`.
    pub style: u8,
    pub glow: f32,
    pub unlit: bool,
    /// Texture of a `DT_Sprite` actor (then `mesh` is empty).
    pub sprite: Option<String>,
    /// Surface normal and roll of an attached decal (then `sprite` is its texture and `location` its centre).
    /// A decal: the half-axes of its square (right, up), already sized.
    pub decal: Option<([f32; 3], [f32; 3])>,
    /// A decal's pieces as `ClipDecal` cuts them: one convex polygon per surface entry and touched node, each vertex
    /// with its texel coordinates (empty: draw the `decal` square).
    pub decal_polys: Vec<Vec<([f32; 3], [f32; 2])>>,
    /// Blend channels (`PlayBlendAnim`: lip sync, blinking, head turns) as weighted absolute mesh frames: the renderer
    /// adds `weight * (frame - frame 0)` to every vertex (`UMesh::GetFrame`).
    pub blends: Vec<(usize, f32)>,
    /// `DrawLodMesh`'s inputs: `Fatness`, `AmbientGlow`, `LODBias` and the draw's poly flags (`draw_poly_flags`).
    pub fatness: u8,
    pub ambient_glow: u8,
    pub lod_bias: f32,
    pub flags: u32,
    /// The environment map: `Texture`, else the zone's then the level's `EnvironmentMap` (its current frame).
    pub env_texture: Option<String>,
    /// The viewing player's `Sprite`: drawn instead of every texture of the mesh (the `Matrix` easter egg).
    pub all_textures: Option<String>,
}

impl MeshDraw {
    /// A draw with no mesh inputs (sprites, decals, tests): normal fatness, no glow, LOD bias 1, no flags.
    pub fn plain() -> MeshDraw {
        MeshDraw {
            mesh: String::new(),
            frame_a: 0,
            frame_b: 0,
            blend: 0.0,
            location: [0.0; 3],
            rotation: [0; 3],
            pre_pivot: [0.0; 3],
            scale: 1.0,
            skins: Vec::new(),
            style: 1,
            glow: 1.0,
            unlit: false,
            sprite: None,
            decal: None,
            decal_polys: Vec::new(),
            blends: Vec::new(),
            fatness: 128,
            ambient_glow: 0,
            lod_bias: 1.0,
            flags: 0,
            env_texture: None,
            all_textures: None,
        }
    }
}

/// Cache key of the main sequence (blend channels are 0..3).
pub(crate) const MAIN_CHANNEL: u8 = 4;

/// Per actor and animation channel (the main sequence and the four blend channels): the last pose drawn, kept by
/// `ULodMesh::GetFrame` (Engine.dll 10355360) in the memory cache so that a tween can start from it.
#[derive(Debug, Clone)]
pub struct PoseCache {
    /// Sequence of the last tween (only a tween changes it).
    pub seq: u32,
    /// Channel frame at the previous tween step.
    pub last: f32,
    /// Accumulated part of the tween done; the pose stops moving once it passes 0.97.
    pub progress: f32,
    /// The pose drawn: absolute mesh frames with weights.
    pub pose: Vec<(usize, f32)>,
}

impl Default for PoseCache {
    fn default() -> Self {
        PoseCache { seq: 0, last: 0.0, progress: 1.0, pose: Vec::new() }
    }
}

/// The pose of a playing channel at frame `f` (0..1) of a sequence of `n` frames from `start`: two neighbouring frames
/// blended, the last one wrapping to the first (the engine does not look at `bAnimLoop` here).
fn playing_pose(f: f32, start: usize, n: usize) -> Vec<(usize, f32)> {
    let x = n as f32 * f.max(0.0);
    let k = (x - 0.5).round_ties_even() as i64;
    let frac = x - k as f32;
    let k = k.rem_euclid(n as i64) as usize;
    if frac > 0.0 {
        vec![(start + k, 1.0 - frac), (start + (k + 1) % n, frac)]
    } else {
        vec![(start + k, 1.0)]
    }
}

/// One drawing step of a channel's pose (`ULodMesh::GetFrame`): `seq` is the channel's sequence (`start`, `n`; `None`
/// when the mesh has no such sequence: frame 0), `frame` its frame (negative: tweening into the first frame).
/// A playing channel (or one without a cache yet) shows its frame. A tweening one moves the cached pose toward the
/// sequence's first frame by `1 - frame / last` of the way left; a new tween (other sequence, or the frame went
/// backwards) starts with no movement and `last = -1 / n`; once the accumulated progress passes 0.97 the pose stays
/// where it is until the tween ends (the progress restarts only for another sequence, so a second tween into the same
/// sequence does not move at all).
pub(crate) fn pose_step(cache: &mut PoseCache, fresh: bool, name: u32, seq: Option<(usize, usize)>, frame: f32) -> Vec<(usize, f32)> {
    if frame >= 0.0 || fresh {
        cache.pose = match seq {
            Some((start, n)) => playing_pose(frame, start, n),
            None => vec![(0, 1.0)],
        };
        return cache.pose.clone();
    }
    let (target, reset_last) = match seq {
        Some((start, n)) => (start, -1.0 / n as f32),
        None => (0, 0.0),
    };
    let mut fraction = 1.0 - frame / cache.last;
    if cache.seq != name {
        cache.progress = 0.0;
    }
    if cache.seq != name || fraction < 0.0 || fraction > 1.0 || fraction.is_nan() {
        fraction = 0.0;
        cache.last = reset_last;
        cache.seq = name;
    }
    cache.progress += (1.0 - cache.progress) * fraction;
    if cache.progress <= 0.97 && fraction > 0.0 {
        let mut p = Vec::new();
        pose_mix(&mut p, &cache.pose, 1.0 - fraction);
        pose_mix(&mut p, &[(target, 1.0)], fraction);
        cache.pose = p;
    }
    cache.last = frame;
    cache.pose.clone()
}

/// A pose as the renderer takes it: two frames and a blend factor, plus extra weighted frames added as
/// `weight * (frame - frame 0)`. A pose of more than two frames (a tween from a tween) is drawn from its heaviest frame
/// with the others as differences to it.
fn split_pose(pose: &[(usize, f32)]) -> (usize, usize, f32, Vec<(usize, f32)>) {
    match pose {
        [] => (0, 0, 0.0, Vec::new()),
        [(a, _)] => (*a, *a, 0.0, Vec::new()),
        [(a, wa), (b, wb)] => (*a, *b, if wa + wb != 0.0 { wb / (wa + wb) } else { 0.0 }, Vec::new()),
        _ => {
            let h = pose.iter().enumerate().max_by(|x, y| x.1 .1.partial_cmp(&y.1 .1).unwrap_or(std::cmp::Ordering::Equal)).map(|x| x.0).unwrap_or(0);
            let (hf, hw) = pose[h];
            let total: f32 = pose.iter().map(|e| e.1).sum();
            let mut extra: Vec<(usize, f32)> = pose.iter().enumerate().filter(|(i, _)| *i != h).map(|(_, &e)| e).collect();
            extra.push((hf, hw - total));
            (hf, hf, 0.0, extra)
        }
    }
}

/// Adds weighted frames, merging equal frames and keeping the 8 heaviest.
fn pose_mix(out: &mut Vec<(usize, f32)>, src: &[(usize, f32)], k: f32) {
    for &(f, w) in src {
        let w = w * k;
        if w == 0.0 {
            continue;
        }
        match out.iter_mut().find(|e| e.0 == f) {
            Some(e) => e.1 += w,
            None => out.push((f, w)),
        }
    }
    if out.len() > 8 {
        out.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(8);
    }
}

/// What the renderer needs to draw a mover (a brush actor): its brush model and where it stands.
#[derive(Debug, Clone)]
pub struct MoverDraw {
    /// The brush `Model` (registry object id; `Registry::obj_path` gives its path).
    pub brush: u32,
    pub location: [f32; 3],
    pub rotation: [i32; 3],
    /// Point of the brush model that sits at `location` (the model's vertices are taken relative to it).
    pub pre_pivot: [f32; 3],
}

impl Vm {
    /// The drawing description of a mover (an actor of class `Mover` with a brush), or `None`.
    pub fn mover_draw(&mut self, a: u32) -> Option<MoverDraw> {
        let o = self.offsets();
        let class = self.class_of(a)?;
        if !self.is_a(class, o.class_mover) || self.bp(a, "bHidden") {
            return None;
        }
        let brush = self.u32_at(a, o.brush);
        if brush == 0 {
            return None;
        }
        let pre_pivot = match self.get_prop_static(a, "PrePivot", 0) {
            Ok(crate::Value::Vector(v)) => v,
            _ => [0.0; 3],
        };
        Some(MoverDraw { brush, location: self.vec_at(a, o.location), rotation: self.rot_of(a), pre_pivot })
    }

    /// The mesh drawing description of an actor, or `None` if it is not drawn as a mesh (no mesh, hidden, other draw type).
    pub fn mesh_draw(&mut self, a: u32) -> Option<MeshDraw> {
        self.mesh_draw_opts(a, false)
    }

    /// Like `mesh_draw`; `overlay` draws the actor's `Mesh` even though it is hidden (the first-person weapon: the engine
    /// draws it through `RenderOverlays`, the actor itself stays `bHidden`).
    /// `DrawActorSprite`'s third-person item of a pawn that was just drawn with a weapon triangle (10b2565b..10b25f80):
    /// its `Weapon`, else its `SelectedItem`. The item takes the pawn's `Style` for the draw; with a `ThirdPersonMesh`
    /// that mesh and `ThirdPersonScale` stand in for `Mesh` and `DrawScale` while its pose is taken (the item's
    /// `Rotation` is left zeroed, as the engine leaves it), `bSteadyToggle` flips when `bSteadyFlash3rd`, `bFirstFrame`
    /// is cleared and `OldFlashCount` follows `FlashCount` (Deus Ex has no `MuzzleFlashMesh`, so no flash is drawn).
    /// Then the pawn's `Shadow` gets `Update(None)`. Returns the item and its pose (placed by the caller on the
    /// pawn's weapon triangle) when there is something to draw.
    pub fn third_person_item(&mut self, pawn: u32) -> Option<(u32, MeshDraw)> {
        let mut out = None;
        let mut item = self.op(pawn, "Weapon");
        if item == 0 {
            item = self.op(pawn, "SelectedItem");
        }
        if item != 0 && self.is_live(item) {
            let saved_style = self.u8p(item, "Style");
            let ps = self.u8p(pawn, "Style");
            self.set_u8p(item, "Style", ps);
            let tpm = self.op(item, "ThirdPersonMesh");
            if tpm != 0 {
                let (mesh, scale) = (self.op(item, "Mesh"), self.fp(item, "DrawScale"));
                let tps = self.fp(item, "ThirdPersonScale");
                let _ = self.set_prop(item, "Mesh", 0, crate::Value::Obj(tpm));
                let _ = self.set_prop(item, "ThirdPersonMesh", 0, crate::Value::Obj(mesh));
                self.set_fp(item, "DrawScale", tps);
                self.set_fp(item, "ThirdPersonScale", scale);
                let _ = self.set_prop(item, "Rotation", 0, crate::Value::Rotator([0; 3]));
                let draw = self.mesh_draw_opts(item, true);
                let _ = self.set_prop(item, "Mesh", 0, crate::Value::Obj(mesh));
                let _ = self.set_prop(item, "ThirdPersonMesh", 0, crate::Value::Obj(tpm));
                self.set_fp(item, "DrawScale", scale);
                self.set_fp(item, "ThirdPersonScale", tps);
                if self.bp(item, "bSteadyFlash3rd") {
                    let t = self.bp(item, "bSteadyToggle");
                    self.set_bp(item, "bSteadyToggle", !t);
                }
                self.set_bp(item, "bFirstFrame", false);
                let fc = self.u8p(item, "FlashCount");
                self.set_u8p(item, "OldFlashCount", fc);
                out = draw.map(|d| (item, d));
            }
            self.set_u8p(item, "Style", saved_style);
        }
        let shadow = self.op(pawn, "Shadow");
        if shadow != 0 && self.is_live(shadow) {
            let _ = self.send_event(shadow, "Update", vec![crate::Value::Obj(0)]);
        }
        out
    }

    /// The actor a viewport looks out of: the live `PlayerPawn` that has a `Player` (0 if none).
    pub fn viewport_actor(&mut self) -> u32 {
        let o = self.offsets();
        let cached = self.player_cache;
        if cached != 0 && self.is_live(cached) && self.is_instance_of(cached, o.class_player_pawn) && self.op(cached, "Player") != 0 {
            return cached;
        }
        self.live_actors().into_iter().find(|&r| self.is_instance_of(r, o.class_player_pawn) && self.op(r, "Player") != 0).unwrap_or(0)
    }

    /// The texture each material slot of actor `a`'s mesh is drawn with, and whether it is the mesh's own (no skin):
    /// `MultiSkins[i]`, slot 0 `Skin`, else the mesh's texture of that slot.
    pub fn mesh_skin_textures(&mut self, a: u32) -> Vec<(u32, bool)> {
        let mesh = self.op(a, "Mesh");
        let own = if mesh != 0 { self.mesh_texture_slots(mesh) } else { Vec::new() };
        let multi = self.prop_at(a, "MultiSkins").map(|(off, _)| off);
        let n = own.len().max(8);
        (0..n)
            .map(|i| {
                let mut r = if i < 8 { multi.map(|off| self.u32_at(a, off + 4 * i)).unwrap_or(0) } else { 0 };
                if i == 0 && r == 0 {
                    r = self.op(a, "Skin");
                }
                if r != 0 {
                    (r, false)
                } else {
                    (own.get(i).copied().unwrap_or(0), true)
                }
            })
            .collect()
    }

    /// `DT_SpriteAnimOnce`'s frame (`DrawActorSprite`): the texture's `AnimNext` chain has `n` frames; with
    /// `t = clamp(1 - LifeSpan / default LifeSpan, 0, 1)` (double) the frame is `round_even(float(n * t) - 0.5)`,
    /// clamped to the chain (the engine cuts the chain there for the draw).
    pub fn anim_once_frame(&mut self, a: u32, tex: u32) -> u32 {
        let mut chain = vec![tex];
        let mut t = self.texture_anim_next(tex);
        while t != 0 && t != tex && chain.len() < 256 {
            chain.push(t);
            t = self.texture_anim_next(t);
        }
        let n = chain.len();
        let life = self.fp(a, "LifeSpan") as f64;
        let default_life = self.class_default_f32(a, "LifeSpan").unwrap_or(0.0) as f64;
        let x = (1.0 - life / default_life).clamp(0.0, 1.0);
        let x = if x.is_nan() { 0.0 } else { x };
        let f = (n as f64 * x) as f32;
        let idx = ((f as f64) - 0.5).round_ties_even() as i64;
        let idx = idx.clamp(0, n as i64 - 1) as usize;
        chain[idx]
    }

    /// `UTexture::Get(Time)` (103212e0) of texture `tex` at the viewport time `time`: `Update` (103ecb70: only when
    /// the time differs from the texture's last update, then `Tick(Time - LastUpdateTime)`), then the current frame
    /// (`AnimCurrent`, or the texture itself). `Tick` (103ed0d0): `ConstantTimeTick` until `PrimeCurrent` reaches
    /// `PrimeCount`; with `MaxFrameRate` 0 one step per update; else the `Accumulator` gathers time and steps once it
    /// passes `1 / MaxFrameRate`, keeping the excess over `1 / MinFrameRate` (rates clamped to 0.01..100).
    /// `ConstantTimeTick` (103ed330): `AnimCurrent = (AnimCurrent or self).AnimNext`, back to the texture at the end.
    /// The state belongs to the texture (shared by everything that draws it) and advances only when it is drawn.
    pub fn texture_get(&mut self, tex: u32, time: f64) -> u32 {
        if tex == 0 {
            return 0;
        }
        let props = self.texture_anim_props(tex);
        let st = self.texture_anim.entry(tex).or_default();
        if time != st.last_update {
            let delta = (time - st.last_update) as f32;
            st.last_update = time;
            let mut steps = 0usize;
            while st.prime_current < props.prime_count {
                st.prime_current += 1;
                steps += 1;
            }
            if props.max_frame_rate == 0.0 {
                steps += 1;
            } else {
                let min_time = 1.0 / props.max_frame_rate.clamp(0.01, 100.0);
                let max_time = 1.0 / props.min_frame_rate.clamp(0.01, 100.0);
                st.accumulator += delta;
                if min_time <= st.accumulator {
                    steps += 1;
                    if st.accumulator < max_time {
                        st.accumulator = 0.0;
                    } else {
                        st.accumulator -= max_time;
                        if max_time < st.accumulator {
                            st.accumulator = max_time;
                        }
                    }
                }
            }
            for _ in 0..steps {
                let cur = self.texture_anim.get(&tex).map(|s| s.current).unwrap_or(0);
                let from = if cur == 0 { tex } else { cur };
                let next = self.texture_anim_next(from);
                self.texture_anim.entry(tex).or_default().current = if next == 0 { tex } else { next };
            }
        }
        let cur = self.texture_anim.get(&tex).map(|s| s.current).unwrap_or(0);
        if cur == 0 { tex } else { cur }
    }

    /// The frame `Get` would return now, without advancing anything.
    pub fn texture_current(&self, tex: u32) -> u32 {
        match self.texture_anim.get(&tex).map(|s| s.current) {
            Some(c) if c != 0 => c,
            _ => tex,
        }
    }

    /// `PrimeCount`, `MinFrameRate`, `MaxFrameRate` of a texture (read once from its properties).
    fn texture_anim_props(&mut self, tex: u32) -> crate::TextureAnimProps {
        if let Some(p) = self.texture_props_cache.get(&tex) {
            return *p;
        }
        let path = self.reg.obj_path(ue_registry::ObjId(tex)).to_string();
        self.reg.ensure_package_for_path(&path);
        let props = (|| {
            let (pi, ei, class) = self.texture_export(&path)?;
            let (inst, _) = self.instantiate_export(class, pi, ei).ok()?;
            let f = |vm: &Vm, n: &str| vm.reg.find_prop(class, n).and_then(|p| inst.data.get(p.offset as usize..p.offset as usize + 4)).map(|b| f32::from_le_bytes(b.try_into().unwrap())).unwrap_or(0.0);
            let prime = self.reg.find_prop(class, "PrimeCount").and_then(|p| inst.data.get(p.offset as usize).copied()).unwrap_or(0);
            let masked = self.reg.find_prop(class, "bMasked").and_then(|p| inst.data.get(p.offset as usize..p.offset as usize + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) & p.bit_mask != 0)).unwrap_or(false);
            Some(crate::TextureAnimProps { prime_count: prime, min_frame_rate: f(self, "MinFrameRate"), max_frame_rate: f(self, "MaxFrameRate"), masked })
        })()
        .unwrap_or_default();
        self.texture_props_cache.insert(tex, props);
        props
    }

    /// A texture's `bMasked` (cached with its animation properties).
    pub(crate) fn texture_masked(&mut self, tex: u32) -> bool {
        tex != 0 && self.texture_anim_props(tex).masked
    }

    /// The export of a texture path (a palette may share its name): package, export, class.
    fn texture_export(&mut self, path: &str) -> Option<(usize, usize, ue_registry::ClassId)> {
        let (pi, _) = self.reg.export_by_path(path)?;
        let tex_class = self.reg.class_by_path("Engine.Texture")?;
        let rest = path.split_once('.')?.1.to_string();
        let n = self.reg.packages[pi].pkg.exports.len();
        (0..n).find_map(|i| {
            let c = self.reg.class_of_export(pi, i)?;
            (self.is_a(c, tex_class) && self.reg.packages[pi].pkg.object_path(ue_package::ObjectRef::Export(i)).as_deref() == Some(rest.as_str())).then_some((pi, i, c))
        })
    }

    /// A texture's `AnimNext` (its next animation frame; 0 for none), read from the texture's properties (cached).
    pub fn texture_anim_next(&mut self, tex: u32) -> u32 {
        if let Some(&v) = self.anim_next_cache.get(&tex) {
            return v;
        }
        let path = self.reg.obj_path(ue_registry::ObjId(tex)).to_string();
        self.reg.ensure_package_for_path(&path);
        let v = (|| {
            let (pi, ei, class) = self.texture_export(&path)?;
            let off = self.reg.find_prop(class, "AnimNext")?.offset as usize;
            let (inst, _) = self.instantiate_export(class, pi, ei).ok()?;
            Some(u32::from_le_bytes(inst.data.get(off..off + 4)?.try_into().ok()?))
        })()
        .unwrap_or(0);
        self.anim_next_cache.insert(tex, v);
        v
    }

    pub fn mesh_draw_opts(&mut self, a: u32, overlay: bool) -> Option<MeshDraw> {
        self.mesh_draw_impl(a, overlay, false)
    }

    /// `mesh_draw` with only what places the mesh (mesh, frames, scale, location, rotation, pre-pivot): no skins,
    /// blend channels, poly flags or environment texture (the visibility pass's bounding box). The main channel's
    /// pose steps as in `mesh_draw` (a repeat call with the same input does not move it).
    pub fn mesh_draw_lite(&mut self, a: u32) -> Option<MeshDraw> {
        self.mesh_draw_impl(a, false, true)
    }

    fn mesh_draw_impl(&mut self, a: u32, overlay: bool, lite: bool) -> Option<MeshDraw> {
        let o = self.offsets();
        let mesh_ref = self.op(a, "Mesh");
        let draw_type = self.u8p(a, "DrawType");
        if self.bp(a, "bHidden") && !overlay {
            return None;
        }
        let style = self.u8p(a, "Style");
        let glow = self.fp(a, "ScaleGlow");
        let unlit = self.bp(a, "bUnlit");
        if self.decals.contains_key(&a) {
            let tex = self.op(a, "Texture");
            if tex == 0 {
                return None;
            }
            let path = self.reg.obj_path(ue_registry::ObjId(tex)).to_string();
            let (center, right, up) = self.decal_axes(a)?;
            return Some(MeshDraw {
                mesh: String::new(),
                frame_a: 0,
                frame_b: 0,
                blend: 0.0,
                location: center,
                rotation: [0; 3],
                scale: 1.0,
                skins: Vec::new(),
                style,
                glow,
                unlit,
                sprite: Some(path),
                decal: Some((right, up)),
                decal_polys: self.decal_pieces(a),
                blends: Vec::new(),
                ..MeshDraw::plain()
            });
        }
        if (draw_type == 1 || draw_type == 7) && !overlay {
            // `DrawActorSprite` (10b25080): a sprite or once-animated sprite with a `Texture` is a screen-facing tile of
            // the texture's size times `DrawScale` (editor icons too: the engine draws any visible sprite)
            let mut tex = self.op(a, "Texture");
            if tex == 0 {
                return None;
            }
            if draw_type == 7 {
                tex = self.anim_once_frame(a, tex);
            }
            // the viewing player's `Sprite` stands in for every sprite's texture (Deus Ex's `Matrix` easter egg)
            let viewer = self.viewport_actor();
            if viewer != 0 {
                let over = self.op(viewer, "Sprite");
                if over != 0 {
                    tex = over;
                }
            }
            // `Texture->Get(CurrentTime)`: the animation frame it shows now (stepped once per engine frame)
            if draw_type == 1 {
                tex = self.texture_current(tex);
            }
            let path = self.reg.obj_path(ue_registry::ObjId(tex)).to_string();
            let scale = self.fp(a, "DrawScale");
            return Some(MeshDraw {
                mesh: String::new(),
                frame_a: 0,
                frame_b: 0,
                blend: 0.0,
                location: self.vec_at(a, o.location),
                rotation: [0; 3],
                scale,
                skins: Vec::new(),
                style,
                glow,
                unlit,
                sprite: Some(path),
                decal: None,
                decal_polys: Vec::new(),
                blends: Vec::new(),
                ..MeshDraw::plain()
            });
        }
        if mesh_ref == 0 || (draw_type != 2 && !overlay) {
            return None;
        }
        let path = self.reg.obj_path(ue_registry::ObjId(mesh_ref)).to_string();
        // animation pose: the sequence's frame range, blended between neighbouring frames; tweens from the last pose
        let name = self.u32_at_prop(a, "AnimSequence");
        let seq = if name != 0 { self.seq_of(a, name).map(|q| (q.start_frame.max(0) as usize, q.num_frames.max(1) as usize)) } else { None };
        let frame = self.fp(a, "AnimFrame");
        let key = (a, MAIN_CHANNEL);
        let fresh = !self.blend_cache.contains_key(&key);
        let pose = pose_step(self.blend_cache.entry(key).or_default(), fresh, name, seq, frame);
        let (frame_a, frame_b, blend, mut main_extra) = split_pose(&pose);
        let scale = {
            let s = self.fp(a, "DrawScale");
            if s == 0.0 { 1.0 } else { s }
        };
        if lite {
            return Some(MeshDraw {
                mesh: path,
                frame_a,
                frame_b,
                blend,
                location: self.vec_at(a, o.location),
                rotation: self.rot_of(a),
                pre_pivot: self.vp(a, "PrePivot"),
                scale,
                style,
                glow,
                unlit,
                ..MeshDraw::plain()
            });
        }
        // per material slot: the actor's skin (`MultiSkins`, slot 0 `Skin`) or, for an animated texture of the mesh's
        // own, its current frame (`Texture->Get(CurrentTime)` in `DrawLodMesh`; stepped once per engine frame)
        let slots = self.mesh_skin_textures(a);
        let mut skins: Vec<Option<String>> = Vec::new();
        for (tex, own) in slots.into_iter() {
            let shown = self.texture_current(tex);
            let path = (tex != 0 && (!own || shown != tex)).then(|| self.reg.obj_path(ue_registry::ObjId(shown)).to_string());
            skins.push(path);
        }
        while skins.len() > 8 && skins.last().map(|p| p.is_none()).unwrap_or(false) {
            skins.pop();
        }
        main_extra.extend(self.blend_poses(a));
        let blends = main_extra;
        let flags = self.draw_poly_flags(a);
        let (fatness, ambient_glow, lod_bias) = (self.u8p(a, "Fatness"), self.u8p(a, "AmbientGlow"), self.fp(a, "LODBias"));
        // `DrawLodMesh`: the environment map is the actor's `Texture`, else its zone's, else the level's
        let mut env = self.op(a, "Texture");
        if env == 0 {
            let zone = self.zone_of_actor(a);
            if zone != 0 {
                env = self.op(zone, "EnvironmentMap");
            }
        }
        if env == 0 {
            let level = self.op(a, "Level");
            if level != 0 {
                env = self.op(level, "EnvironmentMap");
            }
        }
        let env_texture = (env != 0).then(|| {
            let shown = self.texture_current(env);
            self.reg.obj_path(ue_registry::ObjId(shown)).to_string()
        });
        let viewer = self.viewport_actor();
        let over = if viewer != 0 { self.op(viewer, "Sprite") } else { 0 };
        let all_textures = (over != 0).then(|| {
            let shown = self.texture_current(over);
            self.reg.obj_path(ue_registry::ObjId(shown)).to_string()
        });
        Some(MeshDraw {
            mesh: path,
            frame_a,
            frame_b,
            blend,
            location: self.vec_at(a, o.location),
            rotation: self.rot_of(a),
            pre_pivot: self.vp(a, "PrePivot"),
            scale,
            skins,
            style,
            glow,
            unlit,
            sprite: None,
            decal: None,
            decal_polys: Vec::new(),
            blends,
            fatness,
            ambient_glow,
            lod_bias,
            flags,
            env_texture,
            all_textures,
        })
    }

    /// The poly flags `DrawActorSprite` hands a mesh draw (Render 10b26230): `Style` masked 0x2, translucent 0x4,
    /// modulated 0x40; `bNoSmooth` 0x800; `bSelected` 0x2000000; `bMeshEnviroMap` 0x10; `PF_Flat` 0x4000 unless
    /// `bMeshCurvy`; `PF_Unlit` 0x400000 when `bUnlit`, in zone 0, or the viewing actor's `RendMap` is not 5 (the
    /// device flag the engine also checks is never set in game).
    pub fn draw_poly_flags(&mut self, a: u32) -> u32 {
        let mut f = match self.u8p(a, "Style") {
            2 => 0x2,
            3 => 0x4,
            4 => 0x40,
            _ => 0,
        };
        if self.bp(a, "bNoSmooth") {
            f |= 0x800;
        }
        if self.bp(a, "bSelected") {
            f |= 0x200_0000;
        }
        if self.bp(a, "bMeshEnviroMap") {
            f |= 0x10;
        }
        if !self.bp(a, "bMeshCurvy") {
            f |= 0x4000;
        }
        let viewer = self.viewport_actor();
        let rend_map = if viewer != 0 { self.ip(viewer, "RendMap") } else { 5 };
        if self.bp(a, "bUnlit") || self.zone_number(a) == 0 || rend_map != 5 {
            f |= 0x40_0000;
        }
        f
    }

    /// The poses of the four blend channels (`ULodMesh::GetFrame`), each kept like the main pose (`pose_step`), as
    /// differences to frame 0.
    fn blend_poses(&mut self, a: u32) -> Vec<(usize, f32)> {
        let mut out = Vec::new();
        let (Some((seq_off, _)), Some((f_off, _))) = (self.prop_at(a, "BlendAnimSequence"), self.prop_at(a, "BlendAnimFrame")) else { return out };
        for i in 0..4usize {
            let name = self.u32_at(a, seq_off + 4 * i);
            if name == 0 {
                continue;
            }
            let seq = self.seq_of(a, name).map(|q| (q.start_frame.max(0) as usize, q.num_frames.max(1) as usize));
            let frame = self.f32_at(a, f_off + 4 * i);
            let key = (a, i as u8);
            let fresh = !self.blend_cache.contains_key(&key);
            let pose = pose_step(self.blend_cache.entry(key).or_default(), fresh, name, seq, frame);
            pose_mix(&mut out, &pose, 1.0);
        }
        out
    }
}

#[cfg(test)]
mod pose_tests {
    use super::*;

    fn weight(p: &[(usize, f32)], f: usize) -> f32 {
        p.iter().filter(|e| e.0 == f).map(|e| e.1).sum()
    }

    /// A tween moves from the pose drawn before it to the first frame of the new sequence, in proportion to how far
    /// the frame has come from -1/n to 0 (the increments multiply out to a straight line).
    #[test]
    fn tween_starts_from_the_last_pose_and_is_linear() {
        let mut c = PoseCache::default();
        // playing sequence A (frames 10..14) at frame 0.5: frames 12/13 half each
        let p = pose_step(&mut c, true, 1, Some((10, 4)), 0.5 - 0.125);
        assert_eq!(p, vec![(11, 0.5), (12, 0.5)]);
        // tween into B (frames 20..24, n 5): first step does not move
        let p = pose_step(&mut c, false, 2, Some((20, 5)), -0.2);
        assert_eq!(weight(&p, 20), 0.0);
        for frame in [-0.15f32, -0.1, -0.05] {
            let p = pose_step(&mut c, false, 2, Some((20, 5)), frame);
            let left = frame / -0.2;
            assert!((weight(&p, 20) - (1.0 - left)).abs() < 1e-5, "{frame}: {p:?}");
            assert!((weight(&p, 11) - 0.5 * left).abs() < 1e-5);
        }
        // past 97 % of the way the pose stops until the sequence plays
        let p = pose_step(&mut c, false, 2, Some((20, 5)), -0.004);
        assert!((weight(&p, 20) - 0.75).abs() < 1e-5, "{p:?}");
        let p = pose_step(&mut c, false, 2, Some((20, 5)), 0.0);
        assert_eq!(p, vec![(20, 1.0)]);
    }

    /// Only another sequence restarts the progress: a second tween into the same sequence shows the old pose until it
    /// ends.
    #[test]
    fn second_tween_into_the_same_sequence_does_not_move() {
        let mut c = PoseCache::default();
        pose_step(&mut c, true, 1, Some((0, 2)), 0.0);
        for f in [-0.5f32, -0.25, -0.01, 0.0] {
            pose_step(&mut c, false, 1, Some((0, 2)), f);
        }
        assert!(c.progress > 0.97);
        pose_step(&mut c, false, 1, Some((0, 2)), 0.5); // playing: frame 1
        for f in [-0.5f32, -0.3, -0.1] {
            let p = pose_step(&mut c, false, 1, Some((0, 2)), f);
            assert_eq!((weight(&p, 0), weight(&p, 1)), (0.0, 1.0), "{f}: {p:?}");
        }
    }

    #[test]
    fn playing_pose_wraps_and_rounds_like_the_engine() {
        // n 4, frame 0.9: x = 3.6, k = round(3.1) = 3, frac 0.6, next wraps to the first frame
        let p = playing_pose(0.9, 10, 4);
        assert_eq!((p[0].0, p[1].0), (13, 10));
        assert!((p[1].1 - 0.6).abs() < 1e-5);
        // x = 3.0: round-half-even(2.5) = 2, so frames 12/13 with all weight on 13
        assert_eq!(playing_pose(0.75, 10, 4), vec![(12, 0.0), (13, 1.0)]);
        assert_eq!(playing_pose(0.0, 10, 4), vec![(10, 1.0)]);
    }

    #[test]
    fn split_pose_keeps_the_sum() {
        let pose = vec![(3usize, 0.5f32), (7, 0.3), (9, 0.2)];
        let (a, b, t, extra) = split_pose(&pose);
        assert_eq!((a, b, t), (3, 3, 0.0));
        // frame 3 + 0.3 (F7 - F0) + 0.2 (F9 - F0) - 0.5 (F3 - F0) = 0.5 F3 + 0.3 F7 + 0.2 F9
        assert_eq!(extra, vec![(7, 0.3), (9, 0.2), (3, -0.5)]);
    }
}

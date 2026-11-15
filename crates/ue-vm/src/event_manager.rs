//! The AI event manager (`UEventManager`, Engine.dll): how pawns notice noises, weapons, distress, carcasses, alarms and
//! the like. Actors *send* events (`AISendEvent` once, `AIStartEvent` until `AIEndEvent`) and actors *listen*
//! (`AISetEventCallback`). Once per tick, after the actors have ticked, every listener looks at the senders of the event
//! it listens to: the nearest one it can see, hear or smell wins and the listener's script callback hears
//! `Begin`, `Pulse`, `End` or `ChangeBest`.
//!
//! The values a sender emitted live in a ring of 16 time slots (one per tick) plus the "current" values that
//! `AIStartEvent` keeps up until `AIEndEvent`; a sender whose slots are all empty is dropped. Deleting is deferred
//! (`deleted` flags) while the manager is processing or running callbacks, as in the original.

use crate::collision::Vec3;
use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

/// `EAIEventType`.
pub const AI_VISUAL: u8 = 0;
pub const AI_AUDIO: u8 = 1;
pub const AI_OLFACTORY: u8 = 2;

/// `EAIEventState`.
const STATE_BEGIN: u8 = 0;
const STATE_END: u8 = 1;
const STATE_PULSE: u8 = 2;
const STATE_CHANGE_BEST: u8 = 3;

/// One `AISetEventCallback` registration (the configuration part).
#[derive(Debug, Clone, PartialEq)]
pub struct AiCallback {
    /// Name of the script function to call when the event is perceived.
    pub callback: u32,
    /// Optional scoring function name (0 when omitted).
    pub score_callback: u32,
    pub check_visibility: bool,
    pub check_dir: bool,
    pub check_cylinder: bool,
    pub check_los: bool,
}

/// What a sender emitted in one tick (or currently emits): visual strength, audio volume, audio radius, smell.
type Values = [f32; 4];

#[derive(Debug, Clone)]
pub struct Sender {
    pub actor: u32,
    pub event: u32,
    deleted: bool,
    /// The score this sender got from the listener being processed.
    score: f32,
    /// Values `AIStartEvent` keeps up until `AIEndEvent`.
    cur: Values,
    ring: [Values; 16],
}

#[derive(Debug, Clone)]
pub struct Receiver {
    pub actor: u32,
    pub event: u32,
    pub cfg: AiCallback,
    deleted: bool,
    pending: bool,
    state: u8,
    /// `XAIParams` for the callback: best actor, score, visibility, volume, smell.
    params: (u32, f32, f32, f32, f32),
    detected: bool,
    last_best: u32,
}

#[derive(Debug, Default)]
pub struct EventManager {
    /// The tick's slot in the senders' rings.
    slot: usize,
    pub senders: Vec<Sender>,
    pub receivers: Vec<Receiver>,
    processing: u32,
    delete_count: u32,
}

impl EventManager {
    fn find_sender(&self, event: u32, actor: u32) -> Option<usize> {
        self.senders.iter().position(|s| !s.deleted && s.event == event && s.actor == actor)
    }

    fn find_receiver(&self, event: u32, actor: u32) -> Option<usize> {
        self.receivers.iter().position(|r| !r.deleted && r.event == event && r.actor == actor)
    }

    fn safe_delete_sender(&mut self, i: usize) {
        if !self.senders[i].deleted {
            self.delete_count += 1;
        }
        self.senders[i].deleted = true;
    }

    fn safe_delete_receiver(&mut self, i: usize) {
        if !self.receivers[i].deleted {
            self.delete_count += 1;
        }
        self.receivers[i].deleted = true;
    }

    /// `AISetEventCallback`: registers `actor` for `event`, or changes its existing registration.
    pub fn set_callback(&mut self, actor: u32, event: u32, cfg: AiCallback) {
        if actor == 0 || event == 0 {
            return;
        }
        match self.find_receiver(event, actor) {
            Some(i) => self.receivers[i].cfg = cfg,
            None if cfg.callback != 0 => self.receivers.push(Receiver {
                actor,
                event,
                cfg,
                deleted: false,
                pending: false,
                state: 0,
                params: (0, 0.0, 0.0, 0.0, 0.0),
                detected: false,
                last_best: 0,
            }),
            None => {}
        }
    }

    pub fn clear_callback(&mut self, actor: u32, event: u32) {
        if actor == 0 || event == 0 {
            return;
        }
        if let Some(i) = self.find_receiver(event, actor) {
            self.safe_delete_receiver(i);
        }
    }

    /// `UEventManager::DestroyActor`.
    pub fn destroy_actor(&mut self, actor: u32) {
        for i in 0..self.senders.len() {
            if !self.senders[i].deleted && self.senders[i].actor == actor {
                self.safe_delete_sender(i);
            }
        }
        for i in 0..self.receivers.len() {
            if self.receivers[i].deleted {
                continue;
            }
            if self.receivers[i].last_best == actor {
                self.receivers[i].last_best = 0;
            }
            if self.receivers[i].actor == actor {
                self.safe_delete_receiver(i);
            }
        }
    }

    pub fn reset(&mut self) {
        *self = EventManager::default();
    }

    /// The live registrations as (actor, event name, configuration), for saving.
    pub fn callbacks(&self) -> impl Iterator<Item = (u32, u32, &AiCallback)> {
        self.receivers.iter().filter(|r| !r.deleted).map(|r| (r.actor, r.event, &r.cfg))
    }

    fn sender_for(&mut self, actor: u32, event: u32) -> usize {
        if let Some(i) = self.find_sender(event, actor) {
            return i;
        }
        self.senders.push(Sender { actor, event, deleted: false, score: 0.0, cur: [0.0; 4], ring: [[0.0; 4]; 16] });
        self.senders.len() - 1
    }

    /// `FUN_10382e60`: records a value for `kind`; `one_shot` values (`AISendEvent`) are not kept as the current ones.
    fn update(&mut self, i: usize, kind: u8, one_shot: bool, value: f32, radius: f32) {
        let slot = self.slot;
        let s = &mut self.senders[i];
        let mut put = |idx: usize, v: f32| {
            if s.ring[slot][idx] < v {
                s.ring[slot][idx] = v;
            }
            if !one_shot {
                s.cur[idx] = v;
            }
        };
        match kind {
            AI_AUDIO => {
                put(1, value);
                put(2, radius);
            }
            AI_VISUAL => put(0, value),
            AI_OLFACTORY => put(3, value),
            _ => {}
        }
    }

    /// The largest value of each kind over the slots from `from` to the current one (the receiver's window).
    fn window(&self, s: &Sender, from: usize) -> Values {
        let mut out = [0.0f32; 4];
        let mut idx = from;
        loop {
            for k in 0..4 {
                if out[k] < s.ring[idx][k] {
                    out[k] = s.ring[idx][k];
                }
            }
            if idx == self.slot {
                break;
            }
            idx = (idx + 1) & 15;
        }
        out
    }
}

impl Vm {
    fn pending_kill(&mut self, a: u32) -> bool {
        let o = self.offsets();
        !self.is_live(a) || self.flag(a, o.b_delete_me.0, o.b_delete_me.1)
    }

    /// `AIStartEvent` (`one_shot` false) and `AISendEvent` (true).
    pub fn ai_emit_event(&mut self, actor: u32, event: u32, kind: u8, one_shot: bool, value: f32, radius: f32) {
        if actor == 0 || event == 0 {
            return;
        }
        let (value, radius) = if self.pending_kill(actor) { (0.0, 0.0) } else { (value, radius) };
        let em = &mut self.ai.events;
        let i = em.sender_for(actor, event);
        em.update(i, kind, one_shot, value, radius);
    }

    /// `AIEndEvent` (one kind) and `AIClearEvent` (all kinds): the sender stops emitting the current values.
    pub fn ai_end_event(&mut self, actor: u32, event: u32, kinds: &[u8]) {
        if actor == 0 || event == 0 {
            return;
        }
        let em = &mut self.ai.events;
        if let Some(i) = em.find_sender(event, actor) {
            for &k in kinds {
                em.update(i, k, false, 0.0, 0.0);
            }
        }
    }

    /// `APawn::AICanHear`: 0..1 how well `pawn` hears a sound of `volume` that carries `radius` units (800 by default)
    /// from `other`; vertical distance counts double.
    pub fn ai_can_hear(&mut self, pawn: u32, other: u32, volume: f32, radius: f32) -> f32 {
        let o = self.offsets();
        if other == 0 || volume <= 0.0 || !self.bp(other, "bDetectable") {
            return 0.0;
        }
        let radius = if radius <= 0.0 { 800.0 } else { radius };
        let (a, b) = (self.vec_at(other, o.location), self.vec_at(pawn, o.location));
        let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], (a[2] - b[2]) * 2.0);
        let d2 = dz * dz + dy * dy + dx * dx;
        if d2 < radius * radius {
            let v = (1.0 - d2.sqrt() / radius) * volume - self.fp(pawn, "HearingThreshold");
            if v >= 0.0 {
                return v.min(1.0);
            }
        }
        0.0
    }

    /// `UEventManager::AIComputeScore`: squared distance plus one, or whatever the listener's score callback makes of it.
    fn ai_compute_score(&mut self, receiver: u32, sender: u32, score_callback: u32) -> Res<f32> {
        let o = self.offsets();
        let (a, b) = (self.vec_at(receiver, o.location), self.vec_at(sender, o.location));
        let base = (a[2] - b[2]).powi(2) + (a[1] - b[1]).powi(2) + (a[0] - b[0]).powi(2) + 1.0;
        if score_callback == 0 {
            return Ok(base);
        }
        let name = self.reg.names.str(score_callback).to_string();
        let r = self.call_by_name(receiver, &name, vec![Value::Obj(receiver), Value::Obj(sender), Value::Float(base)])?;
        Ok(r.as_f32().unwrap_or(0.0))
    }
}

impl Vm {
    /// `UEventManager::AIProcess` followed by `CleanupEvents`: one tick of the event manager.
    pub fn ai_event_tick(&mut self) -> Res<()> {
        if self.level == 0 {
            return Ok(());
        }
        let o = self.offsets();
        self.ai.events.processing += 1;
        let cur = self.ai.events.slot;
        let next = (cur + 1) & 15;
        let now = self.get_prop(self.level, "TimeSeconds", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0);
        let n_recv = self.ai.events.receivers.len();
        // a script error in one listener's score or callback is reported after the tick; the others go on (the engine
        // logs script errors and carries on)
        let mut first_err: Option<crate::VmError> = None;
        for ri in 0..n_recv {
            if self.ai.events.receivers[ri].deleted {
                continue;
            }
            let (r_actor, r_event, cfg) = {
                let r = &self.ai.events.receivers[ri];
                (r.actor, r.event, r.cfg.clone())
            };
            if self.pending_kill(r_actor) {
                self.ai.events.safe_delete_receiver(ri);
                continue;
            }
            {
                let r = &mut self.ai.events.receivers[ri];
                r.pending = false;
                r.params = (0, 0.0, 0.0, 0.0, 0.0);
            }
            // actors that were seen lately or are near the player listen at any range; the others only within 400 units
            let near = !self.ai_in_stasis(r_actor) && (now - self.fp(r_actor, "LastRenderTime") <= 5.0 || self.fp(r_actor, "DistanceFromPlayer") <= 1200.0);
            let r_loc = self.vec_at(r_actor, o.location);
            let mut candidates: Vec<usize> = Vec::new();
            let sender_ids: Vec<usize> = (0..self.ai.events.senders.len()).filter(|&i| !self.ai.events.senders[i].deleted && self.ai.events.senders[i].event == r_event).collect();
            for si in sender_ids {
                let s_actor = self.ai.events.senders[si].actor;
                if s_actor == r_actor {
                    continue;
                }
                let d2 = if near {
                    0.0
                } else {
                    let l = self.vec_at(s_actor, o.location);
                    (r_loc[2] - l[2]).powi(2) + (r_loc[1] - l[1]).powi(2) + (r_loc[0] - l[0]).powi(2)
                };
                if d2 <= 160_000.0 {
                    let score = match self.ai_compute_score(r_actor, s_actor, cfg.score_callback) {
                        Ok(v) => v,
                        Err(e) => {
                            first_err.get_or_insert(e);
                            0.0
                        }
                    };
                    self.ai.events.senders[si].score = score;
                    if score > 0.0 {
                        candidates.push(si);
                    }
                    if candidates.len() > 255 {
                        break;
                    }
                }
            }
            // the closest (lowest score) sender first: `appQsort` with comparator 10382420, which breaks ties by the
            // two items' current places in the array being sorted
            let em = &self.ai.events;
            crate::crt::qsort(&mut candidates, |v, a, b| match crate::crt::x87_cmp(em.senders[v[a]].score, em.senders[v[b]].score) {
                0 => (a > b) as i32 - (a < b) as i32,
                c => c,
            });
            let mut best: Option<usize> = None;
            for &si in &candidates {
                let (s_actor, w) = {
                    let s = &self.ai.events.senders[si];
                    (s.actor, self.ai.events.window(s, cur))
                };
                let (vis, vol, smell) = match self.ai_sense_detection_for(r_actor, &cfg, s_actor, w) {
                    Ok(v) => v,
                    Err(e) => {
                        first_err.get_or_insert(e);
                        (0.0, 0.0, 0.0)
                    }
                };
                if vis > 0.0 || vol > 0.0 || smell > 0.0 {
                    let score = self.ai.events.senders[si].score;
                    self.ai.events.receivers[ri].params = (s_actor, score, vis, vol, smell);
                    best = Some(si);
                    break;
                }
            }
            let (best_actor, best_score, persistent) = match best {
                Some(si) => {
                    let s = &self.ai.events.senders[si];
                    (s.actor, s.score, s.cur[0] > 0.0 || s.cur[1] > 0.0 || s.cur[3] > 0.0)
                }
                None => (0, 0.0, false),
            };
            let detected = best.is_some();
            let r = &mut self.ai.events.receivers[ri];
            if !r.detected {
                if detected {
                    r.pending = true;
                    r.state = if persistent { STATE_BEGIN } else { STATE_PULSE };
                }
            } else if !detected {
                r.pending = true;
                r.state = STATE_END;
            } else if r.last_best != best_actor {
                r.pending = true;
                r.state = STATE_CHANGE_BEST;
            }
            r.detected = detected;
            if r.pending && r.state == STATE_PULSE {
                r.detected = false;
            }
            let _ = best_score;
            r.last_best = best_actor;
        }
        self.ai.events.slot = next;
        // senders: the new slot starts from the current values; senders with nothing left in any slot are dropped
        for si in 0..self.ai.events.senders.len() {
            if self.ai.events.senders[si].deleted {
                continue;
            }
            let actor = self.ai.events.senders[si].actor;
            if self.pending_kill(actor) {
                self.ai.events.senders[si].cur = [0.0; 4];
            }
            let s = &mut self.ai.events.senders[si];
            s.ring[next] = s.cur;
            let empty = s.ring.iter().all(|v| !(v[0] > 0.0 || v[1] > 0.0 || v[3] > 0.0));
            if empty {
                self.ai.events.safe_delete_sender(si);
            }
        }
        // callbacks
        for ri in 0..n_recv.min(self.ai.events.receivers.len()) {
            let (deleted, pending) = {
                let r = &self.ai.events.receivers[ri];
                (r.deleted, r.pending)
            };
            if deleted || !pending {
                continue;
            }
            let (actor, event, state, params, callback) = {
                let r = &self.ai.events.receivers[ri];
                (r.actor, r.event, r.state, r.params, r.cfg.callback)
            };
            self.ai.events.receivers[ri].pending = false;
            if let Err(e) = self.ai_event_callback(actor, event, state, params, callback) {
                first_err.get_or_insert(e);
            }
        }
        self.ai.events.processing -= 1;
        // `CleanupEvents`
        if self.ai.events.processing == 0 && self.ai.events.delete_count > 0 {
            self.ai.events.senders.retain(|s| !s.deleted);
            self.ai.events.receivers.retain(|r| !r.deleted);
            self.ai.events.delete_count = 0;
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// `UEventManager::AIEvent`: calls the listener's callback `(event name, state, XAIParams)`.
    fn ai_event_callback(&mut self, actor: u32, event: u32, state: u8, p: (u32, f32, f32, f32, f32), callback: u32) -> Res<()> {
        if actor == 0 || event == 0 || !self.is_live(actor) {
            return Ok(());
        }
        let name = if callback == 0 { "AIEvent".to_string() } else { self.reg.names.str(callback).to_string() };
        let Some(sid) = self.reg.struct_id_by_path("Engine.Actor.XAIParams") else { return Ok(()) };
        let mut bytes = Vec::with_capacity(20);
        bytes.extend_from_slice(&p.0.to_le_bytes());
        for f in [p.1, p.2, p.3, p.4] {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        self.send_event(actor, &name, vec![Value::Name(event), Value::Byte(state), Value::Struct(sid, bytes)])?;
        Ok(())
    }

    /// `AActor::InStasis` (see `Vm::in_stasis`).
    pub fn ai_in_stasis(&mut self, a: u32) -> bool {
        self.in_stasis(a)
    }

    /// `ComputeSenseDetection` for the listener `receiver` configured as `cfg` and a sender's window `w`.
    fn ai_sense_detection_for(&mut self, receiver: u32, cfg: &AiCallback, sender_actor: u32, w: Values) -> Res<(f32, f32, f32)> {
        let o = self.offsets();
        let pawn = self.is_instance_of(receiver, o.class_pawn);
        let mut scale = 1.0f32;
        let mut target = sender_actor;
        if self.is_instance_of(sender_actor, o.class_inventory) {
            let owner = self.u32_at(sender_actor, o.owner);
            if owner != 0 {
                let (ri, hi) = self.cylinder(sender_actor);
                let (ro, ho) = self.cylinder(owner);
                let mut area = hi * ri;
                let area_o = ho * ro;
                if area_o != 0.0 {
                    area /= area_o;
                }
                scale = area.sqrt();
                target = owner;
            }
        }
        let (r_loc, s_loc): (Vec3, Vec3) = (self.vec_at(receiver, o.location), self.vec_at(sender_actor, o.location));
        let mut vis = w[0] * scale;
        if vis > 0.0 {
            if !pawn {
                let hit = self.single_line_check(receiver, s_loc, r_loc, 6, [0.0; 3], 0);
                if hit.time != 1.0 {
                    vis = 0.0;
                }
            } else {
                vis = self.ai_can_see(receiver, target, vis, cfg.check_visibility, cfg.check_dir, cfg.check_cylinder, cfg.check_los)?;
            }
        }
        let mut vol = w[1];
        if vol > 0.0 {
            if !pawn {
                let d2 = (s_loc[2] - r_loc[2]).powi(2) + (s_loc[1] - r_loc[1]).powi(2) + (s_loc[0] - r_loc[0]).powi(2);
                if w[2] * w[2] <= d2 {
                    vol = 0.0;
                }
            } else {
                vol = self.ai_can_hear(receiver, target, vol, w[2]);
            }
        }
        // `APawn::AICanSmell` always answers 0
        Ok((vis, vol, 0.0))
    }
}

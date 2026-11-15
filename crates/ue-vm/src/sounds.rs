//! The engine side of game sounds (docs/galaxy/engine-sounds.md): `AActor::execPlaySound` (103e1f60) and
//! `execPlayOwnedSound` (103e29d0) in a standalone game, `CheckHearSound` (103e1b00), the native body of
//! `ClientHearSound` (103dfc40) and `execStopSound` (103e27d0). What reaches the audio subsystem is recorded as a
//! `SoundEvent` with the values the original passes to `UAudioSubsystem::PlaySound`/`StopSoundId`.
use crate::ai::{SoundEvent, SoundRequest};
use crate::{ref_instance, Vm};

/// The parameters of a `PlaySound` call after the defaults.
pub struct PlayArgs {
    pub sound: u32,
    pub slot: u8,
    pub volume: f32,
    pub no_override: bool,
    pub radius: f32,
    pub pitch: f32,
}

impl Vm {
    /// `Actor.Index`: the object's slot in the object table (the original's `GObjObjects` index; the port's slots
    /// are not reused the way the original's are, engine-sounds.md 1.5).
    pub fn object_index(&self, a: u32) -> i32 {
        ref_instance(a).unwrap_or(0) as i32
    }

    /// `execPlaySound` (`owned`: `execPlayOwnedSound`). Returns the sound id (`PlaySound`'s result).
    pub fn play_sound_native(&mut self, this: u32, a: PlayArgs, owned: bool) -> i32 {
        let mut radius = a.radius;
        let r0 = if owned {
            // no 800 rule; a 0 or NaN radius counts as 1600 for the range
            if radius == 0.0 || radius.is_nan() {
                1600.0
            } else {
                radius
            }
        } else {
            if !(radius > 0.0) {
                radius = 800.0;
            }
            radius
        };
        let id = (a.no_override as i32).wrapping_add(self.object_index(this).wrapping_mul(16)).wrapping_add((a.slot as i32) * 2);
        let radius_sq = r0 * r0;
        let params = [100.0f32 * a.volume, radius, 100.0f32 * a.pitch];
        // standalone: the pawn-list walk over bIsPlayer pawns
        let mut h = self.op(self.level, "PawnList");
        let mut guard = 0;
        while h != 0 && guard < 4096 {
            guard += 1;
            let next = self.op(h, "nextPawn");
            if self.is_live(h) && self.bp(h, "bIsPlayer") {
                self.check_hear_sound(this, h, id, a.sound, params, radius_sq);
            }
            h = next;
        }
        id
    }

    /// `CheckHearSound` (103e1b00) on hearer `h`, `this` the source.
    fn check_hear_sound(&mut self, this: u32, h: u32, id: i32, sound: u32, mut params: [f32; 3], radius_sq: f32) {
        let pp = self.reg.class_by_name("PlayerPawn");
        let is_pp = pp.is_some_and(|c| self.is_instance_of(h, c));
        let vt = if is_pp { self.op(h, "ViewTarget") } else { 0 };
        let e = if vt != 0 && self.is_live(vt) { self.location(vt) } else { self.location(h) };
        let s = self.location(this);
        let max_sq = radius_sq * 0.7692308f32;
        let (dx, dy, dz) = (e[0] as f64 - s[0] as f64, e[1] as f64 - s[1] as f64, e[2] as f64 - s[2] as f64);
        let sum = (dz * dz + dy * dy) + dx * dx;
        if sum >= max_sq as f64 {
            return;
        }
        let dist_sq = sum as f32;
        let clear = self.bsp.as_ref().map(|b| b.fast_line_check(e, s)).unwrap_or(true);
        if !clear {
            let instigator = self.op(this, "Instigator");
            let limit = if instigator != h { max_sq as f64 * 0.6f32 as f64 } else { max_sq as f64 };
            params[0] *= 0.35f32;
            if dist_sq as f64 > limit {
                return;
            }
        }
        self.client_hear_sound(h, this, id, sound, s, params);
    }

    /// The native body of `ClientHearSound` (103dfc40): a local player's audio subsystem gets the sound.
    fn client_hear_sound(&mut self, h: u32, actor: u32, id: i32, sound: u32, location: [f32; 3], params: [f32; 3]) {
        let volume = (params[0] as f64 * 0.01) as f32;
        let pitch = (params[2] as f64 * 0.01) as f32;
        let pp = self.reg.class_by_name("PlayerPawn");
        if !pp.is_some_and(|c| self.is_instance_of(h, c)) || self.op(h, "Player") == 0 {
            return;
        }
        let actor = if actor != 0 && (!self.is_live(actor) || self.bp(actor, "bDeleteMe")) { 0 } else { actor };
        let radius = if params[1] == 0.0 || params[1].is_nan() { 1600.0 } else { params[1] };
        self.audio_play_sound(actor, id, sound, location, volume, radius, pitch);
    }

    /// `UAudioSubsystem::PlaySound(Actor, Id, Sound, Location, Volume, Radius, Pitch)`: recorded for the audio
    /// subsystem (it runs at the next frame in the port).
    #[allow(clippy::too_many_arguments)]
    pub fn audio_play_sound(&mut self, actor: u32, id: i32, sound: u32, location: [f32; 3], volume: f32, radius: f32, pitch: f32) {
        let req = SoundRequest { id, source: actor, sound, slot: ((id >> 1) & 7) as u8, volume, no_override: id & 1 != 0, radius, pitch, location: Some(location) };
        if self.ai.sound_events.len() < 4096 {
            self.ai.sound_events.push(SoundEvent::Play(req.clone()));
        }
        if self.ai.sounds.len() >= 1024 {
            self.ai.sounds.remove(0);
        }
        self.ai.sounds.push(req);
    }

    /// `execStopSound` (103e27d0): `StopSoundId(Id)` whatever the actor.
    pub fn stop_sound_native(&mut self, id: i32) {
        if self.ai.sound_events.len() < 4096 {
            self.ai.sound_events.push(SoundEvent::Stop(id));
        }
    }
}

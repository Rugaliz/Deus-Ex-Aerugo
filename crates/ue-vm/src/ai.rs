//! Bookkeeping for the AI event natives (`AISetEventCallback`, `AIStartEvent`, ...), sound and console requests
//! and other engine calls that only record what the scripts asked for.
//!
//! The event manager itself is in `event_manager.rs`.

use std::collections::HashSet;

pub use crate::event_manager::AiCallback;

/// One `Actor.PlaySound` call (the audio mixer will consume these).
#[derive(Debug, Clone, PartialEq)]
pub struct SoundRequest {
    pub id: i32,
    pub source: u32,
    pub sound: u32,
    pub slot: u8,
    pub volume: f32,
    pub no_override: bool,
    pub radius: f32,
    pub pitch: f32,
    /// Where the sound plays when it is not at its source actor (interface sounds: next to the player).
    pub location: Option<[f32; 3]>,
}

/// What the audio system must do, in order (taken with `Vm::take_sound_events`).
#[derive(Debug, Clone, PartialEq)]
pub enum SoundEvent {
    Play(SoundRequest),
    /// `StopSound(id)` with the id `PlaySound` returned.
    Stop(i32),
}

#[derive(Debug, Default)]
pub struct AiState {
    /// Pending audio commands; the game loop drains them every frame.
    pub sound_events: Vec<SoundEvent>,
    /// Sounds requested by actors, oldest first; ids start at 1.
    pub sounds: Vec<SoundRequest>,
    pub next_sound_id: i32,
    /// Volumes scripts set with `SetInstantMusicVolume`/`SetInstantSoundVolume`/`SetInstantSpeechVolume` (music, effects, speech;
    /// 0-255), `None` until set. Not applied to the mixer yet.
    pub instant_volumes: [Option<u8>; 3],
    /// The AI event manager: senders, listeners and the per-tick perception pass.
    pub events: crate::event_manager::EventManager,
    /// Actors that ran `ConBindEvents` (conversation event binding, done by the ConSys subsystem later).
    pub con_bound: HashSet<u32>,
}

impl AiState {
    pub fn reset(&mut self) {
        *self = AiState::default();
    }

    /// Drops everything that belongs to a destroyed actor.
    pub fn forget(&mut self, obj: u32) {
        self.events.destroy_actor(obj);
        self.con_bound.remove(&obj);
    }
}

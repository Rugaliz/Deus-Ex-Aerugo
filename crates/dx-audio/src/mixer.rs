//! The audio mixer: Galaxy's software mixer (`crate::galaxy`) with its effect voices (game sounds, started and
//! updated by the subsystem's `Update`, sounds.md) and its music voices (the tracker song). The device pulls
//! interleaved f32 frames; they are Galaxy's output as the original writes it into its DirectSound ring:
//! `sat(sat(music) + sat(effects))` per 16-bit value, the music lagging the effects of the same mix pass.
use crate::galaxy::sample::Sample;
use crate::galaxy::song::Pcm;
use crate::galaxy::Engine;
use std::collections::VecDeque;
use std::sync::Arc;

/// Frames Galaxy's timer renders per callback: `Latency / 2` ms (Latency 40 at 44.1 kHz), mixed in chunks of at most
/// 512 (mixer.md sections 2.2, 4.1).
pub const TIMER_FRAMES: usize = 882;

/// How long the music is heard after the effects mixed in the same pass, in frames at 44.1 kHz with Latency 40.
/// Galaxy writes the music bus just behind the DirectSound play cursor of its one-second ring and the effect bus
/// `Latency` ahead of the write cursor (mixer.md section 3): the music lags by the ring minus that distance, taken
/// here as `rate - Latency - Latency/2` (the write cursor one timer period ahead). Measured on the original (a click
/// and a song asked for at the same moment): 41381 frames including the wait for the first row tick.
pub fn music_delay_frames(rate: u32, latency_ms: u32) -> usize {
    (rate as usize * (1000 - latency_ms as usize - latency_ms as usize / 2)) / 1000
}

/// What the lip-sync analysis reads of a playing voice (sounds.md 5.5 s).
pub struct VoiceData {
    pub data: Arc<Pcm>,
    /// Position, ring start and ring end (samples).
    pub pos: i32,
    pub start: i32,
    pub loop_end: i32,
    pub sflags: u16,
}

pub struct Mixer {
    pub rate: u32,
    /// Galaxy's mixer with its effect voices and the loaded song.
    galaxy: Engine,
    /// Rendered frames not yet played (interleaved i16): music (with the lag) and effects.
    music_out: VecDeque<i16>,
    fx_out: VecDeque<i16>,
    /// The frames one timer callback renders.
    pub timer_frames: usize,
    /// The music's lag behind the effects in frames (`music_delay_frames`).
    pub music_delay: usize,
    /// Music volume (`MusicVolume / 255`, 0..1) and `MusicFade` (1 = full, below 0 = silent).
    pub music_volume: f32,
    pub music_fade: f32,
    /// `SoundVolume`, `SpeechVolume` (0..255): the effects master is `127 * max(S, P) / 255` (`SetVolumes`).
    pub sound_volume: u8,
    pub speech_volume: u8,
    /// Sounds started since creation (tests and statistics).
    pub started: u64,
}

impl Mixer {
    pub fn new(rate: u32) -> Mixer {
        Mixer::with_channels(rate, 16)
    }

    /// A mixer with `effects` effect voices (`EffectsChannels`).
    pub fn with_channels(rate: u32, effects: usize) -> Mixer {
        let delay = music_delay_frames(rate, 40);
        Mixer {
            rate,
            galaxy: Engine::new(rate, effects),
            music_out: std::iter::repeat_n(0i16, delay * 2).collect(),
            fx_out: VecDeque::new(),
            timer_frames: TIMER_FRAMES,
            music_delay: delay,
            music_volume: 0.7,
            music_fade: 1.0,
            sound_volume: 204,
            speech_volume: 255,
            started: 0,
        }
    }

    /// Sets the number of effect voices (`EffectsChannels`, Galaxy's voice allocation 1060ebc0 at `Init`): the mixer
    /// starts over (no sound plays yet at that point).
    pub fn set_effects_channels(&mut self, n: usize) {
        if n != self.effects() {
            self.galaxy = Engine::new(self.rate, n.min(crate::galaxy::mixer::VOICES));
        }
    }

    /// The number of effect voices (`EffectsChannels`).
    pub fn effects(&self) -> usize {
        self.galaxy.mix.effects
    }

    /// Starts a sound on effect voice `ch` (Galaxy's voice start 10612310); a stereo sound also takes `ch + 1`.
    pub fn start_voice(&mut self, ch: usize, s: &Arc<Sample>, freq: u32, vol: u16, pan: u16) -> bool {
        self.started += 1;
        self.galaxy.mix.start_effect(ch, s, freq, vol, pan)
    }

    /// The per-frame update of a playing effect voice (1060d280).
    pub fn update_voice(&mut self, ch: usize, freq: u32, vol: u16, pan: u16) {
        self.galaxy.mix.update_effect(ch, freq, vol, pan);
    }

    /// `Update` writes the target pan once more after the update.
    pub fn set_voice_pan_target(&mut self, ch: usize, pan: u16) {
        if let Some(v) = self.galaxy.mix.voices.get_mut(ch) {
            v.pan_target = pan;
        }
    }

    /// Installs, replaces or removes the effect reverb (106113d0).
    pub fn set_reverb(&mut self, p: &crate::galaxy::reverb::ReverbParams) {
        self.galaxy.mix.set_reverb(p);
    }

    /// Whether the effect reverb is installed (tests).
    pub fn has_reverb(&self) -> bool {
        self.galaxy.mix.reverb.is_some()
    }

    /// Whether an effect voice is still playing (1060cf00 op 2: its playing byte).
    pub fn voice_playing(&self, ch: usize) -> bool {
        self.galaxy.mix.voices.get(ch).is_some_and(|v| v.playing)
    }

    /// Stops an effect voice and its linked voice (10618410).
    pub fn stop_voice(&mut self, ch: usize) {
        self.galaxy.mix.stop_voice(ch);
    }

    /// The effect voices playing (tests, statistics).
    pub fn voice_count(&self) -> usize {
        self.galaxy.mix.voices[..self.effects()].iter().filter(|v| v.playing).count()
    }

    /// A voice's data and position for the lip-sync analysis.
    pub fn voice_data(&self, ch: usize) -> Option<VoiceData> {
        let v = self.galaxy.mix.voices.get(ch)?;
        Some(VoiceData { data: v.data.clone()?, pos: v.pos, start: v.start, loop_end: v.loop_end, sflags: v.sflags })
    }

    /// The effects master level (`SetVolumes` 106061b0): `trunc(127 * max(SoundVolume / 255, SpeechVolume / 255))`.
    pub fn fx_level(&self) -> u8 {
        let a = self.sound_volume as f32 / 255.0;
        let b = self.speech_volume as f32 / 255.0;
        (127.0 * a.max(b)) as i32 as u8
    }

    /// Loads a tracker module (the whole file) as Galaxy's song (`RegisterMusic` from memory, 10618be0): the previous
    /// song is stopped and unloaded first. Returns the song's name.
    pub fn load_music(&mut self, module: &[u8]) -> Result<String, String> {
        self.galaxy.load_music(module).map_err(|e| format!("Galaxy could not load the module: {e:?}"))?;
        let name = &self.galaxy.player.as_ref().unwrap().song.name;
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        Ok(String::from_utf8_lossy(&name[..end]).trim().to_string())
    }

    /// `UnregisterMusic` (10606e60): stop and unload the song.
    pub fn unload_music(&mut self) {
        if let Some(p) = self.galaxy.player.as_mut() {
            p.stop(&mut self.galaxy.mix);
        }
        self.galaxy.player = None;
    }

    /// What `Update` does to play the loaded song from `section` (player.md 5.4): stop, cut the music voices, start,
    /// jump to the order at the next row; section 255 only stops.
    pub fn start_music_section(&mut self, section: u8) {
        if let Some(p) = self.galaxy.player.as_mut() {
            p.play_section(&mut self.galaxy.mix, section);
        }
    }

    /// Loads a module and plays it from `section` (255: loaded but stopped). Returns the song's name.
    pub fn play_music(&mut self, module: &[u8], section: u8) -> Result<String, String> {
        let name = self.load_music(module)?;
        self.start_music_section(section);
        Ok(name)
    }

    /// The music master level as `SetVolumes` (106061b0) sets it: `trunc(max(fade, 0) * volume * 127)` (0..127).
    pub fn music_level(&self) -> u8 {
        let v = self.music_volume.clamp(0.0, 1.0) as f64;
        (self.music_fade.max(0.0) as f64 * v * 127.0) as i32 as u8
    }

    /// The music's gain as a fraction (tests, statistics): the level squared, as the mixer applies it.
    pub fn music_gain(&self) -> f32 {
        let l = self.music_level() as f32;
        l * l * 2.0 / 32768.0
    }

    /// Galaxy's current order (`10698201`; 255 when no song has started), which `Update` reports as the player's
    /// `SongSection`; `None` when no song is loaded.
    pub fn music_order(&mut self) -> Option<i32> {
        self.galaxy.player.as_ref().map(|p| p.order as i32)
    }

    /// Stops the music (`106183a0`); the song stays loaded.
    pub fn stop_music(&mut self) {
        if let Some(p) = self.galaxy.player.as_mut() {
            p.stop(&mut self.galaxy.mix);
        }
    }

    pub fn has_music(&self) -> bool {
        self.galaxy.player.as_ref().is_some_and(|p| p.playing)
    }

    /// Fills `out` (interleaved stereo, any even length) with the next audio, rendering Galaxy in timer-sized
    /// steps as needed.
    pub fn mix(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        let level = self.music_level();
        let fx = self.fx_level();
        let m = &mut self.galaxy.mix;
        m.music_master = level;
        m.music_target = level;
        m.music_step = 0;
        m.fx_master = fx;
        m.fx_target = fx;
        m.fx_step = 0;
        while self.fx_out.len() < frames * 2 {
            let (mut mu, mut fx) = (Vec::new(), Vec::new());
            self.galaxy.render(self.timer_frames, &mut mu, &mut fx);
            self.music_out.extend(mu);
            self.fx_out.extend(fx);
        }
        for x in out.iter_mut().take(frames * 2) {
            let a = self.music_out.pop_front().unwrap_or(0);
            let b = self.fx_out.pop_front().unwrap_or(0);
            *x = a.saturating_add(b) as f32 / 32768.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::galaxy::sample::{F_16BIT, F_MONO};

    fn constant(frames: usize, v: i16) -> Arc<Sample> {
        let mut d = vec![v; frames];
        d.resize(frames + 16, 0);
        Arc::new(Sample { flags: F_MONO | F_16BIT, pan: 0x4000, volume: 0x7fff, length: frames as u32, loop_start: 0, loop_end: 0, rate: 44100, data: Some(Arc::new(Pcm::S16(d))), stream: None })
    }

    #[test]
    fn an_effect_voice_plays_at_galaxys_level_and_ends() {
        let mut m = Mixer::new(44100);
        m.sound_volume = 255;
        m.music_volume = 0.0;
        let s = constant(4410, 16384);
        assert!(m.start_voice(15, &s, 44100, 0x7fff, 0x4000));
        let mut out = vec![0f32; 2048 * 2];
        m.mix(&mut out);
        // centre pan: T[0x4000] = 23170; volume chain 0x7e02 * 127^2*2 ... * 0x7fff; the interpolation scales by 0x7fff
        let l = out[2000 * 2];
        assert!(l > 0.2 && l < 0.4, "{l}");
        // the centre is T[0x3fff] on the left, T[0x4000] on the right
        assert!((out[2000 * 2] - out[2000 * 2 + 1]).abs() < 1e-4);
        assert_eq!(m.voice_count(), 1);
        let mut more = vec![0f32; 4096 * 2];
        m.mix(&mut more);
        assert_eq!(m.voice_count(), 0, "a sound without a loop ends");
        assert!(!m.voice_playing(15));
    }

    #[test]
    fn pan_uses_galaxys_table() {
        let mut m = Mixer::new(44100);
        m.sound_volume = 255;
        m.music_volume = 0.0;
        let s = constant(44100, 16384);
        m.start_voice(3, &s, 44100, 0x7fff, 0);
        let mut out = vec![0f32; 1024 * 2];
        m.mix(&mut out);
        assert!(out[1000 * 2] > 0.3 && out[1000 * 2 + 1] == 0.0, "{:?}", &out[2000..2002]);
    }
}

//! Galaxy.dll's tracker music engine and software mixer, ported from the original (docs/galaxy/loader.md,
//! player.md, mixer.md).
pub mod mixer;
pub mod mp3;
mod mp3_tables;
pub mod player;
pub mod reverb;
pub mod sample;
pub mod song;
pub mod tables;

use mixer::{MixState, CHUNK};
use player::Player;

/// The mixer with its (optional) song.
pub struct Engine {
    pub mix: MixState,
    pub player: Option<Player>,
}

impl Engine {
    /// The state after library init and an output open at `rate` with `effects` effect voices.
    pub fn new(rate: u32, effects: usize) -> Engine {
        Engine { mix: MixState::new(rate, effects), player: None }
    }

    /// Loads a module (replacing the current song: stop and unload first).
    pub fn load_music(&mut self, data: &[u8]) -> Result<(), song::LoadError> {
        if let Some(p) = self.player.as_mut() {
            p.stop(&mut self.mix);
        }
        self.player = None;
        let s = song::load(data)?;
        self.player = Some(Player::new(s, &mut self.mix));
        Ok(())
    }

    /// Mixes `frames` frames in chunks of at most 512 and appends the converted music and effect buses
    /// (interleaved L R) to `music` and `fx`.
    pub fn render(&mut self, frames: usize, music: &mut Vec<i16>, fx: &mut Vec<i16>) {
        let mut left = frames;
        let mut m = vec![0i16; CHUNK * 2];
        let mut f = vec![0i16; CHUNK * 2];
        while left > 0 {
            let n = left.min(CHUNK);
            let player = &mut self.player;
            let got = self.mix.mix_chunk(n, &mut |mix| {
                if let Some(p) = player.as_mut() {
                    p.row_tick(mix);
                }
            });
            self.mix.run_hooks(got);
            self.mix.convert(got, &mut m, &mut f);
            music.extend_from_slice(&m[..got * 2]);
            fx.extend_from_slice(&f[..got * 2]);
            left -= got;
        }
    }
}

impl Engine {
    /// Mixes `frames` frames in chunks of `chunk` (at most 512) and appends the raw music dry bus (f32 L R) to `out`:
    /// the comparison format of the Galaxy oracle (tools/galaxy-oracle).
    pub fn render_music_bus(&mut self, frames: usize, chunk: usize, out: &mut Vec<f32>) {
        let mut left = frames;
        while left > 0 {
            let n = left.min(chunk);
            let player = &mut self.player;
            let got = self.mix.mix_chunk(n, &mut |mix| {
                if let Some(p) = player.as_mut() {
                    p.row_tick(mix);
                }
            });
            for f in &self.mix.buses().music_dry()[..got] {
                out.extend_from_slice(f);
            }
            left -= got;
        }
    }
}

/// The output's sum of the two buses: `sat(music + fx)` per value.
pub fn add_saturating(a: i16, b: i16) -> i16 {
    a.saturating_add(b)
}

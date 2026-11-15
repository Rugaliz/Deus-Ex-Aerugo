//! Procedural ("fractal") textures: pixels generated at run time from a few parameters, as the original engine's Fire
//! package does. Water so far (`WetTexture` distorts another texture, `WaveTexture` is a lit wave surface); the spec is
//! `docs/formats/water-texture.md`.
//!
//! Everything here is plain data processing with no engine dependency: the caller loads the parameters (see
//! `ue_assets::procedural::WaterParams`), steps the simulation when the texture's clock says so, and turns the 8-bit
//! result into colours with the texture's palette.

pub mod clock;
pub mod fire;
pub mod ice;
pub mod water;

pub use clock::AnimClock;
pub use fire::Fire;
pub use ice::Ice;
pub use water::{Drop, Water};

/// Tables the Fire package shares between all its textures: a 256-entry sine wave and a small random generator.
pub struct Shared {
    /// `round(sin(i * 2pi / 256) * 127.5 + 127.45)`: 0..255, 127 at i = 0.
    pub sine: [u8; 256],
    pub rng: Rng,
}

impl Default for Shared {
    fn default() -> Self {
        Shared::new()
    }
}

impl Shared {
    pub fn new() -> Shared {
        let mut sine = [0u8; 256];
        for (i, s) in sine.iter_mut().enumerate() {
            // angle computed in floating point with a single-precision 2pi, the result stored as single precision and then
            // rounded to the nearest integer (ties to even), as the engine's start-up code does
            let angle = (i as f64 / 256.0) * (std::f32::consts::TAU as f64);
            let v = (angle.sin() * 127.5 + 127.45) as f32;
            *s = v.round_ties_even() as i32 as u8;
        }
        Shared { sine, rng: Rng::new(1) }
    }
}

/// The Fire package's random source: 512 random bytes (only the first 256 are ever read) used as 64 words; each draw
/// reads the word 32 places ahead of a moving cursor and mixes it into the word under the cursor.
pub struct Rng {
    bytes: [u8; 512],
    cursor: usize,
}

impl Rng {
    /// Fills the table with the C runtime's `rand()` sequence for `seed` (the engine fills it with `appRand`, whose state
    /// depends on everything that ran before; any seed gives the same behaviour).
    pub fn new(seed: u32) -> Rng {
        let mut state = seed;
        let mut bytes = [0u8; 512];
        for b in bytes.iter_mut() {
            state = state.wrapping_mul(214013).wrapping_add(2531011);
            *b = ((state >> 16) & 0x7fff) as u8;
        }
        Rng { bytes, cursor: 0 }
    }

    pub fn next(&mut self) -> u32 {
        let word = |b: &[u8; 512], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        let read = (self.cursor + 0x80) & 0xfc;
        self.cursor = (self.cursor + 4) & 0xfc;
        let r = word(&self.bytes, read);
        let mixed = word(&self.bytes, self.cursor) ^ r;
        self.bytes[self.cursor..self.cursor + 4].copy_from_slice(&mixed.to_le_bytes());
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_table_has_the_engine_shape() {
        let s = Shared::new().sine;
        // 127 at 0 and 128, peak 255 at a quarter turn, trough 0 at three quarters
        assert_eq!((s[0], s[64], s[128], s[192]), (127, 255, 127, 0));
        assert!(s.windows(2).take(64).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn rng_mixes_the_word_under_the_cursor() {
        let mut r = Rng::new(7);
        let before: Vec<u8> = r.bytes[..256].to_vec();
        let a = r.next();
        let expect_a = u32::from_le_bytes(before[0x80..0x84].try_into().unwrap());
        assert_eq!(a, expect_a);
        let w4 = u32::from_le_bytes(before[4..8].try_into().unwrap());
        assert_eq!(u32::from_le_bytes(r.bytes[4..8].try_into().unwrap()), w4 ^ expect_a);
    }
}

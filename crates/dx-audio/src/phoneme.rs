//! Lip sync: the phoneme detector of the original audio driver (Galaxy.dll). Speaking pawns get their `nextPhoneme`
//! from the sound they are playing, not from script: every 0.1 s of play time the driver takes 1024 samples, weights
//! them with a rising ramp, runs a radix-2 FFT, and picks a mouth letter from the frequency of the strongest of the
//! first 512 bins. `Pawn.LipSynch` then turns the letter into a mouth blend animation.
//!
//! Quirks kept on purpose (they change which letter comes out): the ramp reaches its top at sample 1024, so it is
//! `i / 1024`, not a triangle; only the real part of the transform is used; 8-bit data is read from the start of the
//! sound into every other slot, so half of the buffer keeps values from the previous call; the loudness test before the
//! peak search always passes.
use crate::PcmSound;

/// Samples per analysis (the FFT size).
pub const WINDOW: usize = 1024;
/// Seconds of play time between two analyses of one sound.
pub const INTERVAL: f64 = 0.1;
/// Samples in the ring buffer that a streamed (MP3) voice is decoded into while it plays.
pub const RING: usize = 0x7fff;
/// The letter written when nothing is said (silence or no clear peak).
pub const SILENT: char = 'X';

/// Single-precision 2*pi as the driver stores it.
const TAU_F32: f32 = 6.283_185_5;

pub struct PhonemeDetector {
    /// The driver's static analysis buffers: real and imaginary parts. The real part keeps its old contents between
    /// calls where the 8-bit path does not overwrite it.
    input: Box<[f32; WINDOW]>,
    re: Box<[f32; WINDOW]>,
    im: Box<[f32; WINDOW]>,
}

impl Default for PhonemeDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl PhonemeDetector {
    pub fn new() -> PhonemeDetector {
        PhonemeDetector { input: Box::new([0.0; WINDOW]), re: Box::new([0.0; WINDOW]), im: Box::new([0.0; WINDOW]) }
    }

    /// Where the analysis of a whole (non-streamed) sound starts: the sample `INTERVAL` seconds after the play time
    /// (both steps truncate, as the driver's float-to-int conversion).
    pub fn whole_start(rate: u32, play_time: f32) -> usize {
        let p = (rate as f64 * play_time as f64).max(0.0) as u64;
        (p as f64 + rate as f64 * INTERVAL) as usize
    }

    /// The window of a streamed voice: it starts at the voice's play position inside the decode ring and ends where
    /// the ring ends (the driver does not wrap around), or where the sound ends.
    pub fn streamed_window(cursor: usize, frames: usize) -> (usize, usize) {
        let room = RING - cursor % RING;
        (cursor, (cursor + room).min(frames))
    }

    /// Analyses `WINDOW` samples of `sound` from sample `start`; samples at or past `end` count as silence. Returns
    /// the mouth letter (`A E F M O T U` or `X`).
    pub fn detect(&mut self, sound: &PcmSound, start: usize, end: usize) -> char {
        let at = |i: usize| {
            let x = sound.samples.get(i).copied().unwrap_or(0.0);
            if sound.bits == 8 {
                // the stored byte (signed after the loader's conversion)
                ((x * 128.0).round().clamp(-128.0, 127.0)) as i32
            } else {
                (x * 32768.0).round().clamp(-32768.0, 32767.0) as i32
            }
        };
        self.detect_raw(sound.rate, sound.bits == 8, &at, start, end)
    }

    /// The analysis on the data as Galaxy stores it: `at(i)` is sample `i` (an `i16`, or for 8-bit data the stored
    /// signed byte, which the driver reads as an unsigned high byte).
    pub fn detect_raw(&mut self, rate: u32, bits8: bool, at: &dyn Fn(usize) -> i32, start: usize, end: usize) -> char {
        self.fill_raw(bits8, at, start, end);
        self.fft();
        for v in self.re.iter_mut() {
            *v = v.abs();
        }
        self.classify(rate)
    }

    /// Copies the samples into the input buffer, scaled to -1..1 and weighted with the ramp.
    fn fill_raw(&mut self, bits8: bool, at: &dyn Fn(usize) -> i32, start: usize, end: usize) {
        let mut put = |idx: usize, raw: f64| {
            let raw = if raw > 32767.0 { raw - 65536.0 } else { raw };
            let ramp = 1.0 - ((idx as f64 - 1024.0) * (1.0 / 1024.0)).abs();
            self.input[idx] = (raw * (1.0 / 32768.0) * ramp) as f32;
        };
        if bits8 {
            // the position decides only whether there is data; the bytes come from the start of the sound
            for k in 0..WINDOW {
                let raw = if start + k < end { ((at(k) as u8) as f64) * 256.0 } else { 0.0 };
                put(k >> 1, raw);
            }
        } else {
            for k in 0..WINDOW {
                let raw = if start + k < end { (at(start + k) as u16) as f64 } else { 0.0 };
                put(k, raw);
            }
        }
    }

    /// In-place radix-2 FFT of the input (imaginary part zero), with the driver's sine/cosine recurrence for the
    /// twiddle factors. Wide intermediates stand in for the x87 registers, `f32` for values the driver stores.
    fn fft(&mut self) {
        for i in 0..WINDOW {
            let r = (i as u32).reverse_bits() >> (32 - 10);
            self.re[r as usize] = self.input[i];
            self.im[r as usize] = 0.0;
        }
        let (re, im) = (&mut *self.re, &mut *self.im);
        let mut half = 1;
        let mut size = 2;
        while size <= WINDOW {
            let delta = TAU_F32 as f64 / size as f64;
            let sm2 = (delta * -2.0).sin();
            let cm2 = (delta * -2.0).cos();
            let d1 = -((delta as f32) as f64);
            let sm1 = d1.sin();
            let cm1 = d1.cos();
            let w = cm1 + cm1;
            let mut group = 0;
            while group < WINDOW {
                let (mut ai2, mut ai1, mut ar2, mut ar1) = (sm2 as f32, sm1, cm2, cm1);
                for n in 0..half {
                    let j = group + n;
                    let k = j + half;
                    let ar2_s = ar2 as f32;
                    let ai1_s = ai1 as f32;
                    let ar0 = (ar1 * w - ar2_s as f64) as f32;
                    let ai0 = ai1_s as f64 * w - ai2 as f64;
                    let tr = (ar0 as f64 * re[k] as f64 - ai0 * im[k] as f64) as f32;
                    let ti = ar0 as f64 * im[k] as f64 + ai0 * re[k] as f64;
                    re[k] = re[j] - tr;
                    im[k] = (im[j] as f64 - ti) as f32;
                    re[j] = tr + re[j];
                    im[j] = (ti + im[j] as f64) as f32;
                    ai2 = ai1_s;
                    ar2 = ar1;
                    ar1 = ar0 as f64;
                    ai1 = ai0;
                }
                group += size;
            }
            half = size;
            size *= 2;
        }
    }

    /// The letter from the magnitudes in `re` (`FUN_10608d70`).
    fn classify(&self, rate: u32) -> char {
        const LETTERS: [char; 8] = ['A', 'E', 'F', 'M', 'O', 'T', 'U', 'X'];
        let rate_f = rate as f32;
        let n = (300.0f64 * 1024.0 / rate_f as f64) as usize;
        let sum: f64 = self.re[..=n.min(WINDOW - 1)].iter().map(|&v| v as f64).sum();
        let average = sum / n as f64 + 1.0;
        if !(average > 0.2) {
            return SILENT;
        }
        let (mut peak, mut at) = (-99999.0f64, 0usize);
        for (i, &v) in self.re[..WINDOW / 2].iter().enumerate() {
            if peak < v as f64 {
                peak = v as f64;
                at = i;
            }
        }
        if !(peak > 5.0) {
            return SILENT;
        }
        let freq = (at as f64 * rate_f as f64 * (1.0 / 1024.0)) as f32;
        let i = if freq > 2000.0 {
            5
        } else if freq > 1500.0 {
            2
        } else if freq > 600.0 {
            0
        } else if freq > 400.0 {
            4
        } else if freq > 250.0 {
            6
        } else if freq > 1000.0 {
            3 // never reached: the driver tests this after the smaller thresholds
        } else {
            1
        };
        LETTERS[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, freq: f32, amp: f32, frames: usize) -> PcmSound {
        let samples = (0..frames).map(|i| (i as f32 * freq * std::f32::consts::TAU / rate as f32).sin() * amp).collect();
        PcmSound { rate, channels: 1, samples, bits: 16, streamed: false }
    }

    /// The letter for each frequency band, from the driver's thresholds (2000, 1500, 600, 400, 250 Hz). Bin width at
    /// 22050 Hz is 21.5 Hz, so each tone sits well inside its band.
    #[test]
    fn tones_map_to_the_driver_bands() {
        let mut d = PhonemeDetector::new();
        for (freq, want) in [(3000.0, 'T'), (1800.0, 'F'), (1000.0, 'A'), (500.0, 'O'), (320.0, 'U'), (150.0, 'E')] {
            let s = tone(22050, freq, 0.5, 4096);
            assert_eq!(d.detect(&s, 0, s.frames()), want, "{freq} Hz");
        }
    }

    /// The peak must be above 5: the ramp halves the energy, so a full-scale bin of a tone of amplitude `a` is about
    /// `a * 1024 / 4 = 256 a`; amplitude 0.01 gives ~2.6 (silent), 0.05 gives ~13 (spoken).
    #[test]
    fn quiet_sound_and_silence_are_x() {
        let mut d = PhonemeDetector::new();
        let quiet = tone(22050, 1000.0, 0.01, 4096);
        assert_eq!(d.detect(&quiet, 0, quiet.frames()), 'X');
        let loud = tone(22050, 1000.0, 0.05, 4096);
        assert_eq!(d.detect(&loud, 0, loud.frames()), 'A');
        // past the end of the data
        assert_eq!(d.detect(&loud, 5000, loud.frames()), 'X');
    }

    #[test]
    fn fft_matches_a_plain_dft() {
        let mut d = PhonemeDetector::new();
        let s = tone(22050, 700.0, 0.3, 2048);
        let at = |i: usize| (s.samples.get(i).copied().unwrap_or(0.0) * 32768.0).round().clamp(-32768.0, 32767.0) as i32;
        d.fill_raw(false, &at, 100, s.frames());
        let x: Vec<f64> = d.input.iter().map(|&v| v as f64).collect();
        d.fft();
        for bin in [0usize, 1, 17, 32, 33, 100, 511] {
            let want: f64 = x.iter().enumerate().map(|(n, v)| v * (std::f64::consts::TAU * (bin * n) as f64 / 1024.0).cos()).sum();
            assert!((d.re[bin] as f64 - want).abs() < 1e-2 * (1.0 + want.abs()), "bin {bin}: {} vs {want}", d.re[bin]);
        }
    }

    #[test]
    fn window_positions() {
        // 22050 Hz, 1.0 s played: sample 22050 + 2205
        assert_eq!(PhonemeDetector::whole_start(22050, 1.0), 24255);
        assert_eq!(PhonemeDetector::streamed_window(10, 100_000), (10, 0x7fff));
        assert_eq!(PhonemeDetector::streamed_window(0x7fff + 5, 40_000), (0x7fff + 5, 40_000));
    }
}

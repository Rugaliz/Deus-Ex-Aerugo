//! Colour helpers of the engine's lighting shared by the lightmap builder and the actor lighting.

/// The engine's hue/saturation/value to RGB (`FGetHSV`). Note that saturation 255 means WHITE and 0 a pure hue.
pub fn hsv_to_rgb(hue: u8, sat: u8, value: u8) -> [f32; 3] {
    let b = value as f64 * (1.4 / 255.0);
    let tone = (0.7 / (b.sqrt() + 0.01) * b).clamp(0.0, 1.0);
    let h = hue as f64;
    let (r, g, bl) = if hue < 0x56 {
        ((85.0 - h) / 85.0, h / 85.0, 0.0)
    } else if hue < 0xab {
        (0.0, (170.0 - h) / 85.0, (h - 85.0) / 85.0)
    } else {
        ((h - 170.0) / 85.0, 0.0, (255.0 - h) / 84.0)
    };
    let s = sat as f64 / 255.0;
    [r, g, bl].map(|c| (((1.0 - c) * s + c) * tone) as f32)
}

/// `GMath.SinTab`: the engine's 16384-entry sine table (65536 rotator units per turn, 4 units per step).
pub fn sin_tab(a: i32) -> f32 {
    use std::sync::OnceLock;
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| (0..16384).map(|i| (i as f64 * 0.000_383_495_196_971_410_3).sin() as f32).collect())[((a >> 2) & 16383) as usize]
}

/// The renderer's per-frame random tables (`FGlobalRandoms`, Render.dll 10b13080): `a` is refilled with 256
/// `appFrand()` values every frame (flickering lights, torch and fire wavers); `b` follows `a` smoothly in steps of
/// 1/35 s of level time (watery shimmer).
#[derive(Debug, Clone)]
pub struct GlobalRandoms {
    pub a: [f32; 256],
    pub b: [f32; 256],
    d: [f32; 256],
    last: u32,
}

impl Default for GlobalRandoms {
    fn default() -> Self {
        GlobalRandoms { a: [0.0; 256], b: [0.0; 256], d: [0.0; 256], last: 0 }
    }
}

impl GlobalRandoms {
    /// `FGlobalRandoms::Tick(TimeSeconds)`, called once per drawn frame; `frand` is the game's `appFrand`.
    pub fn tick(&mut self, time: f32, frand: &mut dyn FnMut() -> f32) {
        let now = ((time as f64 * 35.0) as i64) as u32;
        for x in self.a.iter_mut() {
            *x = frand();
        }
        let last = self.last;
        if now.wrapping_sub(last).wrapping_mul(16) < 256 && last != 0 {
            // a few ticks on: the slots between the old and the new tick aim at fresh values, the others move on
            let to = (now & 0xf) * 16;
            let from = (last & 0xf) * 16;
            let mut i = from;
            while i != to {
                self.d[i as usize] = (self.a[i as usize] - self.b[i as usize]) * 0.0625;
                i = (i + 1) & 0xff;
            }
            let steps = (now.wrapping_sub(last) & 0xff) as f32;
            let mut i = to;
            loop {
                self.b[i as usize] += steps * self.d[i as usize];
                i = (i + 1) & 0xff;
                if i == from {
                    break;
                }
            }
        } else {
            for i in 0..256 {
                self.b[i] = frand();
                self.d[i] = (self.a[i] - self.b[i]) * 0.0625;
            }
        }
        for x in self.b.iter_mut() {
            *x = x.clamp(0.0, 1.0);
        }
        self.last = now;
    }
}

/// The strobe state shared by every strobing light (`GlobalLighting`'s strobe flips once per new level time).
#[derive(Debug, Clone, Copy, Default)]
pub struct Strobe {
    last_time: f32,
    off: bool,
}

/// `URender::GlobalLighting`'s `LightType` functions (jump table at 10b2a284): the brightness `b` (`LightBrightness /
/// 255`) as the type changes it at level time `time`, clamped to 0..1. `id_byte` stands for the low byte of the
/// actor's address that the original's flicker picks its random value with.
#[allow(clippy::too_many_arguments)]
pub fn light_type_brightness(light_type: u8, b: f32, period: u8, phase: u8, id_byte: u8, time: f32, randoms: &GlobalRandoms, strobe: &mut Strobe) -> f32 {
    // `_ftol` truncates and the low 32 bits are used
    let units = |div: f64| (((time as f64) * 2_293_760.0 / div + (phase as u32 * 256) as f64) as i64) as i32;
    let v = match light_type {
        0 => 0.0,
        2 => (((sin_tab(units(period.max(1) as f64)) as f64) * 0.39 + 0.6) * b as f64) as f32,
        3 => {
            if units(period as f64 + 1.0) & 1 != 0 {
                0.0
            } else {
                b
            }
        }
        4 => {
            let r = randoms.a[id_byte as usize];
            if (r as f64) < 0.5 {
                0.0
            } else {
                r * b
            }
        }
        5 => {
            if strobe.last_time != time {
                strobe.off = !strobe.off;
                strobe.last_time = time;
            }
            if strobe.off {
                0.0
            } else {
                b
            }
        }
        7 => (((sin_tab(units(period.max(1) as f64)) as f64) * 0.09 + 0.9) * b as f64) as f32,
        // steady, backdrop; the texture-palette types (8, 9) are not used by the shipped maps
        _ => b,
    };
    v.clamp(0.0, 1.0)
}

#[cfg(test)]
mod type_tests {
    use super::*;

    #[test]
    fn pulse_and_subtle_pulse_follow_the_sine_table() {
        let r = GlobalRandoms::default();
        let mut st = Strobe::default();
        for (time, period, phase) in [(0.0f32, 32u8, 0u8), (1.25, 32, 0), (3.7, 64, 17), (10.0, 1, 200)] {
            let units = (time as f64 * 2_293_760.0 / period as f64 + phase as f64 * 256.0) as i64 as i32;
            let s = ((((units >> 2) & 16383) as f64) * std::f64::consts::TAU / 16384.0).sin();
            let pulse = light_type_brightness(2, 0.8, period, phase, 0, time, &r, &mut st);
            assert!(((pulse as f64) - (s * 0.39 + 0.6) * 0.8).abs() < 1e-5, "{time} {pulse}");
            let subtle = light_type_brightness(7, 0.8, period, phase, 0, time, &r, &mut st);
            assert!(((subtle as f64) - (s * 0.09 + 0.9) * 0.8).abs() < 1e-5);
        }
    }

    #[test]
    fn blink_flicker_strobe() {
        let mut r = GlobalRandoms::default();
        let mut st = Strobe::default();
        // blink: off when the truncated phase angle is odd
        let on = light_type_brightness(3, 1.0, 0, 0, 0, 0.0, &r, &mut st);
        assert_eq!(on, 1.0);
        let t = 1.0 / 2_293_760.0f32 * 3.0; // three units
        let units = (t as f64 * 2_293_760.0) as i64;
        assert_eq!(light_type_brightness(3, 1.0, 0, 0, 0, t, &r, &mut st), if units & 1 == 1 { 0.0 } else { 1.0 });
        // flicker: the frame random of the actor's byte; off below one half
        r.a[7] = 0.3;
        r.a[8] = 0.75;
        assert_eq!(light_type_brightness(4, 0.8, 0, 0, 7, 0.0, &r, &mut st), 0.0);
        assert_eq!(light_type_brightness(4, 0.8, 0, 0, 8, 0.0, &r, &mut st), 0.75 * 0.8);
        // strobe: flips once per new level time, shared by all strobing lights
        let mut st = Strobe::default();
        let a = light_type_brightness(5, 1.0, 0, 0, 0, 1.0, &r, &mut st);
        let b = light_type_brightness(5, 1.0, 0, 0, 0, 1.0, &r, &mut st);
        let c = light_type_brightness(5, 1.0, 0, 0, 0, 1.1, &r, &mut st);
        assert_eq!((a, b, c), (0.0, 0.0, 1.0));
        assert_eq!(light_type_brightness(0, 1.0, 0, 0, 0, 0.0, &r, &mut st), 0.0);
        assert_eq!(light_type_brightness(1, 1.5, 0, 0, 0, 0.0, &r, &mut st), 1.0);
    }

    #[test]
    fn randoms_refill_and_follow() {
        let mut g = GlobalRandoms::default();
        let mut n = 0u32;
        let mut frand = || {
            n += 1;
            (n % 97) as f32 / 96.0
        };
        // first tick: both tables fresh (512 values)
        g.tick(1.0, &mut frand);
        let b0 = g.b;
        assert!(g.a.iter().all(|x| (0.0..=1.0).contains(x)));
        // one tick of 1/35 s later: `a` refilled, `b` moves by (a_old - b_old)/16 per tick on most slots
        g.tick(1.0 + 1.0 / 35.0 + 1e-4, &mut frand);
        let moved = (0..256).filter(|&i| g.b[i] != b0[i]).count();
        assert!(moved > 200, "{moved}");
        assert!(g.b.iter().all(|x| (0.0..=1.0).contains(x)));
    }
}

/// Render.dll's table `x^-0.5` (`FLightManager::Init`, 10b024b0): by the top 10 mantissa bits and the exponent byte.
pub fn fast_inv_sqrt(x: f32) -> f32 {
    use std::sync::OnceLock;
    static T: OnceLock<(Vec<f32>, Vec<f32>)> = OnceLock::new();
    let (mant, exp) = T.get_or_init(|| {
        let pow = |x: f32| -> f32 {
            let v = (x.abs() as f64).powf(-0.5) as f32;
            if v.is_nan() {
                0.0
            } else {
                v
            }
        };
        ((0..1024u32).map(|j| pow(f32::from_bits(0x3f80_0000 + j * 0x2000))).collect(), (0..512u32).map(|k| pow(f32::from_bits(k << 23))).collect())
    });
    let b = x.to_bits();
    mant[((b >> 13) & 0x3ff) as usize] * exp[(b >> 23) as usize]
}

/// Render.dll's table square root (`FLightManager::Init`, 10b02080): 2048 entries by the low exponent bit and the top
/// 10 mantissa bits, plus half the exponent.
pub fn fast_sqrt(x: f32) -> f32 {
    use std::sync::OnceLock;
    static T: OnceLock<Vec<u32>> = OnceLock::new();
    let t = T.get_or_init(|| {
        let mut t = vec![0u32; 2048];
        for i in 0..1024u32 {
            let m = f32::from_bits(i * 0x2000 + 0x3f80_0000);
            let s = (m as f64).sqrt() as f32;
            let h = f32::from_bits(s.to_bits().wrapping_sub(0x2000_0000));
            t[i as usize] = ((h as f64 * 2f64.sqrt()) as f32).to_bits();
            t[1024 + i as usize] = (h + h).to_bits();
        }
        t
    });
    let b = x.to_bits();
    f32::from_bits(t[((b >> 13) & 0x7ff) as usize].wrapping_add((b & 0x7f00_0000) >> 1))
}

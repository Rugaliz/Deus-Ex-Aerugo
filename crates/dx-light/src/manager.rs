//! The light manager's static lightmap of a surface (Render.dll `FLightManager`, see `docs/formats/lightmap-format.md`):
//! the zone ambient as start value, then per light a mask from its stored shadow bits (10b02650), the illumination of
//! the light's `LightEffect` (spatial functions 10b03360, 10b04f80, 10b054e0, 10b05740, 10b04a80) over the light's
//! rectangle on the surface (light entry 10b05fa0) and the merge through the light's colour table (10b03040), all in the
//! engine's fixed-point and table arithmetic (tables from `FLightManager::Init` 10b02080).

use crate::Light;
use std::sync::OnceLock;

type V3 = [f32; 3];

/// `LightEffect` entries of the engine's effect table (10b2a0f0): time-dependent effects (lightmap redone every frame).
pub const EFFECT_ANIMATED: [bool; 20] = [false, false, false, false, true, true, true, true, false, true, true, false, false, false, false, false, true, false, true, false];
/// The "waver" effects (torch, fire, water): the mask is shaken per texel every frame.
pub const EFFECT_WAVER: [bool; 20] = [false, true, true, true, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false];
const LE_SEARCHLIGHT: u8 = 4;
const LE_SLOW_WAVE: u8 = 5;
const LE_FAST_WAVE: u8 = 6;
const LE_STATIC_SPOT: u8 = 8;
const LE_DISCO: u8 = 10;
const LE_ROTOR: u8 = 18;
const LE_SPOTLIGHT: u8 = 12;
const LE_NON_INCIDENCE: u8 = 13;
const LE_SHELL: u8 = 14;
const LE_OMNI_BUMP_MAP: u8 = 15;
const LE_CYLINDER: u8 = 17;
/// Light entries per surface (`FLightManager`'s array, 0x10b401c0..0x10b495c0 at 148 bytes each).
pub const MAX_LIGHTS: usize = 256;

/// Actor flag bits read by the light manager (`Actor` bit word at 0x28).
pub const F_STATIC: u32 = 0x1;
pub const F_NO_DELETE: u32 = 0x4;
pub const F_DYNAMIC_LIGHT: u32 = 0x800;

struct Tables {
    /// `(1 - 3s^2 + 2s^3) / s` at `s = sqrt((i + 1) / 4096)`.
    falloff: Vec<f32>,
    /// `x^-0.5` by mantissa (top 10 bits) and by exponent byte (with sign).
    isqrt_mant: Vec<f32>,
    isqrt_exp: Vec<f32>,
    /// Shadow-bit filter: per 7-bit pattern and kernel row, four packed texel bytes.
    mask: Vec<[u32; 3]>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let pow = |x: f32| -> f32 {
            let v = (x.abs() as f64).powf(-0.5) as f32;
            if v.is_nan() {
                0.0
            } else {
                v
            }
        };
        let isqrt_exp = (0..512u32).map(|k| pow(f32::from_bits(k << 23))).collect();
        let isqrt_mant = (0..1024u32).map(|j| pow(f32::from_bits(0x3f80_0000 + j * 0x2000))).collect();
        let falloff = (0..4096u32)
            .map(|i| {
                let t = ((i + 1) as f64 * (1.0 / 4096.0)).sqrt();
                let t3 = t * t * t;
                (((t3 + t3) - t * t * 3.0 + 1.0) / t) as f32
            })
            .collect();
        // kernel rows 24 40 24 / 40 64 40 / 24 40 24 at columns 1..3 of an 8-wide row (total 320)
        let kernel: [[u32; 8]; 3] = [[0, 24, 40, 24, 0, 0, 0, 0], [0, 40, 64, 40, 0, 0, 0, 0], [0, 24, 40, 24, 0, 0, 0, 0]];
        let total: u32 = kernel.iter().flatten().sum();
        let mut mask = vec![[0u32; 3]; 128];
        for (p, entry) in mask.iter_mut().enumerate() {
            for (r, row) in kernel.iter().enumerate() {
                for t in 0..4 {
                    let sum: u32 = (0..8).filter(|&k| p & (1 << (k + t)) != 0).map(|k| row[k]).sum();
                    entry[r] += (sum * 255 / total) << (8 * t);
                }
            }
        }
        Tables { falloff, isqrt_mant, isqrt_exp, mask }
    })
}

/// `GMath.SinTab`: the engine's 16384-entry sine table (65536 rotator units per turn).
pub fn sin_tab(a: i32) -> f32 {
    ue_assets::light::sin_tab(a)
}

/// `fistp` with the FPU's round-to-nearest-even.
fn rint(x: f64) -> i32 {
    x.round_ties_even() as i64 as i32
}

/// The light manager's table square root (`FLightManager::Init` 10b02080).
pub fn fast_sqrt(x: f32) -> f32 {
    ue_assets::light::fast_sqrt(x)
}

/// The table `x^-0.5` (10b024b0).
pub fn fast_inv_sqrt(x: f32) -> f32 {
    let b = x.to_bits();
    let t = tables();
    t.isqrt_mant[((b >> 13) & 0x3ff) as usize] * t.isqrt_exp[(b >> 23) as usize]
}

/// The mask of one light (10b02650): the shadow bits (`ceil(w / 8)` bytes per row, LSB first) filtered by the 3x3
/// kernel, one byte per texel, rows `ceil(w / 8) * 8` apart; 127 everywhere for a light without bits.
pub fn mask_buffer(bits: Option<&[u8]>, w: usize, h: usize) -> Vec<u8> {
    let bpr = (w + 7) / 8;
    let stride = bpr * 8;
    let Some(bits) = bits else { return vec![0x7f; stride * h] };
    let t = tables();
    let row_words = stride / 4;
    // one row more: with a single row the engine's "row below" lies past the buffer
    let mut out = vec![0u32; row_words * (h + 1)];
    let (mut above, mut cur, mut below) = (0usize, 0usize, row_words);
    for r in 0..h {
        let mut src = r * bpr;
        let mut u9 = (bits.get(src).copied().unwrap_or(0) as u32) << 10;
        if u9 & 0x400 != 0 {
            u9 |= 0x300;
        }
        src += 1;
        for byte in 0..bpr {
            let next = if byte + 1 < bpr {
                let v = (bits.get(src).copied().unwrap_or(0) as u32) << 10;
                src += 1;
                v
            } else if (u9 >> 8) & 0x200 != 0 {
                0xc00
            } else {
                0
            };
            u9 = (u9 >> 8).wrapping_add(next);
            for (half, p) in [(0usize, u9 & 0x7f), (1, (u9 >> 4) & 0x7f)] {
                let e = t.mask[p as usize];
                out[above + 2 * byte + half] = out[above + 2 * byte + half].wrapping_add(e[0]);
                out[cur + 2 * byte + half] = out[cur + 2 * byte + half].wrapping_add(e[1]);
                out[below + 2 * byte + half] = out[below + 2 * byte + half].wrapping_add(e[2]);
            }
        }
        if r > 0 {
            above += row_words;
        }
        cur += row_words;
        if r + 2 != h {
            below += row_words;
        }
    }
    let mut bytes: Vec<u8> = out.iter().flat_map(|w| w.to_le_bytes()).collect();
    bytes.truncate(stride * h);
    bytes
}

/// A surface's frame (`FCoords`: origin `pBase`, X = `vTextureU`, Y = `vTextureV`, Z = `vNormal`) and its lightmap.
#[derive(Debug, Clone, Copy)]
pub struct Surface {
    pub origin: V3,
    pub u: V3,
    pub v: V3,
    pub n: V3,
    pub pan: [f32; 2],
    pub scale: [f32; 2],
    pub w: usize,
    pub h: usize,
}

/// `FCoords::Inverse` (Core.dll) of the axes: the inverse axes as the engine computes them.
fn inverse_axes(x: V3, y: V3, z: V3) -> [V3; 3] {
    let det = ((y[1] * z[2] - z[1] * y[2]) * x[0] + (z[1] * y[0] - y[1] * z[0]) * x[2]) + (y[2] * z[0] - y[0] * z[2]) * x[1];
    let r = 1.0 / det;
    [
        [(z[2] * y[1] - z[1] * y[2]) * r, (z[1] * x[2] - z[2] * x[1]) * r, (y[2] * x[1] - y[1] * x[2]) * r],
        [(y[2] * z[0] - z[2] * y[0]) * r, (z[2] * x[0] - x[2] * z[0]) * r, (x[2] * y[0] - y[2] * x[0]) * r],
        [(z[1] * y[0] - y[1] * z[0]) * r, (x[1] * z[0] - z[1] * x[0]) * r, (y[1] * x[0] - x[1] * y[0]) * r],
    ]
}

/// The world position of texel (0, 0) and the steps of one texel along U and V (light entry, once per surface).
pub fn texel_frame(s: &Surface) -> (V3, V3, V3) {
    let inv = inverse_axes(s.u, s.v, s.n);
    let cx = [inv[0][0], inv[1][0], inv[2][0]];
    let cy = [inv[0][1], inv[1][1], inv[2][1]];
    let (px, py) = (s.pan[0], s.pan[1]);
    let o = s.origin;
    let origin = [(px * cx[0] + o[0]) + py * cy[0], (cx[1] * px + o[1]) + py * cy[1], (cx[2] * px + o[2]) + py * cy[2]];
    let us = [s.scale[0] * cx[0], s.scale[0] * cx[1], s.scale[0] * cx[2]];
    let vs = [s.scale[1] * cy[0], s.scale[1] * cy[1], s.scale[1] * cy[2]];
    (origin, us, vs)
}

/// What the light entry (10b05fa0) prepares for one light on one surface.
struct Entry {
    pos: V3,
    effect: u8,
    /// `1 / max(R, 1)`, `(1/R)^2 * 4093`, and `|distance from the plane| / R`.
    inv_r: f32,
    k: f32,
    h: f32,
    /// Texel rectangle `u0..u1`, `v0..v1`.
    rect: [i32; 4],
    /// Spot axis (the light's rotation as a direction) and cone.
    axis: V3,
    cone: u8,
    period: u8,
    phase: u8,
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The colour table of a light (light entry): per illumination 0..255 the packed 7-bit colour added to the lightmap.
fn colour_table(rgb: V3) -> Vec<u32> {
    let steps = rgb.map(|c| rint(c as f64 * 65536.0 - 0.5));
    (0..256i32)
        .map(|i| {
            let ch = |s: i32| ((i.wrapping_mul(s) >> 16).min(0x7f)) as u32;
            0xff00_0000 | ch(steps[0]) << 16 | ch(steps[1]) << 8 | ch(steps[2])
        })
        .collect()
}

/// `URender::GlobalLighting` (10b05e90) for a steady light: `FGetHSV(hue, saturation, 255)` and the brightness clamped to
/// 0..1; the entry then multiplies the colour by `Level.Brightness * brightness`.
fn steady_colour(l: &Light, level_brightness: f32) -> V3 {
    let brightness = (l.brightness as f32 * (1.0 / 255.0f32)).clamp(0.0, 1.0);
    let f = level_brightness * brightness;
    crate::hsv_to_rgb(l.hue, l.saturation, 255).map(|c| c * f)
}

fn entry(l: &Light, s: &Surface) -> Entry {
    let radius = 25.0 * (l.radius_byte as f32 + 1.0);
    let inv_r = 1.0 / if radius <= 1.0 { 1.0 } else { radius };
    let k = (inv_r * inv_r) as f64 * 4093.0;
    let rel = sub(l.pos, s.origin);
    let h = ((rel[0] * s.n[0] + rel[1] * s.n[1]) + rel[2] * s.n[2]) * inv_r;
    // the light's disc on the surface plane, in texels
    let hs = ((l.pos[0] - s.origin[0]) * s.n[0] + (l.pos[2] - s.origin[2]) * s.n[2]) + (l.pos[1] - s.origin[1]) * s.n[1];
    let r2 = ((radius as f64 * radius as f64 * 1.05 - hs as f64 * hs as f64).max(0.0)) as f32;
    let disc = fast_sqrt(r2);
    let p = [l.pos[0] - hs * s.n[0], l.pos[1] - hs * s.n[1], l.pos[2] - hs * s.n[2]];
    let (x, y) = (s.u, s.v);
    let cu = (((p[0] - s.origin[0]) * x[0] + (p[2] - s.origin[2]) * x[2]) + (p[1] - s.origin[1]) * x[1]) - s.pan[0];
    let cv = (((p[0] - s.origin[0]) * y[0] + (p[2] - s.origin[2]) * y[2]) + (p[1] - s.origin[1]) * y[1]) - s.pan[1];
    let ru = fast_sqrt((x[2] * x[2] + x[1] * x[1]) + x[0] * x[0]) * disc;
    let rv = fast_sqrt((y[2] * y[2] + y[1] * y[1]) + y[0] * y[0]) * disc;
    let u0 = rint(((cu - ru) / s.scale[0]) as f64).max(0);
    let v0 = rint(((cv - rv) / s.scale[1]) as f64).max(0);
    let u1 = rint(((ru + cu) / s.scale[0]) as f64).min(s.w as i32);
    let v1 = rint(((rv + cv) / s.scale[1]) as f64).min(s.h as i32);
    Entry { pos: l.pos, effect: if l.effect < 20 { l.effect } else { 0 }, inv_r, k: k as f32, h: h.abs(), rect: [u0, u1, v0, v1], axis: l.dir, cone: l.cone, period: l.period, phase: l.phase }
}

/// The spatial function of the light's effect: one illumination byte per texel of the rectangle into `out` (row
/// stride `w`), from the mask (row stride `ceil(w / 8) * 8`).
fn spatial(e: &Entry, s: &Surface, frame: (V3, V3, V3), mask: &[u8], out: &mut [u8], time: f32) {
    let (o, us, vs) = frame;
    let t = tables();
    let mstride = (s.w + 7) / 8 * 8;
    let [u0, u1, v0, v1] = e.rect;
    let (u0f, v0f) = (u0 as f32, v0 as f32);
    match e.effect {
        LE_SEARCHLIGHT | LE_SLOW_WAVE | LE_FAST_WAVE | LE_DISCO | LE_ROTOR => animated(e, s, frame, mask, out, time),
        LE_STATIC_SPOT | LE_SPOTLIGHT | LE_NON_INCIDENCE | LE_SHELL | LE_CYLINDER => {
            // float stepping from the rectangle's first texel
            let mut start = if matches!(e.effect, LE_STATIC_SPOT | LE_SPOTLIGHT) {
                [(v0f * vs[0] + o[0]) + u0f * us[0], (o[1] + v0f * vs[1]) + u0f * us[1], (v0f * vs[2] + o[2]) + u0f * us[2]]
            } else {
                [(vs[0] * v0f + o[0]) + us[0] * u0f, (vs[1] * v0f + o[1]) + us[1] * u0f, (v0f * vs[2] + o[2]) + u0f * us[2]]
            };
            // spot: cone cosine c, 1 / (1 - c), c / (1 - c), c^2
            let c = 1.0 - e.cone as f32 * (1.0 / 256.0);
            let r1 = 1.0 / (1.0 - c);
            let (cr, c2) = (r1 * c, c * c);
            for v in v0..v1 {
                let mut d = sub(start, e.pos);
                for u in u0..u1 {
                    let m = mask[v as usize * mstride + u as usize];
                    let o = &mut out[v as usize * s.w + u as usize];
                    *o = match e.effect {
                        LE_CYLINDER => {
                            let r = rint(((m as f32) * (1.0 - (d[0] * d[0] + d[1] * d[1]) * e.inv_r * e.inv_r)) as f64 - 0.5);
                            if r < 1 {
                                0
                            } else {
                                r as u8
                            }
                        }
                        _ if m == 0 => 0,
                        LE_NON_INCIDENCE | LE_SHELL => {
                            let d2 = (d[0] * d[0] + d[2] * d[2]) + d[1] * d[1];
                            if rint((d2 * e.k) as f64) as u32 > 0xfff {
                                0
                            } else if e.effect == LE_NON_INCIDENCE {
                                rint(m as f64 * (1.02 - (fast_sqrt(d2) * e.inv_r) as f64) - 0.5) as u8
                            } else {
                                let f = fast_sqrt(d2) * e.inv_r;
                                if f >= 1.0 || f as f64 <= 0.8 {
                                    0
                                } else {
                                    let g = ((f as f64 - 0.9) as f32).abs();
                                    rint(((m as f32) * (1.0 - g * 10.0)) as f64 - 0.5) as u8
                                }
                            }
                        }
                        _ => {
                            // spot
                            let d2 = (d[2] * d[2] + d[1] * d[1]) + d[0] * d[0];
                            let idx = rint((d2 * e.k) as f64) as u32;
                            let dot = (e.axis[0] * d[0] + e.axis[1] * d[1]) + e.axis[2] * d[2];
                            if idx >= 0x1000 || dot <= 0.0 || d2 * c2 >= dot * dot {
                                0
                            } else {
                                let tt = fast_inv_sqrt(d2) * dot * r1 - cr;
                                let val = tt * tt * t.falloff[idx as usize] * m as f32 * e.h;
                                rint(val as f64 - 0.5) as u8
                            }
                        }
                    };
                    d = [d[0] + us[0], d[1] + us[1], d[2] + us[2]];
                }
                start = [start[0] + vs[0], start[1] + vs[1], start[2] + vs[2]];
            }
        }
        _ => {
            // the default (omni) light: squared distance in fixed point by forward differences
            let k = e.k * 4096.0;
            let p = [(o[0] - e.pos[0]) + us[0] * u0f + vs[0] * v0f, (o[1] - e.pos[1]) + us[1] * u0f + vs[1] * v0f, us[2] * u0f + (o[2] - e.pos[2]) + vs[2] * v0f];
            let r = |x: f32| rint(x as f64);
            let pp = rint((((p[0] * p[0] + p[1] * p[1]) + p[2] * p[2]) * k) as f64) as u32;
            let pu = r(((p[0] * us[0] + us[1] * p[1]) + us[2] * p[2]) * k);
            let pv = r(((p[0] * vs[0] + vs[1] * p[1]) + vs[2] * p[2]) * k);
            let uu = r(((us[0] * us[0] + us[1] * us[1]) + us[2] * us[2]) * k);
            let vv = r(((vs[0] * vs[0] + vs[1] * vs[1]) + vs[2] * vs[2]) * k);
            let uv = r(((us[0] * vs[0] + vs[1] * us[1]) + vs[2] * us[2]) * k);
            let (mut row_acc, mut row_d1, row_d2) = (pp, (vv + pv * 2) as u32, (vv * 2) as u32);
            let (mut col_d1, col_d1_step, col_d2) = ((uu + pu * 2) as u32, (uv * 2) as u32, (uu * 2) as u32);
            for v in v0..v1 {
                let (mut acc, mut d1) = (row_acc, col_d1);
                for u in u0..u1 {
                    let m = mask[v as usize * mstride + u as usize];
                    out[v as usize * s.w + u as usize] = if m == 0 || acc > 0xff_ffff {
                        0
                    } else {
                        // adding 2^23 rounds to the nearest integer; the low byte is kept
                        ((m as f32 * t.falloff[(acc >> 12) as usize] * e.h + 8_388_608.0).to_bits() & 0xff) as u8
                    };
                    acc = acc.wrapping_add(d1);
                    d1 = d1.wrapping_add(col_d2);
                }
                row_acc = row_acc.wrapping_add(row_d1);
                row_d1 = row_d1.wrapping_add(row_d2);
                col_d1 = col_d1.wrapping_add(col_d1_step);
            }
        }
    }
}

/// `_ftol`: truncation toward zero, low 32 bits.
fn ftol(x: f64) -> i32 {
    (x as i64) as i32
}

/// `(cos(angle) + 1) / 2` through the engine sine table: the angle is `ftol(x * 65536 / -2pi)` rotator units.
fn half_cos(x: f64) -> f64 {
    let a = ftol(x * 65536.0 * -0.159_154_943_091_895_35);
    (sin_tab(0x4000i32.wrapping_sub(a)) as f64 + 1.0) * 0.5
}

/// The time-dependent spatial functions: Searchlight (10b03770), SlowWave (10b03de0), FastWave (10b040b0), Disco
/// (10b046f0), Rotor (10b03ae0); `time` is the level's `TimeSeconds`.
fn animated(e: &Entry, s: &Surface, frame: (V3, V3, V3), mask: &[u8], out: &mut [u8], time: f32) {
    use std::f64::consts::PI;
    let (o, us, vs) = frame;
    let t = tables();
    let mstride = (s.w + 7) / 8 * 8;
    let [u0, u1, v0, v1] = e.rect;
    let (u0f, v0f) = (u0 as f32, v0 as f32);
    let mut start = [(vs[0] * v0f + o[0]) + us[0] * u0f, (vs[1] * v0f + o[1]) + us[1] * u0f, (v0f * vs[2] + o[2]) + u0f * us[2]];
    // searchlight: its phase of the moment
    let t0 = if e.period == 0 { 0.0 } else { time as f64 * 35.0 / e.period as f64 };
    let search_off = (e.phase as f64 * 0.098_174_770_424_681_03 + t0 + 2.0 * PI) as f32;
    let byte = |v: f64| rint((v as f32) as f64 - 0.5) as u8;
    for v in v0..v1 {
        let mut d = sub(start, e.pos);
        for u in u0..u1 {
            let m = mask[v as usize * mstride + u as usize];
            let (x, y, z) = (d[0], d[1], d[2]);
            let o = &mut out[v as usize * s.w + u as usize];
            *o = if m == 0 {
                0
            } else {
                match e.effect {
                    LE_SLOW_WAVE | LE_FAST_WAVE => {
                        let d2 = (y * y + z * z) + x * x;
                        let idx = rint((d2 * e.k) as f64) as u32;
                        if idx >= 0x1000 {
                            0
                        } else {
                            let dist = ftol(fast_sqrt(d2) as f64);
                            let a = if e.effect == LE_SLOW_WAVE { ftol((dist as f64 - time as f64 * 35.0) * 1024.0) } else { ftol(((dist >> 2) as f64 - time as f64 * 35.0) * 2048.0) };
                            byte((sin_tab(a) as f64 * 0.3 + 0.7) * t.falloff[idx as usize] as f64 * m as f64 * e.h as f64)
                        }
                    }
                    _ => {
                        let (y2, x2) = (y * y, x * x);
                        let idx = rint((((z * z + y2) + x2) * e.k) as f64) as u32;
                        if idx >= 0x1000 {
                            0
                        } else {
                            let base = m as f64 * t.falloff[idx as usize] as f64;
                            match e.effect {
                                LE_SEARCHLIGHT => {
                                    // C fmod: the remainder keeps the dividend's sign
                                    let v = (((x as f64).atan2(y as f64) * 4.0 + search_off as f64) % (8.0 * PI)) as f32;
                                    if (v as f64) < PI || v as f64 > 3.0 * PI {
                                        0
                                    } else {
                                        let mut f = half_cos(v as f64);
                                        let r = (y2 + x2) as f64 * 6e-05;
                                        if r < 1.0 {
                                            f *= r;
                                        }
                                        byte(base * f * e.h as f64)
                                    }
                                }
                                LE_ROTOR => {
                                    let mut f = half_cos((x as f64).atan2(y as f64) * 6.0 + time as f64 * 3.5);
                                    let r = (y2 + x2) as f64 * 1e-4;
                                    if r < 1.0 {
                                        f = (1.0 - r) + r * f;
                                    }
                                    byte(base * f * e.h as f64)
                                }
                                _ => {
                                    // disco
                                    let a1 = ((x as f64).atan2(y as f64) * 11.0) as f32;
                                    let r2 = y2 + x2;
                                    let a2 = (fast_sqrt(r2) as f64).atan2(z as f64) * 11.0;
                                    let tt = time as f64 * 5.0;
                                    let c1 = half_cos(a1 as f64 + tt) as f32 as f64;
                                    let c2 = half_cos(a2 + tt);
                                    let mut f = (c2 + c1) - c2 * c1;
                                    let q = r2 as f64 * 5e-05;
                                    if q < 1.0 {
                                        f *= q;
                                    }
                                    byte(t.falloff[idx as usize] as f64 * (1.0 - f) * m as f64 * e.h as f64)
                                }
                            }
                        }
                    }
                }
            };
            d = [d[0] + us[0], d[1] + us[1], d[2] + us[2]];
        }
        start = [start[0] + vs[0], start[1] + vs[1], start[2] + vs[2]];
    }
}

/// A fog light of a surface as the light entry prepares it (10b05fa0 with `VolumeRadius`, `VolumeBrightness`,
/// `VolumeFog`): its position in view space, its colour of the moment (as for the lightmap) and its brightness.
#[derive(Debug, Clone, Copy)]
pub struct FogLight {
    pub view: V3,
    pub volume_radius: u8,
    pub volume_brightness: u8,
    pub volume_fog: u8,
    /// `GlobalLighting`'s brightness (0..1) and the colour `FGetHSV(hue, sat, 255) * Level.Brightness * brightness`.
    pub brightness: f32,
    pub colour: V3,
}

/// The fog colour table of a light: per value 0..255 the 7-bit colour and, in alpha, `VolumeFog / 255` in the same
/// 16.16 steps.
fn fog_table(l: &FogLight) -> Vec<[u8; 4]> {
    let a = l.volume_fog as f32 * (1.0 / 255.0f32);
    let steps = [l.colour[0], l.colour[1], l.colour[2], a].map(|c| rint((c * 65536.0) as f64 - 0.5));
    (0..256i32).map(|i| steps.map(|st| (i.wrapping_mul(st) >> 16).min(0x7f) as u8)).collect()
}

/// The fog map of a surface (`FLightManager::SetupForSurf`'s volumetric part, 10b06c90): per lightmap texel, the light
/// each fog light's volume adds along the eye ray to the texel, through the light's fog colour table; lights from the
/// farthest (view depth) to the nearest, each later one laid over (`c * (127 - a) / 127 + c'`, alpha added, at most 127).
/// `camera` is the frame's origin and axes; `skip` is the engine's persistent "ray misses" shortcut flag. RGBA, `w * h`.
pub fn fog_map(s: &Surface, camera: [V3; 4], lights: &[FogLight], skip: &mut bool) -> Vec<[u8; 4]> {
    let mut out = vec![[0u8; 4]; s.w * s.h];
    let (o, us, vs) = texel_frame(s);
    let [cam, ax, ay, az] = camera;
    let view = |v: V3, a: V3| (v[2] * a[2] + v[1] * a[1]) + v[0] * a[0];
    let d0 = sub(o, cam);
    let start = [view(d0, ax), view(d0, ay), view(d0, az)];
    let (ux, uy, uz) = (view(us, ax), view(us, ay), view(us, az));
    let (vx, vy, vz) = (view(vs, ax), view(vs, ay), view(vs, az));
    let mut order: Vec<&FogLight> = lights.iter().collect();
    order.sort_by(|a, b| b.view[2].total_cmp(&a.view[2]));
    for (k, l) in order.into_iter().enumerate() {
        let c = Chord::of(l);
        let lv = l.view;
        let table = fog_table(l);
        let mut row = start;
        for j in 0..s.h {
            let (mut x, mut y, mut z) = (row[0], row[1], row[2]);
            for i in 0..s.w {
                let dot = (x * lv[0] + y * lv[1]) + z * lv[2];
                let val = fog_chord(dot, (y * y + z * z) + x * x, &c, skip);
                let t = &mut out[j * s.w + i];
                if k == 0 {
                    *t = table[rint((val * 255.0) as f64) as usize & 0xff];
                } else if val != 0.0 {
                    let e = table[rint((val * 255.0) as f64) as usize & 0xff];
                    let a = e[3] as u32;
                    for c in 0..3 {
                        t[c] = ((t[c] as u32 * (0x7f - a) / 0x7f) + e[c] as u32).min(0x7f) as u8;
                    }
                    t[3] = (t[3] as u32 + a).min(0x7f) as u8;
                }
                x += ux;
                y += uy;
                z += uz;
            }
            row = [row[0] + vx, row[1] + vy, row[2] + vz];
        }
    }
    out
}

/// A fog light's volume as the light entry sets it up (10b05fa0): radius `25 (VolumeRadius + 1)`, its square and
/// inverses, the light's view distance squared, whether the camera is inside, and the strength
/// `VolumeBrightness * brightness / 64`.
struct Chord {
    view: V3,
    r2: f32,
    inv_r: f32,
    inv_r2: f32,
    l2: f32,
    inside: bool,
    b: f32,
}

impl Chord {
    fn of(l: &FogLight) -> Chord {
        let r = (l.volume_radius as i32 + 1) as f32 * 25.0;
        let (r2, inv_r) = (r * r, 1.0 / r);
        let lv = l.view;
        let l2 = (lv[2] * lv[2] + lv[1] * lv[1]) + lv[0] * lv[0];
        Chord { view: lv, r2, inv_r, inv_r2: inv_r * inv_r, l2, inside: l2 < r2, b: l.volume_brightness as f32 * l.brightness * 0.015625 }
    }
}

/// The fog a light's volume adds along the eye ray to a point (`dot` = light view position . point, `v2` = point
/// distance squared): the integral of `(1 - h^2/R^2)` over the chord inside the volume up to the point, clamped to
/// 0..1. `skip` is the light manager's persistent "ray misses" shortcut flag (10b4ea00), shared by surfaces and actors.
fn fog_chord(dot: f32, v2: f32, c: &Chord, skip: &mut bool) -> f32 {
    let mut val = 0.0f32;
    if c.inside || !dot.is_sign_negative() {
        if !*skip || v2 * c.l2 - dot * dot <= v2 * c.r2 {
            let h2 = c.l2 - (dot * dot) / v2;
            if h2 <= c.r2 {
                *skip = false;
                let vl = fast_sqrt(v2);
                let t = dot / vl;
                let w = fast_sqrt(c.r2 - h2);
                let d = t - vl;
                let (mut lo, mut hi, mut full) = (-w, t, false);
                if t <= w {
                    if -w <= d {
                        lo = d;
                    }
                } else {
                    hi = w;
                    if -w <= d {
                        lo = d;
                    } else {
                        full = true;
                    }
                }
                if lo < hi {
                    let a = h2 * c.inv_r2;
                    let hn = hi * c.inv_r;
                    let v = if full {
                        let v = ((3.0 - a * 3.0) - hn * hn) * hn * c.b;
                        v + v
                    } else {
                        let ln = lo * c.inv_r;
                        let a3 = 3.0 - a * 3.0;
                        ((a3 - hn * hn) * hn - (a3 - ln * ln) * ln) * c.b
                    };
                    val = if v >= 0.0 { if v.to_bits() > 0x3f7f_ffff { 1.0 } else { v } } else { 0.0 };
                }
            } else {
                *skip = true;
            }
        }
    }
    val
}

/// The fog at a point of an actor (`FLightManager::Fog`, 10b02c00): `point` in view space, `lights` the actor's fog
/// lights sorted farthest first (`SetupForActor`). Each light's chord value is doubled, then laid over the colour so
/// far: `c = (1 - a) c + v c_light` with `a = min(1, v * VolumeFog / 255)`, alpha summed; each channel at most 1.
/// Without `PF_RenderFog` the engine returns zero. RGBA.
pub fn fog_at(point: V3, lights: &[FogLight], skip: &mut bool) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    let clamp1 = |x: f32| if x.to_bits() > 0x3f7f_ffff { 1.0 } else { x };
    for l in lights {
        let c = Chord::of(l);
        let lv = c.view;
        let dot = (lv[0] * point[0] + lv[1] * point[1]) + lv[2] * point[2];
        let v2 = (point[0] * point[0] + point[1] * point[1]) + point[2] * point[2];
        let v = fog_chord(dot, v2, &c, skip);
        let v = v + v;
        if v == 0.0 {
            continue;
        }
        let fa = v * (l.volume_fog as f32 * (1.0 / 255.0f32));
        let a = if fa.to_bits() < 0x3f80_0000 { fa } else { 1.0 };
        let inv = 1.0 - a;
        for k in 0..3 {
            out[k] = clamp1(inv * out[k] + v * l.colour[k]);
        }
        out[3] = clamp1(out[3] + a);
    }
    out
}

/// The ambient start value of every texel: the zone's `FGetHSV(AmbientHue, AmbientSaturation, AmbientBrightness)` per
/// channel as `round(64 c - 0.5)`, 0..255.
pub fn ambient_texel(rgb: V3) -> [u8; 3] {
    rgb.map(|c| rint(((c * 0.25) * 256.0) as f64 - 0.5).clamp(0, 255) as u8)
}

/// How the light manager uses a light on a level surface ("add light" 10b08b30 with no mover).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightKind {
    /// Not a light here (no type, no brightness, `OmniBumpMap`).
    None,
    /// Baked into the cached static map.
    Static,
    /// Static or no-delete but animated (type or effect) or not static: merged every frame with its shadow bits.
    PerFrame,
    /// Any other light (`bDynamicLight` included): merged every frame without shadows.
    Dynamic,
}

pub fn light_kind(l: &Light) -> LightKind {
    let e = if l.effect < 20 { l.effect as usize } else { 0 };
    if l.light_type == 0 || l.brightness == 0 || l.effect == LE_OMNI_BUMP_MAP {
        return LightKind::None;
    }
    if l.flags & F_DYNAMIC_LIGHT == 0 && l.flags & (F_STATIC | F_NO_DELETE) != 0 {
        if l.flags & F_STATIC == 0 || l.light_type != 1 || EFFECT_ANIMATED[e] || EFFECT_WAVER[e] {
            LightKind::PerFrame
        } else {
            LightKind::Static
        }
    } else {
        LightKind::Dynamic
    }
}

/// Does a light of a surface's stored list go into the cached static lightmap (`FLightManager` "add light", 10b08b30)?
/// Others are either redone every frame (animated or not static) or not lights at all.
pub fn is_static_light(l: &Light) -> bool {
    let e = if l.effect < 20 { l.effect as usize } else { 0 };
    l.light_type != 0 && l.brightness != 0 && l.effect != LE_OMNI_BUMP_MAP && l.flags & F_DYNAMIC_LIGHT == 0 && l.flags & F_STATIC != 0 && l.light_type == 1 && !EFFECT_ANIMATED[e] && !EFFECT_WAVER[e]
}

/// One surface's lightmap: `lights` with their shadow bits (`None`: unshadowed), ambient start value; texels as RGB
/// bytes (0..127 from the lights; the ambient can start higher), row-major `w * h`.
pub fn surface_lightmap(s: &Surface, lights: &[(&Light, Option<&[u8]>)], ambient: [u8; 3], level_brightness: f32) -> Vec<[u8; 3]> {
    let start = 0x7f00_0000 | (ambient[0] as u32) << 16 | (ambient[1] as u32) << 8 | ambient[2] as u32;
    let mut lm = vec![start; s.w * s.h];
    if s.w == 0 || s.h == 0 {
        return Vec::new();
    }
    for (l, bits) in lights.iter().take(MAX_LIGHTS) {
        let lit = illumination(l, s, *bits);
        merge_light(&lit, s, steady_colour(l, level_brightness), None, &mut lm);
    }
    unpack(&lm)
}

fn unpack(lm: &[u32]) -> Vec<[u8; 3]> {
    lm.iter().map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8]).collect()
}

/// A light's illumination of a surface (entry geometry, mask, spatial function): what the engine caches for lights
/// it merges again every frame.
#[derive(Clone)]
pub struct Illumination {
    rect: [i32; 4],
    effect: u8,
    bytes: Vec<u8>,
}

pub fn illumination(l: &Light, s: &Surface, bits: Option<&[u8]>) -> Illumination {
    illumination_at(l, s, bits, 0.0)
}

/// The illumination at level time `time` (only the time-dependent effects depend on it).
pub fn illumination_at(l: &Light, s: &Surface, bits: Option<&[u8]>, time: f32) -> Illumination {
    let e = entry(l, s);
    let mask = mask_buffer(bits, s.w, s.h);
    let mut bytes = vec![0u8; s.w * s.h];
    spatial(&e, s, texel_frame(s), &mask, &mut bytes, time);
    Illumination { rect: e.rect, effect: e.effect, bytes }
}

/// The merge of one light into a lightmap (10b03040) with its colour (`FGetHSV(hue, saturation, 255) * Level.Brightness
/// * brightness`). Waver effects first shake each illumination byte with the frame randoms: torch `0.95 + 0.05 a`, fire
/// `0.8 + 0.2 a`, water `0.6 + 0.4 b`, one random per texel of the rectangle in order.
pub fn merge_light(lit: &Illumination, s: &Surface, colour: V3, randoms: Option<&ue_assets::light::GlobalRandoms>, lm: &mut [u32]) {
    let table = colour_table(colour);
    let [u0, u1, v0, v1] = lit.rect;
    let waver = match (lit.effect, randoms) {
        (1, Some(r)) => Some((&r.a, 0.05, 0.95)),
        (2, Some(r)) => Some((&r.a, 0.2, 0.8)),
        (3, Some(r)) => Some((&r.b, 0.4, 0.6)),
        _ => None,
    };
    let mut counter = 0u32;
    for v in v0..v1 {
        for u in u0..u1 {
            let i = v as usize * s.w + u as usize;
            let mut b = lit.bytes[i];
            if let Some((t, k, c)) = waver {
                let f = t[(counter & 0xff) as usize] as f64 * k + c;
                counter = counter.wrapping_add(1);
                b = rint(((b as f64) * f) as f32 as f64 - 0.5) as u8;
            }
            let sum = lm[i].wrapping_add(table[b as usize]);
            let over = sum & 0x8080_8080;
            lm[i] = if over != 0 { (sum & 0x7f7f_7f7f) | over.wrapping_sub(over >> 7) } else { sum };
        }
    }
}

/// The per-frame lightmap of a surface: its static texels plus every per-frame light with its illumination and current
/// colour (`SetupForSurf`'s dynamic map).
pub fn frame_lightmap(s: &Surface, static_texels: &[[u8; 3]], lights: &[(&Illumination, V3)], randoms: &ue_assets::light::GlobalRandoms) -> Vec<[u8; 3]> {
    let mut lm: Vec<u32> = static_texels.iter().map(|t| 0x7f00_0000 | (t[0] as u32) << 16 | (t[1] as u32) << 8 | t[2] as u32).collect();
    for (lit, colour) in lights {
        merge_light(lit, s, *colour, Some(randoms), &mut lm);
    }
    unpack(&lm)
}

/// A light's colour with brightness `b` (after `GlobalLighting`): `FGetHSV(hue, saturation, 255) * Level.Brightness * b`.
pub fn light_colour(l: &Light, b: f32, level_brightness: f32) -> V3 {
    let f = level_brightness * b;
    crate::hsv_to_rgb(l.hue, l.saturation, 255).map(|c| c * f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_fog_behind_a_volume_takes_its_colour_and_a_nearer_one_lays_over() {
        // radius 25 * 4 = 100, 1000 ahead of the camera; strength 8 * 1 / 64 = 1/8: a ray through the centre gets
        // 4 b = 0.5, doubled for actors to 1.0
        let far = FogLight { view: [0.0, 0.0, 1000.0], volume_radius: 3, volume_brightness: 8, volume_fog: 255, brightness: 1.0, colour: [0.5, 0.25, 0.125] };
        let mut skip = false;
        assert_eq!(fog_at([0.0, 0.0, 2000.0], &[far], &mut skip), [0.5, 0.25, 0.125, 1.0]);
        // a point in front of the volume gets nothing
        assert_eq!(fog_at([0.0, 0.0, 500.0], &[far], &mut skip), [0.0; 4]);
        // half the fog amount: alpha 0.5; a nearer full light hides the farther colour completely
        let half = FogLight { volume_fog: 128, ..far };
        let f = fog_at([0.0, 0.0, 2000.0], &[half], &mut skip);
        assert!((f[3] - 128.0 / 255.0).abs() < 1e-6);
        let near = FogLight { view: [0.0, 0.0, 600.0], colour: [0.0, 1.0, 0.0], ..far };
        assert_eq!(fog_at([0.0, 0.0, 2000.0], &[far, near], &mut skip), [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn fast_sqrt_is_close_and_exact_on_powers_of_four() {
        assert_eq!(fast_sqrt(4.0), 2.0);
        assert_eq!(fast_sqrt(1.0), 1.0);
        for x in [2.0f32, 3.0, 10.0, 1234.5, 0.37] {
            assert!((fast_sqrt(x) - x.sqrt()).abs() / x.sqrt() < 2e-3, "{x}");
            assert!((fast_inv_sqrt(x) - 1.0 / x.sqrt()).abs() * x.sqrt() < 2e-3, "{x}");
        }
    }

    #[test]
    fn falloff_table_is_the_smoothstep_over_s() {
        let t = tables();
        for i in [0usize, 100, 2047, 4095] {
            let s = ((i + 1) as f64 / 4096.0).sqrt();
            let want = (1.0 - 3.0 * s * s + 2.0 * s * s * s) / s;
            assert!((t.falloff[i] as f64 - want).abs() < 1e-5 * want.abs().max(1.0));
        }
        assert!(t.falloff[4095].abs() < 1e-6);
    }

    #[test]
    fn mask_of_fully_lit_and_dark_bits() {
        // 10 x 3 texels: two bytes per row
        let lit = vec![0xffu8; 2 * 3];
        let m = mask_buffer(Some(&lit), 10, 3);
        assert_eq!(m.len(), 16 * 3);
        // per kernel row floor(part * 255 / 320): 70 + 114 + 70
        assert_eq!(m[16 + 4], 254);
        assert!(m[..10].iter().all(|&v| v == 254), "edges repeat: {:?}", &m[..10]);
        let dark = vec![0u8; 6];
        assert!(mask_buffer(Some(&dark), 10, 3).iter().all(|&v| v == 0));
        assert!(mask_buffer(None, 10, 3).iter().all(|&v| v == 127));
        // a single lit texel spreads by the kernel: centre 64, sides 40, corners 24 (each * 255 / 320, floored)
        let mut one = vec![0u8; 6];
        one[2] = 0b0000_1000; // row 1, texel 3
        let m = mask_buffer(Some(&one), 10, 3);
        assert_eq!((m[16 + 3], m[16 + 2], m[3], m[2]), ((64 * 255 / 320) as u8, (40 * 255 / 320) as u8, (40 * 255 / 320) as u8, (24 * 255 / 320) as u8));
    }

    #[test]
    fn colour_table_steps_in_16_16_and_saturates() {
        let t = colour_table([1.0, 0.5, 0.0]);
        assert_eq!(t[0] & 0xffffff, 0);
        // full red: step round-even(65536 - 0.5) = 65536, so illumination i gives i, capped at 127
        assert_eq!((t[100] >> 16) & 0xff, 100);
        assert_eq!((t[254] >> 16) & 0xff, 127);
        assert_eq!((t[100] >> 8) & 0xff, 50);
    }

    #[test]
    fn texel_frame_inverts_the_texture_axes() {
        let s = Surface { origin: [10.0, 20.0, 30.0], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0], n: [0.0, 0.0, 1.0], pan: [4.0, 8.0], scale: [16.0, 16.0], w: 4, h: 4 };
        let (o, us, vs) = texel_frame(&s);
        assert_eq!(o, [14.0, 28.0, 30.0]);
        assert_eq!((us, vs), ([16.0, 0.0, 0.0], [0.0, 16.0, 0.0]));
    }

    #[test]
    fn merging_a_light_every_frame_equals_baking_it() {
        let s = Surface { origin: [0.0; 3], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0], n: [0.0, 0.0, 1.0], pan: [0.0, 0.0], scale: [16.0, 16.0], w: 9, h: 7 };
        let l1 = Light { pos: [48.0, 48.0, 64.0], brightness: 200, hue: 30, saturation: 100, radius_byte: 10, light_type: 1, effect: 0, cone: 0, rotation: [0; 3], dir: [1.0, 0.0, 0.0], flags: F_STATIC, period: 0, phase: 0, special_lit: false };
        let l2 = Light { pos: [100.0, 20.0, 40.0], brightness: 255, hue: 150, saturation: 50, radius_byte: 8, light_type: 2, effect: 13, ..l1 };
        let bits = vec![0b1011_0110u8; 2 * 7];
        let both = surface_lightmap(&s, &[(&l1, Some(&bits)), (&l2, Some(&bits))], [3, 4, 5], 0.9);
        let base = surface_lightmap(&s, &[(&l1, Some(&bits))], [3, 4, 5], 0.9);
        let lit = illumination(&l2, &s, Some(&bits));
        let r = ue_assets::light::GlobalRandoms::default();
        let per_frame = frame_lightmap(&s, &base, &[(&lit, light_colour(&l2, 1.0, 0.9))], &r);
        assert_eq!(both, per_frame);
    }

    /// The time-dependent effects against plain formulas: a floor 12 x 12 texels of 16 units, light 48 above the
    /// middle, unshadowed (mask 127).
    #[test]
    fn time_dependent_effects_follow_their_formulas() {
        use std::f64::consts::PI;
        let s = Surface { origin: [0.0; 3], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0], n: [0.0, 0.0, 1.0], pan: [0.0, 0.0], scale: [16.0, 16.0], w: 12, h: 12 };
        let base = Light { pos: [96.0, 96.0, 48.0], brightness: 255, hue: 0, saturation: 255, radius_byte: 12, light_type: 1, effect: 0, cone: 0, rotation: [0; 3], dir: [1.0, 0.0, 0.0], flags: 0, period: 40, phase: 10, special_lit: false };
        let r = 25.0 * 13.0;
        let at = |i: usize, j: usize| [i as f64 * 16.0 - 96.0, j as f64 * 16.0 - 96.0, -48.0f64];
        let falloff = |d: [f64; 3]| {
            let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            let sq = d2 / (r * r);
            let s = sq.sqrt();
            (1.0 - 3.0 * sq + 2.0 * sq * s) * (48.0 / r) / s * 127.0
        };
        let hc = |a: f64| (a.cos() + 1.0) * 0.5;
        let mut checked = 0;
        for (effect, time) in [(5u8, 3.0f32), (6, 3.0), (18, 2.5), (10, 1.25), (4, 0.7)] {
            let l = Light { effect, ..base };
            let lit = illumination_at(&l, &s, None, time);
            let t = time as f64;
            for (i, j) in [(2usize, 3usize), (5, 9), (8, 8), (10, 1), (7, 4)] {
                let d = at(i, j);
                let (x, y, z) = (d[0], d[1], d[2]);
                let dist = (x * x + y * y + z * z).sqrt();
                let want = match effect {
                    5 => ((dist.trunc() - t * 35.0) * 1024.0 * PI / 32768.0).sin() * 0.3 + 0.7,
                    6 => ((((dist.trunc() as i64) >> 2) as f64 - t * 35.0) * 2048.0 * PI / 32768.0).sin() * 0.3 + 0.7,
                    18 => {
                        let f = hc(x.atan2(y) * 6.0 + t * 3.5);
                        let q = (x * x + y * y) * 1e-4;
                        if q < 1.0 { (1.0 - q) + q * f } else { f }
                    }
                    10 => {
                        let c1 = hc(x.atan2(y) * 11.0 + t * 5.0);
                        let c2 = hc((x * x + y * y).sqrt().atan2(z) * 11.0 + t * 5.0);
                        let mut f = c1 + c2 - c1 * c2;
                        let q = (x * x + y * y) * 5e-5;
                        if q < 1.0 {
                            f *= q;
                        }
                        1.0 - f
                    }
                    _ => {
                        let v = (x.atan2(y) * 4.0 + 10.0 * 2.0 * PI / 64.0 + t * 35.0 / 40.0 + 2.0 * PI) % (8.0 * PI);
                        if v < PI || v > 3.0 * PI {
                            0.0
                        } else {
                            let f = hc(v);
                            let q = (x * x + y * y) * 6e-5;
                            if q < 1.0 { f * q } else { f }
                        }
                    }
                } * falloff(d);
                let got = lit.bytes[j * 12 + i] as f64;
                assert!((got - want).abs() <= 2.5, "effect {effect} texel ({i}, {j}): {got} vs {want:.2}");
                checked += 1;
            }
            // time moves the pattern
            if effect != 4 {
                assert_ne!(lit.bytes, illumination_at(&l, &s, None, time + 0.37).bytes, "effect {effect} is still");
            }
        }
        assert_eq!(checked, 25);
    }

    /// A fog light halfway between the camera and a wall: rays through its whole volume give `4 b (1 - h^2/R^2)^1.5`
    /// (`h` the ray's distance from the light), through the light's fog table; outside the volume nothing.
    #[test]
    fn fog_through_a_whole_volume_follows_the_chord_integral() {
        // wall in the plane x = 600 facing the camera at the origin looking along +x; texels 16 units, 40 x 40
        let s = Surface { origin: [600.0, -320.0, -320.0], u: [0.0, 1.0, 0.0], v: [0.0, 0.0, 1.0], n: [-1.0, 0.0, 0.0], pan: [0.0, 0.0], scale: [16.0, 16.0], w: 40, h: 40 };
        // camera axes: x right = +y, y down = -z, z forward = +x
        let cam = [[0.0; 3], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]];
        // light at (300, 0, 0): view (0, 0, 300); VolumeRadius 5 -> R = 150
        let l = FogLight { view: [0.0, 0.0, 300.0], volume_radius: 5, volume_brightness: 8, volume_fog: 255, brightness: 1.0, colour: [1.0, 1.0, 1.0] };
        let mut skip = false;
        let fog = fog_map(&s, cam, &[l], &mut skip);
        let b = 8.0 * 0.015625;
        let r = 150.0f64;
        for (i, j) in [(20usize, 20usize), (24, 20), (20, 26), (27, 25), (0, 0), (39, 39)] {
            let p = [600.0f64, -320.0 + 16.0 * i as f64, -320.0 + 16.0 * j as f64];
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            let t = 300.0 * p[0] / len;
            let h2 = 300.0f64 * 300.0 - t * t;
            let val = if h2 < r * r { (4.0 * b * (1.0 - h2 / (r * r)).powf(1.5)).min(1.0) } else { 0.0 };
            let idx = (val * 255.0).round();
            let want = (idx * 65536.0 / 65536.0).min(127.0);
            let got = fog[j * 40 + i][0] as f64;
            assert!((got - want).abs() <= 2.0, "texel ({i}, {j}): {got} vs {want}");
            assert_eq!(fog[j * 40 + i][3], fog[j * 40 + i][0], "white light, full fog: alpha follows");
        }
        assert!(fog[20 * 40 + 20][0] > 100 && fog[0][0] == 0);
    }

    #[test]
    fn an_omni_light_over_a_floor() {
        // floor 8 x 8 texels of 16 units, light 64 above texel (3, 3), radius 25 * 11 = 275, unshadowed
        let s = Surface { origin: [0.0; 3], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0], n: [0.0, 0.0, 1.0], pan: [0.0, 0.0], scale: [16.0, 16.0], w: 8, h: 8 };
        let l = Light { pos: [48.0, 48.0, 64.0], brightness: 255, hue: 0, saturation: 255, radius_byte: 10, light_type: 1, effect: 0, cone: 0, rotation: [0; 3], dir: [1.0, 0.0, 0.0], flags: F_STATIC, period: 0, phase: 0, special_lit: false };
        let lm = surface_lightmap(&s, &[(&l, None)], [0; 3], 1.0);
        // under the light: mask 127 * (smoothstep(64/275) * 64/64) -> about 116; colour white 0.82: about 95
        let c = lm[3 * 8 + 3][0] as f32;
        let sm = 64.0f32 / 275.0;
        let want = (127.0 * (1.0 - 3.0 * sm * sm + 2.0 * sm * sm * sm)).round() * crate::hsv_to_rgb(0, 255, 255)[0];
        assert!((c - want).abs() <= 2.0, "{c} vs {want}");
        // white light: equal channels; farther texels darker
        assert_eq!(lm[3 * 8 + 3][0], lm[3 * 8 + 3][2]);
        assert!(lm[7 * 8 + 7][0] < lm[3 * 8 + 3][0]);
    }
}

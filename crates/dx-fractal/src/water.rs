//! Water ripples (`WaterTexture` and its subclasses `WetTexture`, `WaveTexture`). Spec: `docs/formats/water-texture.md`.
//!
//! The water is a height field half as wide as the texture and as tall, on a torus, with odd rows shifted half a cell
//! to the right (a hexagonal grid). Each step updates every other row (alternating) with a damped wave equation, and
//! turns the slopes of the rows around it into two rows of pixels through a 1024-entry table (`render_table`): signed
//! horizontal offsets for a wet texture, light levels for a wave texture.

use crate::Shared;

/// One drop: something that disturbs the water every step (`WaterTexture.ADrop`). The engine keeps per-drop state in
/// the drop itself (phases, counters, a wandering position), so these change as the water runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Drop {
    pub kind: u8,
    pub depth: u8,
    pub x: u8,
    pub y: u8,
    pub a: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
}

pub struct Water {
    u: usize,
    v: usize,
    u_bits: u32,
    /// Heights, `u/2` cells by `v` rows, 128 = level.
    field: Vec<u8>,
    /// The texture's pixels: refraction values or light levels after a step, `u` by `v`.
    pub bitmap: Vec<u8>,
    /// Pixel value for a slope `k` (-512..511) at index `k + 512`.
    pub render_table: Vec<u8>,
    /// New height for `k` = (sum of the 4 neighbours - 2 * old height) at index `k + 512`.
    water_table: Vec<u8>,
    parity: u8,
    pub drops: Vec<Drop>,
    /// Scratch for `pass`: a new row of heights, and the slopes of a pair of rows.
    row: Vec<u8>,
    slopes: Vec<(i32, i32)>,
}

impl Water {
    /// `u`, `v`: powers of two, at least 8 (the engine never steps smaller textures).
    pub fn new(u: usize, v: usize, drops: Vec<Drop>) -> Water {
        assert!(u.is_power_of_two() && v.is_power_of_two() && u >= 2 && v >= 2);
        // half of k, rounded towards the level of 128 (up below 256, down above), clamped to a byte
        let water_table = (0..1536i32)
            .map(|j| {
                let k = j - 512;
                ((k < 256) as i32 + (j / 2) - 256).clamp(0, 255) as u8
            })
            .collect();
        Water {
            u,
            v,
            u_bits: u.trailing_zeros(),
            field: vec![0x80; u * v / 2],
            bitmap: vec![0; u * v],
            render_table: vec![0; 1028],
            water_table,
            parity: 0,
            row: Vec::new(),
            slopes: Vec::new(),
            drops,
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.u, self.v)
    }

    /// The height field (`u/2` by `v`), for tests and tools.
    pub fn field(&self) -> &[u8] {
        &self.field
    }

    pub fn field_mut(&mut self) -> &mut [u8] {
        &mut self.field
    }

    /// Wet texture: slopes become horizontal pixel offsets, `trunc((k + 1) * WaveAmp / 512)` clamped to -128..127.
    pub fn set_refraction(&mut self, wave_amp: u8) {
        for i in 0..1024 {
            let k = i as f64 - 511.0;
            let v = (k * wave_amp as f64 * 0.001953125).trunc() as i32;
            self.render_table[i] = v.clamp(-128, 127) as i8 as u8;
        }
    }

    /// Wave texture: slopes become light levels, a diffuse term (cosine between the surface tilt and the light) plus a
    /// highlight inside a narrow band (`PhongSize`), as `UWaveTexture::SetWaveLight` computes them.
    pub fn set_wave_light(&mut self, wave_amp: u8, light: u8, angle: u8, phong_range: u8, phong_size: u8) {
        const UNIT: f64 = 0.012319971190548208; // the byte angles' scale (about 2pi / 510)
        let light_angle = (light as f64 * UNIT) as f32 as f64;
        let bump_angle = (angle as f64 * UNIT) as f32 as f64;
        let amp = wave_amp as f64 * (1.0f32 / 255.0) as f64;
        for i in 0..1024 {
            let x = amp * (512.0 - i as f64) * (1.0f32 / 196.0) as f64;
            // the tilt: x / (|x| + 1) scaled to just under a quarter turn, around a quarter turn
            let tilt = x * 1.5707499980926514 / (x.abs() + 1.0) + std::f64::consts::FRAC_PI_2;
            let diffuse = ((256 - (phong_range as i32 >> 1)) as f64 * (tilt - light_angle).cos()).trunc() as i32;
            let size = phong_size as f64 * 0.001953125;
            let tilt32 = tilt as f32 as f64;
            let d = tilt32 + tilt32 - light_angle - bump_angle;
            let mut level = diffuse;
            if d * d < size * size {
                level += ((size - d.abs()) * (2 * phong_range as i32) as f64 / size).trunc() as i32;
            }
            self.render_table[i] = level.clamp(0, 255) as u8;
        }
    }

    /// Draws every drop into the height field (`UWaterTexture::WaterRedrawDrops`).
    pub fn redraw_drops(&mut self, shared: &mut Shared) {
        let xm = (((self.u - 1) >> 1) & 0xff) as u32;
        let ym = (((self.v - 1) >> 1) & 0xff) as u32;
        let w = self.u / 2;
        let ub = self.u_bits;
        let len = self.field.len();
        let field = &mut self.field;
        // a drop at (x, y) sets the cell in row 2y and the one below it, in row 2y + 1
        let mut set = |cell: u32, top: u8, bottom: u8| {
            let c = cell as usize % len;
            field[c] = top;
            field[(c + w) % len] = bottom;
        };
        let sine = &shared.sine;
        let rng = &mut shared.rng;
        for d in self.drops.iter_mut() {
            let (x, y) = (d.x as u32, d.y as u32);
            let row = y << ub;
            let at = row + x;
            match d.kind {
                // fixed depth
                0 => set(at, d.d, d.d),
                // phased spots: the depth byte is the phase, D its speed
                1..=3 => {
                    d.depth = d.depth.wrapping_add(d.d);
                    let s = sine[d.depth as usize];
                    let v = match d.kind {
                        1 => s,
                        2 => (s >> 1) + 0x40,
                        _ => s.max(0x80),
                    };
                    set(at, v, v);
                }
                // random mover: one step of -3..3 in x and y, marks where it was
                4 => {
                    let (r1, r2) = (rng.next() as u8, rng.next() as u8);
                    d.x = ((r2 & 3).wrapping_sub(r1 & 3).wrapping_add(d.x) as u32 & xm) as u8;
                    let (r3, r4) = (rng.next() as u8, rng.next() as u8);
                    d.y = ((r4 & 3).wrapping_sub(r3 & 3).wrapping_add(d.y) as u32 & ym) as u8;
                    set(at, 0xb9, 0x47);
                }
                // fixed spot with random heights
                5 => {
                    let top = rng.next() as u8;
                    let bottom = rng.next() as u8;
                    set(at, top, bottom);
                }
                // whirls: (B,A) is a 16-bit angle turning by (D,C) per step, forwards (6, 7) or backwards (64, 65);
                // small circles (radius 0..15) or big ones (0..31)
                6 | 7 | 0x40 | 0x41 => {
                    let angle = u16::from_le_bytes([d.a, d.b]);
                    let speed = u16::from_le_bytes([d.c, d.d]);
                    let angle = if d.kind < 0x40 { angle.wrapping_add(speed) } else { angle.wrapping_sub(speed) };
                    [d.a, d.b] = angle.to_le_bytes();
                    let shift = if d.kind == 6 || d.kind == 0x40 { 4 } else { 3 };
                    let hi = d.b as usize;
                    let px = ((sine[hi] >> shift) as u32 + x) & xm;
                    let py = (sine[(hi + 0x40) & 0xff] >> shift) as u32;
                    set(px + (((py + y) & ym) << ub), d.depth, d.depth);
                }
                // lines of (D/2 + 1) cells: horizontal, vertical, '/', '\'; 12-15 the same with a phased height
                8..=15 => {
                    let v = if d.kind >= 12 {
                        d.depth = d.depth.wrapping_add(d.c);
                        sine[d.depth as usize]
                    } else {
                        d.depth
                    };
                    let n = (d.d >> 1) as u32 + 1;
                    for i in 0..n {
                        let cell = match d.kind & 3 {
                            0 => row + ((x + i) & xm),
                            1 => (((y + i) & ym) << ub) + x,
                            2 => (((y + i) & ym) << ub) + (x.wrapping_sub(i) & xm),
                            _ => (((y + i) & ym) << ub) + ((x + i) & xm),
                        };
                        set(cell, v, v);
                    }
                }
                // rain: one step in 16, a drop somewhere within D of the spot (the cell below gets the inverse)
                16 => {
                    if rng.next() & 15 == 0 {
                        let radius = d.d as u32;
                        let ry = rng.next() & 0xff;
                        let rx = rng.next() & 0xff;
                        let cell = ((((ry * radius) >> 8) + y) & ym) << ub;
                        set(cell + ((((rx * radius) >> 8) + x) & xm), d.depth, !d.depth);
                    }
                }
                // area clamp: a (D/2) square held at the depth
                17 => {
                    let s = (d.d >> 1) as u32;
                    for r in 0..s {
                        for c in 0..s {
                            set(((x + c) & xm) + (((y + r) & ym) << ub), d.depth, d.depth);
                        }
                    }
                }
                // taps: a counter A grows by D each step; when it wraps, a drip (the drippy tap restarts at random)
                18 | 19 => {
                    d.a = d.a.wrapping_add(d.d);
                    if d.a <= d.d {
                        if d.kind == 19 {
                            d.a = rng.next() as u8;
                        }
                        set(at, d.depth, !d.depth);
                    }
                }
                _ => {}
            }
        }
    }

    /// One step of the water (`UWaterTexture::CalculateWater`): the rows of one parity get new heights, and every pixel
    /// of the bitmap is redrawn from the slopes.
    pub fn calculate(&mut self) {
        self.parity = self.parity.wrapping_add(1);
        self.pass(self.parity & 1 == 1);
    }

    /// The half of a step that updates the odd rows (`true`) or the even rows.
    pub fn pass(&mut self, odd_rows: bool) {
        // sizes are powers of two (`new`): wrapping is a mask. Row y reads only rows y - 1 and y + 1 (the other parity)
        // and itself, so each new row can go into the field as soon as it is made.
        let (u, v, w) = (self.u, self.v, self.u / 2);
        let (um, vm, wm) = (u - 1, v - 1, w - 1);
        let odd = odd_rows as usize;
        // even rows lean left (neighbours x-1, x above and below), odd rows right (x, x+1)
        let lean = if odd == 0 { wm } else { 1 };
        // pixel columns: slope of the row below at x + base and x + base - 2
        let base = odd;
        let mut row = std::mem::take(&mut self.row);
        row.resize(w, 0);
        let mut g = std::mem::take(&mut self.slopes);
        g.resize(w, (0, 0));
        for y in (odd..v).step_by(2) {
            let (up, down) = ((y + vm) & vm, (y + 1) & vm);
            {
                let f = &self.field;
                let (fu, fd, fy) = (&f[up * w..up * w + w], &f[down * w..down * w + w], &f[y * w..y * w + w]);
                for x in 0..w {
                    let xl = (x + lean) & wm;
                    let sum = fu[x] as i32 + fd[x] as i32 + fu[xl] as i32 + fd[xl] as i32 - 2 * fy[x] as i32;
                    row[x] = self.water_table[(512 + sum) as usize];
                }
                // gx(x) for the row below and gy(x) (average with the row above), each for x in 0..w
                for x in 0..w {
                    let (c, l) = ((x + base) & wm, (x + base + wm - 1) & wm);
                    let gx = fd[c] as i32 - fd[l] as i32;
                    let gy = (gx + fu[c] as i32 - fu[l] as i32) >> 1;
                    g[x] = (gx, gy);
                }
            }
            let r = |k: i32| self.render_table[(512 + k) as usize];
            let (row_y, row_up) = (y * u, up * u);
            for x in 0..w {
                let (gx, gy) = g[x];
                let (gxl, gyl) = g[(x + wm) & wm];
                let c0 = (2 * x + base + um) & um;
                let c1 = (2 * x + base) & um;
                self.bitmap[row_y + c0] = r(gx + gxl);
                self.bitmap[row_y + c1] = r(2 * gx);
                self.bitmap[row_up + c0] = r(gy + gyl);
                self.bitmap[row_up + c1] = r(2 * gy);
            }
            self.field[y * w..y * w + w].copy_from_slice(&row);
        }
        self.row = row;
        self.slopes = g;
    }

    /// Wet texture: replaces each pixel (a signed offset after `calculate`) with the source pixel that many places to
    /// its right in the same row, wrapping (`UWetTexture::ApplyWetTexture`). `source` is `u` by `v`.
    pub fn apply_wet(&mut self, source: &[u8]) {
        let (u, v) = (self.u, self.v);
        let mask = (u - 1) as isize;
        for y in 0..v {
            let src = &source[y * u..(y + 1) * u];
            let row = &mut self.bitmap[y * u..(y + 1) * u];
            for (x, p) in row.iter_mut().enumerate() {
                *p = src[((*p as i8 as isize + x as isize) & mask) as usize];
            }
        }
    }
}

/// A wet texture's source picture at the wet texture's size: the source's own pixels when the sizes match, else
/// enlarged by repeating pixels. `None` when the source is larger in either direction (the engine then drops it and
/// the wet texture stays still).
pub fn wet_source(u: usize, v: usize, src_u: usize, src_v: usize, src: &[u8]) -> Option<Vec<u8>> {
    if src_u > u || src_v > v || src.len() < src_u * src_v {
        return None;
    }
    let (du, dv) = ((u / src_u).trailing_zeros(), (v / src_v).trailing_zeros());
    let src_bits = src_u.trailing_zeros();
    let mut out = vec![0u8; u * v];
    for y in 0..v {
        for x in 0..u {
            out[y * u + x] = src[((y >> dv) << src_bits) + (x >> du)];
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pass as first written (wrapping by `rem_euclid` and `%`, the new rows applied after all are made): the
    /// faster `pass` must give the same field and bitmap.
    fn pass_reference(wt: &mut Water, odd_rows: bool) {
        let (u, v, w) = (wt.u, wt.v, wt.u / 2);
        let odd = odd_rows as u8;
        let f = wt.field.clone();
        let at = |y: usize, x: isize| f[(y % v) * w + x.rem_euclid(w as isize) as usize] as i32;
        let r = |k: i32| wt.render_table[(512 + k) as usize];
        let t = |k: i32| wt.water_table[(512 + k) as usize];
        let mut new_rows: Vec<(usize, Vec<u8>)> = Vec::new();
        let lean: isize = if odd == 0 { -1 } else { 1 };
        let mut bitmap = wt.bitmap.clone();
        for y in (odd as usize..v).step_by(2) {
            let (up, down) = (y + v - 1, y + 1);
            let row: Vec<u8> = (0..w as isize).map(|x| t(at(up, x) + at(down, x) + at(up, x + lean) + at(down, x + lean) - 2 * at(y, x))).collect();
            let base = if odd == 0 { 0isize } else { 1 };
            let gx = |x: isize| at(down, x + base) - at(down, x + base - 2);
            let gy = |x: isize| (gx(x) + at(up, x + base) - at(up, x + base - 2)) >> 1;
            let prev_row = (y + v - 1) % v;
            for x in 0..w as isize {
                let c0 = ((2 * x + base - 1).rem_euclid(u as isize)) as usize;
                let c1 = ((2 * x + base) as usize) % u;
                bitmap[y * u + c0] = r(gx(x) + gx(x - 1));
                bitmap[y * u + c1] = r(2 * gx(x));
                bitmap[prev_row * u + c0] = r(gy(x) + gy(x - 1));
                bitmap[prev_row * u + c1] = r(2 * gy(x));
            }
            new_rows.push((y, row));
        }
        wt.bitmap = bitmap;
        for (y, row) in new_rows {
            wt.field[y * w..(y + 1) * w].copy_from_slice(&row);
        }
    }

    #[test]
    fn the_pass_matches_its_first_form() {
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        for (u, v) in [(4, 2), (8, 8), (16, 4), (32, 64), (64, 64), (256, 128), (128, 256)] {
            let mut a = Water::new(u, v, Vec::new());
            a.set_refraction(100);
            for h in a.field.iter_mut() {
                *h = rnd();
            }
            for p in a.bitmap.iter_mut() {
                *p = rnd();
            }
            let mut b = Water::new(u, v, Vec::new());
            b.set_refraction(100);
            b.field = a.field.clone();
            b.bitmap = a.bitmap.clone();
            for k in 0..20 {
                a.pass(k % 2 == 1);
                pass_reference(&mut b, k % 2 == 1);
                assert_eq!(a.field, b.field, "{u}x{v} pass {k}: field");
                assert_eq!(a.bitmap, b.bitmap, "{u}x{v} pass {k}: bitmap");
            }
        }
    }
}

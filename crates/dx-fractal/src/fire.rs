//! Fire (`FireTexture`). Spec: `docs/formats/fire-texture.md`.
//!
//! Every step the sparks draw heat into the bitmap (and spawn, move and age short-lived particles), then the whole
//! bitmap is blurred upwards through a 1024-entry table: each pixel becomes the table's value for the sum of itself,
//! its left and right neighbours and the pixel below (one row lower again for a rising fire). Pixels are palette
//! indices into the texture's own palette.

use crate::Shared;

// spark byte offsets
const TYPE: usize = 0;
const HEAT: usize = 1;
const X: usize = 2;
const Y: usize = 3;
const A: usize = 4;
const B: usize = 5;
const C: usize = 6;
const D: usize = 7;

pub struct Fire {
    u: usize,
    v: usize,
    u_bits: u32,
    umask: u32,
    vmask: u32,
    /// The texture's pixels, `u` by `v`.
    pub bitmap: Vec<u8>,
    /// New pixel for a neighbourhood sum `0..1023` (4 spare bytes, as in the engine).
    pub render_table: Vec<u8>,
    /// The engine's spark buffer: `SparksLimit` slots, the first `num_sparks` alive. A spawned spark inherits whatever
    /// a dead spark left in the bytes it does not set.
    pub sparks: Vec<[u8; 8]>,
    pub num_sparks: usize,
    /// The bitmap moves up one row per step.
    pub rising: bool,
    /// `FX_Frequency`: added to `aux_phase` every step.
    pub frequency: u8,
    pub aux_phase: u8,
    pub global_phase: i32,
    /// Star sparks keep their pixel out of the blur while this is set (cleared once no star is left).
    star_status: bool,
    /// Scratch for `propagate`: the bitmap before the blur.
    prev: Vec<u8>,
}

impl Fire {
    /// `sparks`: the stored sparks; `sparks_limit` the `SparksLimit` property (clamped to 4..8192 like the engine).
    pub fn new(u: usize, v: usize, sparks: &[[u8; 8]], sparks_limit: i32, render_heat: u8, rising: bool, frequency: u8) -> Fire {
        assert!(u.is_power_of_two() && v.is_power_of_two());
        let limit = sparks_limit.clamp(4, 0x2000) as usize;
        let mut buf = sparks.to_vec();
        let num = buf.len().min(limit);
        buf.resize(limit.max(buf.len()), [0; 8]);
        buf.truncate(limit);
        let mut f = Fire {
            u,
            v,
            u_bits: u.trailing_zeros(),
            umask: (u - 1) as u32,
            vmask: (v - 1) as u32,
            bitmap: vec![0; u * v],
            prev: Vec::new(),
            render_table: vec![0; 1028],
            sparks: buf,
            num_sparks: num,
            rising,
            frequency,
            aux_phase: 0,
            global_phase: 0,
            star_status: true,
        };
        f.set_render_heat(render_heat);
        f
    }

    pub fn size(&self) -> (usize, usize) {
        (self.u, self.v)
    }

    pub fn limit(&self) -> usize {
        self.sparks.len()
    }

    /// `trunc(clamp(i/4 + 1 - (255 - RenderHeat)/16, 0, 255))`: heat fades by a quarter of the neighbourhood sum.
    pub fn set_render_heat(&mut self, render_heat: u8) {
        for i in 0..1024 {
            let v = i as f64 * 0.25 + 1.0 - (255 - render_heat as i32) as f64 * 0.0625;
            self.render_table[i] = if v < 0.0 {
                0
            } else if v < 255.0 {
                v.trunc() as u8
            } else {
                255
            };
        }
    }

    /// One step (`UFireTexture::ConstantTimeTick`): sparks, blur, stars.
    pub fn step(&mut self, shared: &mut Shared) {
        if self.u < 8 || self.v < 8 {
            return;
        }
        self.redraw_sparks(shared);
        self.propagate();
        self.post_draw_sparks();
    }

    fn put(&mut self, at: u32, value: u8) {
        let n = self.bitmap.len();
        self.bitmap[at as usize % n] = value;
    }

    /// The blur: each pixel from the table entry for (left + self + right + below), read from the previous bitmap,
    /// wrapping; a rising fire reads everything one row lower.
    pub fn propagate(&mut self) {
        // sizes are powers of two (`new`): wrapping is a mask; the previous bitmap is kept in `prev` (no new buffer)
        let (u, v) = (self.u, self.v);
        let (um, vm) = (u - 1, v - 1);
        let o = self.rising as usize;
        let mut src = std::mem::take(&mut self.prev);
        src.clear();
        src.extend_from_slice(&self.bitmap);
        for y in 0..v {
            let (a, b) = (((y + o) & vm) * u, ((y + o + 1) & vm) * u);
            let (ra, rb) = (&src[a..a + u], &src[b..b + u]);
            let out = &mut self.bitmap[y * u..y * u + u];
            for x in 0..u {
                let sum = ra[(x + um) & um] as usize + ra[x] as usize + ra[(x + 1) & um] as usize + rb[x] as usize;
                out[x] = self.render_table[sum];
            }
        }
        self.prev = src;
    }

    fn post_draw_sparks(&mut self) {
        if !self.star_status {
            return;
        }
        let mut any = false;
        for i in 0..self.num_sparks {
            let s = self.sparks[i];
            if s[TYPE] == 0x16 {
                any = true;
                let at = ((s[Y] as usize) << self.u_bits) + s[X] as usize;
                let at = at % self.bitmap.len();
                let under = self.bitmap[at];
                self.sparks[i][B] = under;
                if under < 0x26 {
                    self.bitmap[at] = s[A];
                }
            }
        }
        if !any {
            self.star_status = false;
        }
    }

    /// Moves a coordinate one step towards the sign of `speed` with probability |speed|/128.
    fn drift(rng: &mut crate::Rng, pos: u8, speed: i8, mask: u32) -> u8 {
        let r = rng.next() & 0x7f;
        if speed < 0 {
            if (r as i32) < -(speed as i32) {
                return (pos.wrapping_sub(1) as u32 & mask) as u8;
            }
        } else if ((r as u8) as i8) < speed {
            return (pos.wrapping_add(1) as u32 & mask) as u8;
        }
        pos
    }

    /// A short random walk of up to 7 cells (`(r1 & 7) - (r2 & 7)`) in x and in y.
    fn jitter(&self, rng: &mut crate::Rng, s: &mut [u8; 8]) {
        let (r1, r2) = (rng.next() as u8, rng.next() as u8);
        s[X] = ((r1 & 7).wrapping_sub(r2 & 7).wrapping_add(s[X]) as u32 & self.umask) as u8;
        let (r1, r2) = (rng.next() as u8, rng.next() as u8);
        s[Y] = ((r1 & 7).wrapping_sub(r2 & 7).wrapping_add(s[Y]) as u32 & self.vmask) as u8;
    }

    /// Half the time a step of -7..8 in x, then the same in y.
    fn wander(&self, rng: &mut crate::Rng, s: &mut [u8; 8]) {
        if rng.next() & 1 != 0 {
            s[X] = (((rng.next() as u8) & 0xf).wrapping_add(s[X]).wrapping_sub(7) as u32 & self.umask) as u8;
        }
        if rng.next() & 1 != 0 {
            s[Y] = (((rng.next() as u8) & 0xf).wrapping_add(s[Y]).wrapping_sub(7) as u32 & self.vmask) as u8;
        }
    }

    /// `UFireTexture::RedrawSparks`.
    pub fn redraw_sparks(&mut self, shared: &mut Shared) {
        self.aux_phase = self.aux_phase.wrapping_add(self.frequency);
        self.global_phase = self.global_phase.wrapping_add(1);
        let sine = shared.sine;
        let glow = |i: u8| (sine[i as usize] as u32 + 0x20).min(255) as u8;
        let s80 = |i: u8| sine[i as usize].wrapping_add(0x80) as i8;
        let (um, vm, ub) = (self.umask, self.vmask, self.u_bits);
        let limit = self.sparks.len();
        let mut i = 0;
        while i < self.num_sparks {
            let rng = &mut shared.rng;
            let lc = self.num_sparks;
            let mut s = self.sparks[i];
            let pos = ((s[Y] as u32) << ub) + s[X] as u32;
            let rb = |rng: &mut crate::Rng| rng.next() as u8;
            // a new spark in the next free slot (the bytes not set keep what was there)
            macro_rules! spawn {
                ($n:expr, $($k:expr => $val:expr),*) => {{
                    let n = $n;
                    $( let value = $val; self.sparks[n][$k] = value; )*
                }};
            }
            let mut remove = false;
            match s[TYPE] {
                0 => {
                    let r = rb(rng);
                    self.put(pos, r);
                }
                1 => {
                    let r1 = rng.next() & 0xff;
                    let r2 = rng.next() & 0xff;
                    let row = (((s[B] as u32 * r2) >> 8) + s[Y] as u32) & vm;
                    let col = um & (((s[A] as u32 * r1) >> 8) + s[X] as u32);
                    self.put((row << ub) + col, s[HEAT]);
                }
                2 => {
                    self.put(pos, s[HEAT]);
                    s[HEAT] = s[HEAT].wrapping_add(s[D]);
                }
                3 => {
                    if s[C] < s[HEAT] {
                        self.put(pos, s[HEAT]);
                    }
                    s[HEAT] = s[HEAT].wrapping_add(s[D]);
                    if s[HEAT] < s[D] {
                        s[HEAT] = rb(rng);
                    }
                }
                4 | 5 | 6 | 7 | 8 | 0xd | 0xe | 0x11 => {
                    let p = if matches!(s[TYPE], 4 | 5 | 0x11) { 0x80 } else { 0x40 };
                    if lc < limit && rb(rng) < p {
                        let n = self.num_sparks;
                        self.num_sparks += 1;
                        match s[TYPE] {
                            4 => spawn!(n, TYPE => 0x20, HEAT => s[HEAT], X => s[X], Y => s[Y], A => rb(rng), B => rb(rng), C => s[C], D => s[D]),
                            5 => spawn!(n, TYPE => 0x21, HEAT => s[HEAT], X => s[X], Y => s[Y], A => (rb(rng) & 0x7f).wrapping_sub(0x3f), B => 0x81, D => 2),
                            6 => spawn!(n, TYPE => 0x22, HEAT => s[HEAT], X => s[X], Y => s[Y], A => (rb(rng) & 0x7f).wrapping_sub(0x3f), B => 0, C => 0x32),
                            7 => spawn!(n, TYPE => 0x22, HEAT => s[HEAT], X => s[X], Y => s[Y], A => (rb(rng) & 0x3f).wrapping_add(0x3f), B => 0xe3, C => s[C]),
                            8 => spawn!(n, TYPE => 0x22, HEAT => s[HEAT], X => s[X], Y => s[Y], A => (rb(rng) & 0x3f).wrapping_add(0x80), B => 0xe3, C => s[C]),
                            0xd => spawn!(n, TYPE => 0x21, HEAT => s[HEAT], X => s[X], Y => s[Y], A => s[A], B => s[B], D => s[D]),
                            0xe => spawn!(n, TYPE => 0x2a, HEAT => s[HEAT], X => s[X], Y => s[Y], A => s[A], B => s[B], D => s[D]),
                            _ => spawn!(n, TYPE => 0x23,
                                X => ((((rng.next() & 0xff) * s[C] as u32) >> 8) as u8).wrapping_add(s[X]) & um as u8,
                                Y => ((((rng.next() & 0xff) * s[C] as u32) >> 8) as u8).wrapping_add(s[Y]) & vm as u8,
                                A => rb(rng).wrapping_sub(0x7f), B => 0x81, C => 0xff),
                        }
                    }
                }
                9 | 10 => {
                    let a = s[A];
                    let up = a.wrapping_add(0x40);
                    if s[TYPE] == 9 || up < 0x80 {
                        let v = (sine[up as usize] as u32 + s[HEAT] as u32).min(255) as u8;
                        let col = ((((sine[a as usize] as u32 * s[B] as u32) >> 8) as u8).wrapping_add(s[X]) as u32) & um;
                        self.put(col + ((s[Y] as u32) << ub), v);
                    }
                    s[A] = a.wrapping_add(s[D]);
                }
                0xb => {
                    let h = s[HEAT] as u32;
                    let row = ((((sine[s[B] as usize] as u32 * h) >> 8) as u8).wrapping_add(s[Y]) as u32) & vm;
                    let col = ((((sine[s[A] as usize] as u32 * h) >> 8) as u8).wrapping_add(s[X]) as u32) & um;
                    self.put((row << ub) + col, glow(s[A].wrapping_add(0x40)));
                    s[A] = s[A].wrapping_add(s[C]);
                    s[B] = s[B].wrapping_add(s[D]);
                }
                0xc => {
                    let a = s[A];
                    let v = (sine[a.wrapping_add(0x40) as usize] as u32 + s[HEAT] as u32).min(255) as u8;
                    let row = ((((sine[a as usize] as u32 * s[B] as u32) >> 8) as u8).wrapping_add(s[Y]) as u32) & vm;
                    self.put((row << ub) + s[X] as u32, v);
                    s[A] = a.wrapping_add(s[D]);
                }
                0xf | 0x1a => {
                    if lc < limit {
                        self.num_sparks = lc + 1;
                        if s[TYPE] == 0xf {
                            spawn!(lc, TYPE => 0x27, X => (rb(rng) & 0x1f).wrapping_add(s[X]) & um as u8, Y => (rb(rng) & 0x1f).wrapping_add(s[Y]) & vm as u8,
                                B => s[A], A => 0, C => s[B], D => s[D], HEAT => s[HEAT]);
                        } else {
                            spawn!(lc, TYPE => 0x27, X => s[X], Y => s[Y], B => s[A], A => 0, C => s[B], D => s[D], HEAT => s[HEAT]);
                        }
                        s[A] = s[A].wrapping_add(s[C]);
                    } else if s[TYPE] == 0x1a {
                        s[A] = s[A].wrapping_add(s[C]);
                    }
                    if s[TYPE] == 0xf {
                        self.jitter(rng, &mut s);
                    }
                }
                0x10 | 0x1b => {
                    if rb(rng) < 0x14 && self.num_sparks < limit {
                        let n = self.num_sparks;
                        self.num_sparks += 1;
                        if s[TYPE] == 0x10 {
                            spawn!(n, HEAT => s[HEAT], TYPE => 0x26, X => (rb(rng) & 0x1f).wrapping_add(s[X]) & um as u8,
                                Y => (rb(rng) & 0x1f).wrapping_add(s[Y]) & vm as u8, A => rb(rng), B => rb(rng), C => s[C]);
                        } else {
                            spawn!(n, TYPE => 0x2b, HEAT => s[HEAT], X => (rb(rng) & 0x1f).wrapping_add(s[X]) & um as u8,
                                Y => (rb(rng) & 0x1f).wrapping_add(s[Y]) & vm as u8, A => rb(rng), C => s[C], D => rb(rng));
                        }
                    }
                    self.wander(rng, &mut s);
                }
                0x12 | 0x13 => {
                    if lc < limit {
                        self.num_sparks = lc + 1;
                        if s[TYPE] == 0x12 {
                            spawn!(lc, TYPE => 0x23, X => (rb(rng) & 0x1f).wrapping_add(s[X]) & um as u8, Y => (rb(rng) & 0x1f).wrapping_add(s[Y]) & vm as u8,
                                A => rb(rng).wrapping_sub(0x7f), B => 0x81, C => 0xff);
                        } else {
                            spawn!(lc, TYPE => 0x24, X => (rb(rng) & 0x1f).wrapping_add(s[X]) & um as u8, Y => (rb(rng) & 0x1f).wrapping_add(s[Y]) & vm as u8,
                                A => (rb(rng) & 0x1f).wrapping_sub(0xf), B => 0x81, C => 0);
                        }
                    }
                    self.wander(rng, &mut s);
                }
                0x14 | 0x15 => {
                    if lc < limit {
                        self.num_sparks = lc + 1;
                        if s[TYPE] == 0x14 {
                            spawn!(lc, TYPE => 0x25, X => (rb(rng) & 0x1f).wrapping_add(s[X]) & um as u8, Y => (rb(rng) & 0x1f).wrapping_add(s[Y]) & vm as u8,
                                A => s[A], B => s[B], C => s[D]);
                        } else {
                            spawn!(lc, TYPE => 0x25,
                                X => ((((rng.next() & 0xff) * s[C] as u32) >> 8) as u8).wrapping_add(s[X]) & um as u8,
                                Y => ((((rng.next() & 0xff) * s[C] as u32) >> 8) as u8).wrapping_add(s[Y]) & vm as u8,
                                A => s[A], B => s[B], C => s[D]);
                        }
                    }
                    if s[TYPE] == 0x14 {
                        self.jitter(rng, &mut s);
                    }
                }
                0x16 => self.put(pos, s[B]),
                0x17 | 0x18 => {
                    if s[HEAT] != 0 {
                        if s[C] == 0 {
                            if s[D] <= rb(rng) {
                                s[C] = rb(rng).wrapping_add(1) & 5;
                            }
                        } else {
                            s[C] -= 1;
                            let h2 = if s[TYPE] == 0x17 { s[HEAT] } else { s[HEAT] >> 3 };
                            self.flash_ramp([s[X], s[Y], s[A], s[B]], s[HEAT], h2, rng);
                        }
                    }
                }
                0x19 => {
                    if s[D] <= rb(rng) {
                        let r = (rng.next() & 0xff) as usize;
                        let c = s[C] as i32;
                        let dx = ((sine[r] as i32 * c) >> 8) - (c >> 1);
                        let dy = ((sine[(r + 0x40) & 0xff] as i32 * c) >> 8) - (c >> 1);
                        // a length with its direction in the lowest bit (odd = negative)
                        let enc = |d: i32| if d < 1 { (d as u8).wrapping_neg() | 1 } else { (d as u8) & 0xfe };
                        self.flash_ramp([s[X], s[Y], enc(dx), enc(dy)], s[HEAT], s[HEAT] >> 2, rng);
                    }
                }
                0x1c => {
                    if lc < limit {
                        self.num_sparks = lc + 1;
                        spawn!(lc, TYPE => 0x28, X => s[X], Y => s[Y], HEAT => s[HEAT], A => s[A], B => s[B], C => s[C], D => 2);
                    }
                    s[A] = s[A].wrapping_add(s[D]);
                }
                0x1d => {
                    let a = s[A];
                    let col = ((((sine[a as usize] as u32 * s[HEAT] as u32) >> 8) as u8).wrapping_add(s[X]) as u32) & um;
                    self.put(col + ((s[Y] as u32) << ub), glow(a.wrapping_add(0x40)));
                    s[A] = a.wrapping_add(s[C]);
                }
                0x1e => {
                    let b = s[B];
                    let row = ((((sine[b as usize] as u32 * s[HEAT] as u32) >> 8) as u8).wrapping_add(s[Y]) as u32) & vm;
                    self.put((row << ub) + s[X] as u32, glow(b.wrapping_add(0x40)));
                    s[B] = b.wrapping_add(s[D]);
                }
                // particles spawned by the emitters above
                0x20 | 0x21 | 0x25 | 0x29 => {
                    let (alive, value) = match s[TYPE] {
                        0x20 => {
                            s[HEAT] = s[HEAT].wrapping_sub(5);
                            (s[HEAT] < 0xfb, s[HEAT])
                        }
                        0x21 => {
                            s[HEAT] = s[HEAT].wrapping_sub(s[D]);
                            (s[HEAT] > s[D], s[HEAT])
                        }
                        0x25 => {
                            s[C] = s[C].wrapping_add(4);
                            (s[C] <= 0xf9, s[C])
                        }
                        _ => {
                            s[HEAT] = s[HEAT].wrapping_sub(s[C]);
                            (s[HEAT] <= 0xf9, s[HEAT])
                        }
                    };
                    if alive {
                        self.put(((s[Y] as u32) << ub) + s[X] as u32, value);
                        s[X] = Self::drift(rng, s[X], s[A] as i8, um);
                        s[Y] = Self::drift(rng, s[Y], s[B] as i8, vm);
                    } else {
                        remove = true;
                    }
                }
                0x22 | 0x2a => {
                    let alive = if s[TYPE] == 0x22 {
                        s[C] = s[C].wrapping_sub(1);
                        s[C] != 0
                    } else {
                        s[HEAT] = s[HEAT].wrapping_sub(s[D]);
                        s[HEAT] >= 0x33
                    };
                    if alive {
                        self.put(((s[Y] as u32) << ub) + s[X] as u32, s[HEAT]);
                        s[X] = Self::drift(rng, s[X], s[A] as i8, um);
                        let b = s[B] as i8;
                        s[Y] = Self::drift(rng, s[Y], b, vm);
                        // the vertical speed builds up (gravity)
                        if s[TYPE] == 0x22 && b < 122 {
                            s[B] = (b + 3) as u8;
                        } else if s[TYPE] == 0x2a && self.global_phase & 1 != 0 && b < 124 {
                            s[B] = (b + 3) as u8;
                        }
                    } else {
                        remove = true;
                    }
                }
                0x23 | 0x24 => {
                    let alive = if s[TYPE] == 0x23 {
                        s[C] = s[C].wrapping_sub(3);
                        s[C] >= 0xbf
                    } else {
                        s[C] = s[C].wrapping_add(4);
                        s[C] <= 0xf9
                    };
                    if alive {
                        self.put(((s[Y] as u32) << ub) + s[X] as u32, s[C]);
                        s[X] = Self::drift(rng, s[X], s[A] as i8, um);
                        s[Y] = (s[Y].wrapping_sub(2) as u32 & vm) as u8;
                    } else {
                        remove = true;
                    }
                }
                0x26 | 0x27 | 0x28 | 0x2b => {
                    s[C] = s[C].wrapping_sub(1);
                    if s[C] == 0xff {
                        remove = true;
                    } else {
                        self.put(((s[Y] as u32) << ub) + s[X] as u32, s[HEAT]);
                        let (dx, dy) = match s[TYPE] {
                            0x26 => (s[A] as i8, s[B] as i8),
                            0x27 => {
                                let b = s[B];
                                let d = (s80(b.wrapping_add(0x40)), s80(b));
                                let ab = u16::from_le_bytes([s[A], s[B]]).wrapping_add(s[D] as u16 * 0x10);
                                [s[A], s[B]] = ab.to_le_bytes();
                                d
                            }
                            0x28 => {
                                let a = s[A];
                                let d = (sine[a.wrapping_add(0x40) as usize].wrapping_sub(0x80) as i8, s[B] as i8);
                                s[A] = s[D].wrapping_add(a);
                                d
                            }
                            _ => {
                                s[A] = s[A].wrapping_add(7);
                                let mut t = s[A] & 0x7f;
                                if t > 0x3f {
                                    t = 0x7f - t;
                                }
                                let t = s[D].wrapping_add(t);
                                (sine[t as usize].wrapping_sub(0x7f) as i8, sine[t.wrapping_add(0x40) as usize].wrapping_sub(0x7f) as i8)
                            }
                        };
                        s[X] = Self::drift(rng, s[X], dx, um);
                        s[Y] = Self::drift(rng, s[Y], dy, vm);
                    }
                }
                _ => {}
            }
            // the engine updates the spark in place before it removes it (the last slot keeps those bytes when the
            // spark removed is the last one, and a later spawn into that slot inherits them)
            self.sparks[i] = s;
            if remove {
                self.num_sparks -= 1;
                self.sparks[i] = self.sparks[self.num_sparks];
            }
            i += 1;
        }
    }

    /// A jagged line of heat (lightning): from `(x, y)` along `(dx, dy)` (lengths with the direction in the lowest bit,
    /// odd = negative), wobbling by random amounts, the heat going from `h1` to `h2` (`UFireTexture::DrawFlashRamp`).
    fn flash_ramp(&mut self, seg: [u8; 4], h1: u8, h2: u8, rng: &mut crate::Rng) {
        let [mut x, mut y, mut a, mut b] = seg;
        let (mut h1, mut h2) = (h1, h2);
        let (ai, bi) = (a as i32, b as i32);
        // a line drawn against its main direction starts from its other end
        let flip = (b & 1 != 0 && 2 * bi >= ai) || (a & 1 != 0 && 2 * bi < ai);
        if flip {
            x = x.wrapping_add(if a & 1 != 0 { (ai as u8).wrapping_neg() } else { ai as u8 });
            y = y.wrapping_add(if b & 1 != 0 { (bi as u8).wrapping_neg() } else { bi as u8 });
            a ^= 1;
            b ^= 1;
            std::mem::swap(&mut h1, &mut h2);
        }
        let n = (a.max(b) as i32) | 1;
        let mut wobble = [0u8; 256];
        let mut sum = 0i32;
        for w in wobble.iter_mut().take(n as usize) {
            let r = rng.next() as u8;
            *w = r;
            sum += r as i32;
        }
        let (sy, step_b) = if b & 1 != 0 { (-1i32, -(b as i32)) } else { (1, b as i32) };
        let (sx, step_a) = if a & 1 != 0 { (-1i32, -(a as i32)) } else { (1, a as i32) };
        let heat_step = ((h2 as i32 - h1 as i32) << 23) / n;
        let mut heat = (h1 as i32) << 23;
        let (um, vm, ub) = (self.umask as i32, self.vmask as i32, self.u_bits);
        if a >= b {
            let step = ((step_b << 6) - sum) / n;
            let mut ypos = (y as i32) << 6;
            for &w in wobble.iter().take(a as usize) {
                ypos = ypos.wrapping_add(w as i32 + step);
                let at = (((ypos >> 6) & vm) << ub) + (x as i32 & um);
                x = x.wrapping_add(sx as u8);
                heat = heat.wrapping_add(heat_step);
                self.put(at as u32, (heat >> 23) as u8);
            }
        } else {
            let step = ((step_a << 6) - sum) / n;
            let mut xpos = (x as i32) << 6;
            for &w in wobble.iter().take(b as usize) {
                xpos = xpos.wrapping_add(w as i32 + step);
                let at = ((y as i32 & vm) << ub) + ((xpos >> 6) & um);
                y = y.wrapping_add(sy as u8);
                heat = heat.wrapping_add(heat_step);
                self.put(at as u32, (heat >> 23) as u8);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blur_matches_its_first_form() {
        let mut seed = 777u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        for (u, v, rising) in [(8, 8, false), (8, 8, true), (32, 16, true), (64, 128, false), (256, 256, true)] {
            let mut f = Fire::new(u, v, &[], 64, 0x20, rising, 0);
            for p in f.bitmap.iter_mut() {
                *p = rnd();
            }
            for k in 0..10 {
                // as first written: a copy of the bitmap, wrapping by `%`
                let src = f.bitmap.clone();
                let o = f.rising as usize;
                let g = |y: usize, x: usize| src[(y % v) * u + (x % u)] as usize;
                let want: Vec<u8> = (0..v).flat_map(|y| (0..u).map(move |x| (y, x))).map(|(y, x)| f.render_table[g(y + o, x + u - 1) + g(y + o, x) + g(y + o, x + 1) + g(y + o + 1, x)]).collect();
                f.propagate();
                assert_eq!(f.bitmap, want, "{u}x{v} rising {rising} step {k}");
            }
        }
    }
}

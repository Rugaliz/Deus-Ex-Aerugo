//! Galaxy's MPEG audio decoder (docs/galaxy/mp3.md): the stream decode step (1061a690), the layer III frame decoder
//! (1061cc10) and the MMX polyphase synthesis (1061ab60 with 1062f5c0), bit-identical to the original for the
//! MPEG-1 layer III data Deus Ex ships (mono and joint stereo with M/S, long and short blocks). Floats follow the
//! original's x87 at 53-bit precision: f64 arithmetic with an f32 store at every `r(..)` of the spec.
use super::mp3_tables::*;
use std::sync::OnceLock;

fn r(x: f64) -> f32 {
    x as f32
}
fn f(bits: u32) -> f64 {
    f32::from_bits(bits) as f64
}
fn sat16(v: i32) -> i32 {
    v.clamp(-32768, 32767)
}
fn cdiv(a: i32, b: i32) -> i32 {
    a / b
}

/// The run-time tables (mp3.md 3.2): `POW43`, `GAIN` (index + 346) and the IMDCT windows.
struct Tables {
    pow43: Vec<f32>,
    gain: Vec<f32>,
    win: [[f32; 36]; 4],
    c1: [[i32; 8]; 8],
    c2: [[i32; 8]; 8],
    c3: [[i32; 16]; 16],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let pow43 = (0..8192).map(|k| (k as f64).powf(f64::from_bits(0x3ff5555555555555)) as f32).collect();
        let gain = (-346..=45).map(|i| 2f64.powf(i as f64 * 0.25) as f32).collect();
        let p = 1f64.atan();
        let c9 = f64::from_bits(0x3fbc71c71c71c71c);
        let c3 = 1.0f64 / 3.0;
        let a = WIN_A.map(f32::from_bits);
        let b = WIN_B.map(f32::from_bits);
        let l = |i: usize, c: f64, t: f32| r((((i as f64 + 0.5) * p) * c).sin() * t as f64 * 32768.0);
        let mut win = [[0f32; 36]; 4];
        for i in 0..36 {
            win[0][i] = l(i, c9, a[i]);
            win[1][i] = if i < 18 {
                l(i, c9, a[i])
            } else if i < 24 {
                r(a[i] as f64 * 32768.0)
            } else if i < 30 {
                l(i - 18, c3, a[i])
            } else {
                0.0
            };
            win[2][i] = if i < 12 { l(i, c3, b[i]) } else { 0.0 };
            win[3][i] = if i < 6 {
                0.0
            } else if i < 12 {
                l(i - 6, c3, a[i])
            } else if i < 18 {
                r(a[i] as f64 * 32768.0)
            } else {
                l(i, c9, a[i])
            };
        }
        let pi = std::f64::consts::PI;
        let mut c1 = [[0; 8]; 8];
        let mut c2 = [[0; 8]; 8];
        for o in 0..8 {
            for k in 0..8 {
                c1[o][k] = (-16384.0 * (((2 * k + 1) * o) as f64 * pi / 16.0).cos()) as i32;
                c2[o][k] = (-16384.0 * (((2 * k + 1) * (2 * o + 1)) as f64 * pi / 32.0).cos()) as i32;
            }
        }
        const M: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 11, 10, 9, 8, 15, 14, 13, 12];
        let mut c3m = [[0; 16]; 16];
        for o in 0..16 {
            for k in 0..16 {
                let sg = if k < 8 { 1.0 } else { -1.0 };
                c3m[o][k] = (sg * 16384.0 * (((2 * o + 1) * (2 * M[k] + 1)) as f64 * pi / 64.0).cos()) as i32;
            }
        }
        Tables { pow43, gain, win, c1, c2, c3: c3m }
    })
}

/// Big-endian bit reads (mp3.md 1.3) over a byte slice (zeros past its end).
fn peek_bits(buf: &[u8], pos: i64, n: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    let at = (pos >> 3) as usize;
    let b = |i: usize| buf.get(at + i).copied().unwrap_or(0) as u32;
    let w = (b(0) << 24) | (b(1) << 16) | (b(2) << 8) | b(3);
    (w << (pos & 7)) >> (32 - n)
}

/// One granule-channel's side information.
#[derive(Clone, Copy, Default)]
struct Granule {
    part2_3_length: i32,
    big_values: i32,
    global_gain: i32,
    scalefac_compress: i32,
    window_switching: i32,
    block_type: i32,
    mixed: i32,
    table_select: [i32; 3],
    subblock_gain: [i32; 3],
    region0_count: i32,
    region1_count: i32,
    preflag: i32,
    scalefac_scale: i32,
    count1table_select: i32,
}

/// The decoder state of one sound (0x8a7c bytes in the original), never reset.
pub struct Mp3Decoder {
    h: [i32; 14],
    scfsi: [[i32; 4]; 2],
    side: [Granule; 4],
    q: [[i32; 580]; 4],
    ov: Box<[[[f32; 18]; 32]; 2]>,
    sbs: Box<[[[f32; 36]; 32]; 2]>,
    v: Box<[[i16; 1024]; 2]>,
    vpos: i32,
    pcm: Box<[[i16; 1152]; 2]>,
    left: i32,
    lidx: i32,
    res: Vec<u8>,
    rpos: i64,
    wpos: i64,
    /// Scale factors of the frame: long `[gr][ch][22]`, short `[gr][ch][13][3]`.
    sfl: [[[i32; 22]; 2]; 2],
    sfs: [[[[i32; 3]; 13]; 2]; 2],
}

impl std::fmt::Debug for Mp3Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mp3Decoder").field("left", &self.left).field("lidx", &self.lidx).finish()
    }
}

impl Default for Mp3Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Mp3Decoder {
    pub fn new() -> Mp3Decoder {
        Mp3Decoder {
            h: [0; 14],
            scfsi: [[0; 4]; 2],
            side: [Granule::default(); 4],
            q: [[0; 580]; 4],
            ov: Box::new([[[0.0; 18]; 32]; 2]),
            sbs: Box::new([[[0.0; 36]; 32]; 2]),
            v: Box::new([[0; 1024]; 2]),
            vpos: 0,
            pcm: Box::new([[0; 1152]; 2]),
            left: 0,
            lidx: 0,
            res: vec![0; 0x800 + 8],
            rpos: 0,
            wpos: 0,
            sfl: [[[0; 22]; 2]; 2],
            sfs: [[[[0; 3]; 13]; 2]; 2],
        }
    }

    fn v_of(&self) -> usize {
        (self.h[1] | self.h[2]) as usize
    }

    fn parse_header(&mut self, buf: &[u8], pos: &mut i64) {
        let w = peek_bits(buf, *pos, 32);
        *pos += 32;
        self.h[0] = (w >> 21) as i32;
        self.h[1] = ((((w >> 20) & 1) ^ 1) << 1) as i32;
        self.h[2] = ((w >> 19) & 1) as i32;
        self.h[3] = ((w >> 17) & 3) as i32;
        self.h[4] = ((w >> 16) & 1) as i32;
        self.h[5] = ((w >> 12) & 15) as i32;
        self.h[6] = ((w >> 10) & 3) as i32;
        self.h[7] = ((w >> 9) & 1) as i32;
        self.h[8] = ((w >> 8) & 1) as i32;
        self.h[9] = ((w >> 6) & 3) as i32;
        self.h[10] = ((w >> 4) & 3) as i32;
        self.h[11] = ((w >> 3) & 1) as i32;
        self.h[12] = ((w >> 2) & 1) as i32;
        self.h[13] = (w & 3) as i32;
    }

    fn rate(&self) -> i32 {
        RATE[(self.h[6] as usize + 4 * self.v_of()).min(11)]
    }
    fn bitrate(&self) -> i32 {
        BITRATE[((self.h[3] as usize + 4 * self.v_of()) * 16 + self.h[5] as usize).min(191)]
    }

    /// The decode step (1061a690): output of up to `max_bytes / 2` samples per channel from `src` (the file's bytes
    /// from the stream position). Returns `(consumed bytes, left samples, right samples)`; empty outputs and 0
    /// consumed when nothing was decoded (status 0x10).
    pub fn decode(&mut self, src: &[u8], max_bytes: u32) -> (usize, Vec<i16>, Vec<i16>) {
        let mut want = (max_bytes >> 1) as i32;
        let (mut out_l, mut out_r) = (Vec::new(), Vec::new());
        let mut lim = src.len() as i64 * 8;
        if lim < 1 {
            return (0, out_l, out_r);
        }
        let mut pos = 0i64;
        while peek_bits(src, pos, 11) != 0x7ff {
            pos += 8;
            if pos >= lim {
                return (0, out_l, out_r);
            }
        }
        let start = pos;
        let mut p = pos;
        self.parse_header(src, &mut p);
        let rate = self.rate();
        let mut fl = if rate != 0 { self.bitrate() * (if self.h[3] == 3 { 75 } else { 225 }) * 5120 / rate } else { 0 };
        if self.h[2] == 0 {
            fl = cdiv(fl, 2);
        }
        if fl != 0 {
            lim = start + ((lim - start) / fl as i64) * fl as i64;
        }
        pos = start;
        loop {
            if want == 0 || pos >= lim {
                return ((pos >> 3) as usize, out_l, out_r);
            }
            if self.left == 0 {
                let mut found = false;
                while pos & 7 != 0 {
                    pos += 1;
                }
                while pos < lim {
                    if peek_bits(src, pos, 11) == 0x7ff {
                        self.parse_header(src, &mut pos);
                        if self.h[4] == 0 {
                            pos += 16;
                        }
                        if self.h[3] == 1 && self.h[2] == 1 {
                            self.layer3(src, &mut pos);
                            self.synthesis(36);
                        }
                        self.left = if self.h[3] == 3 {
                            384
                        } else if self.h[3] == 1 && self.h[2] == 0 {
                            576
                        } else {
                            1152
                        };
                        self.lidx = 0;
                        found = true;
                        break;
                    }
                    pos += 8;
                }
                if !found {
                    self.left = 0;
                    self.lidx = 0;
                }
            }
            let n = self.left.min(want);
            let li = self.lidx as usize;
            let right_ch = if self.h[9] == 3 { 0 } else { 1 };
            out_l.extend_from_slice(&self.pcm[0][li..li + n as usize]);
            out_r.extend_from_slice(&self.pcm[right_ch][li..li + n as usize]);
            want -= n;
            self.lidx += n;
            self.left -= n;
        }
    }

    /// The layer III frame decoder (1061cc10), MPEG-1.
    fn layer3(&mut self, src: &[u8], pos: &mut i64) {
        let nch = if self.h[9] == 3 { 1 } else { 2 };
        let rate = self.rate();
        let frame_bytes = if rate != 0 { self.bitrate() * 144000 / rate + self.h[7] } else { 0 };
        let side_bytes = 4 + if nch == 1 { 17 } else { 32 } + if self.h[4] == 0 { 2 } else { 0 };
        let rd = |n: u32, pos: &mut i64| -> i32 {
            let v = peek_bits(src, *pos, n);
            *pos += n as i64;
            v as i32
        };
        let mdb = rd(9, pos);
        rd(if nch == 1 { 5 } else { 3 }, pos);
        for ch in 0..nch {
            for k in 0..4 {
                self.scfsi[ch][k] = rd(1, pos);
            }
        }
        for gr in 0..2 {
            for ch in 0..nch {
                let g = &mut self.side[gr * 2 + ch];
                g.part2_3_length = rd(12, pos);
                g.big_values = rd(9, pos).min(288);
                g.global_gain = rd(8, pos);
                g.scalefac_compress = rd(4, pos);
                g.window_switching = rd(1, pos);
                if g.window_switching != 0 {
                    g.block_type = rd(2, pos);
                    g.mixed = rd(1, pos);
                    g.table_select[0] = rd(5, pos);
                    g.table_select[1] = rd(5, pos);
                    for w in 0..3 {
                        g.subblock_gain[w] = rd(3, pos);
                    }
                    g.region0_count = if g.block_type == 2 && g.mixed == 0 { 8 } else { 7 };
                    g.region1_count = 20 - g.region0_count;
                } else {
                    for t in 0..3 {
                        g.table_select[t] = rd(5, pos);
                    }
                    g.region0_count = rd(4, pos);
                    g.region1_count = rd(3, pos);
                    g.block_type = 0;
                    g.mixed = 0;
                }
                g.preflag = rd(1, pos);
                g.scalefac_scale = rd(1, pos);
                g.count1table_select = rd(1, pos);
            }
        }
        // the bit reservoir
        let held = (self.wpos >> 3) as i32;
        if mdb <= held {
            let s = (held - mdb) as usize;
            self.res.copy_within(s..s + mdb as usize, 0);
        }
        self.rpos = 0;
        self.wpos = mdb as i64 * 8;
        for _ in 0..(frame_bytes - side_bytes).max(0) {
            let b = rd(8, pos) as u8;
            let at = (self.wpos >> 3) as usize;
            if at < self.res.len() {
                self.res[at] = b;
            }
            self.wpos += 8;
        }
        let row = (self.v_of() * 3 + self.h[6] as usize).min(8);
        let mut xr = vec![[[0f32; 576]; 2]; 2];
        let mut nz = [[0i32; 2]; 2];
        for gr in 0..2 {
            for ch in 0..nch {
                let part2_start = self.rpos;
                self.scale_factors(gr, ch);
                nz[gr][ch] = self.huffman(gr, ch, row, part2_start);
                self.requantise(gr, ch, row, nz[gr][ch], &mut xr[gr][ch]);
            }
        }
        let mut run = 576;
        for gr in 0..2 {
            if self.h[9] == 1 {
                self.stereo(gr, row, &mut xr[gr], &mut nz[gr], &mut run);
            }
            for ch in 0..nch {
                self.imdct(gr, ch, nz[gr][ch], &mut xr[gr][ch]);
            }
        }
    }

    fn read_res(&mut self, n: u32) -> i32 {
        let v = peek_bits(&self.res, self.rpos, n);
        self.rpos += n as i64;
        v as i32
    }

    fn scale_factors(&mut self, gr: usize, ch: usize) {
        let g = self.side[gr * 2 + ch];
        let s1 = SLEN1[g.scalefac_compress as usize] as u32;
        let s2 = SLEN2[g.scalefac_compress as usize] as u32;
        if g.window_switching != 0 && g.block_type == 2 {
            if g.mixed != 0 {
                for sfb in 0..8 {
                    self.sfl[gr][ch][sfb] = self.read_res(s1);
                }
                for sfb in 3..12 {
                    for w in 0..3 {
                        self.sfs[gr][ch][sfb][w] = self.read_res(if sfb < 6 { s1 } else { s2 });
                    }
                }
            } else {
                for sfb in 0..12 {
                    for w in 0..3 {
                        self.sfs[gr][ch][sfb][w] = self.read_res(if sfb < 6 { s1 } else { s2 });
                    }
                }
            }
            self.sfs[gr][ch][12] = [0; 3];
        } else {
            const GROUPS: [(usize, usize); 4] = [(0, 6), (6, 11), (11, 16), (16, 21)];
            for (k, &(a, b)) in GROUPS.iter().enumerate() {
                for sfb in a..b {
                    self.sfl[gr][ch][sfb] = if self.scfsi[ch][k] != 0 && gr == 1 { self.sfl[0][ch][sfb] } else { self.read_res(if k < 2 { s1 } else { s2 }) };
                }
            }
            self.sfl[gr][ch][21] = 0;
        }
    }

    /// Huffman decoding (mp3.md 6.4); returns the nonzero count.
    fn huffman(&mut self, gr: usize, ch: usize, row: usize, part2_start: i64) -> i32 {
        let g = self.side[gr * 2 + ch];
        let sfl_flat = |i: usize| -> i32 { SFL.as_flattened().get(row * 23 + i).copied().unwrap_or(576) };
        let (r1, r2) = if g.window_switching != 0 && g.block_type == 2 { (36, 576) } else { (sfl_flat(g.region0_count as usize + 1), sfl_flat((g.region0_count + g.region1_count) as usize + 2)) };
        let qi = gr * 2 + ch;
        let mut t = g.table_select[0] as usize;
        let mut i = 0i32;
        while i < 2 * g.big_values {
            if i == r1 {
                t = g.table_select[1] as usize;
            }
            if i == r2 {
                t = g.table_select[2] as usize;
            }
            let t = t & 31;
            let mut e = HUFF_FIRST[HUFF_FIRST_INDEX[t]][peek_bits(&self.res, self.rpos, 8) as usize] as u16 as u32;
            self.rpos += ((e >> 8) & 0x7f) as i64;
            if e & 0x8000 != 0 {
                let mut k = (e & 0xff) as usize;
                let si = HUFF_SECOND_INDEX[t];
                loop {
                    let e2 = if si >= 0 { HUFF_SECOND[si as usize].get(k).copied().unwrap_or(0) } else { 0 };
                    let n = (e2 >> 8) & 0x7f;
                    let v = peek_bits(&self.res, self.rpos, n);
                    if (e2 >> 16) == v || k >= 255 {
                        self.rpos += n as i64;
                        e = e2;
                        break;
                    }
                    k += 1;
                }
            }
            let mut x = ((e >> 4) & 15) as i32;
            let mut y = (e & 15) as i32;
            let lb = LINBITS[t] as u32;
            if x == 15 && lb != 0 {
                x = 15 + self.read_res(lb);
            }
            if x != 0 && self.read_res(1) == 1 {
                x = -x;
            }
            if y == 15 && lb != 0 {
                y = 15 + self.read_res(lb);
            }
            if y != 0 && self.read_res(1) == 1 {
                y = -y;
            }
            self.q[qi][i as usize] = x;
            self.q[qi][i as usize + 1] = y;
            i += 2;
        }
        let end = part2_start + g.part2_3_length as i64;
        let mut c1 = 0;
        while i < 576 {
            if self.rpos >= end {
                for k in i..576 {
                    self.q[qi][k as usize] = 0;
                }
                break;
            }
            let e = COUNT1[(g.count1table_select as usize) * 64 + peek_bits(&self.res, self.rpos, 6) as usize] as i32;
            self.rpos += (e >> 4) as i64;
            let mut vals = [(e >> 3) & 1, (e >> 2) & 1, (e >> 1) & 1, e & 1];
            for v in vals.iter_mut() {
                if *v == 1 && self.read_res(1) == 1 {
                    *v = -1;
                }
            }
            for (k, v) in vals.iter().enumerate() {
                self.q[qi][i as usize + k] = *v;
            }
            i += 4;
            c1 += 1;
        }
        if (g.big_values + 2 * c1) * 2 > 576 {
            c1 = cdiv((288 - g.big_values) * 2, 4);
        }
        self.rpos = end;
        (g.big_values + 2 * c1) * 2
    }

    fn requantise(&mut self, gr: usize, ch: usize, row: usize, nz: i32, xr: &mut [f32; 576]) {
        let t = tables();
        let g = self.side[gr * 2 + ch];
        let q = &self.q[gr * 2 + ch];
        let sh = g.scalefac_scale + 1;
        let dq = |v: i32, idx: i32| -> f32 {
            let gn = t.gain[(idx + 346).clamp(0, 391) as usize] as f64;
            if v < 0 {
                r(-(t.pow43[(-v).min(8191) as usize] as f64 * gn))
            } else {
                r(gn * t.pow43[v.min(8191) as usize] as f64)
            }
        };
        if !(g.window_switching != 0 && g.block_type == 2) {
            let mut sfb = 0usize;
            let mut endl = 0usize;
            while sfb < 22 && SFL[row][sfb] < nz {
                let mut idx = g.global_gain - (self.sfl[gr][ch][sfb] << sh) - 210;
                if g.preflag != 0 {
                    idx -= (PRETAB[sfb] as i32) << sh;
                }
                for i in SFL[row][sfb] as usize..SFL[row][sfb + 1] as usize {
                    xr[i] = dq(q[i], idx);
                }
                endl = SFL[row][sfb + 1] as usize;
                sfb += 1;
            }
            for x in xr.iter_mut().skip(endl) {
                *x = 0.0;
            }
        } else if g.mixed == 0 {
            let mut sfb = 0usize;
            let mut inp = 0usize;
            while sfb < 13 && SFS[row][sfb] * 3 < nz {
                let a = SFS[row][sfb] as usize;
                let width = (SFS[row][sfb + 1] - SFS[row][sfb]) as usize;
                for w in 0..3 {
                    let idx = g.global_gain - (self.sfs[gr][ch][sfb][w] << sh) - 8 * g.subblock_gain[w] - 210;
                    for j in 0..width {
                        xr[3 * a + 3 * j + w] = dq(q[inp + j], idx);
                    }
                    inp += width;
                }
                sfb += 1;
            }
            for x in xr.iter_mut().skip(inp) {
                *x = 0.0;
            }
        } else {
            // mixed blocks (no Deus Ex data): long bands below line 36, then short bands from 3
            let mut sfb = 0usize;
            let mut endl = 0usize;
            while sfb < 22 && SFL[row][sfb] < 36 && SFL[row][sfb] < nz {
                let mut idx = g.global_gain - (self.sfl[gr][ch][sfb] << sh) - 210;
                if g.preflag != 0 {
                    idx -= (PRETAB[sfb] as i32) << sh;
                }
                for i in SFL[row][sfb] as usize..SFL[row][sfb + 1] as usize {
                    xr[i] = dq(q[i], idx);
                }
                endl = SFL[row][sfb + 1] as usize;
                sfb += 1;
            }
            let mut inp = endl;
            let mut sfb = 3usize;
            while sfb < 13 && SFS[row][sfb] * 3 < nz {
                let a = SFS[row][sfb] as usize;
                let width = (SFS[row][sfb + 1] - SFS[row][sfb]) as usize;
                for w in 0..3 {
                    let idx = g.global_gain - (self.sfs[gr][ch][sfb][w] << sh) - 8 * g.subblock_gain[w] - 210;
                    for j in 0..width {
                        if 3 * a + 3 * j + w < 576 && inp + j < 580 {
                            xr[3 * a + 3 * j + w] = dq(q[inp + j], idx);
                        }
                    }
                    inp += width;
                }
                sfb += 1;
            }
            for x in xr.iter_mut().skip(inp.min(576)) {
                *x = 0.0;
            }
        }
    }

    /// M/S stereo with the late block-type test (mp3.md 6.6).
    fn stereo(&mut self, gr: usize, row: usize, xr: &mut [[f32; 576]; 2], nz: &mut [i32; 2], run: &mut i32) {
        let e = gr * 2 + 2;
        let (ws, bt) = if e < 4 { (self.side[e].window_switching, self.side[e].block_type) } else { (self.side[0].mixed, self.side[0].subblock_gain[0]) };
        let short = ws != 0 && bt == 2;
        let lim = |sfb: usize| -> i32 {
            if short {
                SFS[row].get(sfb).copied().unwrap_or(192) * 3
            } else {
                SFL[row].get(sfb).copied().unwrap_or(576)
            }
        };
        let k = f(KA0);
        let ms = |xr: &mut [[f32; 576]; 2], a: i32, b: i32| {
            for i in a.max(0) as usize..(b.min(576)) as usize {
                let m = xr[0][i] as f64;
                let s = xr[1][i] as f64;
                xr[0][i] = r((s + m) * k);
                xr[1][i] = r((m - s) * k);
            }
        };
        let (nzl, nzr) = (nz[0], nz[1]);
        if self.h[10] & 2 != 0 {
            let mut sfb = 0usize;
            if lim(0) < nzr {
                loop {
                    *run = lim(sfb);
                    ms(xr, lim(sfb), lim(sfb + 1));
                    *run = lim(sfb + 1);
                    sfb += 1;
                    if !(lim(sfb) < nzr) {
                        break;
                    }
                }
            }
            while lim(sfb) < nzl {
                ms(xr, lim(sfb), lim(sfb + 1));
                *run = lim(sfb + 1);
                sfb += 1;
            }
        }
        let n = cdiv(*run, 2) * 2;
        nz[0] = n;
        nz[1] = n;
    }

    /// Alias reduction, IMDCT, overlap and frequency inversion of one granule and channel (mp3.md 6.7).
    fn imdct(&mut self, gr: usize, ch: usize, nz: i32, xr: &mut [f32; 576]) {
        let t = tables();
        let g = self.side[gr * 2 + ch];
        let nsb = (cdiv(nz - 1, 18) + 1).clamp(1, 32) as usize;
        let block2 = g.window_switching != 0 && g.block_type == 2;
        let cs = CS.map(f);
        let ca = CA.map(f);
        for sb in 0..nsb {
            let long = !block2 || (g.mixed != 0 && sb < 2);
            let mut out = [0f32; 18];
            if long {
                if sb < 31 && (!block2 || sb == 0) {
                    for i in 0..8 {
                        let a = xr[18 * sb + 17 - i] as f64;
                        let b = xr[18 * sb + 18 + i] as f64;
                        xr[18 * sb + 17 - i] = r(cs[i] * a - ca[i] * b);
                        xr[18 * sb + 18 + i] = r(cs[i] * b + ca[i] * a);
                    }
                }
                let bt = if block2 { 0 } else { g.block_type as usize };
                let mut x = [0f32; 18];
                x.copy_from_slice(&xr[18 * sb..18 * sb + 18]);
                imdct_long(&x, &t.win[bt & 3], &mut self.ov[ch][sb], &mut out);
            } else {
                let mut x = [0f32; 18];
                x.copy_from_slice(&xr[18 * sb..18 * sb + 18]);
                imdct_short(&x, &t.win[2], &mut self.ov[ch][sb], &mut out);
            }
            if sb & 1 == 1 {
                for k in (1..18).step_by(2) {
                    out[k] = -out[k];
                }
            }
            self.sbs[ch][sb][gr * 18..gr * 18 + 18].copy_from_slice(&out);
        }
        for sb in nsb..32 {
            for k in 0..18 {
                self.sbs[ch][sb][gr * 18 + k] = 0.0;
            }
        }
    }

    /// The synthesis driver (1061ab60): `slots` time slots of every channel.
    fn synthesis(&mut self, slots: usize) {
        let nch = if self.h[9] == 3 { 1 } else { 2 };
        for t in 0..slots {
            for ch in 0..nch {
                let mut inp = [0i32; 32];
                for (sb, x) in inp.iter_mut().enumerate() {
                    *x = to16(self.sbs[ch][sb][t]);
                }
                self.synth_slot(&inp, ch, t);
            }
        }
    }

    /// One slot of the MMX polyphase synthesis (1062f5c0).
    fn synth_slot(&mut self, inp: &[i32; 32], ch: usize, slot: usize) {
        let t = tables();
        if ch == 0 {
            self.vpos = (self.vpos - 64) & 1023;
        }
        let mut s = [0i32; 16];
        for i in 0..16 {
            s[i] = sat16(inp[i] + inp[31 - i]);
        }
        let mut ab0 = [0i32; 8];
        let mut ab1 = [0i32; 8];
        for i in 0..4 {
            ab0[i] = sat16(s[i] + s[15 - i]);
            ab1[i] = sat16(s[i] - s[15 - i]);
            ab0[4 + i] = sat16(s[4 + i] + s[11 - i]);
            ab1[4 + i] = sat16(s[4 + i] - s[11 - i]);
        }
        let mut d = [0i32; 16];
        for i in 0..8 {
            d[i] = sat16(inp[31 - i] - inp[i]);
        }
        for i in 0..4 {
            d[8 + i] = sat16(inp[11 - i] - inp[20 + i]);
            d[12 + i] = sat16(inp[15 - i] - inp[16 + i]);
        }
        let stage = |c: &[i32], x: &[i32]| -> i32 {
            let mut sum: i32 = 0;
            for k in 0..x.len() {
                sum = sum.wrapping_add(c[k].wrapping_mul(x[k]));
            }
            sat16(sum >> 14)
        };
        let mut dt = [0i32; 32];
        for o in 0..8 {
            dt[o] = stage(&t.c1[o], &ab0);
            dt[8 + o] = stage(&t.c2[o], &ab1);
        }
        for o in 0..16 {
            dt[16 + o] = stage(&t.c3[o], &d);
        }
        let neg = |x: i32| sat16(-x);
        const ROW0: [usize; 16] = [4, 24, 12, 25, 5, 26, 13, 27, 6, 28, 14, 29, 7, 30, 15, 31];
        const ROW1: [usize; 15] = [31, 15, 30, 7, 29, 14, 28, 6, 27, 13, 26, 5, 25, 12, 24];
        const ROW2: [usize; 16] = [4, 23, 11, 22, 3, 21, 10, 20, 2, 19, 9, 18, 1, 17, 8, 16];
        const ROW3: [usize; 16] = [0, 16, 8, 17, 1, 18, 9, 19, 2, 20, 10, 21, 3, 22, 11, 23];
        let vp = self.vpos as usize;
        let v = &mut self.v[ch];
        for j in 0..16 {
            v[vp + j] = neg(dt[ROW0[j]]) as i16;
        }
        v[vp + 16] = 0;
        for j in 0..15 {
            v[vp + 17 + j] = dt[ROW1[j]] as i16;
        }
        for j in 0..16 {
            v[vp + 32 + j] = dt[ROW2[j]] as i16;
            v[vp + 48 + j] = dt[ROW3[j]] as i16;
        }
        for o in 0..32 {
            let (g, j) = (o >> 3, o & 7);
            let mut acc: i32 = 0;
            for m in 0..8 {
                let a = v[(vp + o + 128 * m) & 1023] as i32;
                let b = v[(vp + o + 96 + 128 * m) & 1023] as i32;
                acc = acc.wrapping_add(a.wrapping_mul(WMMX[64 * m + 16 * g + 2 * j] as i32));
                acc = acc.wrapping_add(b.wrapping_mul(WMMX[64 * m + 16 * g + 2 * j + 1] as i32));
            }
            if acc < 0 {
                acc = acc.wrapping_add(0x3fff);
            }
            self.pcm[ch][32 * slot + o] = sat16(acc >> 14) as i16;
        }
    }
}

/// A subband sample to a 16-bit integer: round to nearest even through the 1.5*2^52 trick, the low 32 bits,
/// clamped.
fn to16(x: f32) -> i32 {
    let d = x as f64 + 6755399441055744.0;
    sat16(d.to_bits() as u32 as i32)
}

/// The long IMDCT (mp3.md 6.8).
fn imdct_long(x: &[f32; 18], w: &[f32; 36], ov: &mut [f32; 18], out: &mut [f32; 18]) {
    let k = K.map(f);
    let (k50, k54, k58, k5c, k60, k64, k68, k6c, k70, k74, k78, k7c, k80, k84, k88, k8c) = (k[0], k[1], k[2], k[3], k[4], k[5], k[6], k[7], k[8], k[9], k[10], k[11], k[12], k[13], k[14], k[15]);
    let ka0 = f(KA0);
    let x: [f64; 18] = x.map(|v| v as f64);
    let s = |a: f64, b: f64| r(a + b) as f64;
    let t1 = s(x[17], x[16]);
    let t2 = s(x[15], x[16]);
    let t3 = s(x[15], x[14]);
    let t4 = s(x[13], x[14]);
    let t5 = s(x[13], x[12]);
    let t6 = s(x[11], x[12]);
    let t7 = s(x[11], x[10]);
    let t8 = s(x[9], x[10]);
    let t9 = s(x[9], x[8]);
    let t10 = s(x[7], x[8]);
    let t11 = s(x[6], x[7]);
    let t12 = s(x[6], x[5]);
    let t13 = s(x[4], x[5]);
    let t14 = s(x[3], x[4]);
    let t15 = s(x[3], x[2]);
    let t16 = s(x[1], x[2]);
    let t17 = s(x[0], x[1]);
    let t18 = s(t3, t1);
    let t19 = s(t3, t5);
    let t20 = s(t7, t5);
    let t21 = s(t9, t7);
    let t22 = s(t9, t11);
    let t23 = s(t13, t11);
    let t24 = s(t15, t13);
    let t25 = s(t15, t17);
    let rr = |v: f64| r(v) as f64;
    let t27 = rr(((t10 + t2) - t14) * k88);
    let t28 = rr((t6 * k88) + x[0]);
    let t29 = rr(((x[0] - (t6 * k88)) - (t6 * k88)) - t27);
    let t30 = rr((t14 + t10) * k7c);
    let t31 = rr((t10 - t2) * k78);
    let t32 = rr((t27 + t27) + ((x[0] - (t6 * k88)) - (t6 * k88)));
    let t33 = rr((t28 - t30) - ((t14 + t2) * k74));
    let t34 = rr((t28 + t31) + t30);
    let t35 = rr((t8 - t4) * k6c);
    let t36 = rr(t12 * k8c);
    let t37 = rr((((t12 * k8c) + t35) + ((t16 + t8) * k70)) + t34);
    let t38 = rr(t34 - (((t12 * k8c) + t35) + ((t16 + t8) * k70)));
    let t39 = rr((t16 + t4) * k68);
    let t40 = rr(((((t14 + t2) * k74) - t31) + t28) + (((t16 + t8) * k70) + (((t16 + t4) * k68) - t36)));
    let t41 = rr(((t8 + t4) - t16) * k8c);
    let t42 = rr(((((t14 + t2) * k74) - t31) + t28) - (((t16 + t8) * k70) + (((t16 + t4) * k68) - t36)));
    let t43 = rr(t29 - t41);
    let t44 = rr(t29 + t41);
    let t45 = rr(t33 + (t35 - (t36 + t39)));
    let t46 = rr(t33 - (t35 - (t36 + t39)));
    let t47 = rr(((t22 + t18) - t24) * k88);
    let t48 = rr((t20 * k88) + t17);
    let t49 = rr(((t17 - (t20 * k88)) - (t20 * k88)) - t47);
    let t50 = rr((t22 + t24) * k7c);
    let t51 = rr((t22 - t18) * k78);
    let t52 = rr(((t47 + t47) + ((t17 - (t20 * k88)) - (t20 * k88))) * ka0);
    let t53 = rr((t48 - t50) - ((t18 + t24) * k74));
    let t54 = rr((t48 + t51) + t50);
    let t55 = rr((t21 - t19) * k6c);
    let t56 = rr(t23 * k8c);
    let t57 = rr(((((t23 * k8c) + t55) + ((t25 + t21) * k70)) + t54) * k64);
    let t58 = rr((t54 - (((t23 * k8c) + t55) + ((t25 + t21) * k70))) * k60);
    let t59 = rr((t25 + t19) * k68);
    let t60 = rr((((((t18 + t24) * k74) - t51) + t48) + (((t25 + t21) * k70) + (((t25 + t19) * k68) - t56))) * k5c);
    let t61 = rr(((t19 + t21) - t25) * k8c);
    let t62 = rr((((((t18 + t24) * k74) - t51) + t48) - (((t25 + t21) * k70) + (((t25 + t19) * k68) - t56))) * k58);
    let t63 = rr((t49 - t61) * k80);
    let t64 = rr((t49 + t61) * k84);
    let t65 = rr((t53 + (t55 - (t56 + t59))) * k54);
    let t66 = rr((t53 - (t55 - (t56 + t59))) * k50);
    let mut u = [0f64; 18];
    u[0] = rr(t37 + t57);
    u[17] = rr(t37 - t57);
    u[1] = rr(t43 + t63);
    u[16] = rr(t43 - t63);
    u[2] = rr(t45 + t65);
    u[15] = rr(t45 - t65);
    u[3] = rr(t40 + t60);
    u[14] = rr(t40 - t60);
    u[4] = rr(t32 + t52);
    u[13] = rr(t32 - t52);
    u[5] = rr(t42 + t62);
    u[12] = rr(t42 - t62);
    u[6] = rr(t46 + t66);
    u[11] = rr(t46 - t66);
    u[7] = rr(t44 + t64);
    u[10] = rr(t44 - t64);
    u[8] = rr(t38 + t58);
    u[9] = rr(t38 - t58);
    let w: [f64; 36] = w.map(|v| v as f64);
    let old = *ov;
    for j in 0..9 {
        out[j] = r(-(w[j] * u[9 + j]) + old[j] as f64);
        out[9 + j] = r((u[17 - j] * w[9 + j]) + old[9 + j] as f64);
        ov[j] = r(w[18 + j] * u[8 - j]);
        ov[9 + j] = r(w[27 + j] * u[j]);
    }
}

/// The short IMDCT (mp3.md 6.9).
fn imdct_short(x: &[f32; 18], ws: &[f32; 36], ov: &mut [f32; 18], out: &mut [f32; 18]) {
    let k = K.map(f);
    let (k80, k84, k88, k8c) = (k[12], k[13], k[14], k[15]);
    let ka0 = f(KA0);
    let ws: [f64; 36] = ws.map(|v| v as f64);
    let mut b = [0f32; 36];
    let rr = |v: f64| r(v) as f64;
    for w in 0..3 {
        let y: [f64; 6] = std::array::from_fn(|j| x[3 * j + w] as f64);
        let a1 = rr(y[5] + y[4]);
        let a2 = rr(y[3] + y[4]);
        let a3 = rr(y[2] + y[3]);
        let a4 = rr(y[2] + y[1]);
        let a5 = rr(y[1] + y[0]);
        let a6 = rr(a3 + a1);
        let a8 = rr(y[0] - a2);
        let a9 = rr(((a2 * k88) + y[0]) + (a4 * k8c));
        let a10 = rr(((a2 * k88) + y[0]) - (a4 * k8c));
        let a11 = rr(a5 - a6);
        let a12 = rr(((a6 * k88) + a5) - ((a3 + a5) * k8c));
        let a13 = rr(a12 * k84);
        let a14 = rr(a11 * ka0);
        let g = (((a3 + a5) * k8c) + ((a6 * k88) + a5)) * k80;
        let a15 = rr(g + a9);
        let a16 = rr(a14 + a8);
        let a17 = rr(a8 - a14);
        let a18 = rr(a13 + a10);
        let a19 = rr(a10 - a13);
        let o = 6 + 6 * w;
        let bb = |i: usize, b: &[f32; 36]| b[o + i] as f64;
        b[o] = r(bb(0, &b) - ((a10 - a13) * ws[0]));
        b[o + 1] = r(bb(1, &b) - (a17 * ws[1]));
        b[o + 2] = r(bb(2, &b) - ((a9 - g) * ws[2]));
        b[o + 3] = r(((a9 - g) * ws[3]) + bb(3, &b));
        b[o + 4] = r((a17 * ws[4]) + bb(4, &b));
        b[o + 5] = r((a19 * ws[5]) + bb(5, &b));
        b[o + 6] = r((a18 * ws[6]) + bb(6, &b));
        b[o + 7] = r((a16 * ws[7]) + bb(7, &b));
        b[o + 8] = r((a15 * ws[8]) + bb(8, &b));
        b[o + 9] = r((a15 * ws[9]) + bb(9, &b));
        b[o + 10] = r((a16 * ws[10]) + bb(10, &b));
        b[o + 11] = r((a18 * ws[11]) + bb(11, &b));
    }
    for k in 0..18 {
        out[k] = r(b[k] as f64 + ov[k] as f64);
        ov[k] = b[18 + k];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_the_original() {
        let t = tables();
        assert_eq!(t.pow43[2].to_bits(), 0x40214518);
        assert_eq!(t.pow43[1752].to_bits(), 0x46a501c6);
        assert_eq!(t.pow43[8191].to_bits(), 0x48213e60);
        assert_eq!(t.gain[346 - 210].to_bits(), 0x253504f3);
        assert_eq!(t.gain[346 - 1].to_bits(), 0x3f5744fd);
        assert_eq!(t.gain[346 + 45].to_bits(), 0x451837f0);
        assert_eq!(t.win[0][0].to_bits(), 0xc4843a95);
        assert_eq!(t.win[0][17].to_bits(), 0xc6bd489c);
        assert_eq!(t.win[1][20].to_bits(), 0xc697c4a9);
        assert_eq!(t.win[2][0].to_bits(), 0xc55b8f01);
        assert_eq!(t.win[2][11].to_bits(), 0xc5287927);
        assert_eq!(t.win[3][30].to_bits(), 0xc5f7e369);
        assert_eq!(to16(1524.4584), 1524);
        assert_eq!(to16(273.74814), 274);
        assert_eq!(to16(-11813.845), -11814);
        assert_eq!(to16(2.5), 2);
    }
}

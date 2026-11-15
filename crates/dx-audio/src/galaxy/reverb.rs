//! Galaxy's effect reverb (docs/galaxy/reverb.md): the parameter block `Update` builds from the viewer's zone, the
//! SSE setup (106109f0) and the SSE processor (10622d33) that runs as hook 2 on the effect dry and reverb send buses.

/// The block's values `Update` fills from a `bReverbZone` (all zero: no reverb).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ReverbParams {
    pub master: f32,
    pub cutoff: f32,
    pub delay: [f32; 6],
    pub gain: [f32; 6],
}

impl ReverbParams {
    /// The block from a zone's `MasterGain`, `CutoffHz`, `Delay[6]`, `Gain[6]` (reverb.md 3.2).
    pub fn from_zone(master: u8, cutoff_hz: i32, delay: [u8; 6], gain: [u8; 6]) -> ReverbParams {
        let mut p = ReverbParams { master: (master as f64 * (1.0f64 / 255.0)) as f32, cutoff: cutoff_hz.clamp(0, 44100) as f32, ..Default::default() };
        for i in 0..6 {
            let d = delay[i] as f32 * 0.002f32;
            p.delay[i] = if d < 0.001 {
                0.001
            } else if d >= 0.34 {
                0.34
            } else {
                d
            };
            let g = gain[i] as f32 * f32::from_bits(0x3b808081);
            p.gain[i] = if g < 0.001 {
                0.001
            } else if g >= 0.999 {
                0.999
            } else {
                g
            };
        }
        p
    }

    /// The bitwise comparison `Update` makes with the block it applied last.
    pub fn same_bits(&self, o: &ReverbParams) -> bool {
        self.master.to_bits() == o.master.to_bits()
            && self.cutoff.to_bits() == o.cutoff.to_bits()
            && self.delay.iter().zip(&o.delay).all(|(a, b)| a.to_bits() == b.to_bits())
            && self.gain.iter().zip(&o.gain).all(|(a, b)| a.to_bits() == b.to_bits())
    }
}

const LINE: usize = 16384;

/// The processor's state (0x60188 bytes in the original).
#[derive(Clone)]
pub struct Reverb {
    w: u32,
    pub taps: [i32; 6],
    pub neg_c: [f32; 6],
    pub c: [f32; 6],
    pub k: [f32; 6],
    pub lp: f32,
    pub one_minus_lp: f32,
    s: [f32; 6],
    y: [f32; 2],
    pub g: f32,
    lines: [Vec<[f32; 2]>; 3],
}

impl Reverb {
    /// The setup (106109f0) at output rate `rate`: `None` when it rejects the block (a master gain outside -1..1 or a
    /// cutoff above the rate).
    pub fn new(p: &ReverbParams, rate: u32) -> Option<Reverb> {
        if p.master < -1.0 || p.master > 1.0 || (rate as f32) < p.cutoff {
            return None;
        }
        let t = 1.0 - ((p.cutoff as f64 * 6.282f32 as f64) / rate as f64).cos();
        let lp = (((t + 2.0) * t).sqrt() - t) as f32;
        let one_minus_lp = (1.0 - lp as f64) as f32;
        let mut r = Reverb {
            w: 0,
            taps: [0; 6],
            neg_c: [0.0; 6],
            c: [0.0; 6],
            k: [0.0; 6],
            lp,
            one_minus_lp,
            s: [0.0; 6],
            y: [0.0; 2],
            g: p.master,
            lines: [vec![[0.0; 2]; LINE], vec![[0.0; 2]; LINE], vec![[0.0; 2]; LINE]],
        };
        for i in 0..6 {
            let d = if p.delay[i] == 0.0 { 0.001f32 } else { p.delay[i] };
            let mut c = 1.0 - p.gain[i] as f64;
            if c == 1.0 {
                c = 0.999f32 as f64;
            }
            r.taps[i] = -((rate as f64 * d as f64) as i32);
            r.c[i] = c as f32;
            r.neg_c[i] = -(c as f32);
            r.k[i] = (1.0 - c * c) as f32;
        }
        Some(r)
    }

    /// The processor (10622d33) over `n` frames: adds the network's output times the master gain to `dry`.
    pub fn process(&mut self, dry: &mut [[f32; 2]], send: &[[f32; 2]], n: usize) {
        for f in 0..n {
            let mut x = [(self.y[1] + send[f][0]) * 0.5, (self.y[0] + send[f][1]) * 0.5];
            let w = self.w as usize;
            for k in 0..3 {
                let mut out = [0.0f32; 2];
                let mut delayed = [0.0f32; 2];
                for ch in 0..2 {
                    let i = 2 * k + ch;
                    delayed[ch] = self.lines[k][(w as i64 + self.taps[i] as i64) as usize & (LINE - 1)][ch];
                }
                for ch in 0..2 {
                    let i = 2 * k + ch;
                    let d = delayed[ch];
                    let a = x[ch] * self.neg_c[i];
                    let fl = d * self.lp + self.s[i] * self.one_minus_lp;
                    self.s[i] = fl;
                    self.lines[k][w][ch] = x[ch] + fl * self.c[i];
                    out[ch] = a + d * self.k[i];
                }
                x = out;
            }
            self.y = x;
            dry[f][0] = x[0] * self.g + dry[f][0];
            dry[f][1] = x[1] * self.g + dry[f][1];
            self.w = (self.w + 1) & (LINE as u32 - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test vector A of reverb.md 7.2 / 7.3 (the original DLL's state and output).
    #[test]
    fn vector_a_matches_the_original() {
        let p = ReverbParams::from_zone(100, 6000, [20, 34, 0, 0, 0, 0], [150, 70, 0, 0, 0, 0]);
        assert_eq!(p.master.to_bits(), 0x3ec8c8c9);
        assert_eq!((p.delay[0].to_bits(), p.gain[0].to_bits(), p.delay[1].to_bits(), p.gain[1].to_bits()), (0x3d23d70b, 0x3f169697, 0x3d8b4396, 0x3e8c8c8d));
        assert_eq!(p.delay[2].to_bits(), 0x3a83126f);
        let mut r = Reverb::new(&p, 44100).unwrap();
        assert_eq!(r.taps, [-1764, -2998, -44, -44, -44, -44]);
        assert_eq!(r.neg_c.map(f32::to_bits), [0xbed2d2d2, 0xbf39b9ba, 0xbf7fbe77, 0xbf7fbe77, 0xbf7fbe77, 0xbf7fbe77]);
        assert_eq!(r.k.map(f32::to_bits), [0x3f54985c, 0x3ef28416, 0x3b0301a8, 0x3b0301a8, 0x3b0301a8, 0x3b0301a8]);
        assert_eq!((r.lp.to_bits(), r.one_minus_lp.to_bits()), (0x3f0dc1d2, 0x3ee47c5c));
        let n = 40000;
        let mut send = vec![[0.0f32; 2]; n];
        send[0][0] = 1.0;
        for (j, s) in send.iter_mut().enumerate().take(20400).skip(20000) {
            *s = [((j * 7919) % 2001) as f32 - 1000.0, ((j * 104729) % 2001) as f32 - 1000.0];
        }
        let mut dry = vec![[0.0f32; 2]; n];
        let mut at = 0;
        while at < n {
            let k = 441.min(n - at);
            r.process(&mut dry[at..at + k], &send[at..at + k], k);
            at += k;
        }
        let bits = |f: usize| (dry[f][0].to_bits(), dry[f][1].to_bits());
        assert_eq!(bits(0), (0xbda50570, 0));
        assert_eq!(bits(1), (0, 0x3ceef6c5));
        assert_eq!(bits(7), (0, 0x37496c6d));
        assert_eq!(bits(16383), (0xb763699e, 0xb65b4a99));
        assert_eq!(bits(16385), (0xb76551ae, 0xb63d4c46));
        assert_eq!(bits(20000), (0x41416253, 0xc204e1f7));
        assert_eq!(bits(20100), (0x42007058, 0xc0fadc86));
        assert_eq!(bits(39999), (0x3fbcb89c, 0xc04975ac));
        assert_eq!(r.w, 7232);
    }

    /// Test vector B: 22050 Hz, a denormal tail.
    #[test]
    fn vector_b_matches_the_original() {
        let p = ReverbParams::from_zone(255, 2000, [5, 40, 80, 120, 160, 170], [255, 200, 128, 64, 10, 0]);
        let mut r = Reverb::new(&p, 22050).unwrap();
        assert_eq!(r.taps, [-220, -1764, -3528, -5292, -7056, -7497]);
        assert_eq!(r.k.map(f32::to_bits), [0x3f7fffef, 0x3f74173b, 0x3f408040, 0x3ee0c0a1, 0x3d9d7a58, 0x3b0301a8]);
        assert_eq!((r.lp.to_bits(), r.one_minus_lp.to_bits()), (0x3eda10eb, 0x3f12f78a));
        let n = 20000;
        let mut send = vec![[0.0f32; 2]; n];
        send[0][1] = 1.0;
        let mut dry = vec![[0.0f32; 2]; n];
        let mut at = 0;
        while at < n {
            let k = 512.min(n - at);
            r.process(&mut dry[at..at + k], &send[at..at + k], k);
            at += k;
        }
        let bits = |f: usize| (dry[f][0].to_bits(), dry[f][1].to_bits());
        assert_eq!(bits(0), (0, 0xbda543da));
        assert_eq!(bits(1), (0x37a1f486, 0));
        assert_eq!(bits(7), (0x2024074c, 0));
        assert_eq!(bits(5000), (0x00000003, 0x80000001));
        assert_eq!(bits(16384), (0xa56b56d3, 0x243cd74a));
        assert_eq!(r.w, 3616);
    }
}

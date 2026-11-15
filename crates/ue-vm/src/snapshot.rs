//! Textures made while the game runs: the snapshots of the screen (`XRootWindow::GenerateSnapshot`) the "snapshot"
//! UI background and the saved games' thumbnails use. Spec: `docs/snapshot.md`.
//!
//! A snapshot texture exists from the call on (scripts hold it, draw it, delete it); its pixels come from the last
//! frame drawn, which only the host can read: the call queues a request (`Vm::snapshot_requests`) the host fulfils
//! before the next frame (`Vm::fill_snapshot`). Scripts never read texels, so the order is not observable.

use crate::Vm;

/// A palettized (P8) texture made at run time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimeTexture {
    pub usize: u32,
    pub vsize: u32,
    /// 256 entries, RGBA.
    pub palette: Vec<[u8; 4]>,
    /// `usize * vsize` palette indices, rows top first.
    pub data: Vec<u8>,
    /// Bumped when the pixels change.
    pub version: u64,
}

impl RuntimeTexture {
    /// The pixels as RGBA8 (index 0 opaque: the texture is not masked).
    pub fn rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() * 4);
        for &i in &self.data {
            let c = self.palette.get(i as usize).copied().unwrap_or([0, 0, 0, 255]);
            out.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        out
    }
}

/// A snapshot to take from the last frame drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotRequest {
    pub texture: u32,
    /// The root's `snapshotWidth`/`snapshotHeight` when it was asked for.
    pub width: i32,
    pub height: i32,
    /// A saved game's picture: written to this file once taken.
    pub save_file: Option<std::path::PathBuf>,
}

/// The smallest power of two at least `v` (`GenerateSnapshot`'s size table: 0 for 0, 0 above 65536).
pub fn pow2_at_least(v: i32) -> u32 {
    if v <= 0 {
        return 0;
    }
    let p = (v as u32).next_power_of_two();
    if p > 65536 {
        0
    } else {
        p
    }
}

/// The file a saved game keeps its picture in (next to `save.dxs`): `DXSNAP1`, `usize`, `vsize` (u32 LE), the 256
/// palette entries (RGBA), the indices.
pub const SAVE_FILE: &str = "snapshot.p8";

impl RuntimeTexture {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = b"DXSNAP1".to_vec();
        out.extend_from_slice(&self.usize.to_le_bytes());
        out.extend_from_slice(&self.vsize.to_le_bytes());
        for k in 0..256 {
            out.extend_from_slice(&self.palette.get(k).copied().unwrap_or([0, 0, 0, 255]));
        }
        out.extend_from_slice(&self.data);
        out
    }

    pub fn decode(b: &[u8]) -> Option<RuntimeTexture> {
        let b = b.strip_prefix(b"DXSNAP1")?;
        let u = u32::from_le_bytes(b.get(0..4)?.try_into().ok()?);
        let v = u32::from_le_bytes(b.get(4..8)?.try_into().ok()?);
        let pal = b.get(8..8 + 1024)?;
        let data = b.get(8 + 1024..8 + 1024 + (u as usize * v as usize))?.to_vec();
        let palette = pal.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        Some(RuntimeTexture { usize: u, vsize: v, palette, data, version: 1 })
    }
}

impl Vm {
    /// A new transient texture of `usize` x `vsize` (all index 0, the grey ramp palette `(i, i, i, 255)`); its id is a
    /// fresh object path.
    pub fn new_runtime_texture(&mut self, usize: u32, vsize: u32) -> u32 {
        self.runtime_serial += 1;
        let id = self.reg.obj_id(&format!("Transient.Snapshot{}", self.runtime_serial)).0;
        let palette = (0..256u32).map(|i| [i as u8, i as u8, i as u8, 255]).collect();
        self.runtime_textures.insert(id, RuntimeTexture { usize, vsize, palette, data: vec![0; (usize as usize) * (vsize as usize)], version: 1 });
        id
    }

    /// The offsets of `RootWindow`'s own `snapshotWidth`/`snapshotHeight` ints (`DeusExRootWindow` declares float
    /// variables of the same names that hide them by name).
    fn snapshot_size_offsets(&self) -> Option<(usize, usize)> {
        let c = self.reg.class_by_name("RootWindow")?;
        Some((self.reg.find_prop(c, "snapshotWidth")?.offset as usize, self.reg.find_prop(c, "snapshotHeight")?.offset as usize))
    }

    /// `XRootWindow::SetSnapshotSize` (10039d50): both truncated toward zero into the root's ints.
    pub fn set_snapshot_size(&mut self, root: u32, w: f32, h: f32) {
        if let Some((ow, oh)) = self.snapshot_size_offsets() {
            self.set_u32_at(root, ow, crate::route::ftol(w as f64) as u32);
            self.set_u32_at(root, oh, crate::route::ftol(h as f64) as u32);
        }
    }

    /// `XRootWindow::GenerateSnapshot(NULL, ...)` for `root`: a new texture sized from the root's `snapshotWidth`/
    /// `snapshotHeight`, filled from the last frame by the host before the next one is drawn.
    pub fn request_snapshot(&mut self, root: u32, save_file: Option<std::path::PathBuf>) -> u32 {
        let (w, h) = match self.snapshot_size_offsets() {
            Some((ow, oh)) => (self.u32_at(root, ow) as i32, self.u32_at(root, oh) as i32),
            None => (0, 0),
        };
        let tex = self.new_runtime_texture(pow2_at_least(w), pow2_at_least(h));
        self.snapshot_requests.push(SnapshotRequest { texture: tex, width: w, height: h, save_file });
        tex
    }

    /// The host gives the last frame drawn (`w` x `h` RGBA8, top row first, before the window's gamma) and the
    /// client's `Brightness` to every queued snapshot; saved games' pictures are written to their files.
    pub fn fill_snapshots(&mut self, w: u32, h: u32, rgba: &[u8], brightness: f32) -> Vec<u32> {
        let reqs = std::mem::take(&mut self.snapshot_requests);
        let mut done = Vec::new();
        for r in reqs {
            if let Some(t) = self.runtime_textures.get_mut(&r.texture) {
                generate(t, w, h, rgba, r.width, r.height, brightness);
                t.version += 1;
                if let Some(f) = &r.save_file {
                    let _ = std::fs::write(f, t.encode());
                }
                done.push(r.texture);
            }
        }
        done
    }

    /// A saved game's picture as a texture (`None` without one).
    pub fn load_save_snapshot(&mut self, file: &std::path::Path) -> Option<u32> {
        let t = RuntimeTexture::decode(&std::fs::read(file).ok()?)?;
        self.runtime_serial += 1;
        let id = self.reg.obj_id(&format!("Transient.Snapshot{}", self.runtime_serial)).0;
        self.runtime_textures.insert(id, t);
        Some(id)
    }
}

/// The D3D device's `ReadPixels` brightness table (D3DDrv 10007610, with the gamma control the shipped settings use):
/// `g = 1.5 * Brightness`, entry `i` = `round(pow(i/255, 1/g) * 255 + 0.5 - 0.5)`.
fn readback_table(brightness: f32) -> [u8; 256] {
    let g = brightness * 1.5f32;
    let inv = 1.0f32 / g;
    let mut t = [0u8; 256];
    for (i, e) in t.iter_mut().enumerate() {
        let x = i as f32 * (1.0f32 / 255.0);
        let v = ((x as f64).powf(inv as f64) * 255.0 + 0.5) as f32;
        let r = (v - 0.5f32).round_ties_even();
        *e = r.clamp(0.0, 255.0) as u8;
    }
    t
}

/// `GenerateSnapshot`'s greyscale path (Extension 10039410; the only one any caller reaches): the frame box-averaged
/// down to `sw` x `sh` texels (f32 steps, `ceil(step)` taps per side, overlapping when the step is fractional), each
/// channel back to a byte, the texel the average of the three; texels outside `sw` x `sh` stay 0.
pub fn generate(t: &mut RuntimeTexture, w: u32, h: u32, rgba: &[u8], sw: i32, sh: i32, brightness: f32) {
    t.palette = (0..256u32).map(|i| [i as u8, i as u8, i as u8, 255]).collect();
    t.data = vec![0; (t.usize as usize) * (t.vsize as usize)];
    if w == 0 || h == 0 || sw <= 0 || sh <= 0 || rgba.len() < (w * h * 4) as usize {
        return;
    }
    let table = readback_table(brightness);
    let (fw, fh) = (w as f32, h as f32);
    let step_x = fw / sw as f32;
    let step_y = fh / sh as f32;
    let k = 1.0f64 / 255.0;
    let pw = t.usize as usize;
    let (mut row, mut src_y, mut row_off) = (0i32, 0.0f32, 0usize);
    if 0.0 < fh {
        loop {
            let (mut col, mut src_x) = (0i32, 0.0f32);
            if 0.0 < fw {
                loop {
                    if col < sw && row < sh {
                        let (mut s, mut n) = ([0.0f32; 3], 0u32);
                        let mut dx = 0i32;
                        while (dx as f32) < step_x {
                            let fx = dx as f32 + src_x;
                            let mut dy = 0i32;
                            while (dy as f32) < step_y {
                                if fx < fw && (dy as f32 + src_y) < fh {
                                    n += 1;
                                    let px = (src_x.trunc() as i64 + dx as i64) as usize;
                                    let py = (src_y.trunc() as i64 + dy as i64) as usize;
                                    let i = (py * w as usize + px) * 4;
                                    for c in 0..3 {
                                        let b = table[rgba.get(i + c).copied().unwrap_or(0) as usize];
                                        s[c] += (b as f64 * k) as f32;
                                    }
                                }
                                dy += 1;
                            }
                            dx += 1;
                        }
                        let inv = (1.0f64 / n.max(1) as f64) as f32;
                        let mut bytes = [0u32; 3];
                        for c in 0..3 {
                            let v = (s[c] * inv) * 256.0f32;
                            let q = (v - 0.5f32).round_ties_even();
                            bytes[c] = if q < 0.0 { 0 } else if q >= 255.0 { 255 } else { q as u32 };
                        }
                        if let Some(d) = t.data.get_mut(row_off + col as usize) {
                            *d = ((bytes[0] + bytes[1] + bytes[2]) / 3) as u8;
                        }
                    }
                    col += 1;
                    src_x += step_x;
                    if !(src_x < fw) {
                        break;
                    }
                }
            }
            src_y += step_y;
            row_off += pw;
            row += 1;
            if !(src_y < fh) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_to_one_snapshot_keeps_the_grey_levels() {
        // brightness 2/3: g = 1, the readback table is the identity
        let (w, h) = (4u32, 2u32);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [(i * 30) as u8, (i * 30) as u8, (i * 30) as u8, 255]).collect();
        let mut t = RuntimeTexture { usize: 4, vsize: 2, ..Default::default() };
        generate(&mut t, w, h, &rgba, 4, 2, 2.0 / 3.0);
        assert_eq!(t.data, (0..8u32).map(|i| (i * 30) as u8).collect::<Vec<_>>());
        assert_eq!(t.palette[200], [200, 200, 200, 255]);
    }

    #[test]
    fn boxes_average_and_the_rest_stays_black() {
        let (w, h) = (4u32, 4u32);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| { let v = if i % 2 == 0 { 0 } else { 200 }; [v, v, v, 255] }).collect();
        let mut t = RuntimeTexture { usize: 4, vsize: 2, ..Default::default() };
        generate(&mut t, w, h, &rgba, 2, 2, 2.0 / 3.0);
        // 2x2 boxes of 0/200 columns: 100
        assert_eq!(&t.data[0..2], &[100, 100]);
        assert_eq!(&t.data[4..6], &[100, 100]);
        assert_eq!(&t.data[2..4], &[0, 0]);
    }

    #[test]
    fn sizes_and_files() {
        assert_eq!((pow2_at_least(0), pow2_at_least(1), pow2_at_least(3), pow2_at_least(160), pow2_at_least(192)), (0, 1, 4, 256, 256));
        let t = RuntimeTexture { usize: 2, vsize: 1, palette: vec![[1, 2, 3, 4]; 256], data: vec![5, 6], version: 3 };
        let back = RuntimeTexture::decode(&t.encode()).unwrap();
        assert_eq!((back.usize, back.vsize, back.data.clone(), back.palette[7]), (2, 1, vec![5, 6], [1, 2, 3, 4]));
    }
}

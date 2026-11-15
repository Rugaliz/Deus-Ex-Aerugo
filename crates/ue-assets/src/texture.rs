//! `Palette` and `Texture` objects (spec: `docs/formats/texture-format.md`).

use crate::payload;
use ue_package::{Error, Package, Reader, Result};

/// A 256-entry (normally) colour table. Entries are R, G, B, A.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub colors: Vec<[u8; 4]>,
}

impl Palette {
    pub fn load(pkg: &Package, export: usize) -> Result<Palette> {
        let p = payload(pkg, export)?;
        let mut r = Reader::new(p.data, p.native_start);
        let n = r.compact_index()?;
        if n < 0 || n as usize * 4 > p.data.len() {
            return Err(Error::Invalid(format!("palette colour count {n}")));
        }
        let mut colors = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let c = r.bytes(4)?;
            colors.push([c[0], c[1], c[2], c[3]]);
        }
        if r.pos() != p.data.len() {
            return Err(Error::Invalid(format!("palette has {} trailing bytes", p.data.len() - r.pos())));
        }
        // Packages older than version 66 do not store alpha: it is forced opaque.
        if pkg.version < 66 {
            for c in &mut colors {
                c[3] = 0xFF;
            }
        }
        Ok(Palette { colors })
    }
}

/// `ETextureFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 8-bit palette indices.
    P8,
    Rgb32,
    Rgb64,
    /// DXT1 block-compressed.
    Dxt1,
    Rgb24,
}

impl Format {
    /// Stored size of one mip level.
    pub fn mip_bytes(self, u: u32, v: u32) -> usize {
        let (u, v) = (u as usize, v as usize);
        match self {
            Format::P8 => u * v,
            Format::Rgb32 => u * v * 4,
            Format::Rgb64 => u * v * 8,
            Format::Rgb24 => u * v * 3,
            Format::Dxt1 => u.div_ceil(4).max(1) * v.div_ceil(4).max(1) * 8,
        }
    }

    fn from_byte(b: u8) -> Result<Format> {
        Ok(match b {
            0 => Format::P8,
            1 => Format::Rgb32,
            2 => Format::Rgb64,
            3 => Format::Dxt1,
            4 => Format::Rgb24,
            _ => return Err(Error::Invalid(format!("texture format {b}"))),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Mip {
    pub u_size: u32,
    pub v_size: u32,
    pub u_bits: u8,
    pub v_bits: u8,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Texture {
    pub format: Format,
    /// Raw object reference (compact index) of the palette, 0 if none.
    pub palette: i32,
    pub u_size: u32,
    pub v_size: u32,
    pub masked: bool,
    /// The texture's own blend flags (`bTransparent`, `bModulate`): actors and decals drawn with it blend that way.
    pub translucent: bool,
    pub modulated: bool,
    /// `SourceTexture` of a wet texture (the picture its water distorts), else 0 (an object reference).
    pub source_texture: i32,
    /// `AnimNext`: the next frame of an animated texture (an object reference), 0 if none.
    pub anim_next: i32,
    /// `DetailTexture` (object reference, 0 for none) and `DrawScale` (the texture's `UScale`/`VScale`, default 1).
    pub detail_texture: i32,
    pub draw_scale: f32,
    pub has_comp: bool,
    pub parametric: bool,
    pub mips: Vec<Mip>,
    pub comp_mips: Vec<Mip>,
}

fn read_mips(r: &mut Reader, file_base: usize, end_marker: bool) -> Result<Vec<Mip>> {
    let n = r.compact_index()?;
    if !(0..=32).contains(&n) {
        return Err(Error::Invalid(format!("mip count {n}")));
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        // lazy array: absolute file position of the end of the data (package version 62 and later), then the
        // counted bytes
        let end = if end_marker { Some(r.i32()?) } else { None };
        let len = r.compact_index()?;
        if len < 0 {
            return Err(Error::Invalid(format!("mip data length {len}")));
        }
        let data = r.bytes(len as usize)?.to_vec();
        if let Some(end) = end {
            if end as usize != file_base + r.pos() {
                return Err(Error::Invalid(format!("mip end marker {end} != {}", file_base + r.pos())));
            }
        }
        let u_size = r.i32()?;
        let v_size = r.i32()?;
        let (u_bits, v_bits) = (r.u8()?, r.u8()?);
        if u_size < 0 || v_size < 0 {
            return Err(Error::Invalid("negative mip size".into()));
        }
        out.push(Mip { u_size: u_size as u32, v_size: v_size as u32, u_bits, v_bits, data });
    }
    Ok(out)
}

impl Texture {
    /// Loads a plain `Texture` object (the procedural subclasses may add their own payload: see
    /// [`Texture::load_prefix`] and the `procedural` module).
    pub fn load(pkg: &Package, export: usize) -> Result<Texture> {
        let (t, end) = Texture::load_prefix(pkg, export)?;
        let total = pkg.export_bytes(export)?.len();
        if end != total {
            return Err(Error::Invalid(format!("texture has {} trailing bytes", total - end)));
        }
        Ok(t)
    }

    /// Like `load`, but also for procedural textures (a `FireTexture` stores its sparks after the texture data): the
    /// texture part only, for its flags, size and palette.
    pub fn load_any(pkg: &Package, export: usize) -> Result<Texture> {
        Texture::load_prefix(pkg, export).map(|(t, _)| t)
    }

    /// Loads the part of a texture object that `UTexture::Serialize` covers and returns it with the position (in the
    /// object's data) where that part ends.
    pub fn load_prefix(pkg: &Package, export: usize) -> Result<(Texture, usize)> {
        let p = payload(pkg, export)?;
        let format = Format::from_byte(p.byte(pkg, "Format").unwrap_or(0))?;
        let has_comp = p.bool(pkg, "bHasComp");
        let parametric = p.bool(pkg, "bParametric");
        let mut r = Reader::new(p.data, p.native_start);
        let base = pkg.exports[export].serial_offset as usize;
        let marker = pkg.version >= 62;
        let mips = read_mips(&mut r, base, marker)?;
        let comp_mips = if has_comp { read_mips(&mut r, base, marker)? } else { Vec::new() };
        for m in mips.iter().chain(&comp_mips) {
            let want = format.mip_bytes(m.u_size, m.v_size);
            if !parametric && m.data.len() != want {
                return Err(Error::Invalid(format!("{:?} mip {}x{} has {} bytes, expected {want}", format, m.u_size, m.v_size, m.data.len())));
            }
        }
        let end = r.pos();
        Ok((Texture {
            format,
            palette: p.object(pkg, "Palette").unwrap_or(0),
            u_size: p.int(pkg, "USize").unwrap_or(0) as u32,
            v_size: p.int(pkg, "VSize").unwrap_or(0) as u32,
            masked: p.bool(pkg, "bMasked"),
            translucent: p.bool(pkg, "bTransparent"),
            modulated: p.bool(pkg, "bModulate"),
            source_texture: p.object(pkg, "SourceTexture").unwrap_or(0),
            anim_next: p.object(pkg, "AnimNext").unwrap_or(0),
            detail_texture: p.object(pkg, "DetailTexture").unwrap_or(0),
            draw_scale: p.float(pkg, "DrawScale").unwrap_or(1.0),
            has_comp,
            parametric,
            mips,
            comp_mips,
        }, end))
    }
}

impl Texture {
    /// Decodes one mip level of a palettised texture to RGBA8 (row-major, top row first). When the texture is masked,
    /// palette index 0 becomes transparent black `(0, 0, 0, 0)`, as the original hardware renderers load it (so a masked
    /// texture drawn translucent adds nothing there, and filtered edges darken). Other formats are not decoded yet.
    pub fn mip_rgba(&self, level: usize, palette: &Palette) -> Result<Vec<u8>> {
        self.mip_rgba_with(level, palette, self.masked)
    }

    /// Like `mip_rgba`, but with palette index 0 transparent when `masked` says so (the user interface draws any texture
    /// masked on request, whatever the texture's own flag).
    pub fn mip_rgba_with(&self, level: usize, palette: &Palette, masked: bool) -> Result<Vec<u8>> {
        let m = self.mips.get(level).ok_or_else(|| Error::Invalid(format!("no mip level {level}")))?;
        match self.format {
            Format::P8 => {}
            // high-colour textures of mods (texture packs): no palette
            Format::Dxt1 => return Ok(decode_dxt1(&m.data, m.u_size, m.v_size)),
            Format::Rgb32 if m.data.len() >= (m.u_size * m.v_size * 4) as usize => {
                // stored B, G, R, A
                return Ok(m.data.chunks_exact(4).take((m.u_size * m.v_size) as usize).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect());
            }
            _ => return Err(Error::Invalid(format!("decoding {:?} is not implemented", self.format))),
        }
        let mut out = Vec::with_capacity(m.data.len() * 4);
        for &i in &m.data {
            let mut c = palette.colors.get(i as usize).copied().unwrap_or([0, 0, 0, 255]);
            c[3] = 255;
            if masked && i == 0 {
                c = [0, 0, 0, 0];
            }
            out.extend_from_slice(&c);
        }
        Ok(out)
    }
}

/// DXT1 (S3TC BC1) blocks to RGBA8: each 4x4 block two RGB565 end colours and 2-bit indices; when the first colour is
/// not above the second, index 3 is transparent black (1-bit alpha).
pub fn decode_dxt1(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let bw = w.div_ceil(4).max(1);
    let rgb = |c: u16| [((c >> 11) & 31) as u32 * 255 / 31, ((c >> 5) & 63) as u32 * 255 / 63, (c & 31) as u32 * 255 / 31];
    for (bi, block) in data.chunks_exact(8).enumerate() {
        let (bx, by) = ((bi % bw) * 4, (bi / bw) * 4);
        if by >= h {
            break;
        }
        let (c0, c1) = (u16::from_le_bytes([block[0], block[1]]), u16::from_le_bytes([block[2], block[3]]));
        let (a, b) = (rgb(c0), rgb(c1));
        let mut pal = [[a[0], a[1], a[2], 255], [b[0], b[1], b[2], 255], [0; 4], [0; 4]];
        if c0 > c1 {
            pal[2] = [(2 * a[0] + b[0]) / 3, (2 * a[1] + b[1]) / 3, (2 * a[2] + b[2]) / 3, 255];
            pal[3] = [(a[0] + 2 * b[0]) / 3, (a[1] + 2 * b[1]) / 3, (a[2] + 2 * b[2]) / 3, 255];
        } else {
            pal[2] = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2, (a[2] + b[2]) / 2, 255];
        }
        let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        for i in 0..16 {
            let (x, y) = (bx + i % 4, by + i / 4);
            if x < w && y < h {
                let c = pal[((bits >> (2 * i)) & 3) as usize];
                let o = (y * w + x) * 4;
                for k in 0..4 {
                    out[o + k] = c[k] as u8;
                }
            }
        }
    }
    out
}

impl Palette {
    /// No colours: what a texture without a palette (DXT1, RGB32) decodes with.
    pub fn none() -> Palette {
        Palette { colors: Vec::new() }
    }
}

#[cfg(test)]
mod dxt1_tests {
    #[test]
    fn a_block_decodes_its_end_colours_and_transparency() {
        // c0 = pure red (0xF800), c1 = pure blue (0x001F); c0 > c1: four opaque colours; indices 0,1,2,3 in row 0
        let block = [0x00, 0xF8, 0x1F, 0x00, 0b11_10_01_00, 0, 0, 0];
        let px = super::decode_dxt1(&block, 4, 4);
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(&px[4..8], &[0, 0, 255, 255]);
        assert_eq!(&px[8..12], &[170, 0, 85, 255]);
        // c0 <= c1: index 3 transparent
        let block = [0x1F, 0x00, 0x00, 0xF8, 0b11_00_00_00, 0, 0, 0];
        assert_eq!(&super::decode_dxt1(&block, 4, 4)[12..16], &[0, 0, 0, 0]);
    }
}

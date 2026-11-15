//! `Font` objects: glyph rectangles on texture pages (spec: `docs/formats/font-format.md`).

use crate::payload;
use ue_package::{Error, Package, Reader, Result};

/// A glyph's rectangle on its page texture, in texels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub start_u: i32,
    pub start_v: i32,
    pub u_size: i32,
    pub v_size: i32,
}

#[derive(Debug, Clone)]
pub struct FontPage {
    /// Raw object reference (compact index) of the page texture.
    pub texture: i32,
    pub glyphs: Vec<Glyph>,
}

#[derive(Debug, Clone)]
pub struct Font {
    pub pages: Vec<FontPage>,
    /// Glyphs per page (a power of two; 256 / characters-per-page pages cover the 8-bit character set).
    pub characters_per_page: i32,
}

impl Font {
    pub fn load(pkg: &Package, export: usize) -> Result<Font> {
        let p = payload(pkg, export)?;
        let r = &mut Reader::new(p.data, p.native_start);
        let n = r.compact_index()?;
        if !(0..=256).contains(&n) {
            return Err(Error::Invalid(format!("font page count {n}")));
        }
        let mut pages = Vec::new();
        for _ in 0..n {
            let texture = r.compact_index()?;
            let g = r.compact_index()?;
            if !(0..=65536).contains(&g) {
                return Err(Error::Invalid(format!("glyph count {g}")));
            }
            let glyphs = (0..g)
                .map(|_| Ok(Glyph { start_u: r.i32()?, start_v: r.i32()?, u_size: r.i32()?, v_size: r.i32()? }))
                .collect::<Result<Vec<_>>>()?;
            pages.push(FontPage { texture, glyphs });
        }
        let characters_per_page = r.i32()?;
        if characters_per_page <= 0 || characters_per_page & (characters_per_page - 1) != 0 {
            return Err(Error::Invalid(format!("characters per page {characters_per_page}")));
        }
        if r.pos() != p.data.len() {
            return Err(Error::Invalid(format!("font has {} trailing bytes", p.data.len() as i64 - r.pos() as i64)));
        }
        Ok(Font { pages, characters_per_page })
    }

    /// The glyph for an 8-bit character code, if the font has one.
    pub fn glyph(&self, ch: u8) -> Option<(&FontPage, Glyph)> {
        let per = self.characters_per_page as usize;
        let page = self.pages.get(ch as usize / per)?;
        Some((page, *page.glyphs.get(ch as usize % per)?))
    }
}

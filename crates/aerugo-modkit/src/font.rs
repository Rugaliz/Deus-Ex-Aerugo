//! Text in the viewer, drawn with one of the game's own fonts (from the player's game data, `DeusExUI`).

use crate::catalog::Catalog;
use crate::render::Canvas;
use ue_assets::font::{Font, Glyph};
use ue_assets::texture::{Palette, Texture};

pub struct Text {
    /// Per page: width, height and coverage (texel not transparent).
    pages: Vec<(u32, u32, Vec<bool>)>,
    font: Font,
    pub height: i32,
}

impl Text {
    /// The first of `names` (a font export's name in `DeusExUI` or `Engine`) the game has.
    pub fn load(cat: &Catalog, names: &[&str]) -> Option<Text> {
        for pkg_name in ["DeusExUI", "Engine"] {
            let Some(pkg) = cat.files.package(pkg_name) else { continue };
            for name in names {
                let Some(e) = (0..pkg.pkg.exports.len()).find(|&i| pkg.pkg.export_class_name(i) == "Font" && pkg.pkg.name(pkg.pkg.exports[i].object_name).eq_ignore_ascii_case(name)) else { continue };
                let Ok(font) = Font::load(&pkg.pkg, e) else { continue };
                let mut pages = Vec::new();
                for p in &font.pages {
                    let page = (|| {
                        let (tp, te) = cat.files.resolve(&pkg, p.texture)?;
                        let t = Texture::load_any(&tp.pkg, te).ok()?;
                        let pal = cat.files.resolve(&tp, t.palette).and_then(|(pp, pe)| Palette::load(&pp.pkg, pe).ok()).unwrap_or_else(Palette::none);
                        let rgba = t.mip_rgba_with(0, &pal, true).ok()?;
                        let cover = rgba.chunks_exact(4).map(|c| c[3] > 0 && (c[0] as u32 + c[1] as u32 + c[2] as u32) > 60).collect();
                        Some((t.mips[0].u_size, t.mips[0].v_size, cover))
                    })();
                    pages.push(page.unwrap_or((0, 0, Vec::new())));
                }
                let height = font.pages.iter().flat_map(|p| p.glyphs.iter().map(|g| g.v_size)).max().unwrap_or(10).max(8);
                return Some(Text { pages, font, height });
            }
        }
        None
    }

    fn glyph(&self, ch: u8) -> Option<(usize, Glyph)> {
        let per = self.font.characters_per_page as usize;
        let g = *self.font.pages.get(ch as usize / per)?.glyphs.get(ch as usize % per)?;
        Some((ch as usize / per, g))
    }

    /// Draws `s` with its top-left corner at (x, y); returns the x after it.
    pub fn draw(&self, c: &mut Canvas, x: i32, y: i32, s: &str, col: [u8; 4]) -> i32 {
        let mut cx = x;
        for ch in s.bytes() {
            let Some((pi, g)) = self.glyph(ch) else { continue };
            let (pw, ph, cover) = &self.pages[pi];
            for gy in 0..g.v_size {
                for gx in 0..g.u_size {
                    let (u, v) = ((g.start_u + gx) as u32, (g.start_v + gy) as u32);
                    if u >= *pw || v >= *ph || !cover[(v * pw + u) as usize] {
                        continue;
                    }
                    let (px, py) = (cx + gx, y + gy);
                    if px >= 0 && py >= 0 && (px as usize) < c.w && (py as usize) < c.h {
                        c.blend(px as usize, py as usize, col);
                    }
                }
            }
            cx += g.u_size;
        }
        cx
    }
}

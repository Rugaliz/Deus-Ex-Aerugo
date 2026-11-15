//! Loads every Font and checks its glyph rectangles against the page textures. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_assets::files::GameFiles;
use ue_assets::font::Font;
use ue_assets::texture::Texture;

#[test]
fn all_fonts_load_with_glyphs_inside_their_pages() {
    let Some(root) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let files = GameFiles::new(&root);
    let mut fonts = 0;
    for package in ["DeusExUI", "Engine", "Extension", "DXFonts"] {
        let pkg = files.package(package).unwrap_or_else(|| panic!("{package} missing"));
        for i in 0..pkg.pkg.exports.len() {
            if pkg.pkg.export_class_name(i) != "Font" {
                continue;
            }
            let font = Font::load(&pkg.pkg, i).unwrap_or_else(|e| panic!("{package} #{i}: {e}"));
            fonts += 1;
            assert_eq!(font.pages.len() * font.characters_per_page as usize, 256, "{package} #{i}: covers the 8-bit character set");
            for page in &font.pages {
                assert_eq!(page.glyphs.len(), font.characters_per_page as usize, "{package} #{i}");
                let (tp, te) = files.resolve(&pkg, page.texture).unwrap_or_else(|| panic!("{package} #{i}: page texture"));
                let t = Texture::load(&tp.pkg, te).unwrap();
                for g in &page.glyphs {
                    assert!(g.u_size >= 0 && g.v_size >= 0 && g.start_u >= 0 && g.start_v >= 0, "{package} #{i}");
                    assert!((g.start_u + g.u_size) as u32 <= t.u_size && (g.start_v + g.v_size) as u32 <= t.v_size, "{package} #{i}: glyph outside page");
                }
            }
        }
    }
    assert_eq!(fonts, 35);
}

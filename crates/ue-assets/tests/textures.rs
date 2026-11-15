//! Loads every Texture and Palette in the game's Textures folder. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_assets::texture::{Format, Palette, Texture};
use ue_package::Package;

fn texture_files() -> Option<Vec<PathBuf>> {
    let dir = PathBuf::from(std::env::var_os("DEUSEX_DIR")?).join("Textures");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("utx")).unwrap_or(false))
        .collect();
    v.sort();
    Some(v)
}

#[test]
fn all_textures_and_palettes_load_with_consistent_mip_chains() {
    let Some(files) = texture_files() else { return };
    let (mut textures, mut palettes) = (0, 0);
    for f in files {
        let pkg = Package::open(&f).unwrap();
        for i in 0..pkg.exports.len() {
            match pkg.export_class_name(i).as_str() {
                "Palette" => {
                    let p = Palette::load(&pkg, i).unwrap_or_else(|e| panic!("{} #{i}: {e}", f.display()));
                    assert_eq!(p.colors.len(), 256, "{} #{i}", f.display());
                    palettes += 1;
                }
                "Texture" => {
                    let t = Texture::load(&pkg, i).unwrap_or_else(|e| panic!("{} #{i}: {e}", f.display()));
                    assert_eq!(t.format, Format::P8);
                    let m0 = &t.mips[0];
                    // the size properties describe the largest level, and every level halves the previous one
                    assert_eq!((m0.u_size, m0.v_size), (t.u_size, t.v_size), "{} #{i}", f.display());
                    assert_eq!((1u32 << m0.u_bits, 1u32 << m0.v_bits), (m0.u_size, m0.v_size));
                    for w in t.mips.windows(2) {
                        assert_eq!(w[1].u_size, (w[0].u_size / 2).max(1), "{} #{i}", f.display());
                        assert_eq!(w[1].v_size, (w[0].v_size / 2).max(1), "{} #{i}", f.display());
                    }
                    textures += 1;
                }
                _ => {}
            }
        }
    }
    // counts for the retail 1112fm install (the other textures live inside map and font packages)
    assert_eq!((textures, palettes), (2446, 1061));
}

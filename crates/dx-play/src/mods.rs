//! Helpers for the mods folder (`ue_package::mods`).

use std::path::Path;
use ue_assets::files::GameFiles;
use ue_assets::texture::{Palette, Texture};
use ue_package::ObjectRef;

/// Writes every texture of the game's texture packages (`Textures/*.utx`) as a PNG into `out`, at the place a
/// replacement goes in a mod: `Textures/<Package>/<Group>/<Name>.png`. Returns how many were written.
pub fn export_textures(game: &Path, out: &Path) -> Result<usize, String> {
    let files = GameFiles::new(game);
    let mut names: Vec<String> = std::fs::read_dir(game.join("Textures"))
        .map_err(|e| format!("{}: {e}", game.join("Textures").display()))?
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".utx").or_else(|| n.strip_suffix(".UTX")).map(str::to_string)))
        .collect();
    names.sort();
    let mut n = 0;
    for name in names {
        let Some(pkg) = files.package(&name) else { continue };
        for e in 0..pkg.pkg.exports.len() {
            if pkg.pkg.export_class_name(e) != "Texture" {
                continue;
            }
            let Ok(t) = Texture::load(&pkg.pkg, e) else { continue };
            let pal = match files.resolve(&pkg, t.palette) {
                Some((pp, pe)) => Palette::load(&pp.pkg, pe).unwrap_or_else(|_| Palette::none()),
                None => Palette::none(),
            };
            let Ok(rgba) = t.mip_rgba(0, &pal) else { continue };
            let Some(path) = pkg.pkg.object_path(ObjectRef::Export(e)) else { continue };
            let file = out.join("Textures").join(&name).join(format!("{}.png", path.replace('.', "/")));
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&file, dx_soft::png(t.mips[0].u_size, t.mips[0].v_size, &rgba)).map_err(|e| format!("{}: {e}", file.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

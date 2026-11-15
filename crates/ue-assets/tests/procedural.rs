//! Loads every procedural texture in the game. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_assets::procedural::ProceduralTexture;
use ue_package::Package;

#[test]
fn all_procedural_textures_load() {
    let Some(root) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let (mut fire, mut wet, mut wave, mut sparks) = (0, 0, 0, 0);
    for folder in ["Textures", "Maps", "System"] {
        for e in std::fs::read_dir(root.join(folder)).unwrap().flatten() {
            let path = e.path();
            let Ok(pkg) = Package::open(&path) else { continue };
            let name = path.display();
            for i in 0..pkg.exports.len() {
                let class = pkg.export_class_name(i);
                if !matches!(class.as_str(), "FireTexture" | "WetTexture" | "WaveTexture") {
                    continue;
                }
                let t = ProceduralTexture::load(&pkg, i).unwrap_or_else(|e| panic!("{name} #{i} [{class}]: {e}"));
                assert!(t.texture.u_size.is_power_of_two() && t.texture.v_size.is_power_of_two(), "{name} #{i}");
                assert!(t.sparks.iter().all(|s| s.kind <= 28), "{name} #{i}: spark type out of range");
                sparks += t.sparks.len();
                match class.as_str() {
                    "FireTexture" => fire += 1,
                    "WetTexture" => wet += 1,
                    _ => wave += 1,
                }
            }
        }
    }
    // counts for the retail 1112fm install
    assert_eq!((fire, wet, wave, sparks), (40, 8, 1, 2909));
}

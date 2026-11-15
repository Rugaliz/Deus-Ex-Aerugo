//! The mods folder (`ue_package::mods`): a zip in `<game>/Mods/` adds a map and replaces a texture's picture. Runs on
//! a stand-in game folder that links to the real one (`DEUSEX_DIR`), so nothing is written into the game. Its own
//! test binary: the mod index is one per process.

use std::io::Write;
use std::path::{Path, PathBuf};
use ue_package::ObjectRef;

fn stand_in_game(real: &Path) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aerugo-mods-game-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Mods")).unwrap();
    for sub in ["System", "Maps", "Textures", "Sounds", "Music"] {
        #[cfg(unix)]
        std::os::unix::fs::symlink(real.join(sub), dir.join(sub)).unwrap();
    }
    dir
}

/// The first `Texture` of a texture package of the game: its object path and size.
fn some_texture(game: &Path) -> (String, (u32, u32)) {
    let files = ue_assets::files::GameFiles::new(game);
    let pkg = files.package("CoreTexBrick").expect("CoreTexBrick.utx");
    let e = (0..pkg.pkg.exports.len()).find(|&i| pkg.pkg.export_class_name(i) == "Texture").unwrap();
    let t = ue_assets::texture::Texture::load_any(&pkg.pkg, e).unwrap();
    (format!("CoreTexBrick.{}", pkg.pkg.object_path(ObjectRef::Export(e)).unwrap()), (t.u_size, t.v_size))
}

#[test]
fn a_zip_mod_adds_a_map_and_replaces_a_texture() {
    let Some(real) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let game = stand_in_game(&real);
    let (tex_path, own) = some_texture(&real);
    // the replacement: four times as big, solid green
    let (w, h) = (own.0 * 4, own.1 * 4);
    let png = dx_soft::png(w, h, &[0u8, 255, 0, 255].repeat((w * h) as usize));
    {
        let mut z = zip::ZipWriter::new(std::fs::File::create(game.join("Mods/TestMod.zip")).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        z.start_file("TestMod/Maps/ModMap.dx", opts).unwrap();
        z.write_all(&std::fs::read(real.join("Maps/00_Training.dx")).unwrap()).unwrap();
        let parts: Vec<&str> = tex_path.split('.').collect();
        z.start_file(format!("TestMod/Textures/{}.png", parts.join("/")), opts).unwrap();
        z.write_all(&png).unwrap();
        z.finish().unwrap();
    }

    // the map only the mod has opens and runs
    let mut s = dx_play::Session::open(&game, "ModMap").expect("the mod's map opens");
    for _ in 0..30 {
        s.step(&dx_play::Input::default(), 1.0 / 30.0);
    }
    assert_eq!(s.map_name, "ModMap");

    // the texture: the mod's picture with every mip level, the game's size for everything else
    let mods = ue_package::mods::current();
    assert_eq!(mods.names, ["TestMod"]);
    let mips = s.view.texture_mips_by_path(&tex_path).unwrap();
    assert_eq!((mips[0].0, mips[0].1), (w, h));
    assert_eq!(&mips[0].2[..4], &[0, 255, 0, 255]);
    assert_eq!(mips.last().map(|m| (m.0, m.1)), Some((1, 1)));
    assert_eq!(s.view.texture_size_by_path(&tex_path), Some(own));
    std::fs::remove_dir_all(&game).unwrap();
}

/// For a human to look at: a stand-in game folder whose mod replaces every texture of the core texture packages with
/// a 4x bigger copy that has a dark line on every original texel border (`DX_MODS_OUT`, default under the temp dir).
/// Render it with `dx-play <that folder> 00_Training --headless --shot ...`: the picture is the game's, with a fine
/// grid on every replaced wall, aligned to the original texels.
#[test]
#[ignore]
fn make_a_grid_texture_mod() {
    let Some(real) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let out = std::env::var_os("DX_MODS_OUT").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("aerugo-grid-game"));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(out.join("Mods/Grid/Textures")).unwrap();
    for sub in ["System", "Maps", "Textures", "Sounds", "Music"] {
        #[cfg(unix)]
        std::os::unix::fs::symlink(real.join(sub), out.join(sub)).unwrap();
    }
    let files = ue_assets::files::GameFiles::new(&real);
    let mut n = 0;
    for name in std::fs::read_dir(real.join("Textures")).unwrap().flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)) {
        let Some(stem) = name.strip_suffix(".utx").filter(|s| s.starts_with("CoreTex")) else { continue };
        let pkg = files.package(stem).unwrap();
        for e in (0..pkg.pkg.exports.len()).filter(|&i| pkg.pkg.export_class_name(i) == "Texture") {
            let Ok(t) = ue_assets::texture::Texture::load(&pkg.pkg, e) else { continue };
            let Some((pp, pe)) = files.resolve(&pkg, t.palette) else { continue };
            let Ok(pal) = ue_assets::texture::Palette::load(&pp.pkg, pe) else { continue };
            let Ok(rgba) = t.mip_rgba(0, &pal) else { continue };
            let (w, h) = (t.mips[0].u_size, t.mips[0].v_size);
            let (bw, bh) = (w * 4, h * 4);
            let mut big = vec![0u8; (bw * bh * 4) as usize];
            for y in 0..bh {
                for x in 0..bw {
                    let s = (((y / 4) * w + x / 4) * 4) as usize;
                    let d = ((y * bw + x) * 4) as usize;
                    let line = x % 4 == 0 || y % 4 == 0;
                    for c in 0..3 {
                        big[d + c] = if line { rgba[s + c] / 3 } else { rgba[s + c] };
                    }
                    big[d + 3] = rgba[s + 3];
                }
            }
            let path = pkg.pkg.object_path(ObjectRef::Export(e)).unwrap().replace('.', "/");
            let file = out.join("Mods/Grid/Textures").join(stem).join(format!("{path}.png"));
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, dx_soft::png(bw, bh, &big)).unwrap();
            n += 1;
        }
    }
    println!("{n} textures in {}", out.display());
}

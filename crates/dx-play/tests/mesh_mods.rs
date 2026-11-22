//! Mesh replacements of the mods folder (`Meshes/<Package>/<Name>.aemesh`, `ue_assets::mesh_mod`): the mesh the game
//! draws takes the mod's geometry and keeps its sequences, texture slots and special vertices. Runs on a stand-in game
//! folder linked to the real one (`DEUSEX_DIR`). Its own test binary: the mod index is one per process.

use std::path::{Path, PathBuf};
use ue_assets::mesh::LodMesh;
use ue_assets::mesh_mod::{GeoTri, Geometry};

fn stand_in_game(real: &Path) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aerugo-mesh-mods-game-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Mods")).unwrap();
    for sub in ["System", "Maps", "Textures", "Sounds", "Music"] {
        #[cfg(unix)]
        std::os::unix::fs::symlink(real.join(sub), dir.join(sub)).unwrap();
    }
    dir
}

#[test]
fn a_mod_mesh_replaces_the_geometry_and_keeps_the_rest() {
    let Some(real) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let game = stand_in_game(&real);
    let files = ue_assets::files::GameFiles::new(&real);
    let items = files.package("DeusExItems").unwrap();
    let orig = LodMesh::load(&items.pkg, items.find("GEPGun").unwrap()).unwrap();
    // the gun with one more triangle whose third corner moves up by one unit a frame
    let mut g = Geometry::from_lod(&orig);
    let n = g.verts() as u16;
    for (f, frame) in g.frames.iter_mut().enumerate() {
        let p = frame[0];
        frame.push([p[0], p[1], p[2] + f as f32]);
    }
    g.tris.push(GeoTri { vertex: [0, 1, n], uv: [[0, 0], [10, 0], [0, 10]], slot: 0, flags: 0 });
    let file = game.join("Mods/Gun/Meshes/DeusExItems/GEPGun.aemesh");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, g.write()).unwrap();
    // a broken one for another mesh (frame count off): reported and left out
    let bad = Geometry { frames: vec![vec![[0.0; 3]; 3]; 2], tris: vec![] };
    std::fs::write(game.join("Mods/Gun/Meshes/DeusExItems/GEPGun3rd.aemesh"), bad.write()).unwrap();

    let mut s = dx_play::Session::open(&game, "00_Training").expect("the map opens");
    let md = s.view.mesh_data("DeusExItems.GEPGun").expect("the gun loads");
    let lod = &md.lod;
    assert_eq!(lod.faces.len(), orig.faces.len() + 1);
    assert_eq!(lod.mesh.anim_frames, orig.mesh.anim_frames);
    assert_eq!(lod.mesh.anim_seqs, orig.mesh.anim_seqs);
    assert_eq!(lod.mesh.textures, orig.mesh.textures);
    assert_eq!(lod.special_verts(), orig.special_verts());
    assert_eq!(md.own_texture_paths.len(), orig.mesh.textures.len());
    let new = lod.special_verts() + n as usize;
    for f in [0usize, 20, 64] {
        assert_eq!(lod.frame_local(f)[new][2], lod.frame_local(f)[lod.special_verts()][2] + f as f32, "frame {f}");
    }
    // the posed mesh the renderer draws holds the new vertex
    let pose = dx_vk::actors::posed_vertices(lod, &dx_soft::ActorDraw { mesh: "DeusExItems.GEPGun".into(), scale: 1.0, ..Default::default() });
    assert_eq!(pose.len(), lod.mesh.frame_verts as usize);
    let third = s.view.mesh_data("DeusExItems.GEPGun3rd").unwrap();
    let orig3 = LodMesh::load(&items.pkg, items.find("GEPGun3rd").unwrap()).unwrap();
    assert_eq!(third.lod.faces.len(), orig3.faces.len(), "a replacement that does not fit is left out");
    assert_eq!(ue_package::mods::current().meshes.len(), 2);
    std::fs::remove_dir_all(&game).unwrap();
}

//! Walls the shipped maps left without a face, under water (an Aerugo repair, `ue_bsp::fill`). Set `DEUSEX_DIR` to the
//! game folder.

use dx_play::Session;
use std::path::PathBuf;

/// The surfaces stored in a map's level model (without the repair).
fn stored_surfaces(game: &std::path::Path, map: &str) -> usize {
    let pkg = ue_package::Package::open(game.join("Maps").join(format!("{map}.dx"))).unwrap();
    let level = (0..pkg.exports.len()).find(|&i| pkg.export_class_name(i) == "Level").unwrap();
    let l = ue_assets::level::Level::load(&pkg, level).unwrap();
    let ue_package::ObjectRef::Export(model) = ue_package::ObjectRef::from_index(l.model) else { panic!("level model") };
    ue_assets::model::Model::load(&pkg, model).unwrap().surfs.len()
}

/// Wan Chai canal, under water at the tester's save (0068): 26 units ahead the canal wall is solid but its underwater
/// part had no face, so the walk ran on through the rock to a wall 2400 units away (the bright patch). The repair gives
/// the wall a face (copied from the brick above the water): the walk lists it, and the far wall is no longer drawn.
#[test]
fn the_canal_wall_under_the_water_has_a_face() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "06_HongKong_WanChai_Canal").unwrap();
    for _ in 0..3 {
        s.step_bound(1.0 / 30.0);
    }
    let stored = stored_surfaces(&g, "06_HongKong_WanChai_Canal");
    assert!(s.static_world_model().surfs.len() > stored, "no face was added");
    let cam = dx_soft::Camera { pos: [86.09, -743.14, -558.61], pitch: 0, yaw: 51765, fov: 75.0, roll: 0 };
    let (_, frames) = s.compute_visibility(&cam, 1024, 768);
    let m = s.vm.world_model.clone().unwrap();
    let listed = &frames[0].vis.surfaces;
    let filled = listed.iter().filter(|&&(n, _, _)| {
        let i = m.nodes[n as usize].i_surf;
        i >= stored as i32 && (i as usize) < s.static_world_model().surfs.len()
    });
    assert!(filled.count() > 0, "the canal wall's new face is not listed");
    // the far wall that showed through (y -2976)
    assert!(!listed.iter().any(|&(n, _, _)| n == 4339), "the far wall still shows through the rock");
}

/// Above water a missing face is often a window onto the backdrop city: the NYC streets keep theirs.
#[test]
fn maps_without_water_keep_their_faces_as_stored() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let s = Session::open(&g, "08_NYC_Street").unwrap();
    assert_eq!(s.static_world_model().surfs.len(), stored_surfaces(&g, "08_NYC_Street"));
}

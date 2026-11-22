//! Replacement geometry (`ue_assets::mesh_mod`) on the game's own meshes: a mesh's geometry written out and applied
//! again gives the mesh back (vertices, faces, bounds). Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_assets::mesh::{LodMesh, Mesh};
use ue_assets::mesh_mod::Geometry;
use ue_package::Package;

#[test]
fn every_mesh_round_trips_through_its_geometry() {
    let Some(root) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let (mut n, mut boxes_equal, mut boxes) = (0, 0, 0);
    for e in std::fs::read_dir(root.join("System")).unwrap().flatten() {
        let path = e.path();
        if !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("u")) {
            continue;
        }
        let Ok(pkg) = Package::open(&path) else { continue };
        for i in 0..pkg.exports.len() {
            let lod = match pkg.export_class_name(i).as_str() {
                "LodMesh" => LodMesh::load(&pkg, i).unwrap(),
                "Mesh" => LodMesh::from_plain(Mesh::load(&pkg, i).unwrap()),
                _ => continue,
            };
            let name = format!("{} #{i}", path.display());
            let g = Geometry::from_lod(&lod);
            let back = Geometry::read(&g.write()).unwrap().apply(&lod).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(Geometry::from_lod(&back), g, "{name}");
            for f in 0..lod.mesh.anim_frames.max(1) as usize {
                assert_eq!(back.frame_local(f), lod.frame_local(f), "{name} frame {f}");
            }
            assert_eq!(back.special_faces, lod.special_faces);
            assert_eq!(back.faces.len(), lod.faces.len());
            // the stored boxes are the boxes around each frame's vertices (where the original's are valid)
            for (a, b) in back.mesh.bounding_boxes.iter().zip(&lod.mesh.bounding_boxes) {
                if b.is_valid {
                    boxes += 1;
                    boxes_equal += (a.min == b.min && a.max == b.max) as usize;
                }
            }
            n += 1;
        }
    }
    eprintln!("{n} meshes; {boxes_equal} of {boxes} frame boxes as stored");
    assert!(n > 100);
}

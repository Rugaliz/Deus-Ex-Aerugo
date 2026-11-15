//! Loads every Mesh and LodMesh in the game's packages and checks internal consistency.
//! Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_assets::mesh::{LodMesh, Mesh};
use ue_package::Package;

#[test]
fn all_meshes_load_with_consistent_indices() {
    let Some(root) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let (mut lod, mut plain) = (0, 0);
    for folder in ["System", "Maps", "Textures"] {
        for e in std::fs::read_dir(root.join(folder)).unwrap().flatten() {
            let path = e.path();
            let Some(pkg) = Package::open(&path).ok() else { continue };
            let name = path.display();
            for i in 0..pkg.exports.len() {
                match pkg.export_class_name(i).as_str() {
                    "Mesh" => {
                        Mesh::load(&pkg, i).unwrap_or_else(|e| panic!("{name} #{i}: {e}"));
                        plain += 1;
                    }
                    "LodMesh" => {
                        let m = LodMesh::load(&pkg, i).unwrap_or_else(|e| panic!("{name} #{i}: {e}"));
                        lod += 1;
                        let stored = if m.remap_anim_verts.is_empty() { m.mesh.frame_verts } else { m.old_frame_verts };
                        assert_eq!(m.mesh.verts.len() as i64, stored as i64 * m.mesh.anim_frames as i64, "{name} #{i}: vertex count");
                        // wedges count from the first model vertex, after the special (attachment) vertices
                        for w in 0..m.wedges.len() {
                            assert!((m.wedge_vertex(w) as i32) < m.mesh.frame_verts, "{name} #{i}: wedge vertex");
                        }
                        assert_eq!(m.special_verts() as i32 + m.lod_params[0] as i32, m.mesh.frame_verts, "{name} #{i}: model + special verts");
                        for f in &m.faces {
                            assert!(f.wedge.iter().all(|&w| (w as usize) < m.wedges.len()), "{name} #{i}: face wedge");
                            assert!((f.material as usize) < m.materials.len(), "{name} #{i}: face material");
                        }
                        // special faces carry no meaningful material (seen: 50575); only their wedges are checked
                        for f in &m.special_faces {
                            assert!(f.wedge.iter().all(|&w| (w as usize) < m.wedges.len()), "{name} #{i}: special face wedge");
                        }
                        for mat in &m.materials {
                            assert!((mat[1] as usize) < m.mesh.textures.len().max(1), "{name} #{i}: texture slot");
                        }
                        // sequence frame ranges are not checked: some meshes store fewer frames than their sequences name
                    }
                    _ => {}
                }
            }
        }
    }
    assert_eq!((lod, plain), (432, 5));
}

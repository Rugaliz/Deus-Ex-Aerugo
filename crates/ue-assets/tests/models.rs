//! Loads every Model and Polys in the game's maps and checks that all internal indices are in range.
//! Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_assets::model::{Model, Polys};
use ue_package::Package;

#[test]
fn all_models_and_polys_load_with_consistent_indices() {
    let Some(dir) = std::env::var_os("DEUSEX_DIR").map(|d| PathBuf::from(d).join("Maps")) else { return };
    let (mut models, mut polys, mut nodes) = (0usize, 0usize, 0usize);
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("dx") {
            continue;
        }
        let pkg = Package::open(&path).unwrap();
        let name = path.display();
        for i in 0..pkg.exports.len() {
            match pkg.export_class_name(i).as_str() {
                "Polys" => {
                    let p = Polys::load(&pkg, i).unwrap_or_else(|e| panic!("{name} #{i}: {e}"));
                    for q in &p.polys {
                        assert!(q.vertices.len() >= 3 && q.vertices.len() <= 64, "{name} #{i}");
                        let len = q.normal.iter().map(|x| x * x).sum::<f32>().sqrt();
                        assert!((len - 1.0).abs() < 0.01 || len == 0.0, "{name} #{i}: normal length {len}");
                    }
                    polys += p.polys.len();
                }
                "Model" => {
                    let m = Model::load(&pkg, i).unwrap_or_else(|e| panic!("{name} #{i}: {e}"));
                    let none_or = |v: i32, n: usize| v == -1 || (v >= 0 && (v as usize) < n);
                    for (k, n) in m.nodes.iter().enumerate() {
                        assert!(none_or(n.i_surf, m.surfs.len()), "{name} #{i} node {k}: iSurf {}", n.i_surf);
                        assert!(none_or(n.i_back, m.nodes.len()) && none_or(n.i_front, m.nodes.len()), "{name} #{i} node {k}");
                        assert!(none_or(n.i_plane, m.nodes.len()), "{name} #{i} node {k}: iPlane");
                        assert!(n.i_vert_pool >= 0 && n.i_vert_pool as usize + n.num_vertices as usize <= m.verts.len(), "{name} #{i} node {k}: vertex pool");
                        assert!((n.i_zone[0] as usize) < m.zones.len().max(1) && (n.i_zone[1] as usize) < m.zones.len().max(1), "{name} #{i} node {k}: zone");
                        assert!(none_or(n.i_leaf[0], m.leaves.len()) && none_or(n.i_leaf[1], m.leaves.len()), "{name} #{i} node {k}: leaf");
                    }
                    // only the vertices a node uses must point into `points`: the pool has unreferenced leftovers
                    for (k, n) in m.nodes.iter().enumerate() {
                        let pool = &m.verts[n.i_vert_pool as usize..][..n.num_vertices as usize];
                        assert!(pool.iter().all(|v| v.p_vertex >= 0 && (v.p_vertex as usize) < m.points.len()), "{name} #{i} node {k}: vertex point");
                    }
                    for s in &m.surfs {
                        assert!(none_or(s.p_base, m.points.len()), "{name} #{i}: pBase");
                        assert!(none_or(s.v_normal, m.vectors.len()) && none_or(s.v_texture_u, m.vectors.len()) && none_or(s.v_texture_v, m.vectors.len()), "{name} #{i}: surface vectors");
                        assert!(none_or(s.i_light_map, m.light_map.len()), "{name} #{i}: iLightMap");
                    }
                    models += 1;
                    nodes += m.nodes.len();
                }
                _ => {}
            }
        }
    }
    assert!(models > 1000 && polys > 100_000 && nodes > 100_000, "{models} {polys} {nodes}");
}

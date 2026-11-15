//! D3DDrv's detail texture pass on real data (WORKLOG 3.127). Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;

#[test]
fn island_start_has_detail_layers_near_the_eye() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    let cam = s.render_camera();
    let (polys, _) = s.detail_polys(&cam);
    assert!(!polys.is_empty(), "the planks under the player have a detail texture");
    let f = s.vis_frames()[0].frame.clone();
    let depth = |p: [f32; 3]| (0..3).map(|k| (p[k] - f.origin[k]) * f.z_axis[k]).sum::<f32>();
    for p in &polys {
        assert!(p.points.len() >= 3);
        assert!(s.view.texture_mips_by_path(&p.texture).is_some(), "{}", p.texture);
        for &(w, _, a) in &p.points {
            assert!((0.0..=1.0).contains(&a));
            // faded-in points lie nearer than the first layer's 380 units; cut points sit on a NearZ
            assert!(depth(w) <= 380.01, "{}", depth(w));
        }
    }
    println!("{} detail layers, first {}", polys.len(), polys[0].texture);
}

/// Movers' surfaces get the detail pass too (the tracker's nodes are on the frame's solid list like the level's): a
/// camera 160 units in front of a door whose texture has a detail texture sees a layer lying in the door's plane.
#[test]
fn doors_get_detail_layers() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let (mut tried, mut found) = (0, 0);
    for map in ["03_NYC_UNATCOHQ", "01_NYC_UNATCOIsland"] {
        let mut s = Session::open(&g, map).unwrap();
        for _ in 0..3 {
            s.step_bound(1.0 / 30.0);
        }
        let mover = s.vm.reg.class_by_name("DeusExMover").unwrap();
        let doors: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, mover)).unwrap_or(false)).collect();
        for d in doors {
            let Some((model, nodes, _)) = s.tracked_model(d) else { continue };
            let Some(&n) = nodes.iter().max_by_key(|&&n| model.nodes[n].num_vertices) else { continue };
            let nd = model.nodes[n].clone();
            let tex = model.surfs[nd.i_surf as usize].texture;
            if s.view.surface_detail(tex).is_none() {
                continue;
            }
            let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| model.points[model.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
            let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
            let nrm = [nd.plane[0], nd.plane[1], nd.plane[2]];
            if nrm[2].abs() > 0.3 {
                continue;
            }
            let eye: [f32; 3] = std::array::from_fn(|k| c[k] + nrm[k] * 160.0);
            if s.vm.point_region(eye).zone_number == 0 {
                continue;
            }
            let mut cam = s.camera();
            cam.pos = eye;
            cam.yaw = ((-nrm[1]).atan2(-nrm[0]) * 32768.0 / std::f32::consts::PI) as i32;
            cam.pitch = 0;
            let (_, frames) = s.compute_visibility(&cam, 800, 600);
            let fv = &frames[0];
            let surfaces: Vec<(u32, u32)> = fv.vis.surfaces.iter().zip(&fv.vis.surface_lists).filter(|(_, &l)| l == 1).map(|(&(n, f, _), _)| (n, f)).collect();
            if !surfaces.iter().any(|&(node, _)| node as usize == n) {
                continue;
            }
            tried += 1;
            let layers = s.details_for(&fv.frame, &[(n as u32, 0)]);
            let on_plane = layers.iter().any(|l| l.points.iter().all(|&(p, _, _)| (nrm[0] * p[0] + nrm[1] * p[1] + nrm[2] * p[2] - nd.plane[3]).abs() < 0.5));
            found += on_plane as usize;
        }
    }
    println!("{tried} doors with a detail texture in view at 160 units, {found} with a layer on the door");
    assert!(tried > 0 && found == tried);
}

/// Mirror frames draw their own detail layers (their solid list, through their own `ClipBspSurf` with the mirror as near
/// clip): 60 units in front of a mirror, the mirror frame's layers lie on the room's side of the mirror plane.
#[test]
fn mirror_frames_get_their_own_detail_layers() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut with_layers = 0;
    for map in ["02_NYC_Hotel", "10_Paris_Club", "06_HongKong_WanChai_Underworld"] {
        let mut s = Session::open(&g, map).unwrap();
        for _ in 0..3 {
            s.step_bound(1.0 / 30.0);
        }
        let player = s.player;
        let eye_h = s.vm.fp(player, "BaseEyeHeight");
        let model = s.view.model.clone();
        for nd in &model.nodes {
            if nd.i_surf < 0 || model.surfs[nd.i_surf as usize].poly_flags & 0x800_0000 == 0 || nd.num_vertices < 3 || nd.plane[2].abs() > 0.3 {
                continue;
            }
            let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| model.points[model.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
            let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
            let n = [nd.plane[0], nd.plane[1], nd.plane[2]];
            let eye = [c[0] + n[0] * 60.0, c[1] + n[1] * 60.0, c[2]];
            s.teleport([eye[0], eye[1], eye[2] - eye_h], 0.0);
            let at = s.vm.location(player);
            if (at[0] - eye[0]).abs() > 1.0 || (at[1] - eye[1]).abs() > 1.0 {
                continue;
            }
            let yaw = ((-n[1]).atan2(-n[0]).to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
            let cam = dx_soft::Camera { pos: [at[0], at[1], at[2] + eye_h], pitch: -2000, yaw, fov: 75.0, roll: 0 };
            let (_, frames) = s.compute_visibility(&cam, 800, 600);
            for fv in frames.iter().filter(|f| f.frame.mirror == -1.0 && f.frame.recursion == 1) {
                let surfaces: Vec<(u32, u32)> = fv.vis.surfaces.iter().zip(&fv.vis.surface_lists).filter(|(_, &l)| l == 1).map(|(&(n, f, _), _)| (n, f)).collect();
                let layers = s.details_for(&fv.frame, &surfaces);
                if layers.is_empty() {
                    continue;
                }
                with_layers += 1;
                for l in &layers {
                    for &(p, _, _) in &l.points {
                        assert!(n[0] * p[0] + n[1] * p[1] + n[2] * p[2] - nd.plane[3] > -0.5, "{map}: a layer behind the mirror");
                    }
                }
            }
        }
    }
    println!("{with_layers} mirror frames with detail layers");
    assert!(with_layers > 0);
}

/// The detail pass's upload sets the detail palette's entry 0 to white (outside the editor): the texture differs from
/// its plain upload only in texels that became white. (None of the 17 shipped detail textures has an index-0 texel,
/// so on the game's data the two uploads are equal.)
#[test]
fn detail_uploads_have_a_white_palette_entry_0() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let view = dx_soft::MapView::open(&g, "01_NYC_UNATCOIsland").unwrap();
    let path = "CoreTexDetail.Detail.DWoodFine_A";
    let (plain, detail) = (view.texture_mips_by_path(path).unwrap(), view.detail_texture_mips(path).unwrap());
    assert_eq!(plain.len(), detail.len());
    let (a, b) = (&plain[0].2, &detail[0].2);
    let changed: Vec<usize> = (0..a.len() / 4).filter(|&i| a[i * 4..i * 4 + 4] != b[i * 4..i * 4 + 4]).collect();
    assert!(changed.iter().all(|&i| b[i * 4..i * 4 + 4] == [255, 255, 255, 255]));
    println!("{} of {} texels white", changed.len(), a.len() / 4);
}

/// Translucent and modulated surfaces of the third list keep their detail layers apart, by node, for the renderer to
/// draw right after each surface (`DrawComplexSurface` draws a surface's detail pass with it): in front of such a
/// surface in the training map, its node's layers are in the by-node map and not among the solid ones.
#[test]
fn blended_surfaces_get_their_layers_by_node() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "00_Training").unwrap();
    for _ in 0..3 {
        s.step_bound(1.0 / 30.0);
    }
    let model = s.view.model.clone();
    let (mut tried, mut found) = (0, 0);
    for (i, nd) in model.nodes.iter().enumerate() {
        if tried >= 12 || nd.i_surf < 0 || nd.num_vertices < 3 {
            continue;
        }
        let surf = &model.surfs[nd.i_surf as usize];
        // (translucent mirrors open child frames: list 0)
        if surf.poly_flags & 0x44 == 0 || surf.poly_flags & 0x800_0000 != 0 || s.view.surface_detail(surf.texture).is_none() {
            continue;
        }
        let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| model.points[model.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
        let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
        let n = [nd.plane[0], nd.plane[1], nd.plane[2]];
        let eye = [c[0] + n[0] * 100.0, c[1] + n[1] * 100.0, c[2] + n[2] * 100.0];
        if s.vm.point_region(eye).zone_number == 0 {
            continue;
        }
        // looking along -n
        let to_units = |rad: f32| (rad / std::f32::consts::TAU * 65536.0) as i32 & 0xffff;
        let cam = dx_soft::Camera { pos: eye, pitch: to_units((-n[2]).clamp(-1.0, 1.0).asin()), yaw: to_units((-n[1]).atan2(-n[0])), fov: 90.0, roll: 0 };
        let (_, frames) = s.compute_visibility(&cam, 800, 600);
        if !frames[0].vis.surfaces.iter().any(|&(node, _, _)| node as usize == i) {
            continue;
        }
        tried += 1;
        // the session's split for the frame just walked
        let fv = &frames[0];
        let listed: Vec<(u32, u32)> = fv.vis.surfaces.iter().zip(&fv.vis.surface_lists).filter(|(&(node, _, _), &l)| node as usize == i && l == 2).map(|(&(node, f, _), _)| (node, f)).collect();
        if listed.is_empty() {
            continue;
        }
        let layers = s.details_for(&fv.frame, &listed);
        found += (!layers.is_empty() && layers.iter().all(|l| l.node as usize == i)) as usize;
    }
    println!("{tried} blended surfaces with detail in view, {found} with layers of their own node");
    assert!(found > 0);
}

/// The first list (mirrors that opened a child frame) is drawn after the child frames, each surface followed by its
/// detail layers: the training map's shiny stone floors (translucent mirrors with `DStone_A`) seen from 100 units above
/// are on the main frame's list 0, in `mirror_order`, and carry layers of their own node in the by-node set.
#[test]
fn first_list_surfaces_get_their_layers_by_node() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "00_Training").unwrap();
    for _ in 0..3 {
        s.step_bound(1.0 / 30.0);
    }
    let model = s.view.model.clone();
    let (mut tried, mut found) = (0, 0);
    for (i, nd) in model.nodes.iter().enumerate() {
        if tried >= 8 || nd.i_surf < 0 || nd.num_vertices < 3 {
            continue;
        }
        let surf = &model.surfs[nd.i_surf as usize];
        if surf.poly_flags & 0x800_0004 != 0x800_0004 || s.view.surface_detail(surf.texture).is_none() {
            continue;
        }
        let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| model.points[model.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
        let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
        let n = [nd.plane[0], nd.plane[1], nd.plane[2]];
        let eye = [c[0] + n[0] * 100.0, c[1] + n[1] * 100.0, c[2] + n[2] * 100.0];
        if s.vm.point_region(eye).zone_number == 0 {
            continue;
        }
        let to_units = |rad: f32| (rad / std::f32::consts::TAU * 65536.0) as i32 & 0xffff;
        let cam = dx_soft::Camera { pos: eye, pitch: to_units((-n[2]).clamp(-1.0, 1.0).asin()), yaw: to_units((-n[1]).atan2(-n[0])), fov: 90.0, roll: 0 };
        s.mark_rendered(&cam);
        let fv = &s.vis_frames()[0];
        let lists: Vec<u8> = fv.vis.surfaces.iter().zip(&fv.vis.surface_lists).filter(|(&(node, _, _), _)| node as usize == i).map(|(_, &l)| l).collect();
        if lists.is_empty() {
            continue;
        }
        tried += 1;
        // a translucent mirror opens a child frame (shiny surfaces on): list 0, drawn in walk order
        assert!(lists.iter().all(|&l| l == 0), "node {i}: lists {lists:?}");
        assert!(s.mirror_order().unwrap().contains(&(i as u32)));
        let (solid, by_node) = s.frame_details(0);
        assert!(solid.iter().all(|p| p.node as usize != i), "node {i}'s layers belong to its first-list draw");
        let own = by_node.get(&(i as u32)).map(|v| v.len()).unwrap_or(0);
        found += (own > 0) as usize;
    }
    println!("{tried} shiny floors in view, {found} with layers of their own");
    assert!(tried > 0 && found == tried);
}

/// `DrawComplexSurface` (1000395f) drops a surface's detail texture when the surface has a fog map: in the Underground
/// map's fog, looking at a volumetric light, the fogged stone surfaces (which have `DStone_A`) get no layers.
#[test]
fn fogged_surfaces_get_no_layers() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "02_NYC_Underground").unwrap();
    s.step_bound(1.0 / 30.0);
    let lights = s.vm.model_light_actors.clone();
    let mut seen = std::collections::HashSet::new();
    let mut checked = 0;
    'lights: for &a in &lights {
        if a == 0 || !seen.insert(a) || s.vm.u8p(a, "VolumeBrightness") == 0 || s.vm.u8p(a, "VolumeRadius") == 0 {
            continue;
        }
        let zone = s.vm.zone_of_actor(a);
        if zone == 0 || !s.vm.bp(zone, "bFogZone") {
            continue;
        }
        let at = s.vm.location(a);
        let rv = (s.vm.u8p(a, "VolumeRadius") as f32 + 1.0) * 25.0;
        for (dx, dy) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let spot = [at[0] + dx * (rv + 40.0), at[1] + dy * (rv + 40.0), at[2]];
            s.teleport([spot[0], spot[1], spot[2] - 40.0], 0.0);
            let p = s.player_location();
            if (p[0] - spot[0]).abs() > 2.0 || (p[1] - spot[1]).abs() > 2.0 || s.vm.zone_of_actor(s.player) != zone {
                continue;
            }
            let yaw = ((-dy).atan2(-dx).to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
            s.mark_rendered(&dx_soft::Camera { pos: spot, pitch: 0, yaw, fov: 75.0, roll: 0 });
            let fogged: std::collections::HashSet<i32> = s.fog_draws().into_iter().map(|(si, _)| si as i32).collect();
            let frame = s.vis_frames()[0].frame.clone();
            let nodes = s.view.model.nodes.clone();
            let surf_of = |n: u32| nodes.get(n as usize).map(|nd| nd.i_surf).unwrap_or(-1);
            let in_fog: Vec<(u32, u32)> = s.vis_frames()[0].vis.surfaces.iter().filter(|&&(n, _, _)| fogged.contains(&surf_of(n))).map(|&(n, f, _)| (n, f)).collect();
            // without the rule these would have layers
            if s.details_for(&frame, &in_fog).is_empty() {
                continue;
            }
            let (solid, by_node) = s.frame_details(0);
            for p in solid.iter().chain(by_node.values().flatten()) {
                assert!(!fogged.contains(&surf_of(p.node)), "node {} has a fog map and detail layers", p.node);
            }
            checked += 1;
            break 'lights;
        }
    }
    assert!(checked > 0, "no fogged surface with a detail texture in view");
}

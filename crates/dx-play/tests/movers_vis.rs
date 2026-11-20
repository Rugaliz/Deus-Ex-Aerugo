//! Movers in the visibility walk: the moving-brush tracker filters their polygons into the level's BSP, so a closed
//! door hides what is behind it. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;

/// A door between the camera and a pawn: with the movers in the BSP the door's surfaces are on the draw lists, and a
/// door that covers the pawn's sprite keeps it from being drawn (not "rendered"); without them the pawn is drawn.
#[test]
fn a_closed_door_hides_what_is_behind_it() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let (mut found, mut tried, mut drawn_doors) = (0, 0, 0);
    for map in ["01_NYC_UNATCOIsland", "03_NYC_UNATCOHQ", "05_NYC_UNATCOMJ12lab"] {
        let mut s = Session::open(&g, map).unwrap();
        for _ in 0..3 {
            s.step_bound(1.0 / 30.0);
        }
        let mover = s.vm.reg.class_by_name("DeusExMover").unwrap();
        let pawn_class = s.vm.reg.class_by_name("ScriptedPawn").unwrap();
        let Some(pawn) = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, pawn_class)).unwrap_or(false)) else { continue };
        let doors: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, mover)).unwrap_or(false)).collect();
        let cam0 = s.camera();
        let _ = s.compute_visibility(&cam0, 800, 600);
        for d in doors {
            let Some((model, nodes, first_dyn)) = s.tracked_model(d) else { continue };
            // the door's largest dynamic node: its plane and the centre of its polygon
            let Some(&n) = nodes.iter().max_by_key(|&&n| model.nodes[n].num_vertices) else { continue };
            let nd = model.nodes[n].clone();
            assert!(nd.i_surf as usize >= first_dyn);
            let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| model.points[model.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
            let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
            let nrm = [nd.plane[0], nd.plane[1], nd.plane[2]];
            if nrm[2].abs() > 0.3 {
                continue;
            }
            let eye: [f32; 3] = std::array::from_fn(|k| c[k] + nrm[k] * 160.0);
            let behind: [f32; 3] = std::array::from_fn(|k| c[k] - nrm[k] * 120.0);
            // both spots must be open space in a real zone, and the door's own surfaces the only thing between them
            if s.vm.point_region(eye).zone_number == 0 || s.vm.point_region(behind).zone_number == 0 {
                continue;
            }
            s.vm.set_prop(pawn, "Location", 0, ue_vm::Value::Vector(behind)).unwrap();
            let mut cam = s.camera();
            cam.pos = eye;
            cam.yaw = ((-nrm[1]).atan2(-nrm[0]) * 32768.0 / std::f32::consts::PI) as i32;
            cam.pitch = 0;
            s.vis_movers = false;
            let (without, _) = s.compute_visibility(&cam, 800, 600);
            if !without.contains(&pawn) {
                continue;
            }
            s.vis_movers = true;
            let (with, frames) = s.compute_visibility(&cam, 800, 600);
            let door_drawn = frames[0].vis.surfaces.iter().any(|&(node, _, _)| nodes.contains(&(node as usize)));
            println!("{map} door {d}: pawn drawn without movers, with movers {}; door surfaces drawn {door_drawn}", with.contains(&pawn));
            tried += 1;
            drawn_doors += door_drawn as usize;
            if !with.contains(&pawn) {
                found += 1;
            }
        }
    }
    println!("{tried} doors between camera and pawn: {drawn_doors} on the draw lists, {found} hid the pawn");
    assert!(found > 0, "no door hid a pawn");
    assert!(drawn_doors * 4 >= tried * 3, "door surfaces missing from the draw lists");
}

/// `Update(Actor)` from `SetupDynamics`: a mover that moved is filtered in again at its new place (its nodes' points
/// move with it); one that did not keeps its nodes.
#[test]
fn a_moved_mover_is_filtered_in_again() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    s.step_bound(1.0 / 30.0);
    let mover = s.vm.reg.class_by_name("DeusExMover").unwrap();
    let doors: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, mover)).unwrap_or(false)).collect();
    let cam = s.camera();
    let _ = s.compute_visibility(&cam, 800, 600);
    let points = |s: &Session, a: u32| -> Vec<[f32; 3]> {
        let (m, nodes, _) = s.tracked_model(a).unwrap();
        nodes.iter().flat_map(|&n| (0..m.nodes[n].num_vertices as usize).map(move |k| m.points[m.verts[m.nodes[n].i_vert_pool as usize + k].p_vertex as usize])).collect()
    };
    let d = *doors.iter().find(|&&d| !points(&s, d).is_empty()).expect("a door with nodes");
    let other = *doors.iter().find(|&&o| o != d && !points(&s, o).is_empty()).unwrap();
    let (before, other_before) = (points(&s, d), points(&s, other));
    let loc = s.vm.location(d);
    s.vm.set_prop(d, "Location", 0, ue_vm::Value::Vector([loc[0], loc[1], loc[2] + 2.0])).unwrap();
    let _ = s.compute_visibility(&cam, 800, 600);
    let after = points(&s, d);
    assert!(!after.is_empty());
    // fragments may split differently, but every point rose by 2 if the door's pieces were cut the same way; at least
    // the lowest point did
    let low = |v: &[[f32; 3]]| v.iter().map(|p| p[2]).fold(f32::INFINITY, f32::min);
    assert!((low(&after) - low(&before) - 2.0).abs() < 1e-3, "{} -> {}", low(&before), low(&after));
    assert_eq!(points(&s, other), other_before, "an unmoved mover was redone");
}

/// The level's collision walks the same BSP (`ULevel` traces use `Level->Model`): with a door's nodes in it, a
/// zero-extent trace through the closed door stops on its plane and `FastLineCheck` is blocked; a box trace is not
/// changed by them (no collision hulls there).
#[test]
fn level_traces_see_closed_doors() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    s.step_bound(1.0 / 30.0);
    let mover = s.vm.reg.class_by_name("DeusExMover").unwrap();
    let doors: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, mover)).unwrap_or(false)).collect();
    let mut blocked = 0;
    for d in doors {
        let Some((model, nodes, _)) = s.tracked_model(d) else { continue };
        let Some(&n) = nodes.iter().filter(|&&n| model.nodes[n].node_flags & 1 == 0).max_by_key(|&&n| model.nodes[n].num_vertices) else { continue };
        let nd = model.nodes[n].clone();
        let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| model.points[model.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
        let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
        let nrm = [nd.plane[0], nd.plane[1], nd.plane[2]];
        let a: [f32; 3] = std::array::from_fn(|k| c[k] + nrm[k] * 40.0);
        let b: [f32; 3] = std::array::from_fn(|k| c[k] - nrm[k] * 40.0);
        let static_bsp = ue_bsp::Bsp::from_model(s.static_world_model());
        // only where the static world leaves the doorway open
        if !static_bsp.fast_line_check(b, a) {
            continue;
        }
        let bsp = s.vm.bsp.as_ref().unwrap();
        assert!(!bsp.fast_line_check(b, a), "door {d}: FastLineCheck goes through");
        let hit = bsp.line_check(a, b, [0.0; 3], 0).expect("a zero-extent trace goes through the door");
        let dist = nrm[0] * hit.location[0] + nrm[1] * hit.location[1] + nrm[2] * hit.location[2] - nd.plane[3];
        assert!(dist >= -0.01 && dist < 1.0, "door {d}: hit {dist} from the door plane");
        assert_eq!(bsp.line_check(a, b, [10.0, 10.0, 10.0], 0), static_bsp.line_check(a, b, [10.0, 10.0, 10.0], 0), "door {d}: box trace changed");
        blocked += 1;
    }
    println!("{blocked} doors block level traces");
    assert!(blocked > 0);
}

/// The Helibase's stored render bounds miss parts of their subtrees (node 3159's box ends at y 200 while its subtree's
/// walls reach y 480), so the original skips the stairs room and the corridor's end door (a mover filtered into that
/// part of the tree) from most views. Aerugo tests boxes grown to what each subtree holds: from the corridor 350
/// units back the door's fragments are listed, and from the stairs the room's walls are.
#[test]
fn the_helibase_corridor_door_and_stairs_room_are_drawn_from_afar() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "06_HongKong_Helibase").unwrap();
    for _ in 0..3 {
        s.step_bound(1.0 / 30.0);
    }
    let corridor = dx_soft::Camera { pos: [350.0, 336.1, 343.4], pitch: 0, yaw: 32675, fov: 75.0, roll: 0 };
    let (_, frames) = s.compute_visibility(&corridor, 1024, 768);
    let m = s.vm.world_model.clone().unwrap();
    let first_dynamic = s.vm.surf_info.len();
    let door = frames[0]
        .vis
        .surfaces
        .iter()
        .filter(|&&(n, _, _)| {
            let nd = &m.nodes[n as usize];
            nd.i_surf >= 0 && nd.i_surf as usize >= first_dynamic && {
                let p = m.points[m.verts[nd.i_vert_pool as usize].p_vertex as usize];
                (-120.0..-90.0).contains(&p[0])
            }
        })
        .count();
    assert!(door > 0, "the corridor's end door is not listed");
    let stairs = dx_soft::Camera { pos: [-235.6, 218.9, 311.4], pitch: 0, yaw: 11813, fov: 75.0, roll: 0 };
    let (_, frames) = s.compute_visibility(&stairs, 1024, 768);
    let listed: Vec<u32> = frames[0].vis.surfaces.iter().map(|x| x.0).collect();
    assert!([3164, 3165, 3181].iter().all(|n| listed.contains(n)), "the stairs room's walls are not listed: {listed:?}");
}

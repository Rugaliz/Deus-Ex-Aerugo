//! Movers as they are drawn. Set `DEUSEX_DIR` to the game folder; pictures need Vulkan.

use std::path::PathBuf;
use ue_vm::Value;

/// A mirrored door (`MainScale` X = -1) of the MJ12 lab, seen from four sides: the brush is drawn through
/// `ABrush::ToWorld` like the engine's moving-brush tracker, so it stands where it collides.
/// `cargo test --release -p dx-play --test movers shot_of_a_mirrored_door -- --ignored`
#[test]
#[ignore]
fn shot_of_a_mirrored_door() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&game, "05_NYC_UNATCOMJ12lab").unwrap();
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    let mover = s.vm.reg.class_by_name("Mover").unwrap();
    let mirrored: Vec<u32> = s
        .vm
        .live_actors()
        .into_iter()
        .filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, mover)).unwrap_or(false))
        .filter(|&a| matches!(s.vm.get_prop(a, "MainScale", 0), Ok(Value::Struct(_, b)) if f32::from_le_bytes([b[0], b[1], b[2], b[3]]) < 0.0))
        .collect();
    assert!(!mirrored.is_empty());
    let which: usize = std::env::var("DOOR").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let door = mirrored[which % mirrored.len()];
    let at = s.vm.location(door);
    println!("{} mirrored movers; door {door} at {at:?}", mirrored.len());
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    let tag = std::env::var("SHOT_TAG").unwrap_or_default();
    let mut r = None;
    for (i, (dx, dy, yaw)) in [(-320.0, 0.0, 0.0), (320.0, 0.0, 180.0), (0.0, -320.0, 90.0), (0.0, 320.0, 270.0)].into_iter().enumerate() {
        s.teleport([at[0] + dx, at[1] + dy, at[2]], yaw);
        s.step_bound(1.0 / 30.0);
        let img = s.render_vk(&mut r, 800, 600).expect("vulkan render");
        std::fs::write(dir.join(format!("r_mirrored_door{tag}_{i}.png")), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    }
    r.take().unwrap().destroy();
}

/// The surfaces the tracker makes for a mover (one per brush polygon, `iLightMap = iBrushPoly`) find their stored
/// lightmap: every lit polygon of every mover lies on its lightmap's texel grid (within a texel), and a door whose
/// brush BSP is older than its polygons gets its glass panes (UNATCO HQ, `Model973`).
#[test]
fn mover_polygons_sit_on_their_stored_lightmaps() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let (mut lit, mut on_grid) = (0, 0);
    for map in ["01_NYC_UNATCOIsland", "03_NYC_UNATCOHQ"] {
        let mut s = dx_play::Session::open(&g, map).unwrap();
        let mover = s.vm.reg.class_by_name("Mover").unwrap();
        let movers: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, mover)).unwrap_or(false)).collect();
        for a in movers {
            let brush = s.vm.op(a, "Brush");
            let path = s.vm.reg.obj_path(ue_registry::ObjId(brush)).to_string();
            let Some((_, export)) = s.vm.reg.export_by_path(&path) else { continue };
            let Some(b) = s.view.model_at(export) else { continue };
            let polys = s.view.brush_polys(export).unwrap_or_default();
            let m = dx_light::mover_model(&b, &polys, false, 0);
            if path.ends_with(".Model973") {
                let glass = m.surfs.iter().filter(|sf| sf.poly_flags & 0x4 != 0).count();
                assert_eq!(glass, 4, "{path}");
            }
            for (si, p) in polys.iter().enumerate() {
                // invisible and unlit polygons have no lightmap
                if p.poly_flags & 0x40_0001 != 0 {
                    continue;
                }
                let sf = dx_light::surface_of(&m, si).unwrap_or_else(|| panic!("{path} poly {si}: no lightmap"));
                lit += 1;
                let off = p.vertices.iter().fold(0.0f32, |w, v| {
                    let d = [v[0] - p.base[0], v[1] - p.base[1], v[2] - p.base[2]];
                    let t = |a: [f32; 3]| d[0] * a[0] + d[1] * a[1] + d[2] * a[2];
                    let (i, j) = ((t(p.texture_u) - sf.pan[0]) / sf.scale[0], (t(p.texture_v) - sf.pan[1]) / sf.scale[1]);
                    w.max(-i).max(i - sf.w as f32).max(-j).max(j - sf.h as f32)
                });
                if off <= 1.0 {
                    on_grid += 1;
                }
            }
        }
    }
    println!("{lit} lit mover polygons, {on_grid} on their lightmap grid");
    assert!(lit > 500, "{lit}");
    assert_eq!(on_grid, lit);
}

/// A camera 120 units in front of dynamic node `n` of the tracked level model, looking at it.
fn camera_at_node(m: &ue_assets::model::Model, n: usize) -> Option<dx_soft::Camera> {
    let nd = &m.nodes[n];
    if nd.num_vertices < 3 {
        return None;
    }
    let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| m.points[m.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
    let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
    let nrm = [nd.plane[0], nd.plane[1], nd.plane[2]];
    let pos = [c[0] + nrm[0] * 120.0, c[1] + nrm[1] * 120.0, c[2] + nrm[2] * 120.0];
    let yaw = ((-nrm[1]).atan2(-nrm[0]).to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
    let pitch = ((-nrm[2]).asin().to_degrees() / 360.0 * 65536.0) as i32;
    Some(dx_soft::Camera { pos, pitch, yaw, fov: 75.0, roll: 0 })
}

/// The mover whose brush model is `model` and that brush's export.
fn mover_with_brush(s: &mut dx_play::Session, model: &str) -> (u32, usize, usize) {
    let mover = s.vm.reg.class_by_name("Mover").unwrap();
    for a in s.vm.live_actors() {
        if !s.vm.class_of(a).map(|c| s.vm.is_a(c, mover)).unwrap_or(false) {
            continue;
        }
        let brush = s.vm.op(a, "Brush");
        let path = s.vm.reg.obj_path(ue_registry::ObjId(brush)).to_string();
        if path.ends_with(model) {
            let (pi, export) = s.vm.reg.export_by_path(&path).unwrap();
            return (a, pi, export);
        }
    }
    panic!("no mover with {model}");
}

/// The lit dynamic nodes of mover `a` (surfaces with a lightmap, neither invisible nor unlit).
fn lit_nodes(s: &dx_play::Session, a: u32) -> (ue_assets::model::Model, Vec<usize>) {
    let (m, nodes, first) = s.tracked_model(a).unwrap();
    let lit = nodes.into_iter().filter(|&n| m.nodes[n].i_surf as usize >= first && m.surfs[m.nodes[n].i_surf as usize].i_light_map >= 0 && m.surfs[m.nodes[n].i_surf as usize].poly_flags & 0x40_0001 == 0).collect();
    (m.clone(), lit)
}

type Rects = Vec<(usize, (u32, u32, u32, u32, Vec<u8>))>;

/// Render's `SetupForSurf` on a mover's surface (10b06c90): a drawn surface gets the static part of its brush model's
/// lightmap at the mover's current pose, as the brush-space bake the renderer starts from would give it; nothing is
/// redone while the mover stands still; a move makes it again (the cache item keeps `Location`/`Rotation`) and moving
/// back gives the first texels.
#[test]
fn drawn_mover_surfaces_are_lit_where_the_mover_stands() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&g, "03_NYC_UNATCOHQ").unwrap();
    s.enable_world_light();
    s.step_bound(1.0 / 30.0);
    let (a, pi, export) = mover_with_brush(&mut s, ".Model962");
    let (m, nodes) = lit_nodes(&s, a);
    let cam = camera_at_node(&m, nodes[0]).unwrap();
    let _ = s.mover_lightmap_updates();
    s.mark_rendered(&cam);
    let mine = |r: Rects| -> Rects {
        let mut r: Rects = r.into_iter().filter(|r| r.0 == export).collect();
        r.sort_by_key(|r| (r.1 .0, r.1 .1));
        r
    };
    let first = mine(s.mover_lightmap_updates());
    assert!(first.len() > 5, "{} rectangles", first.len());

    // the same surfaces baked in the brush's own space with the map's light properties
    let z = s.vis_frames()[0].vis.surfaces.iter().find(|x| x.0 as usize == nodes[0]).unwrap().2;
    let b = s.view.model_at(export).unwrap();
    let polys = s.view.brush_polys(export).unwrap();
    let mm = dx_light::mover_model(&b, &polys, false, z);
    let (loc, rot) = (s.vm.location(a), s.vm.rotation(a));
    let c = s.vm.brush_to_world(a, loc, rot);
    let place = dx_vk::world::Placement::from_coords(c.origin, c.x, c.y, c.z);
    let level = dx_light::collect(&mut s.vm.reg, pi, &s.view.model);
    let mut lighting = dx_light::collect(&mut s.vm.reg, pi, &b).transformed(|p| place.to_local(p), |d| place.dir_to_local(d));
    lighting.zone_ambient = level.zone_ambient.clone();
    let maps = dx_light::generate(&mm, &lighting);
    let mut compared = 0;
    for (_, (x, y, w, h, rgba)) in &first {
        let k = maps.surfaces.iter().position(|sm| sm.map(|sm| (sm.x - dx_light::BORDER, sm.y - dx_light::BORDER)) == Some((*x, *y))).expect("a rectangle at a polygon's atlas place");
        let sm = maps.surfaces[k].unwrap();
        assert_eq!((*w, *h), (sm.w + 2 * dx_light::BORDER, sm.h + 2 * dx_light::BORDER));
        // only lists of static lights (the per-frame ones are not in the bake)
        let all_static = dx_light::lightmap_lights(&b, polys[k].i_brush_poly as usize).iter().all(|(at, _)| lighting.lights[*at].map(|l| dx_light::manager::is_static_light(&l) || dx_light::manager::light_kind(&l) == dx_light::manager::LightKind::None).unwrap_or(true));
        if !all_static {
            continue;
        }
        let want = maps.texels(k).unwrap();
        for j in 0..sm.h as usize {
            for i in 0..sm.w as usize {
                let b = dx_light::BORDER as usize;
                let o = ((j + b) * *w as usize + i + b) * 4;
                let d = (0..3).map(|c| (rgba[o + c] as i32 - want[j * sm.w as usize + i][c] as i32).abs()).max().unwrap();
                assert!(d <= 1, "polygon {k} texel ({i}, {j}): {:?} vs {:?}", &rgba[o..o + 3], want[j * sm.w as usize + i]);
            }
        }
        compared += 1;
    }
    println!("{} rectangles, {compared} compared with the brush-space bake", first.len());
    assert!(compared > 5, "{compared}");

    s.mark_rendered(&cam);
    assert!(mine(s.mover_lightmap_updates()).is_empty(), "redone while standing still");
    s.vm.set_prop(a, "Location", 0, ue_vm::Value::Vector([loc[0] + 8.0, loc[1] + 8.0, loc[2]])).unwrap();
    s.mark_rendered(&cam);
    let moved = mine(s.mover_lightmap_updates());
    assert!(moved.iter().any(|r| first.iter().any(|f| (f.1 .0, f.1 .1) == (r.1 .0, r.1 .1) && f.1 .4 != r.1 .4)), "moving changes nothing");
    s.vm.set_prop(a, "Location", 0, ue_vm::Value::Vector(loc)).unwrap();
    s.mark_rendered(&cam);
    let back = mine(s.mover_lightmap_updates());
    // surfaces drawn at the moved pose are redone with their first texels; one not drawn there kept its item
    assert!(!back.is_empty());
    for f in &first {
        let key = (f.1 .0, f.1 .1);
        match back.iter().find(|r| (r.1 .0, r.1 .1) == key) {
            Some(r) => assert!(r.1 .4 == f.1 .4, "texels differ after moving back"),
            None => assert!(!moved.iter().any(|r| (r.1 .0, r.1 .1) == key), "drawn when moved but not redone when moved back"),
        }
    }
}

/// A mover lit by flickering lights (Battery Park's `Model832`, three `LT_Flicker` lights in its brush's lists) gets
/// them merged every frame like a level surface.
#[test]
fn flickering_lights_on_a_mover_change_its_lightmap_every_frame() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&g, "02_NYC_BatteryPark").unwrap();
    s.enable_world_light();
    s.step_bound(1.0 / 30.0);
    let (a, _, export) = mover_with_brush(&mut s, ".Model832");
    let (m, nodes) = lit_nodes(&s, a);
    let mut varying = 0;
    for &n in &nodes {
        let Some(cam) = camera_at_node(&m, n) else { continue };
        let mut seen: std::collections::HashMap<(u32, u32), Vec<Vec<u8>>> = Default::default();
        for _ in 0..12 {
            s.step_bound(1.0 / 30.0);
            let _ = s.mover_lightmap_updates();
            s.mark_rendered(&cam);
            for (e, (x, y, _, _, rgba)) in s.mover_lightmap_updates() {
                if e == export {
                    seen.entry((x, y)).or_default().push(rgba);
                }
            }
        }
        varying += seen.values().filter(|v| v.len() > 1 && v.windows(2).any(|p| p[0] != p[1])).count();
        if varying > 0 {
            break;
        }
    }
    assert!(varying > 0, "no surface of the mover changed over time ({} lit nodes)", nodes.len());
}

/// Movers drawn through the main frame's lists (`OccludeBsp` draws the tracker's dynamic nodes like level nodes): the
/// fragments of a door seen head-on lie on their surfaces' world planes and stand for the nodes the walk drew; a mover
/// moved out of the level (as a broken `DeusExMover` is, 20000 units up) leaves no fragment, since the tracker drops
/// pieces that land in solid space.
#[test]
fn movers_are_drawn_from_the_fragments_the_walk_drew() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&g, "03_NYC_UNATCOHQ").unwrap();
    s.step_bound(1.0 / 30.0);
    let (a, _, export) = mover_with_brush(&mut s, ".Model962");
    let (m, nodes) = lit_nodes(&s, a);
    let cam = camera_at_node(&m, nodes[0]).unwrap();
    s.mark_rendered(&cam);
    let listed = s.listed_movers().expect("a walk with the movers");
    assert!(listed.owners.contains(&(a, export)));
    let frags = &listed.fragments[&export];
    let polys = s.view.brush_polys(export).unwrap();
    assert!(!frags.is_empty());
    for f in frags {
        assert!(f.surf < polys.len() && f.verts.len() >= 3);
        let l = (f.normal[0].powi(2) + f.normal[1].powi(2) + f.normal[2].powi(2)).sqrt();
        for p in &f.verts {
            let d = ((p[0] - f.base[0]) * f.normal[0] + (p[1] - f.base[1]) * f.normal[1] + (p[2] - f.base[2]) * f.normal[2]) / l;
            assert!(d.abs() < 0.5, "fragment of polygon {} off its plane by {d}", f.surf);
        }
    }
    let (tm, _, _) = s.tracked_model(a).unwrap();
    for (&node, &(e, i)) in &listed.nodes {
        if e == export {
            let nd = &tm.nodes[node as usize];
            assert_eq!(nd.num_vertices as usize, frags[i].verts.len());
        }
    }
    println!("{} fragments of {} polygons", frags.len(), polys.len());

    let loc = s.vm.location(a);
    s.vm.set_prop(a, "Location", 0, ue_vm::Value::Vector([loc[0], loc[1], loc[2] + 20000.0])).unwrap();
    s.mark_rendered(&cam);
    let listed = s.listed_movers().unwrap();
    assert!(!listed.fragments.contains_key(&export), "a mover out of the level still drawn");
}

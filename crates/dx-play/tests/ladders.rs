//! Climbing ladders (world surfaces whose texture is in the `Ladder` group). Set `DEUSEX_DIR` to the game folder.

use dx_input::key_code;
use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
    }
}

/// Ladder polygons of the loaded map: (centre, outward normal, lowest z, highest z), biggest first.
fn ladders(s: &mut Session) -> Vec<([f32; 3], [f32; 3], f32, f32)> {
    let ladder = s.vm.reg.names.intern("Ladder");
    let m = &s.view.model;
    let mut out = Vec::new();
    for n in &m.nodes {
        if n.num_vertices == 0 || n.i_surf < 0 {
            continue;
        }
        if s.vm.surf_info.get(n.i_surf as usize).map(|i| i.texture_group) != Some(ladder) || n.plane[2].abs() > 0.3 {
            continue;
        }
        let pts: Vec<[f32; 3]> =
            (0..n.num_vertices as i32).map(|k| m.points[m.verts[(n.i_vert_pool + k) as usize].p_vertex as usize]).collect();
        let c = pts.iter().fold([0.0; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
        let c = [c[0] / pts.len() as f32, c[1] / pts.len() as f32, c[2] / pts.len() as f32];
        let lo = pts.iter().map(|p| p[2]).fold(f32::MAX, f32::min);
        let hi = pts.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
        out.push((c, [n.plane[0], n.plane[1], n.plane[2]], lo, hi));
    }
    out.sort_by(|a, b| (b.3 - b.2).partial_cmp(&(a.3 - a.2)).unwrap());
    out
}

fn z(s: &mut Session) -> f32 {
    s.player_location()[2]
}

#[test]
fn the_player_climbs_a_ladder() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    run(&mut s, 5);
    let found = ladders(&mut s);
    eprintln!("{} ladder polygons; tallest {:?}", found.len(), found.first());
    let player = s.player;
    let radius = s.vm.get_prop(player, "CollisionRadius", 0).ok().and_then(|v| v.as_f32()).unwrap();
    let height = s.vm.get_prop(player, "CollisionHeight", 0).ok().and_then(|v| v.as_f32()).unwrap();
    let mut climbed = false;
    for (c, n, lo, hi) in found.into_iter().take(8) {
        if hi - lo < 2.0 * height {
            continue;
        }
        let spot = [c[0] + n[0] * (radius + 6.0), c[1] + n[1] * (radius + 6.0), lo + height + 8.0];
        let yaw = (-n[1]).atan2(-n[0]).to_degrees();
        s.teleport(spot, yaw);
        s.vm.set_prop(player, "Velocity", 0, Value::Vector([0.0; 3])).unwrap();
        run(&mut s, 10);
        // face the ladder from the side the player stands on
        let p = s.player_location();
        let side = if (p[0] - c[0]) * n[0] + (p[1] - c[1]) * n[1] >= 0.0 { 1.0 } else { -1.0 };
        let yaw = (-n[1] * side).atan2(-n[0] * side).to_degrees();
        s.teleport(p, yaw);
        run(&mut s, 2);
        let z0 = z(&mut s);
        // look level and push forward: climb
        let r = match s.vm.get_prop(player, "ViewRotation", 0) { Ok(Value::Rotator(r)) => r, _ => [0; 3] };
        s.vm.set_prop(player, "ViewRotation", 0, Value::Rotator([0, r[1], 0])).unwrap();
        s.input.key(key_code("W").unwrap(), true);
        run(&mut s, 30);
        s.input.key(key_code("W").unwrap(), false);
        let z1 = z(&mut s);
        // let go: hang on the ladder
        run(&mut s, 30);
        let z2 = z(&mut s);
        eprintln!("ladder at {c:?} ({lo:.0}..{hi:.0}): z {z0:.0} -> {z1:.0} climbing -> {z2:.0} hanging");
        if z1 - z0 < 40.0 {
            continue;
        }
        assert!((z2 - z1).abs() < 4.0, "slid down while hanging: {z1} -> {z2}");
        // look down and push forward: climb down
        s.vm.set_prop(player, "ViewRotation", 0, Value::Rotator([65536 - 12000, r[1], 0])).unwrap();
        s.input.key(key_code("W").unwrap(), true);
        run(&mut s, 15);
        s.input.key(key_code("W").unwrap(), false);
        let z3 = z(&mut s);
        eprintln!("looking down: {z2:.0} -> {z3:.0}");
        assert!(z3 < z2 - 10.0, "did not climb down");
        climbed = true;
        break;
    }
    assert!(climbed, "no ladder could be climbed (log: {:?})", s.vm.log);
}

/// Grates, fences and ladders whose surface is not flagged masked but whose texture is (`ladder_a`, `HevyMetlGrate_A`,
/// `metalgrate_a`, ...): `Texture`'s 32 surface bools are its `PolyFlags` (declared in flag order, packed in one
/// word), and the walk and the renderer OR them into the surface's. The port looked for a `PolyFlags` property that
/// does not exist and drew these solid with black between the bars (the tester's "black backgrounds"); the walk also
/// let them hide what lay behind. A ceiling grate of the sewers, looked at from below.
#[test]
fn masked_textures_make_their_surfaces_masked() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "02_NYC_Underground").unwrap();
    run(&mut s, 3);
    let pkg = ue_package::Package::open(g.join("Maps").join("02_NYC_Underground.dx")).unwrap();
    let grate = |raw: i32| pkg.object_path(ue_package::ObjectRef::from_index(raw)).map(|p| p.ends_with("HevyMetlGrate_A")).unwrap_or(false);
    // every grate surface of the map: not masked itself, masked through its texture
    let model = s.view.model.clone();
    let surfs: Vec<usize> = (0..model.surfs.len()).filter(|&i| grate(model.surfs[i].texture)).collect();
    assert!(!surfs.is_empty());
    assert!(surfs.iter().all(|&i| model.surfs[i].poly_flags & 2 == 0), "the map flags them masked itself");
    assert!(surfs.iter().all(|&i| s.view.texture_poly_flags(model.surfs[i].texture) & 2 != 0), "the texture's PolyFlags carry bMasked");
    // the walk lists the ceiling grate with the merged flags
    let cam = dx_soft::Camera { pos: [-2988.0, 994.0, -1060.0], pitch: 14000, yaw: 49152, fov: 90.0, roll: 0 };
    let (_, frames) = s.compute_visibility(&cam, 800, 600);
    let listed: Vec<u32> = frames[0].vis.surfaces.iter().filter(|(n, _, _)| model.nodes.get(*n as usize).map(|nd| nd.i_surf >= 0 && grate(model.surfs[nd.i_surf as usize].texture)).unwrap_or(false)).map(|&(_, f, _)| f).collect();
    assert!(!listed.is_empty(), "the grate is not in view");
    assert!(listed.iter().all(|f| f & 2 != 0), "the walk lists the grate unmasked: {listed:x?}");
}

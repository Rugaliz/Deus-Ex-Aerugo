//! The interactive session without a window. Set `DEUSEX_DIR` to the game folder.

use dx_play::{Input, Session};
use std::path::PathBuf;

fn open(map: &str) -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    Some(Session::open(&g, map).unwrap())
}

const DT: f32 = 1.0 / 30.0;

/// The player settles on the floor, walks along its facing at the walking pace, stops at the end of the corridor
/// without leaving the world, and jumping lifts it and brings it back down.
#[test]
fn player_walks_stops_at_walls_and_jumps() {
    let Some(mut s) = open("00_Training") else { return };
    let idle = Input::default();
    for _ in 0..30 {
        s.step(&idle, DT);
    }
    assert!(s.on_ground(), "the player did not land");
    // jump (before walking anywhere: the open floor at the start)
    let floor = s.player_location()[2];
    s.step(&Input { jump: true, ..Default::default() }, DT);
    let mut top = floor;
    for _ in 0..60 {
        s.step(&idle, DT);
        top = top.max(s.player_location()[2]);
    }
    assert!(top - floor > 30.0 && top - floor < 120.0, "jump height {}", top - floor);
    assert!(s.on_ground() && (s.player_location()[2] - floor).abs() < 5.0, "did not land back on the floor");
    let start = s.player_location();
    // walk forward for one second: the scripts' walking pace is 0.3 of the 320 ground speed (96 units/s once up to speed)
    let walk = Input { forward: 1.0, ..Default::default() };
    for _ in 0..30 {
        s.step(&walk, DT);
    }
    let p = s.player_location();
    let yaw = s.yaw as f32 * std::f32::consts::TAU / 65536.0;
    let along = (p[0] - start[0]) * yaw.cos() + (p[1] - start[1]) * yaw.sin();
    assert!(along > 60.0 && along < 100.0, "walked {along} units in a second");
    // keep running into whatever is ahead: the player ends up still, inside the world
    let run = Input { forward: 1.0, run: true, ..Default::default() };
    for _ in 0..600 {
        s.step(&run, DT);
        let at = s.player_location();
        let zone = s.vm.point_region(at).zone_number;
        assert_ne!(zone, 0, "the player left the world at {at:?}");
    }
    let a = s.player_location();
    for _ in 0..30 {
        s.step(&run, DT);
    }
    let b = s.player_location();
    let moved = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
    assert!(moved < 20.0, "still moving after 20 s of walking into the wall: {moved}");
}

/// Looking turns the view, the render shows the world, and the markers find the training map's people.
#[test]
fn view_turns_renders_and_marks_pawns() {
    let Some(mut s) = open("00_Training") else { return };
    for _ in 0..30 {
        s.step(&Input::default(), DT);
    }
    let yaw0 = s.yaw;
    s.step(&Input { yaw: 8192, pitch: -4000, ..Default::default() }, DT);
    assert_eq!((s.yaw - yaw0) & 0xFFFF, 8192, "a quarter-turn of yaw units");
    assert_eq!(s.pitch, -4000);
    let img = s.render(320, 180);
    let mut colors = std::collections::HashSet::new();
    for px in img.rgba.chunks(4).step_by(37) {
        colors.insert([px[0] / 16, px[1] / 16, px[2] / 16]);
    }
    assert!(colors.len() > 20, "the picture is nearly flat ({} colours)", colors.len());
    // next to the people of the training map, somewhere on a full turn a pawn is in sight
    s.teleport([200.0, 1206.0, -50.0], 0.0);
    for _ in 0..10 {
        s.step(&Input::default(), DT);
    }
    let mut seen = 0;
    for _ in 0..16 {
        s.step(&Input { yaw: 4096, ..Default::default() }, DT);
        let cam = s.camera();
        seen += s.pawn_boxes(&cam, 640, 360).len();
    }
    assert!(seen > 0, "no pawn was ever in sight during a full turn");
}

/// Actor meshes are drawn into the view where the actors stand: next to the people of the training map the picture
/// changes a lot when meshes are on, and far from everyone it does not.
#[test]
fn actor_meshes_appear_in_the_view() {
    let Some(mut s) = open("00_Training") else { return };
    s.teleport([200.0, 1206.0, -50.0], 0.0);
    for _ in 0..10 {
        s.step(&Input::default(), DT);
    }
    s.draw_meshes = false;
    let without = s.render(320, 180);
    s.draw_meshes = true;
    let with = s.render(320, 180);
    assert!(s.meshes_drawn >= 1, "no mesh was drawn");
    let differing = with.rgba.chunks(4).zip(without.rgba.chunks(4)).filter(|(a, b)| a != b).count();
    let total = 320 * 180;
    assert!(differing * 20 > total, "only {differing} of {total} pixels changed with meshes on");
    // the first corridor has no mesh actors in sight: the picture is the same with and without
    s.teleport([-1149.0, 825.0, -50.0], 0.0);
    for _ in 0..10 {
        s.step(&Input::default(), DT);
    }
    s.draw_meshes = false;
    let a = s.render(160, 90);
    s.draw_meshes = true;
    let b = s.render(160, 90);
    let changed = a.rgba.chunks(4).zip(b.rgba.chunks(4)).filter(|(x, y)| x != y).count();
    assert!(changed * 50 < 160 * 90, "{changed} pixels changed in an empty corridor");
}

/// Where the CPU time of a frame goes on Liberty Island (simulation, actor list, window system).
#[test]
#[ignore]
fn frame_cost_on_liberty_island() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..30 {
        s.step_bound(1.0 / 60.0);
    }
    let (mut sim, mut act, mut ui) = (0.0, 0.0, 0.0);
    let n: usize = std::env::var("DX_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
    for _ in 0..n {
        let t = std::time::Instant::now();
        s.step_bound(1.0 / 60.0);
        sim += t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let cam = s.camera();
        let d = s.actor_draws(&cam);
        act += t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let q = s.ui_quads(1280, 720);
        ui += t.elapsed().as_secs_f64();
        std::hint::black_box((d.len(), q.len()));
    }
    eprintln!("per frame: simulation {:.1} ms, actor list {:.1} ms, window system {:.1} ms", sim * 1000.0 / n as f64, act * 1000.0 / n as f64, ui * 1000.0 / n as f64);
    if std::env::var_os("DX_VM_PROFILE").is_some() {
        eprintln!("{}", s.vm.profile_report(25));
    }
}

/// Walking NPCs face where they go: the angle between the velocity and the facing (yaw) of every moving pawn.
#[test]
fn walking_npcs_face_their_direction() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland");
    let sp = s.vm.reg.class_by_name("ScriptedPawn").unwrap();
    let pawns: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, sp)).unwrap_or(false)).collect();
    let mut worst: Vec<(f32, String)> = Vec::new();
    let mut samples = 0;
    let mut last: std::collections::HashMap<u32, [f32; 3]> = Default::default();
    for f in 0..900 {
        s.step_bound(1.0 / 30.0);
        if f < 60 || f % 3 != 0 {
            continue;
        }
        for &a in &pawns {
            if !s.vm.is_live(a) {
                continue;
            }
            let l = s.vm.location(a);
            let Some(prev) = last.insert(a, l) else { continue };
            // displacement over the last 3 frames, as a speed
            let v = [(l[0] - prev[0]) * 10.0, (l[1] - prev[1]) * 10.0, 0.0];
            let reported = s.vm.get_prop(a, "Velocity", 0).ok();
            let speed = (v[0] * v[0] + v[1] * v[1]).sqrt();
            if speed < 40.0 {
                continue;
            }
            let yaw = match s.vm.get_prop(a, "Rotation", 0) { Ok(ue_vm::Value::Rotator(r)) => r[1], _ => continue };
            let face = (yaw & 0xFFFF) as f32 / 65536.0 * 360.0;
            let go = v[1].atan2(v[0]).to_degrees().rem_euclid(360.0);
            let diff = ((face - go + 540.0).rem_euclid(360.0) - 180.0).abs();
            samples += 1;
            let class = s.vm.class_of(a).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
            worst.push((diff, format!("{class} state {:?} speed {speed:.0} facing {face:.0} moving {go:.0} Velocity {reported:?}", s.vm.state_name(a))));
        }
    }
    worst.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (d, w) in worst.iter().take(8) {
        eprintln!("{d:.0} deg off: {w}");
    }
    let bad = worst.iter().filter(|w| w.0 > 45.0).count();
    eprintln!("{samples} samples of moving pawns, {bad} more than 45 degrees off");
    assert!(samples > 10);
    assert!(bad * 10 < samples, "{bad} of {samples} samples: pawns move sideways");
}

/// How many mesh actors are near the start of Liberty Island (the actor list keeps the nearest ones).
#[test]
#[ignore]
fn survey_mesh_actors_at_the_island_start() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    let cam = s.camera();
    let mut dists: Vec<(f32, String)> = Vec::new();
    for a in s.vm.live_actors() {
        if a == s.player {
            continue;
        }
        let Some(d) = s.vm.mesh_draw(a) else { continue };
        if d.sprite.is_some() || d.decal.is_some() {
            continue;
        }
        let l = d.location;
        let dist = ((l[0] - cam.pos[0]).powi(2) + (l[1] - cam.pos[1]).powi(2) + (l[2] - cam.pos[2]).powi(2)).sqrt();
        dists.push((dist, d.mesh.clone()));
    }
    dists.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    eprintln!("{} mesh actors; within 5000: {}; the 80th is at {:.0}", dists.len(), dists.iter().filter(|d| d.0 < 5000.0).count(), dists.get(79).map(|d| d.0).unwrap_or(0.0));
    let crates: Vec<_> = dists.iter().filter(|d| d.1.to_ascii_lowercase().contains("crate")).take(10).collect();
    eprintln!("nearest crates: {crates:?}");
    eprintln!("drawn now: {}", s.actor_draws(&cam).len());
}

/// Pictures from the start of Liberty Island toward the nearest crates and around; needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_the_island_start() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    let start = s.player_location();
    eprintln!("start {start:?} yaw {}", s.yaw as f32 * 360.0 / 65536.0);
    let mut r = None;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    for k in 0..8 {
        let yaw = k as f32 * 45.0;
        s.teleport(start, yaw);
        for _ in 0..3 {
            s.step_bound(1.0 / 30.0);
        }
        let img = s.render_vk(&mut r, 800, 600).expect("render");
        std::fs::write(dir.join(format!("s11_island_start_{:03}.png", yaw as i32)), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    }
    r.take().unwrap().destroy();
}

#[test]
#[ignore]
fn survey_sky_zone_rotations() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
    for map in ["01_NYC_UNATCOIsland", "00_Training", "02_NYC_BatteryPark", "06_HongKong_Helibase", "10_Paris_Catacombs", "15_Area51_Bunker"] {
        let mut s = dx_play::Session::open(&g, map).unwrap();
        let c = s.vm.reg.class_by_name("SkyZoneInfo").unwrap();
        let skies: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).collect();
        for a in skies {
            let cls = s.vm.class_of(a).map(|k| s.vm.reg.class(k).path.clone()).unwrap_or_default();
            let (loc, rot) = (s.vm.location(a), s.vm.get_prop(a, "Rotation", 0));
            eprintln!("{map}: {cls} at {loc:?} rotation {rot:?}");
        }
    }
}

/// No mirror surface is in sight on the pier: the picture must not change when the map's mirror planes are dropped
/// (reflections used to be drawn over the whole screen and showed upside-down copies of actors through the water).
/// Needs a Vulkan device.
#[test]
#[ignore]
fn reflections_stay_inside_mirror_surfaces() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    let start = s.player_location();
    let mut r = None;
    for yaw in [45.0, 90.0] {
        s.teleport(start, yaw);
        s.step_bound(1.0 / 30.0);
        let with = s.render_vk(&mut r, 800, 600).expect("render").rgba;
        let planes = std::mem::take(&mut r.as_mut().unwrap().world.mirror_planes);
        assert!(!planes.is_empty(), "the island has mirror surfaces (UNATCO HQ)");
        let without = s.render_vk(&mut r, 800, 600).expect("render").rgba;
        r.as_mut().unwrap().world.mirror_planes = planes;
        let differ = with.chunks(4).zip(without.chunks(4)).filter(|(a, b)| a != b).count();
        assert_eq!(differ, 0, "yaw {yaw}: {differ} pixels come from reflections");
    }
    r.take().unwrap().destroy();
}

/// `XGameEngineExt::Destroy` -> `XRootWindow::DestroyAllWindows`: at shutdown every root window and its tree go
/// (`SafeDestroy`, their `DestroyWindow` events run); nothing is left (WORKLOG 3.193).
#[test]
fn shutdown_destroys_every_window() {
    let Some(mut s) = open("01_NYC_UNATCOIsland") else { return };
    for _ in 0..3 {
        s.step_bound(1.0 / 30.0);
    }
    let root = match s.vm.get_prop(s.player, "rootWindow", 0).unwrap() {
        ue_vm::Value::Obj(r) => r,
        v => panic!("{v:?}"),
    };
    assert!(root != 0 && s.vm.is_live(root));
    s.vm.ui_destroy_all_windows();
    assert!(!s.vm.is_live(root));
    assert!(s.vm.ui.roots.is_empty());
}

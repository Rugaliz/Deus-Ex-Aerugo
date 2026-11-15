//! Camera paths (`PHYS_Interpolating`) over `InterpolationPoint`s. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::collections::BTreeMap;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn new_game() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(DT);
    }
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland");
    Some(s)
}

/// The paths of the map: tag -> points sorted by position (object, location).
fn paths(s: &mut Session) -> BTreeMap<String, Vec<(u32, [f32; 3])>> {
    let ip = s.vm.reg.class_by_name("InterpolationPoint").unwrap();
    let mut out: BTreeMap<String, Vec<(i32, u32, [f32; 3])>> = BTreeMap::new();
    for a in s.vm.live_actors() {
        if !s.vm.class_of(a).map(|c| s.vm.is_a(c, ip)).unwrap_or(false) {
            continue;
        }
        let tag = match s.vm.get_prop(a, "Tag", 0) { Ok(Value::Name(n)) => s.vm.reg.names.str(n).to_string(), _ => continue };
        let pos = s.vm.get_prop(a, "Position", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0);
        let loc = s.vm.location(a);
        out.entry(tag).or_default().push((pos, a, loc));
    }
    out.into_iter().map(|(k, mut v)| { v.sort_by_key(|p| p.0); (k, v.into_iter().map(|p| (p.1, p.2)).collect()) }).collect()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[test]
fn the_player_rides_a_camera_path() {
    let Some(mut s) = new_game() else { return };
    let all = paths(&mut s);
    for (tag, pts) in &all {
        eprintln!("path {tag}: {} points", pts.len());
    }
    // follow the `Next` links from a point at position 0 until the end of the path (or a loop)
    let chain = |s: &mut Session, start: u32| -> Vec<(u32, [f32; 3])> {
        let mut out = vec![];
        let mut cur = start;
        while cur != 0 && out.len() < 40 && !out.iter().any(|(o, _)| *o == cur) {
            out.push((cur, s.vm.location(cur)));
            if matches!(s.vm.get_prop(cur, "bEndOfPath", 0), Ok(Value::Bool(true))) {
                break;
            }
            cur = match s.vm.get_prop(cur, "Next", 0) { Ok(Value::Obj(n)) => n, _ => 0 };
        }
        out
    };
    let mut best: Vec<(u32, [f32; 3])> = vec![];
    for (_, v) in &all {
        let c = chain(&mut s, v[0].0);
        if c.len() > best.len() {
            best = c;
        }
    }
    let pts = best;
    assert!(pts.len() >= 4, "no path with four points on the map");
    eprintln!("riding a path of {} points from {:?}", pts.len(), pts[0].1);
    let p = s.player;
    // the way the SpecialEvent PlayerPath does it
    s.vm.call_by_name(p, "GotoState", vec![Value::Name(0)]).ok();
    s.vm.call_by_name(p, "SetCollision", vec![Value::Bool(true), Value::Bool(false), Value::Bool(false)]).unwrap();
    s.vm.set_prop(p, "bCollideWorld", 0, Value::Bool(false)).unwrap();
    s.vm.set_prop(p, "Target", 0, Value::Obj(pts[0].0)).unwrap();
    s.vm.call_by_name(p, "SetPhysics", vec![Value::Byte(8)]).unwrap();
    s.vm.set_prop(p, "PhysRate", 0, Value::Float(1.0)).unwrap();
    s.vm.set_prop(p, "PhysAlpha", 0, Value::Float(0.0)).unwrap();
    s.vm.set_prop(p, "bInterpolating", 0, Value::Bool(true)).unwrap();
    // follow it: the nearest the player comes to each point must be small (the curve passes through the points)
    let mut nearest = vec![f32::MAX; pts.len()];
    let mut frames = 0;
    let mut last = s.vm.location(p);
    let mut max_step = 0.0f32;
    while frames < 3000 && matches!(s.vm.get_prop(p, "bInterpolating", 0), Ok(Value::Bool(true))) {
        s.step_bound(DT);
        frames += 1;
        let loc = s.vm.location(p);
        if frames > 1 {
            max_step = max_step.max(dist(loc, last));
        }
        last = loc;
        for (k, (_, l)) in pts.iter().enumerate() {
            nearest[k] = nearest[k].min(dist(loc, *l));
        }
    }
    eprintln!("{frames} frames, nearest passes {nearest:?}, largest step {max_step}");
    assert!(frames > 5 && frames < 3000, "the path never ended or never ran ({frames} frames)");
    // the curve smooths the corners (it does not pass through inner points) but starts and ends on the end points
    assert!(nearest[0] < 20.0 && *nearest.last().unwrap() < 20.0, "the ends of the path were missed");
    assert!(dist(last, pts.last().unwrap().1) < 20.0, "the player did not stop at the end of the path");
    assert!(max_step < 150.0, "the player jumped {max_step} units in a frame");
}

/// What the game's own intro does with the camera: `StartNewGame` with no map goes to `00_Intro`.
#[test]
fn the_intro_map_moves_the_camera() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 00_Intro");
    for _ in 0..5 {
        s.step_bound(DT);
    }
    eprintln!("map {}", s.map_name);
    let p = s.player;
    let mut prev = s.vm.location(p);
    let mut moved = 0.0f32;
    for f in 0..1800 {
        s.step_bound(DT);
        let l = s.vm.location(p);
        moved += dist(l, prev);
        prev = l;
        if f % 150 == 0 {
            let phys = s.vm.get_prop(p, "Physics", 0).ok();
            let interp = s.vm.get_prop(p, "bInterpolating", 0).ok();
            eprintln!("frame {f}: at {l:?} physics {phys:?} interpolating {interp:?} state {:?} map {}", s.vm.state_name(p), s.map_name);
        }
    }
    eprintln!("moved {moved}");
}

fn shot(s: &mut Session, r: &mut Option<dx_vk::renderer::Renderer>, name: &str) {
    let img = s.render_vk(r, 640, 480).expect("vulkan render");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
}

/// Pictures of the intro over time; needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_the_intro() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 00_Intro");
    let mut r = None;
    for k in 0..12 {
        for _ in 0..150 {
            s.step_bound(DT);
        }
        shot(&mut s, &mut r, &format!("s10_intro_{k:02}.png"));
    }
    r.take().unwrap().destroy();
}

/// Pictures of the logo level the game starts on, without the menu the port opens over it; needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_the_logo_level() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    let mut r = None;
    let p = s.player;
    for k in 0..12 {
        for _ in 0..90 {
            s.step_bound(DT);
        }
        let cam = s.camera();
        let nd = s.actor_draws(&cam).len();
        eprintln!("draws {nd}, camera {:?}", (cam.pos, cam.pitch, cam.yaw));
        if k == 6 || k == 9 {
            for d in s.actor_draws(&cam) {
                eprintln!("   {} at {:?} scale {} style {} glow {} unlit {} sprite {:?}", d.mesh, d.location, d.scale, d.style, d.glow, d.unlit, d.sprite);
            }
        }
        eprintln!("t={}s at {:?} physics {:?} interpolating {:?} state {:?}", (k + 1) * 3, s.vm.location(p), s.vm.get_prop(p, "Physics", 0).ok(), s.vm.get_prop(p, "bInterpolating", 0).ok(), s.vm.state_name(p));
        shot(&mut s, &mut r, &format!("s10_logo_{k:02}.png"));
        eprintln!("meshes drawn {}", s.meshes_drawn);
    }
    r.take().unwrap().destroy();
}

#[test]
fn the_logo_level_runs_to_its_end() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    let p = s.player;
    let mut ended = None;
    for f in 0..4000 {
        s.step_bound(DT);
        if f % 150 == 0 {
            let lvl = s.vm.level;
            eprintln!("t={:.0}s dilation {:?} interp {:?} state {:?} menu {}", f as f32 * DT, s.vm.get_prop(lvl, "TimeDilation", 0).ok(), s.vm.get_prop(p, "bInterpolating", 0).ok(), s.vm.state_name(p), s.ui_wants_mouse());
        }
        if ended.is_none() && !matches!(s.vm.get_prop(p, "bInterpolating", 0), Ok(Value::Bool(true))) {
            ended = Some(f);
        }
    }
    eprintln!("path ended at frame {ended:?}");
}

/// During the logo cutscene a click or the menu key brings up the main menu, as in the original.
#[test]
fn a_key_press_ends_the_logo_with_the_menu() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    for key in ["LeftMouse", "Escape"] {
        let mut s = Session::open(&g, "DX").unwrap();
        for _ in 0..120 {
            s.step_bound(DT);
        }
        assert!(!s.ui_wants_mouse(), "the menu is up before any key ({key})");
        s.input.key(dx_input::key_code(key).unwrap(), true);
        for _ in 0..5 {
            s.step_bound(DT);
        }
        s.input.key(dx_input::key_code(key).unwrap(), false);
        for _ in 0..5 {
            s.step_bound(DT);
        }
        assert!(s.ui_wants_mouse(), "{key} did not open the main menu during the logo");
    }
}

#[test]
#[ignore]
fn survey_the_logo_level() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    let mut h: BTreeMap<String, usize> = BTreeMap::new();
    for a in s.vm.live_actors() {
        let c = s.vm.class_of(a).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
        *h.entry(c).or_default() += 1;
    }
    for (c, n) in &h {
        eprintln!("{n:4} {c}");
    }
    let cam = s.camera();
    eprintln!("camera {:?}", (cam.pos, cam.pitch, cam.yaw, cam.fov));
    let draws = s.actor_draws(&cam);
    eprintln!("{} actor draws", draws.len());
}

/// The logo level every 5 seconds at 800x600 and neutral brightness, for comparison with the original (`tools/oracle/screens.sh`).
#[test]
#[ignore]
fn shots_of_the_logo_timeline() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.brightness = 0.5;
    let mut r = None;
    for k in 1..=22 {
        for _ in 0..150 {
            s.step_bound(DT);
        }
        let img = s.render_vk(&mut r, 800, 600).expect("vulkan render");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
        std::fs::write(dir.join(format!("ours_s11_logo_{k:02}.png")), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    }
    r.take().unwrap().destroy();
}

#[test]
#[ignore]
fn survey_logo_actor_props() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let s = Session::open(&g, "DX").unwrap();
    for name in ["DXLogo", "IonStormLogo", "EidosLogo", "DXText"] {
        let c = s.vm.reg.class_by_name(name).unwrap();
        let a = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(c)).unwrap();
        let mut line = format!("{name}:");
        for p in ["Mesh", "Texture", "Skin", "MultiSkins", "bMeshEnviroMap", "Style", "ScaleGlow", "AmbientGlow", "bUnlit", "DrawType", "Fatness", "bMeshCurvy", "LightType", "bParticles"] {
            let i = if p == "MultiSkins" { 0 } else { 0 };
            line += &format!(" {p}={:?}", s.vm.get_prop(a, p, i).ok());
        }
        eprintln!("{line}");
    }
}

#[test]
#[ignore]
fn survey_logo_mesh_materials() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    for name in ["DeusExDeco.DXLogo", "DeusExDeco.EidosLogo", "DeusExDeco.IonStormLogo"] {
        let m = s.view.mesh_data(name).unwrap();
        eprintln!("{name}: materials (flags, slot) {:x?}, own textures {:?}", m.lod.materials, m.own_texture_paths);
    }
    let c = s.vm.reg.class_by_name("DXLogo").unwrap();
    let a = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(c)).unwrap();
    if let Ok(Value::Obj(t)) = s.vm.get_prop(a, "Texture", 0) {
        eprintln!("actor Texture = {}", s.vm.reg.obj_path(ue_registry::ObjId(t)));
    }
}

/// Every camera shot of the intro (new game, `00_Intro`), found by its cut (the view jumps more than 256 units in a
/// step) and pictured 1 s into the shot, at the middle and 0.2 s before the next cut, at 16:9 (`DX_SHOT_W`/`DX_SHOT_H`,
/// default 1280x720). Writes `out/screenshots/intro_shotNN_{a,b,c}.png` and `intro_shots.txt` (step, time, camera
/// position, rotation, field of view). Deterministic: the same steps give the same pictures. Needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_each_intro_camera() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let w: u32 = std::env::var("DX_SHOT_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1280);
    let h: u32 = std::env::var("DX_SHOT_H").ok().and_then(|v| v.parse().ok()).unwrap_or(720);
    let start = |g: &PathBuf| {
        let mut s = Session::open(g, "DX").unwrap();
        s.exec("StartNewGame 00_Intro");
        s
    };
    // pass 1: where the cuts are
    let mut s = start(&g);
    let mut cuts = vec![0usize];
    let mut prev = s.camera().pos;
    let total = 30 * 150;
    for k in 1..=total {
        s.step_bound(DT);
        if s.map_name != "00_Intro" {
            cuts.push(k);
            break;
        }
        let p = s.camera().pos;
        if dist(p, prev) > 256.0 {
            cuts.push(k);
        }
        prev = p;
    }
    let end = *cuts.last().unwrap();
    if cuts.len() < 2 || end != total {
        cuts.push(end.max(total));
    }
    // pass 2: the pictures
    let mut want: Vec<(usize, String)> = Vec::new();
    for (i, pair) in cuts.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        if b <= a + 12 {
            continue;
        }
        want.push(((a + 30).min(b - 1), format!("intro_shot{i:02}_a")));
        want.push(((a + b) / 2, format!("intro_shot{i:02}_b")));
        want.push((b.saturating_sub(6).max(a + 1), format!("intro_shot{i:02}_c")));
    }
    want.sort();
    want.dedup_by_key(|x| x.0);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = start(&g);
    let mut r = None;
    let mut log = format!("cuts at steps {cuts:?} (30 steps a second), {w}x{h}\n");
    let mut k = 0;
    for (at, name) in want {
        while k < at {
            s.step_bound(DT);
            k += 1;
        }
        if s.map_name != "00_Intro" {
            break;
        }
        let img = s.render_vk(&mut r, w, h).expect("vulkan render");
        std::fs::write(dir.join(format!("{name}.png")), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
        let c = s.camera();
        log += &format!("{name}: step {k} t={:.2}s pos {:?} pitch {} yaw {} roll {} fov {}\n", k as f32 * DT, c.pos, c.pitch, c.yaw, c.roll, c.fov);
    }
    std::fs::write(dir.join("intro_shots.txt"), &log).unwrap();
    eprint!("{log}");
    if let Some(r) = r.take() {
        r.destroy();
    }
}

/// The eyes blink (`Pawn.LipSynch` plays `Blink` on blend channel 1 during a conversation): `PlayBlendAnim` tweens to
/// the closed pose and plays back to the open one, where the channel stops (`BlendAnimFrame` = `BlendAnimLast`). The
/// eyes are open most of the time and do blink (WORKLOG 3.186).
#[test]
fn intro_eyes_blink_and_stay_open() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 00_Intro");
    let blink = s.vm.reg.names.get("Blink").expect("Blink name");
    let (mut open, mut closed, mut seen) = (0, 0, std::collections::BTreeSet::new());
    for _ in 0..900 {
        s.step_bound(DT);
        for a in s.vm.live_actors() {
            if s.vm.get_prop(a, "BlendAnimSequence", 1).ok() != Some(Value::Name(blink)) {
                continue;
            }
            seen.insert(a);
            let f = s.vm.get_prop(a, "BlendAnimFrame", 1).unwrap().as_f32().unwrap();
            let last = s.vm.get_prop(a, "BlendAnimLast", 1).unwrap().as_f32().unwrap();
            assert!((last - 0.5).abs() < 1e-6, "Blink is two frames: last {last}");
            if f >= last {
                open += 1;
            } else {
                closed += 1;
            }
        }
    }
    eprintln!("{} blinking actors, open {open}, blinking {closed}", seen.len());
    assert!(!seen.is_empty());
    assert!(closed > 0, "nobody blinked");
    assert!(open > closed * 4, "eyes open most of the time: open {open}, blinking {closed}");
}

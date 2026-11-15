//! Viewport windows (3D views inside the window system). Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
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

/// A viewport window of 200x150 at (20, 20) looking from `pos` with `yaw`, child of the root window.
fn add_viewport(s: &mut Session, pos: [f32; 3], yaw: i32) -> u32 {
    let root = s.vm.ui_root(s.player);
    let c = s.vm.reg.class_by_name("ViewportWindow").unwrap();
    let path = s.vm.reg.class(c).path.clone();
    let cls = Value::Obj(s.vm.reg.obj_id(&path).0);
    let win = match s.vm.call_by_name(root, "NewChild", vec![cls, Value::Bool(true)]) {
        Ok(Value::Obj(w)) if w != 0 => w,
        other => panic!("NewChild: {other:?}"),
    };
    s.vm.call_by_name(win, "SetPos", vec![Value::Float(20.0), Value::Float(20.0)]).unwrap();
    s.vm.call_by_name(win, "SetSize", vec![Value::Float(200.0), Value::Float(150.0)]).unwrap();
    s.vm.call_by_name(win, "SetViewportLocation", vec![Value::Vector(pos), Value::Bool(true)]).unwrap();
    s.vm.call_by_name(win, "SetRotation", vec![Value::Rotator([0, yaw, 0])]).unwrap();
    win
}

#[test]
fn a_viewport_window_hands_its_camera_to_the_renderer() {
    let Some(mut s) = new_game() else { return };
    let p = s.player_location();
    let win = add_viewport(&mut s, [p[0] + 100.0, p[1], p[2] + 50.0], 32768);
    s.vm.call_by_name(win, "SetFOVAngle", vec![Value::Float(70.0)]).unwrap();
    let quads = s.ui_quads(800, 600);
    assert!(!quads.is_empty());
    let views = s.ui_subviews();
    assert_eq!(views.len(), 1, "no 3D view for the viewport window");
    let v = &views[0];
    assert_eq!(v.cam.pos, [p[0] + 100.0, p[1], p[2] + 50.0]);
    assert_eq!((v.cam.yaw, v.cam.fov), (32768, 70.0));
    assert_eq!(v.rect, [20.0, 20.0, 200.0, 150.0]);
    // a disabled viewport shows its default texture instead
    s.vm.call_by_name(win, "EnableViewport", vec![Value::Bool(false)]).unwrap();
    s.ui_quads(800, 600);
    assert!(s.ui_subviews().is_empty());
}

#[test]
fn a_viewport_follows_an_actor_and_survives_its_destruction() {
    let Some(mut s) = new_game() else { return };
    let p = s.player_location();
    let win = add_viewport(&mut s, p, 0);
    // look out of a spawned actor, then destroy it: the view stays where it was
    let c = s.vm.reg.class_by_name("Flare").or_else(|| s.vm.reg.class_by_name("Spark")).unwrap();
    let path = s.vm.reg.class(c).path.clone();
    let cls = Value::Obj(s.vm.reg.obj_id(&path).0);
    let actor = match s.vm.call_by_name(s.player, "Spawn", vec![cls, Value::Void, Value::Void, Value::Vector([p[0] + 200.0, p[1], p[2] + 30.0])]) {
        Ok(Value::Obj(a)) if a != 0 => a,
        other => panic!("spawn: {other:?}"),
    };
    s.vm.call_by_name(win, "SetViewportActor", vec![Value::Obj(actor), Value::Bool(false), Value::Bool(true)]).unwrap();
    s.ui_quads(800, 600);
    let v = s.ui_subviews();
    assert_eq!(v[0].cam.pos, s.vm.location(actor));
    let at = s.vm.location(actor);
    s.vm.call_by_name(actor, "Destroy", vec![]).unwrap();
    s.ui_quads(800, 600);
    let v = s.ui_subviews();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].cam.pos, at, "the view jumped when its actor was destroyed");
}

fn shot(s: &mut Session, r: &mut Option<dx_vk::renderer::Renderer>, name: &str) {
    // the window system paints once, then a step runs the views' own visibility passes, which the picture draws
    s.render_vk(r, 800, 600).expect("vulkan render");
    s.step_bound(DT);
    let img = s.render_vk(r, 800, 600).expect("vulkan render");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
}

/// A picture of a viewport window that looks back at the player's surroundings; needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_a_viewport_window() {
    let Some(mut s) = new_game() else { return };
    let cam = s.camera();
    let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
    let (sy, cy) = ang(cam.yaw).sin_cos();
    // the same eye and direction as the player's own view: the inset must show the same picture, smaller
    let win = add_viewport(&mut s, cam.pos, cam.yaw);
    s.vm.call_by_name(win, "SetRotation", vec![Value::Rotator([cam.pitch, cam.yaw, 0])]).unwrap();
    s.vm.call_by_name(win, "SetFOVAngle", vec![Value::Float(cam.fov)]).unwrap();
    let _ = (ang, sy, cy);
    let mut r = None;
    for _ in 0..3 {
        s.step_bound(DT);
    }
    shot(&mut s, &mut r, "s10_viewport.png");
    r.take().unwrap().destroy();
}

fn run(s: &mut Session, n: usize) {
    for _ in 0..n {
        s.step_bound(DT);
    }
}

/// Opens the security computer of a map the way the player frobbing it does.
fn open_security_screen(s: &mut Session) -> Option<u32> {
    let c = s.vm.reg.class_by_name("ComputerSecurity")?;
    let comp = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false))?;
    let p = s.player;
    s.vm.call_by_name(p, "InvokeComputerScreen", vec![Value::Obj(comp), Value::Float(0.0), Value::Float(0.0)]).ok()?;
    // skip the login: go straight to the camera screen
    let term = match s.vm.get_prop(comp, "termwindow", 0) { Ok(Value::Obj(w)) if w != 0 => w, _ => return None };
    let c = s.vm.reg.class_by_name("ComputerScreenSecurity")?;
    let path = s.vm.reg.class(c).path.clone();
    let cls = Value::Obj(s.vm.reg.obj_id(&path).0);
    s.vm.call_by_name(term, "ShowScreen", vec![cls]).ok()?;
    Some(comp)
}

#[test]
fn the_security_computer_shows_camera_views() {
    let Some(mut s) = new_game() else { return };
    s.vm.log.clear();
    let comp = open_security_screen(&mut s).expect("no security computer on the map");
    run(&mut s, 5);
    let quads = s.ui_quads(800, 600);
    let views = s.ui_subviews();
    eprintln!("computer {comp}: {} quads, {} views", quads.len(), views.len());
    for v in &views {
        eprintln!("view rect {:?} cam {:?}", v.rect, v.cam.pos);
    }
    // one camera is wired up on the island: one live view, the other two feeds show static
    assert_eq!(views.len(), 1, "the security camera view is missing");
    assert_eq!(views[0].rect[2..], [200.0, 150.0]);
    let failed: Vec<_> = s.vm.log.iter().filter(|l| l.contains("window event")).collect();
    assert!(failed.is_empty(), "failed window events: {failed:?}");
    // the view is drawn with the full renderer (`XViewportWindow::RenderFrame` -> `DrawWorld`): its own visibility
    // pass decides what it shows and stamps it, so actors seen only on the screen count as rendered
    run(&mut s, 1);
    let seen = s.subview_drawn(0);
    assert!(!seen.is_empty(), "the camera view draws nothing");
    let now = s.vm.fp(s.vm.level, "TimeSeconds");
    for &a in &seen {
        assert_eq!(s.vm.fp(a, "LastRenderTime"), now);
    }
    let main = s.main_drawn();
    assert!(seen.iter().any(|a| !main.contains(a)), "nothing seen only through the camera");
}

/// A picture of the security computer's screen; needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_the_security_computer() {
    let Some(mut s) = new_game() else { return };
    open_security_screen(&mut s).expect("no security computer");
    run(&mut s, 5);
    let mut r = None;
    shot(&mut s, &mut r, "s10_security.png");
    r.take().unwrap().destroy();
}

/// `DrawFrame` runs its corona pass for every master frame, so a viewport window's view draws coronas too: looking at
/// Liberty Island's dock lamp from the player's eye, the view gets the lamp's corona tile inside its rectangle,
/// `view width * DrawScale * 0.2` wide (the frame's own width).
#[test]
fn a_viewport_window_draws_its_coronas() {
    let Some(mut s) = new_game() else { return };
    for _ in 0..25 {
        s.step_bound(DT);
    }
    let (l, _) = *s.coronas().first().expect("the dock lamp's corona");
    let lp = s.vm.location(l);
    let eye = s.camera().pos;
    let yaw = ((lp[1] - eye[1]).atan2(lp[0] - eye[0]).to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
    add_viewport(&mut s, eye, yaw);
    s.ui_quads(800, 600);
    run(&mut s, 3);
    s.ui_quads(800, 600);
    let views = s.ui_subviews();
    assert_eq!(views.len(), 1);
    let v = &views[0];
    let q = v.coronas.iter().find(|q| q.texture.contains("Corona")).expect("a corona tile in the view");
    let size = v.rect[2] * s.vm.fp(l, "DrawScale") * 0.2;
    assert!((q.rect[2] - size).abs() < 1e-3, "{} vs {size}", q.rect[2]);
    let centre = [q.rect[0] + q.rect[2] / 2.0, q.rect[1] + q.rect[3] / 2.0];
    assert!((centre[0] - (v.rect[0] + v.rect[2] / 2.0)).abs() < 10.0, "the lamp is ahead: {centre:?} in {:?}", v.rect);
}

/// Picture of the dock lamp's corona inside a viewport window (`../out/screenshots/viewport_corona.png`), facing away
/// from the lamp so the main view has none; needs a Vulkan device.
#[test]
#[ignore]
fn shot_of_a_corona_in_a_viewport_window() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 25);
    let (l, _) = *s.coronas().first().expect("the dock lamp's corona");
    let lp = s.vm.location(l);
    let eye = s.camera().pos;
    let yaw = ((lp[1] - eye[1]).atan2(lp[0] - eye[0]).to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
    add_viewport(&mut s, eye, yaw);
    let p = s.player_location();
    s.teleport(p, (yaw as f32 / 65536.0 * 360.0) + 180.0);
    s.ui_quads(800, 600);
    run(&mut s, 3);
    let mut r = None;
    shot(&mut s, &mut r, "viewport_corona.png");
    r.take().unwrap().destroy();
}

/// A viewport window's view is a master frame of its own (`XViewportWindow::Draw` -> `DrawWorld` -> `DrawFrame`): its
/// movers come from the dynamic nodes its own pass drew, like the main view's. A view 160 units in front of an island
/// door the main view does not see: its lists hold the door's nodes, the renderer's mover fragments include them, and
/// the main frame's do not.
#[test]
fn a_viewport_window_draws_movers_from_its_own_pass() {
    let Some(mut s) = new_game() else { return };
    let mover = s.vm.reg.class_by_name("DeusExMover").unwrap();
    let doors: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, mover)).unwrap_or(false)).collect();
    let mut checked = 0;
    for d in doors {
        let Some((model, nodes, _)) = s.tracked_model(d) else { continue };
        let Some(&n) = nodes.iter().max_by_key(|&&n| model.nodes[n].num_vertices) else { continue };
        let nd = model.nodes[n].clone();
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
        let yaw = ((-nrm[1]).atan2(-nrm[0]) / std::f32::consts::TAU * 65536.0) as i32 & 0xffff;
        let win = add_viewport(&mut s, eye, yaw);
        s.ui_quads(800, 600);
        s.step_bound(DT);
        s.ui_quads(800, 600);
        let views = s.ui_subviews();
        let door_nodes: Vec<u32> = s.tracked_model(d).map(|(_, n, _)| n.into_iter().map(|x| x as u32).collect()).unwrap_or_default();
        let lists = views.first().and_then(|v| v.lists.clone());
        let listed = s.listed_movers().unwrap();
        s.vm.call_by_name(win, "Destroy", vec![]).unwrap();
        let Some(lists) = lists else { continue };
        let seen: Vec<u32> = lists.dynamic.iter().copied().filter(|n| door_nodes.contains(n)).collect();
        if seen.is_empty() || seen.iter().any(|n| listed.main.contains(n)) {
            continue;
        }
        for n in &seen {
            assert!(listed.nodes.contains_key(n), "the view's node {n} has no fragment");
        }
        println!("door {d}: the view drew {} of its nodes, the main view none", seen.len());
        checked += 1;
        if checked >= 2 {
            break;
        }
    }
    assert!(checked > 0, "no door seen by a viewport window alone");
}

/// `ConvertVectorToCoordinates`: the root window projects through the main view's master frame (`rootFrame`), a
/// viewport window through the frame it last drew, any other window through its parent minus its own position.
/// Points 400 units ahead and 100.5 right, 50.5 down land at `FX2 + 100.5 Proj.Z / 400` (truncated, through the root's
/// multiplier and render offset); behind the eye the root answers its frame's centre and false.
#[test]
fn windows_project_world_points_through_their_master_frames() {
    let Some(mut s) = new_game() else { return };
    s.ui_quads(800, 600);
    let cam = s.render_camera();
    let at = |c: &dx_soft::Camera, f: f32, r: f32, d: f32| -> [f32; 3] {
        let (fw, rt, u) = c.axes();
        std::array::from_fn(|k| c.pos[k] + fw[k] * f + rt[k] * r - u[k] * d)
    };
    let root = s.vm.ui_root(s.player);
    let hm = s.vm.get_prop(root, "hMultiplier", 0).unwrap().as_i32().unwrap() as f64;
    let vm_ = s.vm.get_prop(root, "vMultiplier", 0).unwrap().as_i32().unwrap() as f64;
    let rx = s.vm.get_prop(root, "renderX", 0).unwrap().as_f32().unwrap() as f64;
    let ry = s.vm.get_prop(root, "renderY", 0).unwrap().as_f32().unwrap() as f64;
    // the main view: 800 x 600 at the camera's field of view (4:3, so no widening)
    let proj = 400.0 / ((cam.fov as f64).to_radians() * 0.5).tan();
    let (ok, x, y) = s.vm.ui_convert_vector(root, at(&cam, 400.0, 100.5, 50.5));
    assert!(ok);
    assert_eq!((x, y), ((((400.0 + 100.5 * proj / 400.0) / hm + rx) as i32) as f32, (((300.0 + 50.5 * proj / 400.0) / vm_ + ry) as i32) as f32));
    assert_eq!(s.vm.ui_convert_vector(root, at(&cam, -100.0, 0.0, 0.0)), (false, 400.0, 300.0));
    // a viewport window (200 x 150 at 20, 20) projects through its own frame, in its own coordinates
    let p = s.player_location();
    let eye = [p[0] + 50.0, p[1] - 30.0, p[2] + 40.0];
    let win = add_viewport(&mut s, eye, 16384);
    s.vm.call_by_name(win, "SetFOVAngle", vec![Value::Float(60.0)]).unwrap();
    s.ui_quads(800, 600);
    let vc = dx_soft::Camera { pos: eye, pitch: 0, yaw: 16384, fov: 60.0, roll: 0 };
    let vproj = 100.0 / (30.0f64).to_radians().tan();
    let (ok, x, y) = s.vm.ui_convert_vector(win, at(&vc, 400.0, 20.5, -10.5));
    assert!(ok);
    assert_eq!((x, y), (((100.0 + 20.5 * vproj / 400.0) as i32) as f32, ((75.0 - 10.5 * vproj / 400.0) as i32) as f32));
    // a child of the root: the root's answer minus the child's position
    let (_, rx2, ry2) = s.vm.ui_convert_vector(root, at(&cam, 400.0, 100.5, 50.5));
    let c = s.vm.reg.class_by_name("Window").unwrap();
    let path = s.vm.reg.class(c).path.clone();
    let cls = Value::Obj(s.vm.reg.obj_id(&path).0);
    let child = match s.vm.call_by_name(root, "NewChild", vec![cls, Value::Bool(true)]) {
        Ok(Value::Obj(w)) if w != 0 => w,
        other => panic!("NewChild: {other:?}"),
    };
    s.vm.call_by_name(child, "SetPos", vec![Value::Float(30.0), Value::Float(40.0)]).unwrap();
    let (ok, cx, cy) = s.vm.ui_convert_vector(child, at(&cam, 400.0, 100.5, 50.5));
    assert!(ok);
    assert_eq!((cx, cy), (rx2 - 30.0, ry2 - 40.0));
}

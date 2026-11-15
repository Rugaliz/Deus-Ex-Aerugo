//! Player input through the key bindings, the way the window path drives it. Set `DEUSEX_DIR` to the game folder.

use dx_input::key_code;
use dx_play::Session;
use std::path::PathBuf;

const DT: f32 = 1.0 / 60.0;

fn open(map: &str) -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    Some(Session::open(&g, map).unwrap())
}

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

#[test]
fn wasd_walks_in_the_view_direction_and_shift_slows_down() {
    let Some(mut s) = open("00_Training") else { return };
    run(&mut s, 60);
    let w = key_code("W").unwrap();
    let start = s.player_location();
    s.input.key(w, true);
    run(&mut s, 60);
    let mid = s.player_location();
    run(&mut s, 30);
    let end = s.player_location();
    let speed = dist(mid, end) / 0.5;
    assert!(speed > 280.0 && speed < 340.0, "running speed {speed}");
    let yaw = s.yaw as f32 * std::f32::consts::TAU / 65536.0;
    let along = (end[0] - start[0]) * yaw.cos() + (end[1] - start[1]) * yaw.sin();
    assert!(along > 0.9 * dist(start, end), "not moving along the view: {along} of {}", dist(start, end));
    // hold shift: walking pace (0.3 of ground speed)
    s.input.key(key_code("LShift").unwrap(), true);
    run(&mut s, 30);
    let a = s.player_location();
    run(&mut s, 30);
    let walk = dist(a, s.player_location()) / 0.5;
    assert!(walk > 70.0 && walk < 120.0, "walking speed {walk}");
    s.input.key(w, false);
    s.input.key(key_code("LShift").unwrap(), false);
    run(&mut s, 60);
    let b = s.player_location();
    run(&mut s, 10);
    assert!(dist(b, s.player_location()) < 2.0, "still sliding after the key was released");
}

#[test]
fn mouse_turns_right_for_positive_x_and_looks_up_for_positive_y() {
    let Some(mut s) = open("00_Training") else { return };
    run(&mut s, 30);
    let (yaw0, pitch0) = (s.yaw, s.pitch);
    for _ in 0..30 {
        s.input.axis(key_code("MouseX").unwrap(), 4.0);
        s.input.axis(key_code("MouseY").unwrap(), 2.0);
        s.step_bound(DT);
    }
    let dyaw = ((s.yaw - yaw0 + 32768) & 0xFFFF) - 32768;
    assert!(dyaw > 0, "yaw {dyaw}");
    assert!(s.pitch - pitch0 > 0, "pitch {}", s.pitch - pitch0);
    // with the shipped sensitivity a few hundred counts are a turn of tens of degrees, not a spin or a twitch
    let degrees = dyaw as f32 * 360.0 / 65536.0;
    assert!(degrees > 5.0 && degrees < 200.0, "{degrees} degrees for 120 counts");
}

#[test]
fn jump_and_belt_commands_reach_the_player_scripts() {
    let Some(mut s) = open("00_Training") else { return };
    run(&mut s, 60);
    let floor = s.player_location()[2];
    let space = key_code("Space").unwrap();
    s.input.key(space, true);
    run(&mut s, 1);
    s.input.key(space, false);
    let mut top = floor;
    for _ in 0..90 {
        run(&mut s, 1);
        top = top.max(s.player_location()[2]);
    }
    assert!(top - floor > 30.0, "no jump: {}", top - floor);
    // exec commands with arguments find their functions; unknown ones are reported, not fatal
    s.input.key(key_code("4").unwrap(), true);
    s.input.key(key_code("4").unwrap(), false);
    run(&mut s, 2);
    let bad: Vec<_> = s.issues.iter().filter(|i| i.class == "input").map(|i| i.error.clone()).collect();
    assert!(bad.is_empty(), "{bad:?}");
}

/// The interact key reaches the game's own use logic: standing in front of a door and pressing F sets the door moving.
#[test]
fn interact_opens_the_door_in_front() {
    let Some(mut s) = open("00_Training") else { return };
    run(&mut s, 30);
    let door_class = s.vm.reg.class_by_name("DeusExMover").unwrap();
    // the sliding doors at the end of the first corridor
    let doors: Vec<u32> = s
        .vm
        .live_actors()
        .into_iter()
        .filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, door_class)).unwrap_or(false) && (s.vm.location(a)[0] + 220.0).abs() < 5.0)
        .collect();
    assert!(!doors.is_empty(), "no doors at the corridor end");
    let before: Vec<_> = doors.iter().map(|&d| s.vm.location(d)).collect();
    s.teleport([-290.0, 832.0, -50.0], 0.0);
    run(&mut s, 60);
    let f = key_code("F").unwrap();
    // the player's own highlight logic (a line trace against the doors' brushes) picks the door
    run(&mut s, 10);
    let target = match s.vm.get_prop(s.player, "FrobTarget", 0).unwrap() {
        ue_vm::Value::Obj(t) => t,
        v => panic!("{v:?}"),
    };
    assert!(doors.contains(&target), "frob target {target:#x} is not one of the doors");
    s.input.key(f, true);
    run(&mut s, 2);
    s.input.key(f, false);
    run(&mut s, 120);
    let moved = doors.iter().zip(&before).any(|(&d, b)| dist(s.vm.location(d), *b) > 10.0 || (s.vm.location(d)[2] - b[2]).abs() > 10.0);
    let bad: Vec<_> = s.issues.iter().filter(|i| i.class == "input").map(|i| i.error.clone()).collect();
    assert!(moved, "the door did not open; input problems: {bad:?}");
}

/// How far the view turns per unit of pending mouse axis, at two step lengths: the renderer shows the not-yet-simulated
/// mouse movement with this factor (`clock::LOOK_PER_MOUSE`), so it must not depend on the step.
#[test]
fn mouse_look_per_axis_unit_does_not_depend_on_the_step() {
    let Some(mut s) = open("00_Training") else { return };
    run(&mut s, 30);
    let mx = key_code("MouseX").unwrap();
    let my = key_code("MouseY").unwrap();
    let mut res = Vec::new();
    for dt in [1.0 / 30.0f32, 1.0 / 60.0, 1.0 / 120.0] {
        for _ in 0..3 {
            s.step_bound(dt);
        }
        let before = (s.yaw, s.pitch);
        s.input.axis(mx, 10.0);
        s.input.axis(my, 5.0);
        let (px, py) = (s.input.pending_axis("aMouseX"), s.input.pending_axis("aMouseY"));
        s.step_bound(dt);
        let dyaw = ((s.yaw - before.0 + 32768).rem_euclid(65536)) - 32768;
        let dpitch = s.pitch - before.1;
        eprintln!("dt {dt:.4}: pending {px} {py} -> yaw {dyaw} pitch {dpitch}: per unit {:.2} {:.2}", dyaw as f32 / px, dpitch as f32 / py);
        res.push((dyaw as f32 / px, dpitch as f32 / py));
    }
    let k = s.look_per_mouse();
    for r in &res {
        assert!((r.0 - res[0].0).abs() <= res[0].0.abs() * 0.05 + 1.0, "yaw per unit changes with the step: {res:?}");
        assert!((r.0 - k[0]).abs() <= k[0].abs() * 0.05 + 1.0 && (r.1 - k[1]).abs() <= k[1].abs() * 0.05 + 1.0, "look_per_mouse {k:?} differs from the measured {r:?}");
    }
}

#[test]
#[ignore]
fn print_player_fov() {
    let Some(mut s) = open("00_Training") else { return };
    run(&mut s, 30);
    for n in ["FovAngle", "DesiredFOV", "DefaultFOV"] {
        eprintln!("{n} = {:?}", s.vm.get_prop(s.player, n, 0));
    }
}

/// The game behaves the same at any frame rate: running and jumping for two seconds through `advance` (fixed simulation
/// steps) ends at the same place at 30, 144 and 240 fps, and at 240 fps the drawn camera moves smoothly (no frame stands
/// still or jumps ahead while the player runs).
#[test]
fn the_game_behaves_the_same_at_any_frame_rate() {
    let mut ends = Vec::new();
    for fps in [30.0f32, 144.0, 240.0] {
        let Some(mut s) = open("00_Training") else { return };
        run(&mut s, 30);
        let w = key_code("W").unwrap();
        let space = key_code("Space").unwrap();
        s.input.key(w, true);
        // two seconds of simulation (120 steps at 60 Hz), the jump pressed at step 30, whatever the frame rate
        let total = (2.0 / s.clock.step).round() as usize;
        let mut steps = 0;
        let mut jumped = false;
        let mut cams: Vec<[f32; 3]> = Vec::new();
        while steps < total {
            if !jumped && steps >= total / 4 {
                s.input.key(space, true);
                jumped = true;
            } else if jumped {
                s.input.key(space, false);
            }
            // never step past the total: the last frame is cut to what is left
            let left = (total - steps) as f32 * s.clock.step;
            steps += s.advance((1.0 / fps).min(left + 1e-6));
            cams.push(s.render_camera().pos);
        }
        let frames = cams.len();
        s.input.key(w, false);
        let end = s.player_location();
        eprintln!("{fps} fps: end {end:?}");
        ends.push(end);
        if fps == 240.0 {
            // horizontal movement per drawn frame while running at full speed (the second half: past the jump)
            let steps: Vec<f32> = cams.windows(2).skip(frames / 2).map(|p| ((p[1][0] - p[0][0]).powi(2) + (p[1][1] - p[0][1]).powi(2)).sqrt()).collect();
            let mean = steps.iter().sum::<f32>() / steps.len() as f32;
            let (min, max) = steps.iter().fold((f32::MAX, 0.0f32), |(a, b), &x| (a.min(x), b.max(x)));
            eprintln!("240 fps camera step: mean {mean:.2} min {min:.2} max {max:.2} units per frame");
            assert!(mean > 0.5, "not moving");
            assert!(min > mean * 0.5 && max < mean * 1.5, "uneven motion at 240 fps: min {min} mean {mean} max {max}");
        }
    }
    for e in &ends[1..] {
        let d = ((e[0] - ends[0][0]).powi(2) + (e[1] - ends[0][1]).powi(2) + (e[2] - ends[0][2]).powi(2)).sqrt();
        assert!(d < 1.0, "the frame rate changed where the player ended: {ends:?}");
    }
}

//! Per-actor physics on a real map: falling, landing, walking. Set `DEUSEX_DIR` to the game folder.
//!
//! The expected values do not come from the physics code: the rest height is found by stepping the actor with
//! `move_actor` alone, and the landing time is the closed-form fall time under the zone's gravity.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::collision::CheckResult;
use ue_vm::{Value, Vm};

fn world(map: &str) -> Option<(Vm, ue_vm::level::LevelLoad)> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut vm = Vm::new(Registry::load_game(&g).unwrap());
    vm.init_world().unwrap();
    let pi = vm.reg.add_package(&g.join("Maps").join(format!("{map}.dx")), map).unwrap();
    let l = vm.load_level(pi).unwrap();
    Some((vm, l))
}

/// Where a designer-placed pawn stands: a spot known to be free standing room.
fn standing_spot(vm: &mut Vm, l: &ue_vm::level::LevelLoad) -> [f32; 3] {
    let pawn = vm.reg.class_by_name("Pawn").unwrap();
    let player = vm.reg.class_by_name("PlayerPawn").unwrap();
    let a = l
        .actors
        .iter()
        .copied()
        .find(|&a| vm.class_of(a).map(|c| vm.is_a(c, pawn) && !vm.is_a(c, player)).unwrap_or(false))
        .expect("a pawn in the map");
    vm.location(a)
}

fn make(vm: &mut Vm, class: &str, at: [f32; 3]) -> u32 {
    let c = vm.reg.class_by_name(class).unwrap();
    let a = vm.spawn(c, 0, None, Some(at), None).unwrap().unwrap();
    vm.set_prop(a, "bMovable", 0, Value::Bool(true)).unwrap();
    vm.set_prop(a, "bStatic", 0, Value::Bool(false)).unwrap();
    vm.set_prop(a, "CollisionRadius", 0, Value::Float(17.0)).unwrap();
    vm.set_prop(a, "CollisionHeight", 0, Value::Float(39.0)).unwrap();
    vm.set_prop(a, "bCollideWorld", 0, Value::Bool(true)).unwrap();
    a
}

fn phys(vm: &mut Vm, a: u32) -> i32 {
    vm.get_prop(a, "Physics", 0).unwrap().as_i32().unwrap()
}

fn velocity(vm: &mut Vm, a: u32) -> [f32; 3] {
    match vm.get_prop(a, "Velocity", 0).unwrap() {
        Value::Vector(v) => v,
        o => panic!("{o:?}"),
    }
}

/// Height at which a box dropped at `at` comes to rest, by stepping it down with plain moves.
fn rest_height(vm: &mut Vm, at: [f32; 3]) -> f32 {
    let t = make(vm, "Actor", at);
    for _ in 0..200 {
        let mut h = CheckResult::none();
        vm.move_actor(t, [0.0, 0.0, -5.0], [0, 0, 0], &mut h, false, false, false, false).unwrap();
        if h.time < 1.0 {
            break;
        }
    }
    let z = vm.location(t)[2];
    vm.destroy(t).unwrap();
    z
}

/// A plain actor dropped from a height falls with the zone's gravity, lands on the floor and stops there. The
/// landing frame is compared with the closed-form fall time for the distance it actually fell.
#[test]
fn falling_actor_lands_when_gravity_says_it_should() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let s = standing_spot(&mut vm, &l);
    let start = [s[0], s[1], s[2] + 40.0];
    let floor_z = rest_height(&mut vm, start);
    let a = make(&mut vm, "Actor", start);
    vm.set_prop(a, "Physics", 0, Value::Byte(2)).unwrap();
    let g = match vm.get_prop(vm.level, "ZoneGravity", 0).unwrap() {
        Value::Vector(v) => -v[2],
        o => panic!("{o:?}"),
    };
    let dt = 1.0 / 30.0;
    let mut landed_at = None;
    for frame in 0..90 {
        vm.perform_physics(a, dt).unwrap();
        if phys(&mut vm, a) != 2 {
            landed_at = Some(frame + 1);
            break;
        }
    }
    let frames = landed_at.expect("the actor never landed");
    let rest = vm.location(a)[2];
    assert!((rest - floor_z).abs() < 6.0, "came to rest at {rest}, stepping says {floor_z}");
    let fell = start[2] - rest;
    let predicted = (2.0 * fell / g).sqrt() / dt;
    assert!((frames as f32 - predicted).abs() <= 3.0, "landed after {frames} frames, closed form says {predicted} (fell {fell}, g {g})");
    assert_eq!(phys(&mut vm, a), 0, "a non-pawn comes to rest with PHYS_None");
    assert_eq!(velocity(&mut vm, a), [0.0; 3]);
}

/// A pawn that lands starts walking; with acceleration it speeds up to `GroundSpeed` and no further, stays on the
/// floor, and friction stops it again once the acceleration is removed.
#[test]
fn walking_pawn_accelerates_caps_speed_and_brakes() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let s = standing_spot(&mut vm, &l);
    let a = make(&mut vm, "Pawn", [s[0], s[1], s[2] + 20.0]);
    vm.set_prop(a, "GroundSpeed", 0, Value::Float(300.0)).unwrap();
    vm.set_prop(a, "AccelRate", 0, Value::Float(2048.0)).unwrap();
    vm.set_prop(a, "DesiredSpeed", 0, Value::Float(1.0)).unwrap();
    vm.set_prop(a, "Physics", 0, Value::Byte(2)).unwrap();
    let dt = 1.0 / 30.0;
    for _ in 0..30 {
        vm.perform_physics(a, dt).unwrap();
    }
    assert_eq!(phys(&mut vm, a), 1, "the pawn lands and walks (log: {:?})", vm.log);
    let rest = vm.location(a);
    // walk in whichever compass direction has the most room
    let (mut dir, mut best) = ([1.0_f32, 0.0], -1.0);
    for k in 0..8 {
        let ang = k as f32 * std::f32::consts::FRAC_PI_4;
        let d = [ang.cos(), ang.sin()];
        let end = [rest[0] + d[0] * 200.0, rest[1] + d[1] * 200.0, rest[2]];
        let h = vm.single_line_check(a, end, rest, 4, [17.0, 17.0, 39.0], 0);
        if h.time > best {
            best = h.time;
            dir = d;
        }
    }
    let mut max_speed: f32 = 0.0;
    for _ in 0..10 {
        vm.set_prop(a, "Acceleration", 0, Value::Vector([dir[0] * 2048.0, dir[1] * 2048.0, 0.0])).unwrap();
        vm.perform_physics(a, dt).unwrap();
        let v = velocity(&mut vm, a);
        max_speed = max_speed.max((v[0] * v[0] + v[1] * v[1]).sqrt());
    }
    let moved = vm.location(a);
    let along = (moved[0] - rest[0]) * dir[0] + (moved[1] - rest[1]) * dir[1];
    assert!(max_speed <= 300.5 && max_speed > 100.0, "speed {max_speed} (GroundSpeed 300)");
    assert!(along > 20.0, "walked only {along}: {rest:?} -> {moved:?}");
    assert_eq!(phys(&mut vm, a), 1, "still walking");
    assert!((moved[2] - rest[2]).abs() < 6.0, "stayed on the floor: {rest:?} -> {moved:?}");
    // release the acceleration: friction brings it to a halt
    vm.set_prop(a, "Acceleration", 0, Value::Vector([0.0; 3])).unwrap();
    for _ in 0..30 {
        vm.perform_physics(a, dt).unwrap();
    }
    let v = velocity(&mut vm, a);
    assert!(v[0].abs() < 1.0 && v[1].abs() < 1.0, "still moving: {v:?}");
}

/// The whole training map ticks for two seconds; no script errors at all (the allow-list is empty on purpose; add to it only for
/// natives that are knowingly missing).
#[test]
fn training_map_ticks_with_only_known_missing_natives() {
    let Some((mut vm, _)) = world("00_Training") else { return };
    let mut issues = Vec::new();
    for _ in 0..60 {
        vm.tick_report(1.0 / 30.0, &mut issues);
    }
    let known: [&str; 0] = [];
    for i in &issues {
        assert!(known.iter().any(|k| i.error.contains(&format!("native function {k}"))), "{} {}: {}", i.class, i.event, i.error);
    }
}

/// `physRolling`: a decoration sliding along the floor loses speed to ground friction and stays supported (Rolling);
/// over an edge it turns into a falling body. Expected: the speed after a second is below the initial speed and
/// above zero for a modest friction, the floor height is unchanged, and an airborne Rolling actor falls.
#[test]
fn rolling_decoration_slides_and_falls_off_edges() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let s = standing_spot(&mut vm, &l);
    let start = [s[0], s[1], s[2] + 40.0];
    let floor_z = rest_height(&mut vm, start);
    // on the floor
    let d = make(&mut vm, "Actor", [s[0], s[1], floor_z]);
    vm.set_prop(d, "Physics", 0, Value::Byte(7)).unwrap();
    vm.set_prop(d, "Velocity", 0, Value::Vector([60.0, 0.0, 0.0])).unwrap();
    for _ in 0..30 {
        vm.perform_physics(d, 1.0 / 60.0).unwrap();
    }
    let v = velocity(&mut vm, d);
    let speed = (v[0] * v[0] + v[1] * v[1]).sqrt();
    let z = vm.location(d)[2];
    assert!(speed < 60.0, "friction did not slow the slide: {speed}");
    assert!((z - floor_z).abs() < 3.0, "left the floor: {z} vs {floor_z}");
    assert_eq!(phys(&mut vm, d), 7, "stopped rolling although it is on the floor");
    // in the air: the probe finds no ground and the actor becomes a falling body
    let air = make(&mut vm, "Actor", [s[0], s[1], floor_z + 300.0]);
    vm.set_prop(air, "Physics", 0, Value::Byte(7)).unwrap();
    vm.perform_physics(air, 1.0 / 60.0).unwrap();
    assert_eq!(phys(&mut vm, air), 2, "an airborne rolling actor must fall");
}

/// `ParabolicTrace`: a body dropped from a height lands after the closed-form fall time (within a step or two of the
/// engine's fixed 0.025 s integration) at the floor under it; thrown sideways it lands further along the same line.
#[test]
fn parabolic_trace_lands_where_the_fall_time_says() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let s = standing_spot(&mut vm, &l);
    let start = [s[0], s[1], s[2] + 40.0];
    let floor_z = rest_height(&mut vm, start);
    let g = match vm.get_prop(vm.level, "ZoneGravity", 0).unwrap() {
        Value::Vector(v) => v[2].abs(),
        o => panic!("{o:?}"),
    };
    let a = make(&mut vm, "Actor", start);
    let from = [s[0], s[1], floor_z + 120.0];
    let (t, end) = vm.parabolic_trace(a, from, [0.0; 3], 5.0, 0.9, false, 60.0, [2.0, 2.0, 2.0], false, 0.025).unwrap();
    // the rest height is the centre of a 39 high box: the floor is 39 below it, and the traced point stops 2 above it
    let fall = (2.0 * (120.0 + 39.0 - 2.0) / g).sqrt();
    assert!((t - fall).abs() < 0.08, "landed after {t} s, closed form {fall} s");
    assert!((end[2] - (floor_z - 39.0 + 2.0)).abs() < 6.0, "ended at z {} vs floor {}", end[2], floor_z - 37.0);
    assert!((end[0] - from[0]).abs() < 1.0 && (end[1] - from[1]).abs() < 1.0);
    // thrown along +X (if there is room): it comes down further along
    let (t2, end2) = vm.parabolic_trace(a, from, [100.0, 0.0, 0.0], 5.0, 0.9, false, 60.0, [2.0, 2.0, 2.0], false, 0.025).unwrap();
    if t2 > 0.0 {
        assert!(end2[0] > from[0] + 5.0, "thrown body did not travel: {end2:?}");
    }
    // a limited flight time that is shorter than the fall gives up (time 0)
    let (t3, end3) = vm.parabolic_trace(a, from, [0.0; 3], 0.1, 0.9, false, 60.0, [2.0, 2.0, 2.0], false, 0.025).unwrap();
    assert_eq!((t3, end3), (0.0, from));
}

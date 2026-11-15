//! Movers (doors) interpolating between key frames on a real map. Set `DEUSEX_DIR` to the game folder.
//! Expected positions come from the mover's own `BasePos`, `KeyPos` and `MoveTime`, not from the physics code.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::{Value, Vm};

fn vec_prop(vm: &Vm, a: u32, name: &str, i: u32) -> [f32; 3] {
    match vm.get_prop(a, name, i).unwrap() {
        Value::Vector(v) => v,
        v => panic!("{name}: {v:?}"),
    }
}

#[test]
fn a_door_slides_to_its_second_key_frame_and_back() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut vm = Vm::new(Registry::load_game(&g).unwrap());
    vm.init_world().unwrap();
    let pi = vm.reg.add_package(&g.join("Maps").join("00_Training.dx"), "00_Training").unwrap();
    let level = vm.load_level(pi).unwrap();
    let mover = vm.reg.class_by_name("Mover").unwrap();
    // a sliding door: key 1 is a different position than key 0
    let door = level
        .actors
        .iter()
        .copied()
        .find(|&a| {
            vm.class_of(a).map(|c| vm.is_a(c, mover)).unwrap_or(false) && {
                let k1 = vec_prop(&vm, a, "KeyPos", 1);
                k1.iter().any(|c| c.abs() > 1.0) && vm.get_prop(a, "MoveTime", 0).unwrap().as_f32().unwrap_or(0.0) > 0.1
            }
        })
        .expect("a sliding mover");
    let base = vec_prop(&vm, door, "BasePos", 0);
    let key1 = vec_prop(&vm, door, "KeyPos", 1);
    let move_time = vm.get_prop(door, "MoveTime", 0).unwrap().as_f32().unwrap();
    let glide = vm.get_prop(door, "MoverGlideType", 0).unwrap().as_i32().unwrap() == 1;
    let target = [base[0] + key1[0], base[1] + key1[1], base[2] + key1[2]];
    let start = vm.location(door);
    assert!((0..3).all(|k| (start[k] - (base[k] + vec_prop(&vm, door, "KeyPos", 0)[k])).abs() < 0.01), "starts at key 0: {start:?}");

    vm.call_by_name(door, "DoOpen", vec![]).unwrap();
    assert_eq!(vm.get_prop(door, "bInterpolating", 0).unwrap().as_i32(), Some(1));
    // a quarter of the way in time: 25% linear, or the smooth step 3a^2 - 2a^3 = 15.6% for gliding doors
    let dt = 1.0 / 60.0;
    let steps_quarter = (move_time * 0.25 / dt).round() as usize;
    for _ in 0..steps_quarter {
        vm.tick(dt).unwrap();
    }
    let a = steps_quarter as f32 * dt / move_time;
    let shaped = if glide { a * a * 3.0 - 2.0 * a * a * a } else { a };
    let at = vm.location(door);
    for k in 0..3 {
        let expect = start[k] + (target[k] - start[k]) * shaped;
        assert!((at[k] - expect).abs() < 1.5, "axis {k}: {} vs {expect} (alpha {a}, glide {glide})", at[k]);
    }
    // arrived
    for _ in 0..((move_time / dt) as usize + 30) {
        vm.tick(dt).unwrap();
    }
    let end = vm.location(door);
    assert!((0..3).all(|k| (end[k] - target[k]).abs() < 0.5), "arrived at {end:?}, wanted {target:?}");
    assert_eq!(vm.get_prop(door, "bInterpolating", 0).unwrap().as_i32(), Some(0));
    assert_eq!(vm.get_prop(door, "KeyNum", 0).unwrap().as_i32(), Some(1));

    // closing returns to key 0
    vm.call_by_name(door, "DoClose", vec![]).unwrap();
    for _ in 0..((move_time / dt) as usize + 30) {
        vm.tick(dt).unwrap();
    }
    let back = vm.location(door);
    assert!((0..3).all(|k| (back[k] - start[k]).abs() < 0.5), "closed again at {back:?}, wanted {start:?}");
}

/// Frobbing a door (what the use key does) makes it open: its script runs `DoOpen`, waits in `FinishInterpolation`, and the door
/// ends at its second key frame.
#[test]
fn frobbing_a_door_opens_it() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut vm = Vm::new(Registry::load_game(&g).unwrap());
    vm.init_world().unwrap();
    let pi = vm.reg.add_package(&g.join("Maps").join("00_Training.dx"), "00_Training").unwrap();
    let level = vm.load_level(pi).unwrap();
    let door_class = vm.reg.class_by_name("DeusExMover").unwrap();
    let door = level
        .actors
        .iter()
        .copied()
        .find(|&a| {
            vm.class_of(a).map(|c| vm.is_a(c, door_class)).unwrap_or(false) && vm.get_prop(a, "bFrobbable", 0).unwrap().as_i32() == Some(1) && vec_prop(&vm, a, "KeyPos", 1).iter().any(|c| c.abs() > 1.0)
        })
        .expect("a frobbable door");
    let start = vm.location(door);
    let pawn_class = vm.reg.class_by_name("Pawn").unwrap();
    let pawn = vm.spawn(pawn_class, 0, None, Some([start[0] - 80.0, start[1], start[2] + 40.0]), None).unwrap().unwrap();
    let t = vm.call_by_name(door, "Frob", vec![Value::Obj(pawn), Value::Obj(0)]);
    assert!(t.is_ok(), "{t:?}");
    for _ in 0..240 {
        vm.tick(1.0 / 60.0).unwrap();
    }
    let at = vm.location(door);
    let moved = (0..3).map(|k| (at[k] - start[k]).abs()).sum::<f32>();
    assert!(moved > 5.0, "the door did not move: {start:?} -> {at:?}, KeyNum {:?}", vm.get_prop(door, "KeyNum", 0));
}

/// `ULevel::CheckEncroachment`: a sliding door that closes/opens onto a pawn standing in its path shoves the pawn along
/// (it is a non-static brush and the pawn is not), instead of passing through it as if it were not there.
#[test]
fn a_moving_door_shoves_a_pawn_in_its_way() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut vm = Vm::new(Registry::load_game(&g).unwrap());
    vm.init_world().unwrap();
    let pi = vm.reg.add_package(&g.join("Maps").join("00_Training.dx"), "00_Training").unwrap();
    let level = vm.load_level(pi).unwrap();
    let mover = vm.reg.class_by_name("Mover").unwrap();
    let door = level
        .actors
        .iter()
        .copied()
        .find(|&a| {
            vm.class_of(a).map(|c| vm.is_a(c, mover)).unwrap_or(false)
                && vm.get_prop(a, "Rotation", 0).map(|r| matches!(r, Value::Rotator([0, 0, 0]))).unwrap_or(false)
                && vec_prop(&vm, a, "KeyPos", 1).iter().any(|c| c.abs() > 20.0)
                && vm.get_prop(a, "MoveTime", 0).unwrap().as_f32().unwrap_or(0.0) > 0.1
                && vm.brush_local_bounds(a).is_some()
        })
        .expect("an unrotated sliding mover");
    let base = vec_prop(&vm, door, "BasePos", 0);
    let key0 = vec_prop(&vm, door, "KeyPos", 0);
    let pre = vec_prop(&vm, door, "PrePivot", 0);
    let (lo, hi) = vm.brush_local_bounds(door).unwrap();
    let dt = 1.0 / 60.0;
    let move_time = vm.get_prop(door, "MoveTime", 0).unwrap().as_f32().unwrap();
    // open it fully first, then stand a pawn in the doorway, then close it onto the pawn
    vm.call_by_name(door, "DoOpen", vec![]).unwrap();
    for _ in 0..((move_time / dt) as usize + 30) {
        vm.tick(dt).unwrap();
    }
    let closed = [0, 1, 2].map(|k| base[k] + key0[k]);
    let mid = [0, 1, 2].map(|k| closed[k] + (lo[k] + hi[k]) * 0.5 - pre[k]);
    let pawn_class = vm.reg.class_by_name("Pawn").unwrap();
    let pawn = vm.spawn(pawn_class, 0, None, Some(mid), None).unwrap().unwrap();
    for (n, v) in [("CollisionRadius", 17.0), ("CollisionHeight", 39.0)] {
        vm.set_prop(pawn, n, 0, Value::Float(v)).unwrap();
    }
    for n in ["bCollideActors", "bBlockActors", "bBlockPlayers", "bCollideWorld"] {
        vm.set_prop(pawn, n, 0, Value::Bool(true)).unwrap();
    }
    vm.set_prop(pawn, "Physics", 0, Value::Byte(0)).unwrap(); // PHYS_None: stays where the door leaves it
    let before = vm.location(pawn);
    vm.call_by_name(door, "DoClose", vec![]).unwrap();
    for _ in 0..((move_time / dt) as usize + 30) {
        vm.tick(dt).unwrap();
    }
    let after = vm.location(pawn);
    let moved = (0..3).map(|k| (after[k] - before[k]).powi(2)).sum::<f32>().sqrt();
    let door_at = vm.location(door);
    let short = (0..3).map(|k| (door_at[k] - closed[k]).powi(2)).sum::<f32>().sqrt();
    assert!(moved > 1.0 || short > 1.0, "the door closed through the pawn (pawn moved {moved}, door {door_at:?}, closed {closed:?})");
}

/// Every mover of `maps`: its brush frame (`ABrush::ToWorld`) against an independent placement (`Location +
/// PostScale (R (MainScale (p - PrePivot)))` with R built from the rotator in double precision; sheered scales are
/// only counted), and for unit scales zero-extent traces through it against the same traces in brush space; also
/// `BuildCoords`' vector frame against its point frame (`(P u) . (V w) = u . w`, sheers included). Returns (movers,
/// rotated, scaled or sheered, worst frame error, trace disagreements, worst vector-frame error).
fn mover_frames(maps: &[String]) -> (usize, usize, usize, f32, usize, f32) {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR").unwrap());
    let (mut n, mut rotated, mut scaled, mut worst, mut disagree, mut vworst) = (0, 0, 0, 0.0f32, 0, 0.0f32);
    for map in maps {
        let mut vm = Vm::new(Registry::load_game(&g).unwrap());
        vm.init_world().unwrap();
        let pi = vm.reg.add_package(&g.join("Maps").join(format!("{map}.dx")), map).unwrap();
        let level = vm.load_level(pi).unwrap();
        let mover = vm.reg.class_by_name("Mover").unwrap();
        for &a in &level.actors {
            if !vm.class_of(a).map(|c| vm.is_a(c, mover)).unwrap_or(false) {
                continue;
            }
            let Some(bsp) = vm.mover_bsp(a) else { continue };
            n += 1;
            let loc = vm.location(a);
            let rot = match vm.get_prop(a, "Rotation", 0).unwrap() {
                Value::Rotator(r) => r,
                v => panic!("{v:?}"),
            };
            let pre = vec_prop(&vm, a, "PrePivot", 0);
            let unit = ["MainScale", "PostScale"].iter().all(|s| match vm.get_prop(a, s, 0).unwrap() {
                Value::Struct(_, b) => {
                    let f = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
                    [f(0), f(4), f(8)] == [1.0; 3] && (b[16] == 0 || f(12) == 0.0)
                }
                _ => true,
            });
            if !unit {
                scaled += 1;
                let sc = |vm: &mut Vm, n: &str| match vm.get_prop(a, n, 0).unwrap() {
                    Value::Struct(_, b) => (0..5).map(|i| if i < 4 { f32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]).to_string() } else { b[16].to_string() }).collect::<Vec<_>>().join(" "),
                    _ => String::new(),
                };
                println!("{map} mover {a}: scaled or sheered: main {} post {} rot {rot:?}", sc(&mut vm, "MainScale"), sc(&mut vm, "PostScale"));
            }
            let (point, vector, _) = vm.brush_coords(a);
            let dot = |a: [f32; 3], b: [f32; 3]| a[0] as f64 * b[0] as f64 + a[1] as f64 * b[1] as f64 + a[2] as f64 * b[2] as f64;
            let probes = [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.6, -0.48, 0.64]];
            for u in probes {
                for w in probes {
                    let e = (dot(point.transform_vector(u), vector.transform_vector(w)) - dot(u, w)).abs() as f32;
                    vworst = vworst.max(e);
                }
            }
            // the scales (sheer rate, sheer axis); a sheered one is only counted
            let scale = |vm: &mut Vm, n: &str| match vm.get_prop(a, n, 0).unwrap() {
                Value::Struct(_, b) => {
                    let f = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as f64;
                    ([f(0), f(4), f(8)], f(12), b[16])
                }
                _ => ([1.0; 3], 0.0, 0),
            };
            let (ms, ms_rate, ms_axis) = scale(&mut vm, "MainScale");
            let (ps, ps_rate, ps_axis) = scale(&mut vm, "PostScale");
            if (ms_axis != 0 && ms_rate != 0.0) || (ps_axis != 0 && ps_rate != 0.0) {
                continue;
            }
            if rot != [0; 3] {
                rotated += 1;
            }
            let c = vm.brush_to_world(a, loc, rot);
            let ang = |u: i32| (u & 0xffff) as f64 * std::f64::consts::TAU / 65536.0;
            let (sp, cp, sy, cy, sr, cr) = (ang(rot[0]).sin(), ang(rot[0]).cos(), ang(rot[1]).sin(), ang(rot[1]).cos(), ang(rot[2]).sin(), ang(rot[2]).cos());
            let ax = [[cp * cy, cp * sy, sp], [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp], [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp]];
            // `Location + PostScale (R (MainScale (p - PrePivot)))`
            let place = |p: [f32; 3]| -> [f32; 3] {
                let l: [f64; 3] = std::array::from_fn(|k| (p[k] - pre[k]) as f64 * ms[k]);
                std::array::from_fn(|k| (loc[k] as f64 + ps[k] * (ax[0][k] * l[0] + ax[1][k] * l[1] + ax[2][k] * l[2])) as f32)
            };
            let unplace = |w: [f32; 3]| -> [f32; 3] {
                let d = [(w[0] - loc[0]) as f64, (w[1] - loc[1]) as f64, (w[2] - loc[2]) as f64];
                std::array::from_fn(|k| (ax[k][0] * d[0] + ax[k][1] * d[1] + ax[k][2] * d[2]) as f32 + pre[k])
            };
            for k in [0.0f32, 64.0, -64.0] {
                let p = [pre[0] + k, pre[1] - k, pre[2] + k * 0.5];
                let (a1, b1) = (c.transform_point(p), place(p));
                worst = worst.max((0..3).map(|i| (a1[i] - b1[i]).abs()).fold(0.0, f32::max));
            }
            if !unit {
                continue;
            }
            // traces through the brush's pivot along the three world axes, both directions
            let centre = place(pre);
            for axis in 0..3 {
                for s in [-1.0f32, 1.0] {
                    let mut start = centre;
                    start[axis] += 300.0 * s;
                    let mut end = centre;
                    end[axis] -= 300.0 * s;
                    let new = bsp.line_check_owned(start, end, [0.0; 3], 0, Some(&c));
                    let old = bsp.line_check(unplace(start), unplace(end), [0.0; 3], 0);
                    match (new, old) {
                        (None, None) => {}
                        (Some(x), Some(y)) if (x.time - y.time).abs() < 1e-3 => {}
                        (x, y) => {
                            disagree += 1;
                            println!("{map} mover {a}: axis {axis} {s}: world {:?} brush {:?}; loc {loc:?} rot {rot:?} pre {pre:?} frame {c:?} start {start:?} local {:?}..{:?}", x.map(|h| (h.time, h.normal)), y.map(|h| (h.time, h.normal)), unplace(start), unplace(end));
                        }
                    }
                }
            }
        }
    }
    (n, rotated, scaled, worst, disagree, vworst)
}

#[test]
fn mover_frames_match_their_placement() {
    if std::env::var_os("DEUSEX_DIR").is_none() {
        return;
    }
    let maps: Vec<String> = std::env::var("MOVER_MAPS").map(|m| m.split(',').map(String::from).collect()).unwrap_or_else(|_| vec!["00_Training".into(), "01_NYC_UNATCOIsland".into()]);
    if maps.len() == 1 {
        mover_frames(&maps);
        return;
    }
    let (n, rotated, _, worst, disagree, vworst) = mover_frames(&maps);
    println!("{n} movers, {rotated} rotated, frame error {worst}, {disagree} trace disagreements, vector frame error {vworst}");
    assert!(n > 10 && rotated > 0, "{n} {rotated}");
    assert!(worst < 0.01, "{worst}");
    assert_eq!(disagree, 0);
    assert!(vworst < 1e-4, "{vworst}");
}

/// All maps (slow): `cargo test --release -p ue-vm --test movers all_mover_frames -- --ignored --nocapture`.
#[test]
#[ignore]
fn all_mover_frames() {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR").unwrap());
    let mut maps: Vec<String> = std::fs::read_dir(g.join("Maps"))
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".dx").map(String::from))
        .collect();
    maps.sort();
    let r = mover_frames(&maps);
    println!("{} maps: {} movers, {} rotated, {} scaled or sheered, frame error {}, {} trace disagreements, vector frame error {}", maps.len(), r.0, r.1, r.2, r.3, r.4, r.5);
}

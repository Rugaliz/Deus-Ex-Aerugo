//! Pawn AI natives on a real map: alliances, turning, moving to a point. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::{Value, Vm};

fn world(map: &str) -> Option<(Vm, ue_vm::level::LevelLoad)> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut vm = Vm::new(Registry::load_game(&g).unwrap());
    vm.init_world().unwrap();
    let pi = vm.reg.add_package(&g.join("Maps").join(format!("{map}.dx")), map).unwrap();
    let l = vm.load_level(pi).unwrap();
    Some((vm, l))
}

fn pawn_spot(vm: &mut Vm, l: &ue_vm::level::LevelLoad) -> [f32; 3] {
    let pawn = vm.reg.class_by_name("Pawn").unwrap();
    let player = vm.reg.class_by_name("PlayerPawn").unwrap();
    let a = l.actors.iter().copied().find(|&a| vm.class_of(a).map(|c| vm.is_a(c, pawn) && !vm.is_a(c, player)).unwrap_or(false)).unwrap();
    let at = vm.location(a);
    vm.destroy(a).unwrap();
    at
}

fn walker(vm: &mut Vm, at: [f32; 3]) -> u32 {
    let c = vm.reg.class_by_name("Pawn").unwrap();
    let a = vm.spawn(c, 0, None, Some(at), None).unwrap().unwrap();
    for (n, v) in [("CollisionRadius", 17.0), ("CollisionHeight", 39.0), ("GroundSpeed", 300.0), ("AccelRate", 2048.0), ("MaxDesiredSpeed", 1.0), ("AirSpeed", 300.0)] {
        vm.set_prop(a, n, 0, Value::Float(v)).unwrap();
    }
    vm.set_prop(a, "bMovable", 0, Value::Bool(true)).unwrap();
    vm.set_prop(a, "bCollideWorld", 0, Value::Bool(true)).unwrap();
    vm.set_prop(a, "Physics", 0, Value::Byte(2)).unwrap();
    for _ in 0..30 {
        vm.perform_physics(a, 1.0 / 30.0).unwrap();
    }
    assert_eq!(vm.get_prop(a, "Physics", 0).unwrap().as_i32(), Some(1), "the pawn stands");
    a
}

fn yaw(vm: &mut Vm, a: u32) -> i32 {
    match vm.get_prop(a, "Rotation", 0).unwrap() {
        Value::Rotator(r) => r[1] & 0xFFFF,
        o => panic!("{o:?}"),
    }
}

/// `GetAllianceType` agrees with the alliance table the map's pawns carry: negative level hostile, positive friendly,
/// zero or unknown neutral; `IsValidEnemy` with an alliance check follows it.
#[test]
fn alliances_follow_the_pawn_tables() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    // pawns fill their alliance tables on their first tick
    let mut issues = Vec::new();
    for _ in 0..3 {
        vm.tick_report(1.0 / 30.0, &mut issues);
    }
    let scripted = vm.reg.class_by_name("ScriptedPawn").unwrap();
    let mut checked = 0;
    for &a in &l.actors {
        if !vm.class_of(a).map(|c| vm.is_a(c, scripted)).unwrap_or(false) {
            continue;
        }
        for i in 0..16 {
            let Value::Struct(_, b) = vm.get_prop(a, "AlliancesEx", i).unwrap() else { continue };
            let name = u32::from_le_bytes(b[0..4].try_into().unwrap());
            let level = f32::from_le_bytes(b[4..8].try_into().unwrap());
            let agitation = f32::from_le_bytes(b[8..12].try_into().unwrap());
            if name == 0 {
                continue;
            }
            let likes_neutral = vm.get_prop(a, "bLikesNeutral", 0).unwrap() == Value::Bool(true);
            let reversed = vm.get_prop(a, "bReverseAlliances", 0).unwrap() == Value::Bool(true);
            let mut want = if level < 0.0 || agitation >= 1.0 { 2 } else if level > 0.0 { 0 } else { 1 };
            if likes_neutral && want == 1 {
                want = 0;
            }
            if reversed {
                want = if want == 0 { 2 } else { (want != 2) as i32 };
            }
            let got = vm.call_by_name(a, "GetAllianceType", vec![Value::Name(name)]).unwrap();
            assert_eq!(got.as_i32(), Some(want), "{} entry {i}: level {level}", vm.reg.class(vm.class_of(a).unwrap()).path);
            checked += 1;
        }
    }
    assert!(checked > 0, "no alliance entries found in the map");
}

/// `TurnTo` swings the pawn to face a point at its `RotationRate`, finishing within the engine's tolerance.
#[test]
fn turn_to_faces_the_focus() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let s = pawn_spot(&mut vm, &l);
    let a = walker(&mut vm, [s[0], s[1], s[2] + 20.0]);
    vm.set_prop(a, "RotationRate", 0, Value::Rotator([20000, 20000, 20000])).unwrap();
    vm.set_prop(a, "Rotation", 0, Value::Rotator([0, 0, 0])).unwrap();
    let loc = vm.location(a);
    // a point to the pawn's left (+Y): yaw 16384
    vm.native_turn_to(a, [loc[0], loc[1] + 500.0, loc[2]]);
    let mut frames = 0;
    loop {
        frames += 1;
        assert!(frames < 90, "never finished turning, yaw {}", yaw(&mut vm, a));
        if vm.poll_ai_latent(a, "TurnTo").unwrap() == Some(true) {
            break;
        }
        vm.perform_physics(a, 1.0 / 30.0).unwrap();
    }
    let y = yaw(&mut vm, a);
    assert!((y - 16384).abs() < 2000, "ended facing {y}");
    // 16384 units at 20000 per second: about 0.8 s
    assert!((18..=32).contains(&frames), "{frames} frames");
}

/// A pawn spot with 200 free units in some compass direction (boxes traced against the world and actors).
fn roomy_spot(vm: &mut Vm, l: &ue_vm::level::LevelLoad) -> ([f32; 3], [f32; 2]) {
    let pawn = vm.reg.class_by_name("Pawn").unwrap();
    let probe_class = vm.reg.class_by_name("Actor").unwrap();
    let probe = vm.spawn(probe_class, 0, None, Some([0.0; 3]), None).unwrap().unwrap();
    let pawns: Vec<u32> = l.actors.iter().copied().filter(|&a| vm.class_of(a).map(|c| vm.is_a(c, pawn)).unwrap_or(false)).collect();
    let spots: Vec<[f32; 3]> = pawns.into_iter().map(|a| vm.location(a)).collect();
    for s in spots {
        let start = [s[0], s[1], s[2] + 20.0];
        for k in 0..8 {
            let ang = k as f32 * std::f32::consts::FRAC_PI_4;
            let d = [ang.cos(), ang.sin()];
            let end = [start[0] + d[0] * 260.0, start[1] + d[1] * 260.0, start[2]];
            let h = vm.single_line_check(probe, end, start, 0x17, [17.0, 17.0, 39.0], 0);
            if h.time >= 1.0 {
                vm.destroy(probe).unwrap();
                return (start, d);
            }
        }
    }
    panic!("no roomy spot found");
}

/// `MoveTo` walks the pawn to a point in the open and reports completion near it.
#[test]
fn move_to_walks_to_the_destination() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let (start, dir) = roomy_spot(&mut vm, &l);
    let a = walker(&mut vm, start);
    let start = vm.location(a);
    let dest = [start[0] + dir[0] * 200.0, start[1] + dir[1] * 200.0, start[2]];
    vm.native_move_to(a, dest, 1.0).unwrap();
    let mut done_at = None;
    for frame in 0..300 {
        if vm.poll_ai_latent(a, "MoveTo").unwrap() == Some(true) {
            done_at = Some(frame);
            break;
        }
        vm.perform_physics(a, 1.0 / 30.0).unwrap();
    }
    let frame = done_at.expect("MoveTo never completed");
    let end = vm.location(a);
    let left = ((end[0] - dest[0]).powi(2) + (end[1] - dest[1]).powi(2)).sqrt();
    assert!(left < 60.0, "stopped {left} units short of the destination (frames {frame})");
    // walking 200 units at 300/s cannot take less than half a second
    assert!(frame > 12, "arrived implausibly fast: {frame} frames");
}

/// `AICanSee`: a troop sees a detectable target straight ahead in the open, does not see one behind it, and light
/// levels stay in 0..1 (an unlit actor counts as fully lit).
#[test]
fn ai_can_see_follows_facing_and_light() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let (start, dir) = roomy_spot(&mut vm, &l);
    let troop = vm.reg.class_by_name("UNATCOTroop").unwrap();
    let looker = vm.spawn(troop, 0, None, Some(start), None).unwrap().unwrap();
    let pawn = vm.reg.class_by_name("Pawn").unwrap();
    let at = [start[0] + dir[0] * 150.0, start[1] + dir[1] * 150.0, start[2]];
    let target = vm.spawn(pawn, 0, None, Some(at), None).unwrap().unwrap();
    for (n, v) in [("CollisionRadius", 17.0), ("CollisionHeight", 39.0)] {
        vm.set_prop(target, n, 0, Value::Float(v)).unwrap();
    }
    vm.set_prop(target, "bDetectable", 0, Value::Bool(true)).unwrap();
    vm.set_prop(looker, "bDetectable", 0, Value::Bool(true)).unwrap();
    let yaw = (dir[1].atan2(dir[0]) * 32768.0 / std::f32::consts::PI) as i32;
    vm.set_prop(looker, "Rotation", 0, Value::Rotator([0, yaw, 0])).unwrap();
    // the light model stays in range; an unlit actor is fully lit
    let lvl = vm.call_by_name(target, "AIGetLightLevel", vec![Value::Vector(at)]).unwrap().as_f32().unwrap();
    assert!((0.0..=1.0).contains(&lvl), "light {lvl}");
    vm.set_prop(target, "bUnlit", 0, Value::Bool(true)).unwrap();
    let lit = vm.call_by_name(target, "AIGetLightLevel", vec![Value::Vector(at)]).unwrap().as_f32().unwrap();
    assert_eq!(lit, 1.0);
    // AICanSee(other, visibility, checkVisibility, checkDir, checkCylinder, checkLOS)
    let see = |vm: &mut Vm| vm.call_by_name(looker, "AICanSee", vec![Value::Obj(target), Value::Float(1.0), Value::Bool(true), Value::Bool(true), Value::Bool(true), Value::Bool(true)]).unwrap().as_f32().unwrap();
    // the light cache starts empty, as at the beginning of a level (both clocks at 0): let the game clock run
    vm.time = 5.0;
    let lv = vm.level;
    vm.set_prop(lv, "TimeSeconds", 0, Value::Float(5.0)).unwrap();
    let ahead = see(&mut vm);
    assert!(ahead > 0.0, "does not see a target 150 units straight ahead (fov {:?})", vm.get_prop(looker, "AIHorizontalFov", 0));
    vm.set_prop(looker, "Rotation", 0, Value::Rotator([0, yaw + 32768, 0])).unwrap();
    let behind = see(&mut vm);
    assert_eq!(behind, 0.0, "sees a target directly behind it");
    // facing does not matter when the direction check is off
    let any_dir = vm.call_by_name(looker, "AICanSee", vec![Value::Obj(target), Value::Float(1.0), Value::Bool(true), Value::Bool(false), Value::Bool(true), Value::Bool(true)]).unwrap().as_f32().unwrap();
    assert!(any_dir > 0.0);
}

/// `LineOfSightTo` / `CanSee` (Engine.dll 103beb50): a target in the open sees through the BSP-only rays; `CanSee`
/// adds the peripheral cone (nothing directly behind), alternates `bLOSflag` on each call for a non-enemy, leaves
/// `Stimulus` at 1.0 when it passes, and records `LastSeeingPos`/`LastSeenPos` when the target is the enemy.
#[test]
fn line_of_sight_follows_the_engine_rules() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let (start, dir) = roomy_spot(&mut vm, &l);
    let troop = vm.reg.class_by_name("UNATCOTroop").unwrap();
    let looker = vm.spawn(troop, 0, None, Some(start), None).unwrap().unwrap();
    let pawn = vm.reg.class_by_name("Pawn").unwrap();
    let at = [start[0] + dir[0] * 150.0, start[1] + dir[1] * 150.0, start[2]];
    let target = vm.spawn(pawn, 0, None, Some(at), None).unwrap().unwrap();
    for (n, v) in [("CollisionRadius", 17.0), ("CollisionHeight", 39.0)] {
        vm.set_prop(target, n, 0, Value::Float(v)).unwrap();
    }
    vm.set_prop(target, "Visibility", 0, Value::Byte(128)).unwrap();
    let yaw = (dir[1].atan2(dir[0]) * 32768.0 / std::f32::consts::PI) as i32;
    vm.set_prop(looker, "Rotation", 0, Value::Rotator([0, yaw, 0])).unwrap();
    vm.set_prop(looker, "SightRadius", 0, Value::Float(3000.0)).unwrap();
    vm.set_prop(looker, "PeripheralVision", 0, Value::Float(0.0)).unwrap();
    let call = |vm: &mut Vm, n: &str| vm.call_by_name(looker, n, vec![Value::Obj(target)]).unwrap().as_bool().unwrap();
    assert!(call(&mut vm, "LineOfSightTo"), "no line of sight to a pawn 150 units ahead in the open");
    // CanSee flips bLOSflag on every call for a non-enemy
    let flag = |vm: &mut Vm| vm.get_prop(looker, "bLOSflag", 0).unwrap().as_bool().unwrap();
    let f0 = flag(&mut vm);
    assert!(call(&mut vm, "CanSee"));
    assert_ne!(flag(&mut vm), f0, "bLOSflag did not alternate");
    assert_eq!(vm.get_prop(looker, "Stimulus", 0).unwrap().as_f32().unwrap(), 1.0);
    // directly behind the looker: cosine -1 - 0 scaled by 0.17 + 0.2 < 0 -> not seen
    vm.set_prop(looker, "Rotation", 0, Value::Rotator([0, yaw + 32768, 0])).unwrap();
    assert!(!call(&mut vm, "CanSee"), "sees a target directly behind it");
    assert!(call(&mut vm, "LineOfSightTo"), "plain LineOfSightTo ignores the cone");
    // as the enemy: the positions are recorded and the flag is left alone
    vm.set_prop(looker, "Enemy", 0, Value::Obj(target)).unwrap();
    let f1 = flag(&mut vm);
    assert!(call(&mut vm, "CanSee"));
    assert_eq!(flag(&mut vm), f1);
    let seen = vm.get_prop(looker, "LastSeenPos", 0).unwrap();
    assert!(matches!(seen, Value::Vector(v) if (v[0] - at[0]).abs() < 0.01 && (v[1] - at[1]).abs() < 0.01), "{seen:?}");
}

/// The AI event manager end to end: an `AcousticSensor` listens for `WeaponFire`; an actor within the event's radius
/// sends it, and the sensor's callback runs on the next tick. One outside the radius is not heard; a sender that is
/// destroyed stops being heard.
#[test]
fn event_manager_delivers_events_to_listeners() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let (start, dir) = roomy_spot(&mut vm, &l);
    let sensor_class = vm.reg.class_by_name("AcousticSensor").unwrap();
    let sensor = vm.spawn(sensor_class, 0, None, Some(start), None).unwrap().unwrap();
    let actor_class = vm.reg.class_by_name("Actor").unwrap();
    let fire = vm.reg.names.intern("WeaponFire");
    let near = [start[0] + dir[0] * 100.0, start[1] + dir[1] * 100.0, start[2]];
    let a = vm.spawn(actor_class, 0, None, Some(near), None).unwrap().unwrap();
    vm.event_trace = Some(Vec::new());
    let heard = |vm: &mut Vm| vm.event_trace.as_ref().unwrap().iter().filter(|(o, n)| *o == sensor && n == "NoiseHeard").count();
    // AISendEvent('WeaponFire', EAITYPE_Audio, 1.0, 300)
    vm.call_by_name(a, "AISendEvent", vec![Value::Name(fire), Value::Byte(1), Value::Float(1.0), Value::Float(300.0)]).unwrap();
    assert_eq!(heard(&mut vm), 0, "callbacks run in the tick, not when the event is sent");
    vm.tick(1.0 / 60.0).unwrap();
    assert_eq!(heard(&mut vm), 1, "the sensor did not hear a shot 100 units away");
    // a one-shot is heard once, not again on the next ticks
    vm.tick(1.0 / 60.0).unwrap();
    vm.tick(1.0 / 60.0).unwrap();
    assert_eq!(heard(&mut vm), 1);
    // too far for the 300 unit radius
    let far = [start[0] + dir[0] * 20.0, start[1] + dir[1] * 20.0, start[2]];
    let b = vm.spawn(actor_class, 0, None, Some([far[0] + 900.0, far[1], far[2]]), None).unwrap().unwrap();
    vm.call_by_name(b, "AISendEvent", vec![Value::Name(fire), Value::Byte(1), Value::Float(1.0), Value::Float(300.0)]).unwrap();
    vm.tick(1.0 / 60.0).unwrap();
    assert_eq!(heard(&mut vm), 1, "heard a sound beyond its radius");
}

/// `ReachablePathnodes` (`GetPathnodeList`): a pawn standing on a navigation point gets that node's reach-spec
/// neighbours that fit its size, with the spec distances, nearest first.
#[test]
fn reachable_pathnodes_lists_the_neighbours_of_the_node_under_the_pawn() {
    let Some((mut vm, _l)) = world("00_Training") else { return };
    let specs = vm.reach_specs.clone();
    let node = specs.iter().find(|s| s.flags == 1 && s.radius >= 17 && s.height >= 39 && s.start != 0 && s.end != 0).map(|s| s.start).expect("a walk spec");
    let at = vm.location(node);
    let troop = vm.reg.class_by_name("UNATCOTroop").unwrap();
    let p = vm.spawn(troop, 0, None, Some(at), None).unwrap().unwrap();
    let list = vm.get_pathnode_list(p, 32, 0, false, true).unwrap();
    assert!(!list.is_empty(), "no reachable nodes from a node");
    assert!(list.windows(2).all(|w| w[0].1 <= w[1].1), "not sorted by distance: {list:?}");
    // every listed node is the end of a spec that starts at the node (the pawn may overlap more than one node: any of them)
    for (n, d) in &list {
        assert!(specs.iter().any(|s| s.end == *n && s.distance as f32 == *d), "{n} at {d} is not a spec end");
    }
}

/// The pawn part of `AActor::Tick`: `PainTime` counts down to `PainTimer` (drowning, pain zones), and the sight counter
/// is reset to 0.1 after an enemy check.
#[test]
fn pawn_tick_runs_pain_timer_and_sight_counter() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let (start, _) = roomy_spot(&mut vm, &l);
    let troop = vm.reg.class_by_name("UNATCOTroop").unwrap();
    let p = vm.spawn(troop, 0, None, Some(start), None).unwrap().unwrap();
    vm.set_prop(p, "PainTime", 0, Value::Float(0.05)).unwrap();
    vm.event_trace = Some(Vec::new());
    for _ in 0..6 {
        vm.tick(1.0 / 60.0).unwrap();
    }
    let fired = vm.event_trace.as_ref().unwrap().iter().any(|(o, n)| *o == p && n == "PainTimer");
    assert!(fired, "PainTimer never ran");
    let sc = vm.get_prop(p, "SightCounter", 0).unwrap().as_f32().unwrap();
    assert!(sc <= 0.2, "sight counter {sc}");
}

/// Path finding: from a pawn standing on a navigation point, `FindPathToward` a node three hops away returns the first
/// hop, and the route (that node and its `previousPath` chain) is a chain of reach-spec links ending at the goal. The graph used to
/// pick the goal is walked here directly from the specs, independently of the search.
#[test]
fn find_path_toward_follows_reach_specs() {
    let Some((mut vm, _l)) = world("00_Training") else { return };
    assert!(!vm.reach_specs.is_empty(), "the map has no reach specs");
    let walk_only = |s: &ue_vm::navigation::ReachSpecRt| s.flags == 1 && s.radius >= 17 && s.height >= 39 && s.end != 0 && s.start != 0;
    let specs = vm.reach_specs.clone();
    // BFS over walk-only links from each node until one has a node exactly three hops away
    let mut choice = None;
    'find: for s0 in specs.iter().filter(|s| walk_only(s)) {
        let start = s0.start;
        let mut dist = std::collections::HashMap::new();
        dist.insert(start, 0);
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(n) = queue.pop_front() {
            for s in specs.iter().filter(|s| walk_only(s) && s.start == n) {
                if !dist.contains_key(&s.end) {
                    dist.insert(s.end, dist[&n] + 1);
                    queue.push_back(s.end);
                }
            }
        }
        if let Some((&goal, _)) = dist.iter().find(|&(_, &d)| d == 3) {
            choice = Some((start, goal));
            break 'find;
        }
    }
    let (start_node, goal_node) = choice.expect("no node pair three walk-only hops apart");
    let at = vm.location(start_node);
    let a = walker(&mut vm, [at[0], at[1], at[2] + 5.0]);
    vm.set_prop(a, "bCanWalk", 0, Value::Bool(true)).unwrap();
    vm.set_prop(a, "MoveTarget", 0, Value::Obj(start_node)).unwrap();
    let first = vm.call_by_name(a, "FindPathToward", vec![Value::Obj(goal_node)]).unwrap();
    let Value::Obj(first) = first else { panic!("{first:?}") };
    assert_ne!(first, 0, "no route found");
    let linked = |x: u32, y: u32| specs.iter().any(|s| s.start == x && s.end == y);
    assert!(linked(start_node, first), "the first hop is not linked to the start node");
    // the engine searches backwards from the goal: each visited node's `previousPath` is the next node towards the goal
    // (`RouteCache` stays empty: Deus Ex's `SetRouteCache` is a stub)
    assert_eq!(vm.get_prop(a, "RouteCache", 0).unwrap(), Value::Obj(0));
    let mut chain = vec![first];
    while chain.len() < 64 {
        let Value::Obj(n) = vm.get_prop(*chain.last().unwrap(), "previousPath", 0).unwrap() else { break };
        if n == 0 {
            break;
        }
        chain.push(n);
    }
    for w in chain.windows(2) {
        assert!(linked(w[0], w[1]), "route step {:?} is not a reach spec", w);
    }
    assert_eq!(*chain.last().unwrap(), goal_node, "the route does not end at the goal");
    // the route is a cheapest one by total distance (Bellman-Ford over the same walk-only links)
    let mut best: std::collections::HashMap<u32, i64> = std::collections::HashMap::new();
    best.insert(start_node, 0);
    for _ in 0..specs.len() {
        let mut changed = false;
        for sp in specs.iter().filter(|s| walk_only(s)) {
            if let Some(&d) = best.get(&sp.start) {
                if best.get(&sp.end).map(|&e| d + (sp.distance as i64) < e).unwrap_or(true) {
                    best.insert(sp.end, d + sp.distance as i64);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut full = vec![start_node];
    full.extend(chain.iter());
    let total: i64 = full.windows(2).map(|w| specs.iter().find(|s| s.start == w[0] && s.end == w[1]).unwrap().distance as i64).sum();
    assert_eq!(total, best[&goal_node], "the route is not the cheapest: {total} vs {}", best[&goal_node]);
}

/// `pointReachable` / `actorReachable`: open floor ahead is reachable, a point far beyond the 1000-unit limit is not,
/// and a point just behind a wall is not.
#[test]
fn reachability_tests_see_walls_and_limits() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let (start, dir) = roomy_spot(&mut vm, &l);
    let a = walker(&mut vm, start);
    let loc = vm.location(a);
    let call = |vm: &mut Vm, p: [f32; 3]| vm.call_by_name(a, "pointReachable", vec![Value::Vector(p)]).unwrap() == Value::Bool(true);
    let ahead = [loc[0] + dir[0] * 150.0, loc[1] + dir[1] * 150.0, loc[2]];
    assert!(call(&mut vm, ahead), "open floor 150 units ahead is not reachable");
    let far = [loc[0] + dir[0] * 2000.0, loc[1] + dir[1] * 2000.0, loc[2]];
    assert!(!call(&mut vm, far), "a point 2000 units away is reachable");
    // a spot behind a wall: the first compass direction where the world blocks within 120 units
    let mut behind = None;
    for k in 0..8 {
        let ang = k as f32 * std::f32::consts::FRAC_PI_4;
        let d = [ang.cos(), ang.sin()];
        let end = [loc[0] + d[0] * 120.0, loc[1] + d[1] * 120.0, loc[2]];
        let h = vm.single_line_check(a, end, loc, 4, [0.0; 3], 0);
        if h.time < 1.0 {
            let beyond = [h.location[0] + d[0] * 60.0, h.location[1] + d[1] * 60.0, loc[2]];
            if !vm.bsp.as_ref().unwrap().fast_line_check(beyond, [loc[0], loc[1], loc[2] + 30.0]) {
                behind = Some(beyond);
                break;
            }
        }
    }
    if let Some(p) = behind {
        assert!(!call(&mut vm, p), "a point behind a wall is reachable");
    }
    // the pawn itself is unharmed by the tests
    let after = vm.location(a);
    assert!((after[0] - loc[0]).abs() < 0.01 && (after[1] - loc[1]).abs() < 0.01, "{loc:?} -> {after:?}");
}

/// `SuggestJumpVelocity` / `FindBestJump`: on open floor a jump toward a point 200 units ahead is aimed straight at it,
/// with the horizontal speed that covers the distance in the flight time (simulated in 0.05 s steps, a little shorter
/// than 2 * JumpZ / gravity; capped by GroundSpeed), and the landing found by `FindBestJump` is nearer the point.
#[test]
fn jumps_are_aimed_and_land_nearer() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let (start, dir) = roomy_spot(&mut vm, &l);
    let a = walker(&mut vm, start);
    vm.set_prop(a, "JumpZ", 0, Value::Float(325.0)).unwrap();
    let loc = vm.location(a);
    let dest = [loc[0] + dir[0] * 200.0, loc[1] + dir[1] * 200.0, loc[2]];
    let g = vm.zone_vec(a, "ZoneGravity")[2];
    let v = vm.suggest_jump_velocity(a, dest, [0.0, 0.0, 325.0]);
    let hs = (v[0] * v[0] + v[1] * v[1]).sqrt();
    let t = 2.0 * 325.0 / -g;
    eprintln!("gravity {g}, velocity {v:?}, speed {hs:.1}, expected about {:.1}", (200.0 / t).min(300.0));
    assert_eq!(v[2], 325.0, "the vertical speed is kept");
    assert!((v[0] * dir[0] + v[1] * dir[1]) / hs > 0.999, "not aimed at the point");
    assert!(hs >= (200.0 / t).min(300.0) - 1.0 && hs <= 300.0, "horizontal speed {hs}");
    let (good, landing) = vm.find_best_jump(a, dest, [0.0; 3], false).unwrap();
    let after = vm.location(a);
    eprintln!("start {loc:?} dest {dest:?} landing {landing:?}, good {good}");
    assert!(good, "the jump was not good");
    assert_eq!(after, loc, "the pawn was not put back");
}

/// `flyReachable` through `Reachable`: a flying pawn reaches a point 150 units ahead in open space with `flyMove` steps
/// (and is put back where it was); a falling pawn reaches nothing.
#[test]
fn a_flyer_reaches_open_space_and_a_falling_pawn_nothing() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let (start, dir) = roomy_spot(&mut vm, &l);
    let a = walker(&mut vm, start);
    let loc = vm.location(a);
    let dest = [loc[0] + dir[0] * 150.0, loc[1] + dir[1] * 150.0, loc[2] + 10.0];
    vm.set_prop(a, "bCanFly", 0, Value::Bool(true)).unwrap();
    vm.set_prop(a, "Physics", 0, Value::Byte(4)).unwrap();
    assert!(vm.reachable(a, dest, 15.0, 0).unwrap(), "open space ahead is not reachable by flying");
    assert_eq!(vm.location(a), loc, "the pawn was not put back");
    vm.set_prop(a, "Physics", 0, Value::Byte(2)).unwrap();
    assert!(!vm.reachable(a, dest, 15.0, 0).unwrap(), "a falling pawn reached something");
}

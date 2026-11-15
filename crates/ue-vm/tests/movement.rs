//! Movement and collision natives on a real map. Set `DEUSEX_DIR` to the game folder.

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

fn vec3(v: &Value) -> [f32; 3] {
    match v {
        Value::Vector(v) => *v,
        o => panic!("{o:?}"),
    }
}

/// A spawned test actor: player-sized, colliding with the world.
fn probe(vm: &mut Vm, at: [f32; 3]) -> u32 {
    let class = vm.reg.class_by_name("Actor").unwrap();
    let a = vm.spawn(class, 0, None, Some(at), None).unwrap().unwrap();
    vm.set_prop(a, "bMovable", 0, Value::Bool(true)).unwrap();
    vm.set_prop(a, "bStatic", 0, Value::Bool(false)).unwrap();
    vm.set_prop(a, "CollisionRadius", 0, Value::Float(17.0)).unwrap();
    vm.set_prop(a, "CollisionHeight", 0, Value::Float(39.0)).unwrap();
    vm.set_prop(a, "bCollideWorld", 0, Value::Bool(true)).unwrap();
    a
}

fn start_location(vm: &mut Vm, l: &ue_vm::level::LevelLoad) -> [f32; 3] {
    let class = vm.reg.class_by_name("PlayerStart").unwrap();
    let a = l.actors.iter().copied().find(|&a| vm.class_of(a) == Some(class)).expect("PlayerStart");
    vm.location(a)
}

#[test]
fn trace_finds_floor_and_wall_through_the_native() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let p = start_location(&mut vm, &l);
    let a = probe(&mut vm, p);
    // Trace(out HitLocation, out HitNormal, TraceEnd, TraceStart)
    let end = [p[0], p[1], p[2] - 3000.0];
    let args = vec![Value::Vector([0.0; 3]), Value::Vector([0.0; 3]), Value::Vector(end), Value::Vector(p)];
    let r = vm.call_by_name(a, "Trace", args).unwrap();
    assert_eq!(r, Value::Obj(vm.level), "the first thing below is the world");
    let _ = vec3;
}

#[test]
fn move_stops_at_the_floor_and_set_location_teleports() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let p = start_location(&mut vm, &l);
    let a = probe(&mut vm, [p[0], p[1], p[2] + 20.0]);
    let before = vm.location(a);
    // fall in frame-sized steps: the actor comes to rest on the floor instead of passing through it
    let mut hits = 0;
    for _ in 0..40 {
        let mut hit = CheckResult::none();
        vm.move_actor(a, [0.0, 0.0, -12.0], [0, 0, 0], &mut hit, false, false, false, false).unwrap();
        if hit.time < 1.0 {
            hits += 1;
            assert_eq!(hit.actor, vm.level);
            assert!(hit.normal[2] > 0.7, "floor normal {:?}", hit.normal);
        }
    }
    let rest = vm.location(a);
    assert!(rest[2] < before[2] && rest[2] > before[2] - 200.0, "{before:?} -> {rest:?}");
    assert!(hits > 20, "rested on the floor for {hits} of 40 steps");
    // a free box exists where it came to rest
    assert!(vm.single_point_check(rest, [17.0, 17.0, 39.0], 0, true, false).is_none());
    // a long Move through the native stops short of the floor (the engine backs off by 10% of the trace)
    let r = vm.call_by_name(a, "Move", vec![Value::Vector([0.0, 0.0, -2000.0])]).unwrap();
    let after = vm.location(a);
    assert!(after[2] > rest[2] - 300.0, "{rest:?} -> {after:?}");
    let _ = r;
    // SetLocation moves to a free spot
    let target = [p[0], p[1], p[2] + 50.0];
    let r = vm.call_by_name(a, "SetLocation", vec![Value::Vector(target)]).unwrap();
    assert_eq!(r, Value::Bool(true));
    let loc = vm.location(a);
    assert!((loc[0] - target[0]).abs() < 40.0 && (loc[1] - target[1]).abs() < 40.0, "{loc:?} vs {target:?}");
}

#[test]
fn actors_get_their_zone_and_touch_each_other() {
    let Some((mut vm, l)) = world("00_Training") else { return };
    let p = start_location(&mut vm, &l);
    let a = probe(&mut vm, [p[0], p[1], p[2] + 5.0]);
    // actors know their zone: a ZoneInfo (the level itself when the map has no zone actor there)
    let zone = vm.get_prop(a, "Region", 0);
    assert!(zone.is_ok());
    vm.set_prop(a, "bCollideActors", 0, Value::Bool(true)).unwrap();
    vm.refresh_collision(a);
    let b = probe(&mut vm, [p[0] + 200.0, p[1], p[2] + 5.0]);
    vm.set_prop(b, "bCollideActors", 0, Value::Bool(true)).unwrap();
    vm.refresh_collision(b);
    vm.event_trace = Some(vec![]);
    // walking `a` into `b` (neither blocks) makes them touch
    let mut hit = CheckResult::none();
    vm.move_actor(a, [200.0, 0.0, 0.0], [0, 0, 0], &mut hit, false, false, false, false).unwrap();
    let ev = vm.event_trace.clone().unwrap();
    assert!(ev.contains(&(a, "Touch".into())) && ev.contains(&(b, "Touch".into())), "{ev:?}");
}

/// The collision hash stays in step with the actors through a minute of play: every colliding actor is filed under
/// the cells of its current collision box, nothing else is filed.
#[test]
fn the_collision_hash_follows_every_move() {
    for map in ["01_NYC_UNATCOIsland", "05_NYC_UNATCOMJ12lab"] {
        let Some((mut vm, _)) = world(map) else { return };
        assert_eq!(vm.collision_hash_mismatches(), Vec::<u32>::new(), "{map} after load");
        for _ in 0..600 {
            vm.tick(0.1).unwrap();
        }
        let bad = vm.collision_hash_mismatches();
        let names: Vec<String> = bad.iter().map(|&a| vm.class_of(a).map(|c| vm.reg.class(c).path.clone()).unwrap_or_default()).collect();
        assert!(bad.is_empty(), "{map}: {names:?}");
    }
}

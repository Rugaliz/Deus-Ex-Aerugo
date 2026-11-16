//! World tests: level object, spawn/destroy, timers, tick loop. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::{Value, Vm};

fn world() -> Option<Vm> {
    let dir = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut vm = Vm::new(Registry::load_game(&dir).expect("load game"));
    vm.init_world().unwrap();
    Some(vm)
}

fn class(vm: &Vm, n: &str) -> ue_registry::ClassId {
    vm.reg.class_by_name(n).unwrap()
}

fn events_of(vm: &Vm, obj: u32) -> Vec<String> {
    vm.event_trace.as_ref().unwrap().iter().filter(|(o, _)| *o == obj).map(|(_, n)| n.clone()).collect()
}

#[test]
fn spawn_runs_events_in_the_original_order() {
    let Some(mut vm) = world() else { return };
    vm.event_trace = Some(Vec::new());
    let c = class(&vm, "Counter");
    let tag = vm.name_value("Wanted");
    let Value::Name(tag_id) = tag else { unreachable!() };
    let o = vm.spawn(c, 0, Some(tag_id), Some([1.0, 2.0, 3.0]), None).unwrap().unwrap();
    assert_eq!(
        events_of(&vm, o),
        ["Spawned", "PreBeginPlay", "BeginPlay", "PostBeginPlay", "SetInitialState", "PostPostBeginPlay"]
    );
    assert_eq!(vm.get_prop(o, "Location", 0).unwrap(), Value::Vector([1.0, 2.0, 3.0]));
    assert_eq!(vm.get_prop(o, "Tag", 0).unwrap(), tag);
    assert_eq!(vm.get_prop(o, "Level", 0).unwrap(), Value::Obj(vm.level));
    // ROLE_Authority: the enum is None, DumbProxy, SimulatedProxy, AutonomousProxy, Authority
    assert_eq!(vm.get_prop(o, "Role", 0).unwrap(), Value::Byte(4));
    // Counter.BeginPlay (a script) copied NumToCount into OriginalNum
    let n = vm.get_prop(o, "NumToCount", 0).unwrap();
    assert_ne!(n, Value::Byte(0));
    assert_eq!(vm.get_prop(o, "OriginalNum", 0).unwrap(), n);
}

#[test]
fn spawn_rejects_non_actors_and_script_spawn_works() {
    let Some(mut vm) = world() else { return };
    let to = class(&vm, "TestObj");
    assert!(vm.spawn(to, 0, None, None, None).is_err());
    // Spawn called through the script-visible native, with the class as an object reference
    let c = class(&vm, "Counter");
    let path = vm.reg.class(c).path.clone();
    let cref = vm.reg.obj_id(&path).0;
    let holder = vm.spawn(class(&vm, "Counter"), 0, None, None, None).unwrap().unwrap();
    let made = vm.call_by_name(holder, "Spawn", vec![Value::Obj(cref)]).unwrap();
    let Value::Obj(made) = made else { panic!("Spawn returned {made:?}") };
    assert!(vm.is_live(made));
    assert_eq!(vm.class_of_ref(made), Some(c));
}

/// `execSpawn` + `SpawnActor`: the spawner's `Instigator` is passed on, `Tag` is the class name for an engine spawn and
/// `SpawnTag` (None when omitted) after a script spawn, `LastRenderTime` starts at -10, and abstract classes are refused
/// with `None` (a log line, no script error).
#[test]
fn script_spawn_sets_instigator_tag_and_render_time_as_the_engine() {
    let Some(mut vm) = world() else { return };
    let c = class(&vm, "Counter");
    let cref = { let p = vm.reg.class(c).path.clone(); vm.reg.obj_id(&p).0 };
    let holder = vm.spawn(c, 0, None, None, None).unwrap().unwrap();
    assert_eq!(vm.get_prop(holder, "Tag", 0).unwrap(), vm.name_value("Counter"));
    assert_eq!(vm.fp(holder, "LastRenderTime"), -10.0);
    let pawn = vm.spawn(class(&vm, "Pawn"), 0, None, None, None).unwrap().unwrap();
    vm.set_prop(holder, "Instigator", 0, Value::Obj(pawn)).unwrap();
    let Value::Obj(a) = vm.call_by_name(holder, "Spawn", vec![Value::Obj(cref)]).unwrap() else { panic!() };
    assert!(a != 0 && vm.is_live(a));
    assert_eq!(vm.get_prop(a, "Instigator", 0).unwrap(), Value::Obj(pawn));
    assert_eq!(vm.get_prop(a, "Tag", 0).unwrap(), Value::Name(0), "no SpawnTag: Tag None");
    let wanted = vm.name_value("Wanted");
    let Value::Obj(b) = vm.call_by_name(holder, "Spawn", vec![Value::Obj(cref), Value::Obj(holder), wanted.clone()]).unwrap() else { panic!() };
    assert_eq!(vm.get_prop(b, "Tag", 0).unwrap(), wanted);
    assert_eq!(vm.get_prop(b, "Owner", 0).unwrap(), Value::Obj(holder));
    let abstract_ref = { let p = vm.reg.class(class(&vm, "Pawn")).path.clone(); vm.reg.obj_id(&p).0 };
    let n = vm.log.len();
    assert_eq!(vm.call_by_name(holder, "Spawn", vec![Value::Obj(abstract_ref)]).unwrap(), Value::Obj(0));
    assert!(vm.log[n..].iter().any(|l| l.contains("abstract")));
}

#[test]
fn destroy_removes_the_actor() {
    let Some(mut vm) = world() else { return };
    let c = class(&vm, "Counter");
    vm.event_trace = Some(Vec::new());
    let a = vm.spawn(c, 0, None, None, None).unwrap().unwrap();
    let b = vm.spawn(c, 0, None, None, None).unwrap().unwrap();
    let ti = vm.new_object(class(&vm, "TestInfo")).unwrap();
    let live_actors = |vm: &mut Vm| {
        let n = vm.log.len();
        vm.call_by_name(ti, "TestContinueForEach", vec![]).unwrap();
        vm.log[n..].iter().filter(|l| l.starts_with("actor ")).count()
    };
    let before = live_actors(&mut vm);
    assert_eq!(vm.call_by_name(a, "Destroy", vec![]).unwrap(), Value::Bool(true));
    assert!(events_of(&vm, a).contains(&"Destroyed".to_string()));
    assert_eq!(vm.get_prop(a, "bDeleteMe", 0).unwrap(), Value::Bool(true));
    assert!(!vm.is_live(a) && vm.is_live(b));
    assert_eq!(live_actors(&mut vm), before - 1);
    // destroying twice is harmless (`DestroyActor` answers true for a deleted actor)
    assert!(vm.destroy(a).unwrap());
    // until `ULevel::CleanupDestroyed` the destroyed actor is still read through (`execContext` only rejects null)
    let n = vm.log.len();
    vm.call_by_name(ti, "TestNullContext", vec![Value::Obj(a)]).unwrap();
    assert!(!vm.log[n..].iter().any(|l| l.contains("Accessed None")), "{:?}", &vm.log[n..]);
    // a level tick does not clean up fewer than 128; a forced cleanup frees it, and then it reads as none
    vm.tick(0.01).unwrap();
    assert!(vm.is_pending_deleted(a));
    vm.cleanup_destroyed(true);
    assert!(!vm.is_pending_deleted(a));
    let n = vm.log.len();
    vm.call_by_name(ti, "TestNullContext", vec![Value::Obj(a)]).unwrap();
    assert!(vm.log[n..].iter().any(|l| l.contains("Accessed None")));
}

/// `ULevel::CleanupDestroyed` clears every actor's reference to a destroyed actor (`UStruct::CleanupDestroyed`), and
/// only once more than 127 wait (or when forced).
#[test]
fn cleanup_clears_references_to_destroyed_actors() {
    let Some(mut vm) = world() else { return };
    let c = class(&vm, "Counter");
    let holder = vm.spawn(c, 0, None, None, None).unwrap().unwrap();
    let mut gone = Vec::new();
    for _ in 0..127 {
        gone.push(vm.spawn(c, 0, None, None, None).unwrap().unwrap());
    }
    vm.set_prop(holder, "Owner", 0, Value::Obj(0)).unwrap();
    vm.set_prop(holder, "Target", 0, Value::Obj(gone[0])).unwrap();
    for &g in &gone {
        vm.destroy(g).unwrap();
    }
    // 127 waiting: the tick leaves them
    vm.tick(0.01).unwrap();
    assert_eq!(vm.get_prop(holder, "Target", 0).unwrap(), Value::Obj(gone[0]));
    // the 128th: the end of the next tick clears the reference
    let one_more = vm.spawn(c, 0, None, None, None).unwrap().unwrap();
    vm.destroy(one_more).unwrap();
    vm.tick(0.01).unwrap();
    assert_eq!(vm.get_prop(holder, "Target", 0).unwrap(), Value::Obj(0));
    assert!(!vm.is_pending_deleted(gone[0]));
}

#[test]
fn level_time_and_tick_event() {
    let Some(mut vm) = world() else { return };
    vm.event_trace = Some(Vec::new());
    let o = vm.spawn(class(&vm, "Counter"), 0, None, None, None).unwrap().unwrap();
    for _ in 0..4 {
        vm.tick(0.25).unwrap();
    }
    assert_eq!(vm.get_prop(vm.level, "TimeSeconds", 0).unwrap(), Value::Float(1.0));
    assert_eq!(events_of(&vm, o).iter().filter(|e| *e == "Tick").count(), 4);
    // Disable('Tick') stops delivery of the event
    let t = vm.name_value("Tick");
    vm.call_by_name(o, "Disable", vec![t]).unwrap();
    vm.tick(0.25).unwrap();
    assert_eq!(events_of(&vm, o).iter().filter(|e| *e == "Tick").count(), 4);
}

#[test]
fn timers_loop_and_one_shot() {
    let Some(mut vm) = world() else { return };
    vm.event_trace = Some(Vec::new());
    let o = vm.spawn(class(&vm, "Counter"), 0, None, None, None).unwrap().unwrap();
    let timers = |vm: &Vm| events_of(vm, o).iter().filter(|e| *e == "Timer").count();
    vm.call_by_name(o, "SetTimer", vec![Value::Float(0.5), Value::Bool(true)]).unwrap();
    vm.tick(0.3).unwrap();
    assert_eq!(timers(&vm), 0);
    vm.tick(0.3).unwrap(); // counter 0.6 >= 0.5: fires, 0.1 left over
    assert_eq!(timers(&vm), 1);
    let c = vm.get_prop(o, "TimerCounter", 0).unwrap().as_f32().unwrap();
    assert!((c - 0.1).abs() < 1e-5, "counter {c}");
    vm.tick(0.25).unwrap(); // 0.35
    vm.tick(0.2).unwrap(); // 0.55 >= 0.5
    assert_eq!(timers(&vm), 2);
    assert_eq!(vm.get_prop(o, "TimerRate", 0).unwrap(), Value::Float(0.5));
    // a one-shot timer fires once and clears its rate
    vm.call_by_name(o, "SetTimer", vec![Value::Float(0.5), Value::Bool(false)]).unwrap();
    vm.tick(0.3).unwrap();
    vm.tick(0.3).unwrap();
    assert_eq!(timers(&vm), 3);
    assert_eq!(vm.get_prop(o, "TimerRate", 0).unwrap(), Value::Float(0.0));
    vm.tick(0.3).unwrap();
    vm.tick(0.3).unwrap();
    assert_eq!(timers(&vm), 3);
    // the engine clamps a frame to 0.4 s: a long hitch does not fire a 0.5 s timer at once
    vm.call_by_name(o, "SetTimer", vec![Value::Float(0.5), Value::Bool(false)]).unwrap();
    vm.tick(1.0).unwrap();
    assert_eq!(timers(&vm), 3, "a 1 s frame was not clamped to 0.4 s");
}

#[test]
fn world_tick_drives_state_code() {
    let Some(mut vm) = world() else { return };
    let d = vm.spawn(class(&vm, "Dispatcher"), 0, None, None, None).unwrap().unwrap();
    let t = vm.spawn(class(&vm, "Counter"), 0, None, None, None).unwrap().unwrap();
    let a = vm.name_value("A");
    vm.set_prop(d, "OutEvents", 0, a.clone()).unwrap();
    vm.set_prop(d, "OutDelays", 0, Value::Float(1.0)).unwrap();
    vm.set_prop(t, "Tag", 0, a).unwrap();
    vm.set_prop(t, "NumToCount", 0, Value::Byte(5)).unwrap();
    let dispatch = vm.reg.names.get("Dispatch").unwrap();
    vm.goto_state(d, Some(dispatch), None).unwrap();
    vm.tick(0.0).unwrap(); // starts the code (the engine makes a frame at least 0.005 s); it sleeps 1 second
    vm.tick(0.3).unwrap();
    vm.tick(0.3).unwrap();
    assert_eq!(vm.get_prop(t, "NumToCount", 0).unwrap(), Value::Byte(5));
    vm.tick(0.3).unwrap();
    vm.tick(0.3).unwrap(); // sleep over: the target is triggered
    assert_eq!(vm.get_prop(t, "NumToCount", 0).unwrap(), Value::Byte(4));
}

#[test]
fn property_text_natives() {
    let Some(mut vm) = world() else { return };
    let o = vm.new_object(class(&vm, "TestInfo")).unwrap();
    let get = |vm: &mut Vm, n: &str| vm.call_by_name(o, "GetPropertyText", vec![Value::Str(n.into())]).unwrap();
    assert_eq!(get(&mut vm, "xnum"), Value::Str("777".into()));
    assert_eq!(get(&mut vm, "bogus"), Value::Str(String::new()));
    vm.call_by_name(o, "SetPropertyText", vec![Value::Str("xnum".into()), Value::Str("999".into())]).unwrap();
    assert_eq!(vm.get_prop(o, "xnum", 0).unwrap(), Value::Int(999));
    assert_eq!(get(&mut vm, "xnum"), Value::Str("999".into()));
}

#[test]
fn testinfo_tick_self_test_runs() {
    let Some(mut vm) = world() else { return };
    // Epic's large self-test: reads Level.TimeSeconds, text properties, class defaults through class references,
    // static calls via `class<testinfo>(C).static`, and many assertions
    let o = vm.new_object(class(&vm, "TestInfo")).unwrap();
    vm.set_prop(o, "Level", 0, Value::Obj(vm.level)).unwrap();
    vm.set_prop(vm.level, "TimeSeconds", 0, Value::Float(12.5)).unwrap();
    let r = vm.send_event(o, "Tick", vec![Value::Float(0.1)]);
    assert!(r.is_ok(), "{r:?}\nlog tail: {:?}", &vm.log[vm.log.len().saturating_sub(5)..]);
    assert!(vm.log.iter().any(|l| l == "time=12.500000"));
    assert!(vm.log.iter().any(|l| l == "All tests passed"));
}

#[test]
fn prebeginplay_shrinks_inventory_collision_and_auto_state() {
    let Some(mut vm) = world() else { return };
    // Actor.PreBeginPlay lowers the collision height of inventory items by 0.75 (SetCollisionSize native).
    // Pick the first concrete Inventory class with a clearly larger default height that spawns.
    let inv = class(&vm, "Inventory");
    let mut checked = 0;
    for i in 0..vm.reg.classes.len() as u32 {
        let c = ue_registry::ClassId(i);
        if vm.reg.class(c).flags & 1 != 0 || !vm.is_a(c, inv) {
            continue;
        }
        let tmp = vm.new_object(c).unwrap();
        let default_h = vm.get_prop(tmp, "CollisionHeight", 0).unwrap().as_f32().unwrap();
        if default_h <= 1.5 {
            continue;
        }
        let Ok(Some(o)) = vm.spawn(c, 0, None, None, None) else { continue };
        let h = vm.get_prop(o, "CollisionHeight", 0).unwrap().as_f32().unwrap();
        assert!((h - (default_h - 0.75)).abs() < 1e-4, "{}: {h} vs {default_h}", vm.reg.class(c).path);
        checked += 1;
        if checked == 5 {
            break;
        }
    }
    assert!(checked > 0, "no inventory class to check");
    // SetInitialState -> GotoState('Auto') enters the class's `auto state`
    let t = vm.spawn(class(&vm, "Trigger"), 0, None, None, None).unwrap().unwrap();
    assert_eq!(vm.state_name(t), Some("NormalTrigger"));
}

#[test]
fn pawn_list_and_rotator_natives() {
    let Some(mut vm) = world() else { return };
    let pc = class(&vm, "ScriptedPawn");
    let a = vm.new_object(pc).unwrap();
    let b = vm.new_object(pc).unwrap();
    vm.call_by_name(a, "AddPawn", vec![]).unwrap();
    vm.call_by_name(b, "AddPawn", vec![]).unwrap();
    assert_eq!(vm.get_prop(vm.level, "PawnList", 0).unwrap(), Value::Obj(b));
    assert_eq!(vm.get_prop(b, "nextPawn", 0).unwrap(), Value::Obj(a));
    vm.call_by_name(b, "RemovePawn", vec![]).unwrap();
    assert_eq!(vm.get_prop(vm.level, "PawnList", 0).unwrap(), Value::Obj(a));
    // direction vector -> rotator: +Y is a quarter turn of yaw, +Z 90 degrees of pitch; `FVector::Rotation` scales by
    // 32768 / 3.14164, so a quarter turn truncates to 16383 (probe `vm.cast.vector.rotator`)
    let r = ue_vm::natives_cast(&mut vm, 0x50, Value::Vector([0.0, 1.0, 0.0])).unwrap();
    assert_eq!(r, Value::Rotator([0, 16383, 0]));
    let r = ue_vm::natives_cast(&mut vm, 0x50, Value::Vector([0.0, 0.0, 1.0])).unwrap();
    assert_eq!(r, Value::Rotator([16383, 0, 0]));
    // and the reverse cast for +X pitch 0 yaw 0 gives the unit X direction
    let v = ue_vm::natives_cast(&mut vm, 0x39, Value::Rotator([0, 0, 0])).unwrap();
    assert_eq!(v, Value::Vector([1.0, 0.0, 0.0]));
}

#[test]
fn ai_event_registration_natives() {
    let Some(mut vm) = world() else { return };
    let a = vm.new_object(class(&vm, "ScriptedPawn")).unwrap();
    let (alarm, heard) = (vm.reg.names.intern("Alarm"), vm.reg.names.intern("AlarmHeard"));
    // AISetEventCallback('Alarm', 'AlarmHeard', , true): the omitted optional arguments take the exec wrapper's
    // defaults (check direction on, cylinder off, line of sight on)
    let args = vec![Value::Name(alarm), Value::Name(heard), Value::Void, Value::Bool(false)];
    vm.call_by_name(a, "AISetEventCallback", args).unwrap();
    let (_, _, cb) = vm.ai.events.callbacks().find(|&(actor, ev, _)| actor == a && ev == alarm).unwrap();
    assert_eq!((cb.callback, cb.score_callback, cb.check_visibility, cb.check_dir, cb.check_cylinder, cb.check_los), (heard, 0, false, true, false, true));
    vm.call_by_name(a, "AIClearEventCallback", vec![Value::Name(alarm)]).unwrap();
    assert!(vm.ai.events.callbacks().next().is_none());
    // registrations disappear with the actor
    vm.call_by_name(a, "ConBindEvents", vec![]).unwrap();
    assert!(vm.ai.con_bound.contains(&a));
}

#[test]
fn vector_rotation_operators() {
    let Some(mut vm) = world() else { return };
    let x = Value::Vector([1.0, 0.0, 0.0]);
    let close = |v: Value, e: [f32; 3]| match v {
        Value::Vector(v) => assert!((0..3).all(|i| (v[i] - e[i]).abs() < 1e-4), "{v:?} != {e:?}"),
        o => panic!("{o:?}"),
    };
    // yaw a quarter turn: +X goes to +Y; and << undoes >>
    let r = Value::Rotator([0, 16384, 0]);
    let y = vm.call_static("Object", "GreaterGreater_VectorRotator", vec![x.clone(), r.clone()]).unwrap();
    close(y.clone(), [0.0, 1.0, 0.0]);
    close(vm.call_static("Object", "LessLess_VectorRotator", vec![y, r]).unwrap(), [1.0, 0.0, 0.0]);
    // pitch a quarter turn: +X goes to +Z
    close(vm.call_static("Object", "GreaterGreater_VectorRotator", vec![x, Value::Rotator([16384, 0, 0])]).unwrap(), [0.0, 0.0, 1.0]);
}

#[test]
fn base_animation_physics_and_sound_natives() {
    let Some(mut vm) = world() else { return };
    let pc = class(&vm, "ScriptedPawn");
    let (a, b) = (vm.new_object(pc).unwrap(), vm.new_object(pc).unwrap());
    // actors live in a level (their events need its `bBegunPlay`)
    for r in [a, b] {
        vm.set_prop(r, "Level", 0, Value::Obj(vm.level)).unwrap();
    }
    // SetBase links, refuses cycles, and a change fires BaseChange on the actor
    vm.event_trace = Some(vec![]);
    vm.call_by_name(a, "SetBase", vec![Value::Obj(b)]).unwrap();
    assert_eq!(vm.get_prop(a, "Base", 0).unwrap(), Value::Obj(b));
    let ev = vm.event_trace.clone().unwrap();
    assert!(ev.contains(&(b, "Attach".into())) && ev.contains(&(a, "BaseChange".into())), "{ev:?}");
    vm.call_by_name(b, "SetBase", vec![Value::Obj(a)]).unwrap();
    assert_eq!(vm.get_prop(b, "Base", 0).unwrap(), Value::Obj(0));
    vm.call_by_name(a, "SetBase", vec![Value::Obj(0)]).unwrap();
    assert_eq!(vm.get_prop(a, "Base", 0).unwrap(), Value::Obj(0));
    // SetPhysics stores the enum value
    vm.call_by_name(a, "SetPhysics", vec![Value::Byte(2)]).unwrap();
    assert_eq!(vm.get_prop(a, "Physics", 0).unwrap().as_i32(), Some(2));
    // PlaySound returns the engine's id `Index * 16 + Slot * 2 + bNoOverride`; with no player to hear it nothing
    // reaches the audio subsystem
    let id = vm.call_by_name(a, "PlaySound", vec![Value::Obj(0x1234), Value::Byte(1), Value::Float(0.5)]).unwrap();
    assert_eq!(id, Value::Int(vm.object_index(a) * 16 + 2));
    assert!(vm.ai.sounds.is_empty());
    vm.call_by_name(a, "StopSound", vec![Value::Int(vm.object_index(a) * 16 + 2)]).unwrap();
    assert!(matches!(vm.ai.sound_events.last(), Some(ue_vm::ai::SoundEvent::Stop(_))));
    // the Actor version goes to the engine: an unknown line is taken by nothing and answers with an empty string;
    // EXIT asks the engine to quit after the frame and says so
    let r = vm.call_by_name(a, "ConsoleCommand", vec![Value::Str("NoSuchCommand 1".into())]).unwrap();
    assert_eq!(r, Value::Str(String::new()));
    let r = vm.call_by_name(a, "ConsoleCommand", vec![Value::Str("exit".into())]).unwrap();
    assert_eq!(r, Value::Str("Closing by request".into()));
    assert_eq!(vm.engine_requests, vec![ue_vm::console::EngineRequest::Exit]);
}

#[test]
fn context_call_arguments_use_the_callers_self() {
    let Some(mut vm) = world() else { return };
    // DeusExPlayer.PostBeginPlay runs `AugmentationSystem.CreateAugmentations(Self)`: Self is the player, not the manager
    // (the spawn itself may end with the player destroyed by the missing game rules; the manager still exists)
    let pc = class(&vm, "DeusExPlayer");
    let _ = vm.spawn(pc, 0, None, None, None).unwrap();
    let mc = class(&vm, "AugmentationManager");
    let m = vm.objects.iter().position(|o| o.class == mc).map(ue_vm::inst_ref).expect("manager was created");
    let Value::Obj(p) = vm.get_prop(m, "Player", 0).unwrap() else { panic!() };
    assert_ne!(p, m);
    assert_eq!(vm.class_of(p), Some(pc));
}

#[test]
fn parent_defaults_bind_to_the_parents_variable() {
    let Some(vm) = world() else { return };
    // PlayerPawn redeclares `bSinglePlayer`; Actor's default of its own variable must stay true for a player
    let mut vm = vm;
    let o = vm.new_object(class(&vm, "DeusExPlayer")).unwrap();
    let actor_var = vm.reg.find_prop(class(&vm, "Actor"), "bSinglePlayer").unwrap().clone();
    let i = ue_vm::ref_instance(o).unwrap();
    let w = u32::from_le_bytes(vm.objects[i].mem.data[actor_var.offset as usize..][..4].try_into().unwrap());
    assert_ne!(w & actor_var.bit_mask, 0);
}

/// `ULevel::DestroyActor` (10395ba0): static and `bNoDelete` actors stay; a destroyed actor's owned actors lose it as
/// owner (it gets `LostChild` for each), actors touching it get `UnTouch` (it does not), its own owner `LostChild`.
#[test]
fn destroy_releases_owned_and_touching_actors() {
    let Some(mut vm) = world() else { return };
    let c = class(&vm, "Counter");
    vm.event_trace = Some(Vec::new());
    let fixed = vm.spawn(c, 0, None, None, None).unwrap().unwrap();
    vm.set_prop(fixed, "bStatic", 0, Value::Bool(true)).unwrap();
    assert_eq!(vm.call_by_name(fixed, "Destroy", vec![]).unwrap(), Value::Bool(false));
    assert!(vm.is_live(fixed));

    let boss = vm.spawn(c, 0, None, None, None).unwrap().unwrap();
    let a = vm.spawn(c, boss, None, None, None).unwrap().unwrap();
    let item = vm.spawn(c, a, None, None, None).unwrap().unwrap();
    let toucher = vm.spawn(c, 0, None, None, None).unwrap().unwrap();
    vm.set_prop(a, "Touching", 0, Value::Obj(toucher)).unwrap();
    vm.set_prop(toucher, "Touching", 0, Value::Obj(a)).unwrap();
    assert_eq!(vm.get_prop(item, "Owner", 0).unwrap(), Value::Obj(a));
    assert!(vm.destroy(a).unwrap());
    assert_eq!(vm.get_prop(item, "Owner", 0).unwrap(), Value::Obj(0));
    assert_eq!(vm.get_prop(toucher, "Touching", 0).unwrap(), Value::Obj(0));
    let ev = events_of(&vm, a);
    assert!(ev.contains(&"LostChild".to_string()), "{ev:?}");
    assert!(!ev.contains(&"UnTouch".to_string()), "{ev:?}");
    assert!(events_of(&vm, toucher).contains(&"UnTouch".to_string()));
    assert!(events_of(&vm, boss).contains(&"LostChild".to_string()));
}

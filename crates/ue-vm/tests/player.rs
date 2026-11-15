//! The real player (`JCDentonMale`) through the engine's login flow, and the host-side flag store.
//! Set `DEUSEX_DIR` to the game folder. Expected values are properties of the scripts' own start flow, not of our code.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::{Value, Vm};

fn open(map: &str) -> Option<Vm> {
    let g = std::env::var_os("DEUSEX_DIR").map(PathBuf::from)?;
    let mut vm = Vm::new(Registry::load_game(&g).unwrap());
    vm.init_world().unwrap();
    let pi = vm.reg.add_package(&g.join("Maps").join(format!("{map}.dx")), map).unwrap();
    vm.load_level(pi).unwrap();
    Some(vm)
}

#[test]
fn login_possess_puts_the_player_into_walking_at_the_player_start() {
    let Some(mut vm) = open("00_Training") else { return };
    let player = vm.login_player("JCDentonMale").unwrap();
    // the editor cameras saved in the map must not be adopted as the player
    let c = vm.class_of(player).unwrap();
    assert_eq!(vm.reg.class(c).path, "DeusEx.JCDentonMale");
    assert!(matches!(vm.get_prop(player, "Player", 0).unwrap(), Value::Obj(v) if v != 0));
    assert_eq!(vm.state_name(player), Some("PlayerWalking"));
    let mut issues = Vec::new();
    for _ in 0..30 {
        vm.player_tick(player, 1.0 / 30.0, 0.0, 0.0).unwrap();
        vm.tick_report(1.0 / 30.0, &mut issues);
    }
    assert!(issues.is_empty(), "{:?}", issues.iter().map(|i| format!("{} {}: {}", i.class, i.event, i.error)).collect::<Vec<_>>());
}

#[test]
fn flags_are_typed_case_insensitive_and_expire() {
    let Some(mut vm) = open("00_Training") else { return };
    let fb = vm.reg.class_by_name("FlagBase").unwrap();
    let f = vm.new_object(fb).unwrap();
    let n = |vm: &mut Vm, s: &str| vm.name_value(s);
    let a = n(&mut vm, "Training_Done");
    let b = n(&mut vm, "TRAINING_DONE");
    assert_eq!(vm.call_by_name(f, "GetBool", vec![a.clone()]).unwrap(), Value::Bool(false));
    assert_eq!(vm.call_by_name(f, "SetBool", vec![a.clone(), Value::Bool(true)]).unwrap(), Value::Bool(true));
    assert_eq!(vm.call_by_name(f, "GetBool", vec![b.clone()]).unwrap(), Value::Bool(true));
    // another type with the same name is a different flag
    assert_eq!(vm.call_by_name(f, "GetInt", vec![a.clone()]).unwrap(), Value::Int(0));
    assert_eq!(vm.call_by_name(f, "SetInt", vec![a.clone(), Value::Int(7), Value::Bool(false)]).unwrap(), Value::Bool(false), "bAdd=false must not create");
    assert_eq!(vm.call_by_name(f, "SetInt", vec![a.clone(), Value::Int(7), Value::Bool(true), Value::Int(5)]).unwrap(), Value::Bool(true));
    vm.call_by_name(f, "DeleteExpiredFlags", vec![Value::Int(4)]).unwrap();
    assert_eq!(vm.call_by_name(f, "GetInt", vec![a.clone()]).unwrap(), Value::Int(7));
    vm.call_by_name(f, "DeleteExpiredFlags", vec![Value::Int(5)]).unwrap();
    assert_eq!(vm.call_by_name(f, "GetInt", vec![a.clone()]).unwrap(), Value::Int(0));
    // the bool flag has no expiration and stays
    assert_eq!(vm.call_by_name(f, "CheckFlag", vec![a, Value::Byte(0)]).unwrap(), Value::Bool(true));
}

/// `ULevel::SpawnPlayActor` (10396bd0) order on a level change: the new pawn's `Possess`, then its `TravelPreAccept`
/// before the carried actors'; theirs run last to first both before and after `AcceptInventory`; the pawn's
/// `TravelPostAccept` comes last.
#[test]
fn travel_accepts_run_in_spawn_play_actor_order() {
    let Some(mut vm) = open("01_NYC_UNATCOIsland") else { return };
    let g = std::env::var_os("DEUSEX_DIR").map(PathBuf::from).unwrap();
    let player = vm.login_player("JCDentonMale").unwrap();
    let pi = vm.reg.add_package(&g.join("Maps").join("01_NYC_UNATCOHQ.dx"), "01_NYC_UNATCOHQ").unwrap();
    vm.event_trace = Some(Vec::new());
    let (pawn, _) = vm.travel_level(player, pi, "", "JCDentonMale", true).unwrap();
    let trace = vm.event_trace.take().unwrap();
    let pos = |name: &str, obj: u32| trace.iter().position(|(o, n)| *o == obj && n == name);
    let possess = pos("Possess", pawn).expect("Possess");
    let pre = pos("TravelPreAccept", pawn).expect("pawn TravelPreAccept");
    let post = pos("TravelPostAccept", pawn).expect("pawn TravelPostAccept");
    assert!(possess < pre && pre < post);
    let others = |name: &str| -> Vec<(usize, u32)> { trace.iter().enumerate().filter(|(_, (o, n))| n == name && *o != pawn).map(|(i, (o, _))| (i, *o)).collect() };
    let (pre_o, post_o) = (others("TravelPreAccept"), others("TravelPostAccept"));
    assert!(!pre_o.is_empty(), "something travels with the player");
    assert!(pre_o.iter().all(|(i, _)| *i > pre), "the pawn's TravelPreAccept comes first");
    assert!(post_o.iter().all(|(i, _)| *i < post), "the pawn's TravelPostAccept comes last");
    let order = |v: &[(usize, u32)]| v.iter().map(|(_, o)| *o).collect::<Vec<_>>();
    assert_eq!(order(&pre_o), order(&post_o));
}

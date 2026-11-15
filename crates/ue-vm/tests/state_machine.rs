//! State machine tests: state inheritance, GotoState, latent Sleep and a real state-code scenario (Dispatcher).
//! Set `DEUSEX_DIR` to the game folder; skipped otherwise.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::{Latent, Value, Vm};

fn vm() -> Option<Vm> {
    let dir = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    Some(Vm::new(Registry::load_game(&dir).expect("load game")))
}

fn make(vm: &mut Vm, class: &str) -> u32 {
    let c = vm.reg.class_by_name(class).unwrap();
    vm.new_object(c).unwrap()
}

fn nm(vm: &mut Vm, s: &str) -> Value {
    vm.name_value(s)
}

#[test]
fn state_inheritance_and_dispatch() {
    let Some(mut vm) = vm() else { return };
    let o = make(&mut vm, "TestInfo");
    let n = vm.name_value("EEDDAA");
    let Value::Name(id) = n else { unreachable!() };
    vm.goto_state(o, Some(id), None).unwrap();
    assert_eq!(vm.state_name(o), Some("EEDDAA"));
    // `f` is overridden in every test state: the call resolves to the current state's own function
    let f = vm.reg.names.get("f").unwrap();
    let loc = vm.resolve_function(o, f).unwrap();
    assert_eq!(loc.state.map(|s| vm.reg.names.str(vm.reg.state_def((loc.class, s)).name)), Some("EEDDAA"));
    // IsInState is true for the state and its ancestors (EEDDAA extends DDAA extends AA), false for siblings
    for (state, want) in [("EEDDAA", true), ("DDAA", true), ("AA", true), ("BB", false), ("CCAA", false)] {
        let v = vm.name_value(state);
        assert_eq!(vm.call_by_name(o, "IsInState", vec![v]).unwrap(), Value::Bool(want), "IsInState({state})");
    }
    let name = vm.call_by_name(o, "GetStateName", vec![]).unwrap();
    assert_eq!(name, vm.name_value("EEDDAA"));
    // no state: function resolves to the class version
    vm.goto_state(o, None, None).unwrap();
    assert_eq!(vm.state_name(o), None);
    let loc = vm.resolve_function(o, f).unwrap();
    assert_eq!(loc.state, None);
}

#[test]
fn dispatcher_state_code_with_sleep_and_foreach() {
    let Some(mut vm) = vm() else { return };
    let d = make(&mut vm, "Dispatcher");
    let c1 = make(&mut vm, "Counter");
    let c2 = make(&mut vm, "Counter");
    let c3 = make(&mut vm, "Counter");
    let (a, b) = (nm(&mut vm, "A"), nm(&mut vm, "B"));
    // dispatcher fires event A after 1.0s and event B after 0.5s
    vm.set_prop(d, "OutEvents", 0, a.clone()).unwrap();
    vm.set_prop(d, "OutDelays", 0, Value::Float(1.0)).unwrap();
    vm.set_prop(d, "OutEvents", 2, b.clone()).unwrap();
    vm.set_prop(d, "OutDelays", 2, Value::Float(0.5)).unwrap();
    // targets: two counters tagged A (3 and 1 to count), one tagged B (2)
    for (c, tag, n) in [(c1, &a, 3u8), (c2, &b, 2), (c3, &a, 1)] {
        vm.set_prop(c, "Tag", 0, tag.clone()).unwrap();
        vm.set_prop(c, "NumToCount", 0, Value::Byte(n)).unwrap();
    }
    let count = |vm: &Vm, c: u32| vm.get_prop(c, "NumToCount", 0).unwrap();

    let dispatch = vm.reg.names.get("Dispatch").unwrap();
    vm.goto_state(d, Some(dispatch), None).unwrap();
    assert_eq!(vm.state_name(d), Some("Dispatch"));
    // code starts at the Begin label on the next tick: disable('Trigger'), then Sleep(1.0) for event A
    vm.tick_state(d, 0.0).unwrap();
    assert_eq!(vm.objects[ue_vm::ref_instance(d).unwrap()].latent, Some(Latent::Sleep(1.0)));
    let trigger = vm.reg.names.get("Trigger").unwrap();
    assert!(vm.objects[ue_vm::ref_instance(d).unwrap()].disabled.contains(&trigger));
    assert_eq!((count(&vm, c1), count(&vm, c2), count(&vm, c3)), (Value::Byte(3), Value::Byte(2), Value::Byte(1)));
    // half the sleep: nothing happens yet
    vm.tick_state(d, 0.5).unwrap();
    assert_eq!(count(&vm, c1), Value::Byte(3));
    // sleep over: the foreach triggers both counters tagged A, then it sleeps 0.5s for event B
    vm.tick_state(d, 0.5).unwrap();
    assert_eq!((count(&vm, c1), count(&vm, c3)), (Value::Byte(2), Value::Byte(0)));
    assert_eq!(count(&vm, c2), Value::Byte(2));
    assert!(matches!(vm.objects[ue_vm::ref_instance(d).unwrap()].latent, Some(Latent::Sleep(t)) if (t - 0.5).abs() < 1e-6));
    // second sleep over: event B fires, the loop ends, Trigger is enabled again
    vm.tick_state(d, 0.5).unwrap();
    assert_eq!(count(&vm, c2), Value::Byte(1));
    assert_eq!(count(&vm, c1), Value::Byte(2));
    let o = &vm.objects[ue_vm::ref_instance(d).unwrap()];
    assert!(o.latent.is_none() && o.exec.is_none());
    assert!(!o.disabled.contains(&trigger));
}

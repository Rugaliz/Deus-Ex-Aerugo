//! Runs the self-tests that Epic wrote into the `TestInfo` script class (assertions inside the scripts), plus
//! checks of log output. Set `DEUSEX_DIR` to the game folder; skipped otherwise.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::{Value, Vm};

fn vm() -> Option<Vm> {
    let dir = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    Some(Vm::new(Registry::load_game(&dir).expect("load game")))
}

fn testinfo(vm: &mut Vm) -> u32 {
    let c = vm.reg.class_by_name("TestInfo").unwrap();
    vm.new_object(c).unwrap()
}

fn count(vm: &Vm, from: usize, prefix: &str) -> usize {
    vm.log[from..].iter().filter(|l| l.starts_with(prefix)).count()
}

#[test]
fn struct_bools_vectors_and_arrays() {
    let Some(mut vm) = vm() else { return };
    let o = testinfo(&mut vm);
    // asserts inside the script abort with an error if they fail
    vm.call_by_name(o, "TestStructBools", vec![]).unwrap();
    vm.call_by_name(o, "TestQ", vec![]).unwrap();
    vm.call_by_name(o, "TestX", vec![Value::Bool(false)]).unwrap();
    vm.call_by_name(o, "TestX", vec![Value::Bool(true)]).unwrap();
    assert_eq!(vm.get_prop(o, "MyArray", 0).unwrap(), Value::Int(1));
    assert_eq!(vm.get_prop(o, "MyArray", 1).unwrap(), Value::Int(1));
}

#[test]
fn optional_out_params_null_context_and_switch() {
    let Some(mut vm) = vm() else { return };
    let o = testinfo(&mut vm);
    vm.call_by_name(o, "TestOptionalOut", vec![]).unwrap();
    assert!(vm.log.iter().any(|l| l == "TestOptionalOut ok!"));
    vm.call_by_name(o, "TestNullContext", vec![Value::Obj(o)]).unwrap();
    let before = vm.log.len();
    vm.call_by_name(o, "TestNullContext", vec![Value::Obj(0)]).unwrap();
    assert!(vm.log[before..].iter().any(|l| l.contains("Accessed None")));
    vm.call_by_name(o, "TestSwitch", vec![]).unwrap();
    assert!(vm.log.iter().any(|l| l == "testswitch succeeded"));
}

#[test]
fn static_calls_and_default_variables() {
    let Some(mut vm) = vm() else { return };
    // static functions read class defaults through `default.`; Self is the class
    vm.call_static("TestInfo", "TestStatic", vec![Value::Int(123)]).unwrap();
    assert_eq!(vm.call_static("TestInfo", "OtherStatic", vec![Value::Int(246)]).unwrap(), Value::Int(555));
    let o = testinfo(&mut vm);
    assert_eq!(vm.call_by_name(o, "RecurseTest", vec![]).unwrap(), Value::Bool(false));
    assert_eq!(vm.get_prop(o, "bBool1", 0).unwrap(), Value::Bool(true));
}

#[test]
fn continue_in_loops() {
    let Some(mut vm) = vm() else { return };
    let o = testinfo(&mut vm);
    let n = vm.log.len();
    vm.call_by_name(o, "TestContinueFor", vec![]).unwrap();
    assert_eq!(count(&vm, n, "iteration "), 20);
    assert_eq!(count(&vm, n, "..."), 17); // 20 minus the continued 7, 9 and 19
    let n = vm.log.len();
    vm.call_by_name(o, "TestContinueWhile", vec![]).unwrap();
    assert_eq!(count(&vm, n, "iteration "), 20);
    assert_eq!(count(&vm, n, "..."), 18); // minus 7 and 9
    let n = vm.log.len();
    vm.call_by_name(o, "TestContinueDoUntil", vec![]).unwrap();
    assert_eq!(count(&vm, n, "iteration "), 21);
    assert_eq!(count(&vm, n, "..."), 16); // minus 7, 9, 19, 20, 21
}

#[test]
fn foreach_all_actors_with_continue() {
    let Some(mut vm) = vm() else { return };
    let o = testinfo(&mut vm);
    let n = vm.log.len();
    vm.call_by_name(o, "TestContinueForEach", vec![]).unwrap();
    // one live object; it is not a Light, so `continue` skips the "..." line
    assert_eq!(count(&vm, n, "actor "), 1);
    assert_eq!(count(&vm, n, "..."), 0);
    assert!(vm.log.last().map(|l| l == "DoneContinue").unwrap_or(false));
    // with two more objects the loop body runs for each of them
    let c = vm.reg.class_by_name("Light").unwrap();
    vm.new_object(c).unwrap();
    let n = vm.log.len();
    vm.call_by_name(o, "TestContinueForEach", vec![]).unwrap();
    assert_eq!(count(&vm, n, "actor "), 2);
    assert_eq!(count(&vm, n, "..."), 1); // only the Light reaches the line after `continue`
}

#[test]
fn object_creation_with_new() {
    let Some(mut vm) = vm() else { return };
    let o = testinfo(&mut vm);
    let before = vm.objects.len();
    vm.call_by_name(o, "BeginPlay", vec![]).unwrap();
    assert!(vm.objects.len() >= before + 5, "BeginPlay creates five TestObj instances");
}

#[test]
fn failing_script_assertions_are_reported() {
    let Some(mut vm) = vm() else { return };
    // OtherStatic asserts i == 246; a wrong argument must make the script fail
    let err = vm.call_static("TestInfo", "OtherStatic", vec![Value::Int(100)]).unwrap_err();
    assert!(err.to_string().contains("assertion failed"), "{err}");
}

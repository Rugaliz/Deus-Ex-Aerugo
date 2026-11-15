//! Core's script natives against the original's rules (WORKLOG 3.136). Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::{Value, Vm};

fn vm() -> Option<(Vm, u32)> {
    let dir = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut vm = Vm::new(Registry::load_game(&dir).expect("load game"));
    let c = vm.reg.class_by_name("Object").unwrap();
    let o = vm.new_object(c).unwrap();
    Some((vm, o))
}

fn s(x: &str) -> Value {
    Value::Str(x.into())
}

/// `FString::Mid` (101086e0) compares unsigned: a negative start lies past the end; a negative count makes the end
/// fall before the start, which then keeps the rest of the string.
#[test]
fn mid_clamps_unsigned() {
    let Some((mut vm, o)) = vm() else { return };
    let mut mid = |a: i32, b: Option<i32>| {
        let mut args = vec![s("Hello"), Value::Int(a)];
        args.push(b.map(Value::Int).unwrap_or(Value::Void));
        vm.call_by_name(o, "Mid", args).unwrap()
    };
    assert_eq!(mid(1, Some(3)), s("ell"));
    assert_eq!(mid(-2, Some(4)), s(""));
    assert_eq!(mid(1, Some(-2)), s("ello"));
    assert_eq!(mid(0, Some(-1)), s("Hello"));
    assert_eq!(mid(3, None), s("lo"));
    assert_eq!(mid(9, Some(2)), s(""));
}

/// `Localize` names the `int` file it last tried when a key is missing.
#[test]
fn localize_marks_missing_keys() {
    let Some((mut vm, o)) = vm() else { return };
    let v = vm.call_by_name(o, "Localize", vec![s("NoSuchSection"), s("NoSuchKey"), s("Core")]).unwrap();
    assert_eq!(v, s("<?int?Core.NoSuchSection.NoSuchKey?>"));
}

fn f(x: f32) -> Value {
    Value::Float(x)
}

/// `~=` compares the difference (kept in a register) with the double 1e-4: the float nearest to 1e-4 (just below it)
/// counts as equal; `Lerp`/`Smerp` ends and middles; `Normal` of a vector shorter than 1e-4 is zero (`SafeNormal`).
#[test]
fn float_maths_follow_the_registers() {
    let Some((mut vm, o)) = vm() else { return };
    let near = 0.0001f32;
    assert!((near as f64) < 1e-4);
    assert_eq!(vm.call_by_name(o, "ComplementEqual_FloatFloat", vec![f(1.0 + 0.0), f(1.0 - 0.0)]).unwrap(), Value::Bool(true));
    assert_eq!(vm.call_by_name(o, "ComplementEqual_FloatFloat", vec![f(near), f(0.0)]).unwrap(), Value::Bool(true));
    assert_eq!(vm.call_by_name(o, "ComplementEqual_FloatFloat", vec![f(0.00011), f(0.0)]).unwrap(), Value::Bool(false));
    assert_eq!(vm.call_by_name(o, "Lerp", vec![f(0.25), f(10.0), f(20.0)]).unwrap(), f(12.5));
    assert_eq!(vm.call_by_name(o, "Smerp", vec![f(0.5), f(10.0), f(20.0)]).unwrap(), f(15.0));
    assert_eq!(vm.call_by_name(o, "Smerp", vec![f(1.0), f(10.0), f(20.0)]).unwrap(), f(20.0));
    assert_eq!(vm.call_by_name(o, "Normal", vec![Value::Vector([0.00005, 0.0, 0.0])]).unwrap(), Value::Vector([0.0; 3]));
    assert_eq!(vm.call_by_name(o, "Normal", vec![Value::Vector([0.0, 3.0, 4.0])]).unwrap(), Value::Vector([0.0, 0.6, 0.8]));
}

/// `GetAxes` is `GMath.UnitCoords / R`: X is the rotator's direction from the sine table (`cos p cos y, cos p sin y,
/// sin p`), Y and Z complete it; `GetUnAxes` (`* R`) gives the transposed matrix.
#[test]
fn get_axes_and_its_transpose() {
    let Some((mut vm, o)) = vm() else { return };
    let r = Value::Rotator([3000, 20000, -5000]);
    let mut axes = |name: &str| {
        let mut args = vec![r.clone(), Value::Void, Value::Void, Value::Void];
        vm.call_by_name_out(o, name, &mut args).unwrap();
        let v = |x: &Value| match x {
            Value::Vector(v) => *v,
            other => panic!("{other:?}"),
        };
        [v(&args[1]), v(&args[2]), v(&args[3])]
    };
    let g = axes("GetAxes");
    let u = axes("GetUnAxes");
    let sin = ue_assets::light::sin_tab;
    let cos = |a: i32| sin(a + 16384);
    let fwd = [cos(3000) * cos(20000), cos(3000) * sin(20000), sin(3000)];
    for k in 0..3 {
        assert!((g[0][k] - fwd[k]).abs() < 1e-6, "{:?} {fwd:?}", g[0]);
        for i in 0..3 {
            assert!((g[i][k] - u[k][i]).abs() < 1e-6);
        }
    }
}

/// `GotoState` (1013dd00, 1012e8f0): an omitted state is the current one (only a jump to the label); a real change
/// rebuilds the probe mask (what `Disable` turned off is on again); `Disable` acts on the probe names only;
/// `GetStateName`/`IsInState` with no state answer with the class (1013edb0/1013ece0).
#[test]
fn states_and_probes() {
    let Some((mut vm, _)) = vm() else { return };
    let c = vm.reg.class_by_name("TestInfo").unwrap();
    let o = vm.new_object(c).unwrap();
    let nm = |vm: &mut Vm, s: &str| vm.name_value(s);
    // no state: the class
    assert_eq!(vm.call_by_name(o, "GetStateName", vec![]).unwrap(), nm(&mut vm, "TestInfo"));
    let info = nm(&mut vm, "Info");
    assert_eq!(vm.call_by_name(o, "IsInState", vec![info]).unwrap(), Value::Bool(true));
    let st = nm(&mut vm, "EEDDAA");
    vm.call_by_name(o, "GotoState", vec![st.clone(), Value::Void]).unwrap();
    assert_eq!(vm.state_name(o), Some("EEDDAA"));
    // omitted state: stays
    vm.call_by_name(o, "GotoState", vec![Value::Void, Value::Void]).unwrap();
    assert_eq!(vm.state_name(o), Some("EEDDAA"));
    // Disable: a probe name is recorded, another name is not
    let i = ue_vm::ref_instance(o).unwrap();
    let (tick, frob) = (nm(&mut vm, "Tick"), nm(&mut vm, "Frob"));
    vm.call_by_name(o, "Disable", vec![tick.clone()]).unwrap();
    vm.call_by_name(o, "Disable", vec![frob]).unwrap();
    let Value::Name(t) = tick else { unreachable!() };
    assert_eq!(vm.objects[i].disabled.iter().copied().collect::<Vec<_>>(), vec![t]);
    // a real state change turns it on again
    let other = nm(&mut vm, "BB");
    vm.call_by_name(o, "GotoState", vec![other, Value::Void]).unwrap();
    assert_eq!(vm.state_name(o), Some("BB"));
    assert!(vm.objects[i].disabled.is_empty());
}

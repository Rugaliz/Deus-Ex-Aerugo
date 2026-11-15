//! Runs real game script functions in the VM. Set `DEUSEX_DIR` to the game folder; skipped otherwise.
//! Expected values were derived by hand from the original script source.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::{Value, Vm};

fn vm() -> Option<Vm> {
    let dir = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    Some(Vm::new(Registry::load_game(&dir).expect("load game")))
}

fn s(x: &str) -> Value {
    Value::Str(x.into())
}

#[test]
fn every_function_compiles() {
    let Some(mut vm) = vm() else { return };
    let (rep, _) = vm.compile_all();
    assert_eq!(rep.failed, 0, "{:?}", rep.errors);
    assert!(rep.functions > 8000);
}

#[test]
fn string_loops_and_natives() {
    let Some(mut vm) = vm() else { return };
    // for-loop over Len/Mid, string concatenation
    assert_eq!(vm.call_static("ConPlayBase", "ConvertSpaces", vec![s("a b  c")]).unwrap(), s("a_b__c"));
    assert_eq!(vm.call_static("ConPlayBase", "ConvertSpaces", vec![s("")]).unwrap(), s(""));
    // while loops over InStr/Left/Right/Len
    assert_eq!(vm.call_static("UBrowserIRCLink", "Chop", vec![s(": hello !")]).unwrap(), s("hello"));
    assert_eq!(vm.call_static("UBrowserIRCLink", "Chop", vec![s("::!")]).unwrap(), s(""));
    // static function calling InStr(Mid(..))
    assert_eq!(vm.call_static("UWindowBase", "InStrAfter", vec![s("hello world"), s("o"), Value::Int(5)]).unwrap(), Value::Int(7));
    assert_eq!(vm.call_static("UWindowBase", "InStrAfter", vec![s("hello"), s("z"), Value::Int(0)]).unwrap(), Value::Int(-1));
    // optional parameter omitted
    let pd = |vm: &mut Vm, n: i32| vm.call_static("UBrowserBufferedTCPLink", "ParseDelimited", vec![s("a,b,c"), s(","), Value::Int(n)]).unwrap();
    assert_eq!(pd(&mut vm, 1), s("a"));
    assert_eq!(pd(&mut vm, 2), s("b"));
    assert_eq!(pd(&mut vm, 3), s("c"));
}

#[test]
fn int_float_conversions_and_math() {
    let Some(mut vm) = vm() else { return };
    // Min/abs with implicit int<->float casts
    assert_eq!(vm.call_static("PlayerPawn", "CompressAccel", vec![Value::Int(200)]).unwrap(), Value::Int(127));
    assert_eq!(vm.call_static("PlayerPawn", "CompressAccel", vec![Value::Int(5)]).unwrap(), Value::Int(5));
    assert_eq!(vm.call_static("PlayerPawn", "CompressAccel", vec![Value::Int(-5)]).unwrap(), Value::Int(133));
    assert_eq!(vm.call_static("PlayerPawn", "CompressAccel", vec![Value::Int(-300)]).unwrap(), Value::Int(255));
    // float formatting, compound assignment on a parameter, int()/string() casts
    let ff = |vm: &mut Vm, v: f32, p: f32| vm.call_static("DeusExWeapon", "FormatFloatString", vec![Value::Float(v), Value::Float(p)]).unwrap();
    assert_eq!(ff(&mut vm, 3.75, 0.1), s("3.7"));
    assert_eq!(ff(&mut vm, 2.25, 0.5), s("2.0"));
    assert_eq!(ff(&mut vm, 1.0, 1.0), s("1"));
    assert_eq!(ff(&mut vm, 1.0, 0.0), s("ERR"));
    let bp = |vm: &mut Vm, v: f32| vm.call_static("DeusExWeapon", "BuildPercentString", vec![Value::Float(v)]).unwrap();
    assert_eq!(bp(&mut vm, 0.25), s("(+25%)"));
    assert_eq!(bp(&mut vm, -0.5), s("(-50%)"));
    assert_eq!(bp(&mut vm, 0.0), s("(+0%)"));
    // float to string then truncate after the decimal point
    assert_eq!(vm.call_static("Console", "FormatFloat", vec![Value::Float(3.14159)]).unwrap(), s("3.14"));
}

#[test]
fn switch_bool_and_name_logic() {
    let Some(mut vm) = vm() else { return };
    let idx = |vm: &mut Vm, n: i32| vm.call_static("HUDKeypadWindow", "IndexToString", vec![Value::Int(n)]).unwrap();
    assert_eq!(idx(&mut vm, 9), s("*"));
    assert_eq!(idx(&mut vm, 10), s("0"));
    assert_eq!(idx(&mut vm, 11), s("#"));
    assert_eq!(idx(&mut vm, 0), s("1"));
    assert_eq!(idx(&mut vm, 4), s("5"));
    // switch on a string expression with Caps()
    let hx = |vm: &mut Vm, d: &str| vm.call_static("UWindowHTMLTextArea", "GetHexDigit", vec![s(d)]).unwrap();
    assert_eq!(hx(&mut vm, "a"), Value::Int(10));
    assert_eq!(hx(&mut vm, "F"), Value::Int(15));
    assert_eq!(hx(&mut vm, "7"), Value::Int(7));
    assert_eq!(hx(&mut vm, "g"), Value::Int(0));
    // short-circuit boolean logic
    let ok = |vm: &mut Vm, n: i32| vm.call_static("HUDObjectBelt", "IsValidPos", vec![Value::Int(n)]).unwrap();
    assert_eq!(ok(&mut vm, 5), Value::Bool(true));
    assert_eq!(ok(&mut vm, 10), Value::Bool(false));
    assert_eq!(ok(&mut vm, -1), Value::Bool(false));
    // name comparisons chained with ||, float returns
    let fl = vm.name_value("Flamed");
    let tg = vm.name_value("TearGas");
    assert_eq!(vm.call_static("WaltonSimons", "ShieldDamage", vec![fl]).unwrap(), Value::Float(0.0));
    assert_eq!(vm.call_static("WaltonSimons", "ShieldDamage", vec![tg]).unwrap(), Value::Float(0.1));
}

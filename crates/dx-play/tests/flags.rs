//! The flag base (`FlagBase`, Extension.dll `XFlagBase`) as scripts use it. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

fn new_game() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    Some(s)
}

/// `DeusExRootWindow.ResetFlags` (run when a new game starts) walks the flags with an iterator and deletes the flag
/// before the current one. The original's iterator follows the hash chains live, so every bool flag without
/// "SKTemp_" goes; an iterator that counts positions would skip every other one.
#[test]
fn reset_flags_deletes_every_ordinary_bool_flag() {
    let Some(mut s) = new_game() else { return };
    let p = s.player;
    let base = match s.vm.get_prop(p, "flagBase", 0) { Ok(Value::Obj(b)) => b, _ => panic!("no flag base") };
    let root = match s.vm.get_prop(p, "rootWindow", 0) { Ok(Value::Obj(r)) => r, _ => panic!("no root window") };
    let names: Vec<String> = (0..40).map(|i| format!("TestFlag{i}")).chain(["SKTemp_Keep".to_string()]).collect();
    for n in &names {
        let id = s.vm.reg.names.intern(n);
        s.vm.call_by_name(base, "SetBool", vec![Value::Name(id), Value::Bool(true)]).unwrap();
    }
    s.vm.call_by_name(root, "ResetFlags", vec![]).unwrap();
    let mut left = Vec::new();
    for n in &names {
        let id = s.vm.reg.names.intern(n);
        if s.vm.call_by_name(base, "CheckFlag", vec![Value::Name(id), Value::Byte(0)]).unwrap() == Value::Bool(true) {
            left.push(n.clone());
        }
    }
    assert_eq!(left, vec!["SKTemp_Keep".to_string()], "flags left after ResetFlags");
}

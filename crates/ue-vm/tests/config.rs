//! Class defaults from ini files and localised text. Set `DEUSEX_DIR` to the game folder.
//! Expected values are read from the shipped ini and .int files themselves, not from the code under test.

use std::path::PathBuf;
use ue_config::Config;
use ue_registry::Registry;
use ue_vm::{Value, Vm};

fn vm(with_config: bool) -> Option<(Vm, PathBuf)> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut reg = Registry::load_game(&g).unwrap();
    if with_config {
        reg.set_config(Config::new(&g.join("System"), None));
    }
    let mut vm = Vm::new(reg);
    vm.init_world().unwrap();
    Some((vm, g))
}

fn text(_vm: &Vm, v: Value) -> String {
    match v {
        Value::Str(s) => s,
        v => panic!("{v:?}"),
    }
}

#[test]
fn config_properties_take_the_value_of_the_ini() {
    let Some((mut vm, g)) = vm(true) else { return };
    // what the shipped files say (DefUser.ini under User.ini)
    let mut c = Config::new(&g.join("System"), None);
    c.load("User");
    let want = c.value("User", "DeusEx.DeusExPlayer", "bAlwaysRun").expect("bAlwaysRun in the ini");
    let class = vm.reg.class_by_name("JCDentonMale").unwrap();
    let o = vm.new_object(class).unwrap();
    assert_eq!(vm.get_prop(o, "bAlwaysRun", 0).unwrap(), Value::Bool(want.eq_ignore_ascii_case("true")));
    // and differs from the package default where the ini says otherwise
    let (mut plain, _) = self::vm(false).unwrap();
    let pc = plain.reg.class_by_name("JCDentonMale").unwrap();
    let po = plain.new_object(pc).unwrap();
    let fov: f32 = c.value("User", "Engine.PlayerPawn", "DesiredFOV").unwrap().parse().unwrap();
    assert_eq!(vm.get_prop(o, "DesiredFOV", 0).unwrap(), Value::Float(fov));
    assert_ne!(plain.get_prop(po, "DesiredFOV", 0).unwrap(), Value::Float(fov));
    let sens: f32 = c.value("User", "Engine.PlayerPawn", "MouseSensitivity").unwrap().parse().unwrap();
    assert_eq!(vm.get_prop(o, "MouseSensitivity", 0).unwrap(), Value::Float(sens));
    // the system ini reaches classes of the other config file
    let mut s = Config::new(&g.join("System"), None);
    s.load("System");
    assert!(s.has_section("System", "Engine.GameInfo"));
}

#[test]
fn localized_text_comes_from_the_int_file() {
    let Some((mut vm, g)) = vm(true) else { return };
    let mut c = Config::new(&g.join("System"), None);
    let want = ue_config::unquote(&c.localized("DeusEx", "DeusExPlayer", "InventoryFull").expect("in DeusEx.int"));
    let class = vm.reg.class_by_name("DeusExPlayer").unwrap();
    let o = vm.new_object(class).unwrap();
    assert_eq!(text(&vm, vm.get_prop(o, "InventoryFull", 0).unwrap()), want);
    // the .int text wins over the script's own default (which here differs by a trailing space)
    let want = ue_config::unquote(&c.localized("Engine", "GameInfo", "NameChangedMessage").expect("in Engine.int"));
    let class = vm.reg.class_by_name("GameInfo").unwrap();
    let o = vm.new_object(class).unwrap();
    assert_eq!(text(&vm, vm.get_prop(o, "NameChangedMessage", 0).unwrap()), want);
    let (mut plain, _) = self::vm(false).unwrap();
    let class = plain.reg.class_by_name("GameInfo").unwrap();
    let o = plain.new_object(class).unwrap();
    assert_ne!(text(&plain, plain.get_prop(o, "NameChangedMessage", 0).unwrap()), want);
}

#[test]
fn console_get_set_localize_and_save_config() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let usr = std::env::temp_dir().join(format!("dx-config-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&usr);
    let mut reg = Registry::load_game(&g).unwrap();
    reg.set_config(Config::new(&g.join("System"), Some(&usr)));
    let mut vm = Vm::new(reg);
    vm.init_world().unwrap();
    let class = vm.reg.class_by_name("JCDentonMale").unwrap();
    let o = vm.new_object(class).unwrap();
    // (a pawn without a viewport answers "" to everything: the engine's route, as `Actor.ConsoleCommand` takes it)
    let cmd = |vm: &mut Vm, _o: u32, s: &str| vm.console_command_actor(s).unwrap();
    assert_eq!(cmd(&mut vm, o, "get DeusExPlayer bAlwaysRun"), "True");
    // set: the live object and later objects change, the user layer is written, the game folder is not
    assert_eq!(cmd(&mut vm, o, "set DeusExPlayer bAlwaysRun False"), "");
    assert_eq!(vm.get_prop(o, "bAlwaysRun", 0).unwrap(), Value::Bool(false));
    let o2 = vm.new_object(class).unwrap();
    assert_eq!(vm.get_prop(o2, "bAlwaysRun", 0).unwrap(), Value::Bool(false));
    assert_eq!(cmd(&mut vm, o, "get DeusExPlayer bAlwaysRun"), "False");
    let saved = std::fs::read_to_string(usr.join("User.ini")).expect("user ini written");
    assert!(saved.contains("[DeusEx.DeusExPlayer]") && saved.to_ascii_lowercase().contains("balwaysrun=false"), "{saved}");
    // a new session reading that directory sees the change
    let mut reg2 = Registry::load_game(&g).unwrap();
    reg2.set_config(Config::new(&g.join("System"), Some(&usr)));
    let mut vm2 = Vm::new(reg2);
    vm2.init_world().unwrap();
    let c2 = vm2.reg.class_by_name("JCDentonMale").unwrap();
    let p2 = vm2.new_object(c2).unwrap();
    assert_eq!(vm2.get_prop(p2, "bAlwaysRun", 0).unwrap(), Value::Bool(false));
    // Localize and GetConfig natives
    let want = ue_config::unquote(&Config::new(&g.join("System"), None).localized("DeusEx", "DeusExPlayer", "InventoryFull").unwrap());
    let r = vm.call_by_name(o, "Localize", vec![Value::Str("DeusExPlayer".into()), Value::Str("InventoryFull".into()), Value::Str("DeusEx".into())]).unwrap();
    assert_eq!(r, Value::Str(want));
    let r = vm.call_by_name(o, "Localize", vec![Value::Str("Nope".into()), Value::Str("Missing".into()), Value::Str("DeusEx".into())]).unwrap();
    // the engine names the last file it tried (`int`)
    assert_eq!(r, Value::Str("<?int?DeusEx.Nope.Missing?>".into()));
    let r = vm.call_by_name(o, "GetConfig", vec![Value::Str("Engine.Engine".into()), Value::Str("GameRenderDevice".into())]).unwrap();
    assert!(matches!(r, Value::Str(ref s) if s.contains("Drv")), "{r:?}");
    // SaveConfig of an object writes its config properties
    vm.set_prop(o, "MouseSensitivity", 0, Value::Float(7.5)).unwrap();
    vm.call_by_name(o, "SaveConfig", vec![]).unwrap();
    let saved = std::fs::read_to_string(usr.join("User.ini")).unwrap();
    assert!(saved.contains("MouseSensitivity=7.5"), "{saved}");
    let _ = std::fs::remove_dir_all(&usr);
    // the game folder's own file is untouched
    assert!(!std::fs::read_to_string(g.join("System/User.ini")).unwrap().contains("MouseSensitivity=7.5"));
}

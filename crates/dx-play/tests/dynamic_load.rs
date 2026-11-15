//! `DynamicLoadObject` as Core's `StaticLoadObject` runs it (`ue_vm::load_object`, WORKLOG 3.140). Set `DEUSEX_DIR`.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

fn session() -> Option<Session> {
    let g = std::env::var_os("DEUSEX_DIR").map(PathBuf::from)?;
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    for _ in 0..3 {
        s.step_bound(1.0 / 30.0);
    }
    Some(s)
}

/// `DynamicLoadObject(name, class'<class_path>', may_fail)` called as the scripts call it; the result's path.
fn load(s: &mut Session, name: &str, class_path: &str, may_fail: bool) -> Option<String> {
    let class = s.vm.reg.obj_id(class_path).0;
    let p = s.player;
    match s.vm.call_by_name(p, "DynamicLoadObject", vec![Value::Str(name.into()), Value::Obj(class), Value::Bool(may_fail)]).unwrap() {
        Value::Obj(0) => None,
        Value::Obj(r) => Some(s.vm.obj_to_string(r)),
        v => panic!("{v:?}"),
    }
}

#[test]
fn the_linker_wants_the_exact_class_in_any_group() {
    let Some(mut s) = session() else { return };
    // a FireTexture inside the group Electricity: the group may be left out or be wrong
    let want = Some("Effects.Electricity.Nano_SFX".to_string());
    assert_eq!(load(&mut s, "Effects.Nano_SFX", "Fire.FireTexture", false), want);
    assert_eq!(load(&mut s, "Effects.NoSuchGroup.Nano_SFX", "Fire.FireTexture", false), want);
    // asked as a Texture: no export of that exact class, and the object is not directly in the package
    assert_eq!(load(&mut s, "Effects.Nano_SFX", "Engine.Texture", true), None);
    // a plain Texture in a group is found as a Texture
    assert_eq!(load(&mut s, "Effects.bluewater", "Engine.Texture", false).as_deref(), Some("Effects.water.bluewater"));
    // a Mesh asked for is tried again as a LodMesh
    assert_eq!(load(&mut s, "DeusExCharacters.RatCarcass", "Engine.Mesh", false).as_deref(), Some("DeusExCharacters.RatCarcass"));
    assert_eq!(load(&mut s, "DeusExCharacters.RatCarcass", "Engine.LodMesh", false).as_deref(), Some("DeusExCharacters.RatCarcass"));
}

#[test]
fn classes_config_names_and_failures() {
    let Some(mut s) = session() else { return };
    assert_eq!(load(&mut s, "DeusEx.JCDentonMale", "Core.Class", false).as_deref(), Some("DeusEx.JCDentonMale"));
    // asked as an Object: no export of class Object, but the class is in memory directly inside its package
    assert_eq!(load(&mut s, "DeusEx.JCDentonMale", "Core.Object", false).as_deref(), Some("DeusEx.JCDentonMale"));
    assert_eq!(load(&mut s, "DeusEx.JCDentonMale", "Engine.Texture", true), None);
    // `ini:Section.Key`: the value of the system configuration names the object (section split at the last dot)
    let game = s.vm.reg.config_value("System", "Engine.Engine", "DefaultGame").expect("DefaultGame configured");
    assert_eq!(load(&mut s, "ini:Engine.Engine.DefaultGame", "Core.Class", false), Some(game));
    let before = s.vm.log.len();
    assert_eq!(load(&mut s, "usr:NoSuchSection.NoSuchKey", "Core.Class", false), None);
    // no package, a package without a file, an empty name
    assert_eq!(load(&mut s, "JCDentonMale", "Core.Class", false), None);
    assert_eq!(load(&mut s, "NoSuchPackageAnywhere.Foo", "Core.Class", false), None);
    assert_eq!(load(&mut s, "DeusEx.", "Core.Class", false), None);
    // without MayFail the failures go to the log (`LOAD_NoWarn`), with it they are quiet
    assert_eq!(s.vm.log.len(), before + 4, "{:?}", &s.vm.log[before..]);
    assert!(s.vm.log[before + 2].starts_with("Failed to load 'Class NoSuchPackageAnywhere.Foo': Can't find file for package"), "{}", s.vm.log[before + 2]);
    load(&mut s, "NoSuchPackageAnywhere.Foo", "Core.Class", true);
    assert_eq!(s.vm.log.len(), before + 4);
}

#[test]
fn actors_of_the_map() {
    let Some(mut s) = session() else { return };
    // a map actor: its export in the map package gives the live actor; the map file keeps actors directly in the
    // package, so the search in memory finds it as any class it is too
    let (actor, path) = s.vm.level_paths.iter().map(|(&r, p)| (r, p.clone())).find(|(r, _)| s.vm.is_live(*r) && s.vm.class_of_ref(*r).is_some_and(|c| s.vm.reg.class(c).path == "Engine.Light")).expect("a Light in the map");
    assert_eq!(load(&mut s, &path, "Engine.Light", false), Some(s.vm.obj_to_string(actor)));
    assert_eq!(load(&mut s, &path, "Engine.Actor", false), Some(s.vm.obj_to_string(actor)));
    assert_eq!(load(&mut s, &path, "Engine.Pawn", true), None);
    // destroyed (a light cannot be: a decoration): nothing (the port keeps no destroyed object around)
    let deco = s.vm.reg.class_by_name("Decoration").unwrap();
    // the first destroyable one by reference (the paths are a hash map: its order changes from run to run)
    let mut decos: Vec<(u32, String)> = s.vm.level_paths.iter().map(|(&r, p)| (r, p.clone())).filter(|(r, _)| s.vm.is_live(*r) && s.vm.class_of_ref(*r).is_some_and(|c| s.vm.is_a(c, deco))).collect();
    decos.sort();
    let (d, dpath) = decos.into_iter().find(|(r, _)| !s.vm.bp(*r, "bStatic") && !s.vm.bp(*r, "bNoDelete")).expect("a decoration in the map");
    assert!(load(&mut s, &dpath, "Engine.Actor", false).is_some());
    s.vm.call_by_name(d, "Destroy", vec![]).unwrap();
    assert!(!s.vm.is_live(d));
    assert_eq!(load(&mut s, &dpath, "Engine.Actor", true), None);
    // a spawned actor sits directly in the map package: found as any class it is
    let p = s.player;
    let player = s.vm.obj_to_string(p);
    assert!(player.starts_with("01_NYC_UNATCOIsland."), "{player}");
    assert_eq!(load(&mut s, &player, "Engine.Actor", false), Some(player.clone()));
    assert_eq!(load(&mut s, &player, "Engine.Light", true), None);
}

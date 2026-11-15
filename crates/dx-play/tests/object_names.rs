//! Objects' `Name` (Core `MakeUniqueObjectName`, map names) and the header members of package objects (WORKLOG 3.138).
//! Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

fn name_of(s: &Session, v: Value) -> String {
    match v {
        Value::Name(n) => s.vm.reg.names.str(n).to_string(),
        other => panic!("{other:?}"),
    }
}

/// `AugmentationCannister.GetAugmentation` matches `AddAugs[i] == anAug.Class.Name`: a class object's `Name` is its
/// own name. Map actors keep the map's names; spawned actors get the class name and the class's counter, skipping
/// names in use.
#[test]
fn objects_have_their_engine_names() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    // a map actor
    let (a, path) = s.vm.level_paths.iter().next().map(|(a, p)| (*a, p.clone())).unwrap();
    assert_eq!(name_of(&s, s.vm.get_prop(a, "Name", 0).unwrap()), path.rsplit('.').next().unwrap());
    // two spawned flares: different names of the form Flare<n>, neither a map actor's
    let c = s.vm.reg.class_by_name("Flare").unwrap();
    let cls = Value::Obj(s.vm.reg.obj_id(&s.vm.reg.class(c).path.clone()).0);
    let mut names = Vec::new();
    for _ in 0..2 {
        let f = match s.vm.call_by_name(s.player, "Spawn", vec![cls.clone()]) {
            Ok(Value::Obj(f)) if f != 0 => f,
            other => panic!("{other:?}"),
        };
        names.push(name_of(&s, s.vm.get_prop(f, "Name", 0).unwrap()));
    }
    assert_ne!(names[0], names[1]);
    let map_names: Vec<String> = s.vm.level_paths.values().map(|p| p.rsplit('.').next().unwrap().to_string()).collect();
    for n in &names {
        assert!(n.starts_with("Flare") && n[5..].parse::<u32>().is_ok(), "{n}");
        assert!(!map_names.contains(n), "{n} is a map actor's name");
    }
    // the augmentation cannister finds its augmentation by the class's name
    let c = s.vm.reg.class_by_name("AugmentationCannister").unwrap();
    let cls = Value::Obj(s.vm.reg.obj_id(&s.vm.reg.class(c).path.clone()).0);
    let can = match s.vm.call_by_name(s.player, "Spawn", vec![cls]) {
        Ok(Value::Obj(x)) if x != 0 => x,
        other => panic!("{other:?}"),
    };
    s.vm.set_prop(can, "Owner", 0, Value::Obj(s.player)).unwrap();
    let want = s.vm.name_value("AugSpeed");
    s.vm.set_prop(can, "AddAugs", 0, want).unwrap();
    let aug = s.vm.call_by_name(can, "GetAugmentation", vec![Value::Int(0)]).unwrap();
    let Value::Obj(aug) = aug else { panic!() };
    assert_ne!(aug, 0, "no augmentation found by class name");
    let ac = s.vm.class_of(aug).unwrap();
    assert_eq!(s.vm.reg.names.str(s.vm.reg.class(ac).name), "AugSpeed");
}

/// `ObjectToString` is the path name: an actor's map package and name (`01_NYC_UNATCOIsland.JCDentonMale0`-style).
#[test]
fn actors_print_as_their_path() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    s.step_bound(1.0 / 30.0);
    let (a, path) = s.vm.level_paths.iter().next().map(|(a, p)| (*a, p.clone())).unwrap();
    assert_eq!(s.vm.obj_to_string(a), path);
    assert_eq!(s.vm.obj_to_string(0), "None");
}

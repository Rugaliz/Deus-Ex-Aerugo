//! Reading a saved game made by the ORIGINAL game (see `tools/oracle`): `out/wine/saves/vanilla_training/00_training.dxs`.
use std::path::PathBuf;
use ue_vm::Value;

#[test]
fn the_level_of_an_original_save_loads() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out/wine/saves/vanilla_training/00_training.dxs");
    if !file.exists() {
        return;
    }
    let mut reg = ue_registry::Registry::load_game(&game).unwrap();
    reg.set_config(ue_config::Config::new(&game.join("System"), None));
    let mut vm = ue_vm::Vm::new(reg);
    vm.init_world().unwrap();
    let pi = vm.reg.add_package(&file, "00_training").unwrap();
    let load = vm.load_level_opts(pi, false).unwrap();
    let live = vm.live_actors();
    let mut hist: std::collections::BTreeMap<String, usize> = Default::default();
    for &a in &live {
        let c = vm.class_of(a).map(|c| vm.reg.class(c).path.clone()).unwrap_or_default();
        *hist.entry(c).or_default() += 1;
    }
    eprintln!("{} actors loaded from the save, {} issues", load.actors.len(), load.issues.len());
    for c in ["DeusEx.JCDentonMale", "Engine.LevelInfo", "DeusEx.UNATCOTroop", "DeusEx.WeaponPistol"] {
        eprintln!("  {c}: {:?}", hist.get(c));
    }
    let level = vm.level;
    eprintln!("level time {:?}", vm.get_prop(level, "TimeSeconds", 0));
    if let Some(&p) = live.iter().find(|&&a| vm.class_of(a).map(|c| vm.reg.class(c).path.ends_with("JCDentonMale")).unwrap_or(false)) {
        eprintln!("player at {:?} health {:?} rotation {:?}", vm.location(p), vm.get_prop(p, "Health", 0), vm.get_prop(p, "Rotation", 0));
        assert!(matches!(vm.get_prop(p, "Health", 0), Ok(Value::Int(h)) if h > 0));
    }
    assert!(load.actors.len() > 400);
}

/// What kinds of objects a saved game holds: the actors of the level table and everything else.
#[test]
#[ignore]
fn survey_the_objects_of_an_original_save() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let Some(file) = std::env::var_os("DX_IMPORT_SAVE").map(PathBuf::from).or_else(save_file) else { return };
    let mut reg = ue_registry::Registry::load_game(&game).unwrap();
    let pi = reg.add_package(&file, "00_training").unwrap();
    let pkg = &reg.packages[pi].pkg;
    let lv = (0..pkg.exports.len()).find(|&i| pkg.export_class_name(i) == "Level").unwrap();
    let level = ue_assets::level::Level::load(pkg, lv).unwrap();
    let actors: std::collections::HashSet<usize> = level.actors.iter().filter_map(|&a| match ue_package::ObjectRef::from_index(a) { ue_package::ObjectRef::Export(e) => Some(e), _ => None }).collect();
    let mut inside: std::collections::BTreeMap<String, usize> = Default::default();
    let mut outside: std::collections::BTreeMap<String, usize> = Default::default();
    for i in 0..pkg.exports.len() {
        let c = pkg.export_class_name(i);
        *(if actors.contains(&i) { &mut inside } else { &mut outside }).entry(c).or_default() += 1;
    }
    let _ = &inside;
    eprintln!("NOT in it: {outside:?}");
    eprintln!("travel list / level fields: actors {}", level.actors.len());
}

fn save_file() -> Option<PathBuf> {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out/wine/saves/vanilla_training/00_training.dxs");
    file.exists().then_some(file)
}

/// Continuing a game saved by the original: the player, its belongings and flags are back and the world runs.
#[test]
fn an_original_save_can_be_continued() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let Some(file) = save_file() else { return };
    let mut s = dx_play::Session::open(&game, "DX").unwrap();
    let report = s.import_original_save(&file).expect("import");
    eprintln!("flags {}, states {}, state problems {:?}", report.flags, report.states, report.state_problems);
    eprintln!("map {} player at {:?} health {:?} state {:?}", s.map_name, s.vm.location(s.player), s.vm.get_prop(s.player, "Health", 0), s.vm.state_name(s.player));
    assert_eq!(s.map_name, "00_Training");
    assert!(report.flags > 0, "no flags were imported");
    assert!(matches!(s.vm.get_prop(s.player, "Health", 0), Ok(Value::Int(h)) if h > 0));
    let class = |s: &mut dx_play::Session, o: u32| s.vm.class_of(o).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
    let pl = s.player;
    assert!(class(&mut s, pl).ends_with("JCDentonMale"));
    // the inventory chain
    let mut items = Vec::new();
    let mut cur = match s.vm.get_prop(s.player, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    while cur != 0 && items.len() < 50 {
        items.push(class(&mut s, cur));
        cur = match s.vm.get_prop(cur, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    }
    eprintln!("inventory {items:?}");
    assert!(!items.is_empty(), "the saved inventory is gone");
    // the world goes on without script errors
    s.issues.clear();
    s.vm.log.clear();
    for _ in 0..120 {
        s.step_bound(1.0 / 30.0);
    }
    for i in s.issues.iter().take(10) {
        eprintln!("issue {} {}: {}", i.class, i.event, i.error);
    }
    for l in s.vm.log.iter().filter(|l| l.contains("Accessed") || l.contains("failed")).take(10) {
        eprintln!("log {l}");
    }
    assert!(s.issues.len() < 5, "{} script errors after the import", s.issues.len());
    assert_ne!(s.vm.ui_root(s.player), 0, "no root window after the import");
}

/// A picture after the import; needs a Vulkan device.
#[test]
#[ignore]
fn shot_of_an_imported_original_save() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let Some(file) = std::env::var_os("DX_IMPORT_SAVE").map(PathBuf::from).or_else(save_file) else { return };
    let mut s = dx_play::Session::open(&game, "DX").unwrap();
    s.import_original_save(&file).expect("import");
    for _ in 0..60 {
        s.step_bound(1.0 / 30.0);
    }
    let mut r = None;
    let img = s.render_vk(&mut r, 800, 600).expect("vulkan render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    let name = if std::env::var_os("DX_IMPORT_SAVE").is_some() { "s10_import_probe.png" } else { "s10_import_training.png" };
    std::fs::write(dir.join(name), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    r.take().unwrap().destroy();
}

/// Probe with any save file: `DX_IMPORT_SAVE=<file.dxs>`; prints what the import makes of it.
#[test]
#[ignore]
fn probe_importing_any_save() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let Some(file) = std::env::var_os("DX_IMPORT_SAVE").map(PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&game, "DX").unwrap();
    let report = s.import_original_save(&file);
    let report = match report { Ok(r) => r, Err(e) => panic!("import failed: {e}") };
    eprintln!("flags {}, states {}, state problems {:?}", report.flags, report.states, &report.state_problems[..report.state_problems.len().min(5)]);
    eprintln!("map {} player at {:?} health {:?} state {:?}", s.map_name, s.vm.location(s.player), s.vm.get_prop(s.player, "Health", 0), s.vm.state_name(s.player));
    eprintln!("list objects {}", report.list_objects);
    let pawns = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.reg.class(c).path.ends_with("JCDentonMale")).unwrap_or(false)).count();
    eprintln!("player pawns {pawns}; login: {:?}", s.vm.log.iter().filter(|l| l.contains("Login")).collect::<Vec<_>>());
    for (head, next) in [("FirstNote", "next"), ("FirstGoal", "next"), ("FirstLog", "next")] {
        let mut n = 0;
        let mut cur = match s.vm.get_prop(s.player, head, 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        while cur != 0 && n < 500 {
            n += 1;
            cur = match s.vm.get_prop(cur, next, 0) { Ok(Value::Obj(o)) if s.vm.is_live(o) => o, _ => 0 };
        }
        eprintln!("{head}: {n} entries");
    }
    s.issues.clear();
    for _ in 0..300 {
        s.step_bound(1.0 / 30.0);
    }
    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    for i in &s.issues {
        *kinds.entry(format!("{} {}: {}", i.class, i.event, i.error)).or_default() += 1;
    }
    eprintln!("{} script errors after 10 s", s.issues.len());
    for (k, n) in kinds.iter().take(10) {
        eprintln!("  x{n} {k}");
    }
    eprintln!("player at {:?} health {:?}", s.vm.location(s.player), s.vm.get_prop(s.player, "Health", 0));
}

/// The flag table of an original save, read straight from its objects (`FlagBase.hashTable[64]` heads and each flag's
/// `flagHash`/`nextFlag`), matches the port: every stored hash equals `flag_hash` of the name, and walking the
/// buckets gives the same order as the port's `FlagStore`. Uses the vanilla save, and `DX_IMPORT_SAVE` when set.
#[test]
fn flag_table_order_matches_an_original_save() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let files: Vec<PathBuf> = save_file().into_iter().chain(std::env::var_os("DX_IMPORT_SAVE").map(PathBuf::from)).collect();
    for file in files {
        let mut reg = ue_registry::Registry::load_game(&game).unwrap();
        let pi = reg.add_package(&file, "flagcheck").unwrap();
        let n = reg.packages[pi].pkg.exports.len();
        // (object name, flag name, type, stored hash, next flag object name)
        let mut flags: std::collections::HashMap<String, (String, u8, u32, String)> = Default::default();
        let mut heads: Vec<String> = Vec::new();
        for i in 0..n {
            let cname = reg.packages[pi].pkg.export_class_name(i).to_string();
            let ty = match cname.as_str() {
                "FlagBool" => 0u8,
                "FlagByte" => 1,
                "FlagInt" => 2,
                "FlagFloat" => 3,
                "FlagName" => 4,
                "FlagVector" => 5,
                "FlagRotator" => 6,
                "FlagBase" => 99,
                _ => continue,
            };
            let Some(class) = reg.class_of_export(pi, i) else { continue };
            let (inst, _) = reg.instantiate_object(class, pi, i).unwrap();
            let u32_at = |o: u32| u32::from_le_bytes(inst.data[o as usize..o as usize + 4].try_into().unwrap());
            let obj_name = |reg: &ue_registry::Registry, id: u32| if id == 0 { String::new() } else { reg.obj_path(ue_registry::ObjId(id)).rsplit('.').next().unwrap().to_string() };
            let me = { let pk = &reg.packages[pi].pkg; pk.name(pk.exports[i].object_name).to_string() };
            if ty == 99 {
                if !heads.is_empty() {
                    continue; // the first flag base (the player's)
                }
                let p = reg.find_prop(class, "hashTable").unwrap().clone();
                heads = (0..64).map(|k| obj_name(&reg, u32_at(p.offset + 4 * k))).collect();
                continue;
            }
            let off = |reg: &ue_registry::Registry, n: &str| reg.find_prop(class, n).unwrap().offset;
            let (o_name, o_hash, o_next) = (off(&reg, "flagName"), off(&reg, "flagHash"), off(&reg, "nextFlag"));
            let name = reg.names.str(u32_at(o_name)).to_string();
            flags.insert(me, (name, ty, u32_at(o_hash), obj_name(&reg, u32_at(o_next))));
        }
        // walk the original's chains
        let mut original: Vec<(String, u8)> = Vec::new();
        for (b, head) in heads.iter().enumerate() {
            let mut cur = head.clone();
            while let Some((name, ty, hash, next)) = flags.get(&cur) {
                assert_eq!(*hash, ue_vm::flag_hash(name), "hash of {name}");
                assert_eq!((*hash & 0x3f) as usize, b, "bucket of {name}");
                original.push((name.to_ascii_lowercase(), *ty));
                cur = next.clone();
            }
        }
        let mut store = ue_vm::FlagStore::default();
        for (name, ty) in &original {
            store.add(ue_vm::Flag { name: 0, key: name.clone(), ty: *ty, value: Value::Void, expiration: 0 });
        }
        let ours: Vec<(String, u8)> = store.flags.iter().map(|f| (f.key.clone(), f.ty)).collect();
        eprintln!("{}: {} flags in {} objects, chains walk {} of them", file.display(), original.len(), flags.len(), original.len());
        assert!(original.len() >= 2);
        assert_eq!(ours, original, "table order differs");
    }
}

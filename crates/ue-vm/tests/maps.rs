//! Loading saved maps into the VM. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::{ref_instance, Value, Vm};

fn game() -> Option<PathBuf> {
    std::env::var_os("DEUSEX_DIR").map(PathBuf::from)
}

fn load(reg: Registry, game: &std::path::Path, map: &str) -> (Vm, ue_vm::level::LevelLoad) {
    let mut vm = Vm::new(reg);
    vm.init_world().unwrap();
    let pi = vm.reg.add_package(&game.join("Maps").join(format!("{map}.dx")), map).unwrap();
    let l = vm.load_level(pi).unwrap();
    (vm, l)
}

#[test]
fn training_map_comes_to_life() {
    let Some(g) = game() else { return };
    let (vm, l) = load(Registry::load_game(&g).unwrap(), &g, "00_Training");
    // the actors loaded for a client (`RF_LoadForClient`): the map's 996 editor-only CSG brushes are not
    assert_eq!(l.actors.len(), 312);
    assert!(l.property_warnings.is_empty());
    // slot 0 is the map's LevelInfo and became the world's level; every actor points at it
    assert_eq!(vm.level, l.actors[0]);
    for &a in &l.actors {
        assert_eq!(vm.get_prop(a, "Level", 0).unwrap(), Value::Obj(vm.level));
    }
    // references between saved actors were linked: some actor property points at another map actor
    let mut linked = 0;
    for &a in &l.actors {
        let class = vm.class_of(a).unwrap();
        for p in vm.reg.all_props(class) {
            if p.kind == ue_package::object::PropertyKind::Object && p.dim == 1 {
                if let Ok(Value::Obj(r)) = vm.get_prop(a, vm.reg.names.str(p.name), 0) {
                    if let Some(i) = ref_instance(r) {
                        assert!(i < vm.objects.len());
                        if l.actors.contains(&r) && r != vm.level && r != a {
                            linked += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(linked > 50, "only {linked} links between actors");
    // every start-up event ran without a script error
    assert!(l.issues.is_empty(), "{:?}", &l.issues[..l.issues.len().min(3)]);
}

#[test]
fn every_map_loads_without_property_warnings() {
    let Some(g) = game() else { return };
    let mut maps: Vec<String> = std::fs::read_dir(g.join("Maps"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "dx").unwrap_or(false))
        .map(|e| e.path().file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    maps.sort();
    assert_eq!(maps.len(), 88);
    let mut actors = 0;
    let mut issues: std::collections::BTreeMap<String, usize> = Default::default();
    // one registry for all maps: map packages only add their own objects
    let mut vm = Vm::new(Registry::load_game(&g).unwrap());
    for m in &maps {
        let pi = vm.reg.add_package(&g.join("Maps").join(format!("{m}.dx")), m).unwrap();
        vm.clear_world();
        vm.init_world().unwrap();
        let l = vm.load_level(pi).unwrap_or_else(|e| panic!("{m}: {e}"));
        assert!(l.property_warnings.is_empty(), "{m}: {:?}", &l.property_warnings[..1]);
        for i in &l.issues {
            *issues.entry(format!("{}: {}", i.event, i.error)).or_insert(0usize) += 1;
        }
        actors += l.actors.len();
    }
    // without the 74,485 editor-only brushes and the editor cameras (`RF_LoadForClient` clear)
    assert_eq!(actors, 55902);
    assert!(issues.is_empty(), "{issues:#?}");
}

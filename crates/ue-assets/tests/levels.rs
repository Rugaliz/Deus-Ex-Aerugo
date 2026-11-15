//! Loads the Level object of every shipped map and checks its references. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_assets::level::Level;
use ue_package::{ObjectRef, Package};

#[test]
fn every_map_level_loads_with_resolvable_references() {
    let Some(dir) = std::env::var_os("DEUSEX_DIR").map(|d| PathBuf::from(d).join("Maps")) else { return };
    let mut levels = 0;
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("dx") {
            continue;
        }
        let pkg = Package::open(&path).unwrap();
        let name = path.display();
        let class_of = |r: i32| match ObjectRef::from_index(r) {
            ObjectRef::Export(i) if i < pkg.exports.len() => Some(pkg.export_class_name(i)),
            _ => None,
        };
        for i in 0..pkg.exports.len() {
            if pkg.export_class_name(i) != "Level" {
                continue;
            }
            let l = Level::load(&pkg, i).unwrap_or_else(|e| panic!("{name}: {e}"));
            levels += 1;
            // slot 0 is the level's own LevelInfo, slot 1 the builder brush; every other live entry is an export
            assert_eq!(class_of(l.actors[0]).as_deref(), Some("LevelInfo"), "{name}");
            assert_eq!(class_of(l.actors[1]).as_deref(), Some("Brush"), "{name}");
            assert!(l.actors.iter().all(|&a| a == 0 || class_of(a).is_some()), "{name}: dangling actor reference");
            assert_eq!(class_of(l.model).as_deref(), Some("Model"), "{name}");
            for r in &l.reach_specs {
                assert!(class_of(r.start).is_some() && class_of(r.end).is_some(), "{name}: reach spec endpoints");
            }
            assert_eq!(class_of(l.text_blocks[6]).as_deref(), Some("TextBuffer"), "{name}");
            assert!(l.url.valid && (l.url.protocol == "deusex" || l.url.protocol == "unreal"), "{name}: {:?}", l.url);
        }
    }
    assert_eq!(levels, 88);
}

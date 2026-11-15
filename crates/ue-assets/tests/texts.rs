//! Loads every ExtString, ConAudioList and ConEventRandomLabel. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_assets::text::{load_con_audio_list, load_con_random_labels, load_ext_string};
use ue_package::{ObjectRef, Package};

#[test]
fn text_objects_load_and_audio_lists_point_at_sounds() {
    let Some(root) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let (mut strings, mut lists, mut labels, mut refs, mut bytes) = (0, 0, 0, 0, 0);
    for folder in ["System", "Maps"] {
        for e in std::fs::read_dir(root.join(folder)).unwrap().flatten() {
            let path = e.path();
            let Ok(pkg) = Package::open(&path) else { continue };
            let name = path.display();
            for i in 0..pkg.exports.len() {
                match pkg.export_class_name(i).as_str() {
                    "ExtString" => {
                        bytes += load_ext_string(&pkg, i).unwrap_or_else(|e| panic!("{name} #{i}: {e}")).len();
                        strings += 1;
                    }
                    "ConAudioList" => {
                        let list = load_con_audio_list(&pkg, i).unwrap_or_else(|e| panic!("{name} #{i}: {e}"));
                        for r in &list {
                            // every entry is a Sound in this package or an import of one
                            let class = match ObjectRef::from_index(*r) {
                                ObjectRef::Export(x) => pkg.export_class_name(x),
                                ObjectRef::Import(x) => pkg.name(pkg.imports[x].class_name).to_string(),
                                ObjectRef::None => String::new(),
                            };
                            assert_eq!(class, "Sound", "{name} #{i}");
                        }
                        refs += list.len();
                        lists += 1;
                    }
                    "ConEventRandomLabel" => {
                        let v = load_con_random_labels(&pkg, i).unwrap_or_else(|e| panic!("{name} #{i}: {e}"));
                        assert!(!v.iter().any(|s| s.is_empty()), "{name} #{i}: empty label");
                        labels += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    // counts for the retail 1112fm install
    assert_eq!((strings, lists, labels), (492, 18, 1241));
    assert_eq!((refs, bytes > 300_000), (10079, true));
}

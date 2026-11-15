//! Loads every Sound and Music object and checks the embedded files. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_assets::audio::{load_music, load_sound, mp3_info, wav_info};
use ue_package::Package;

#[test]
fn all_sounds_and_music_load_and_validate() {
    let Some(root) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let (mut wav, mut mp3, mut music) = (0, 0, 0);
    for folder in ["System", "Sounds", "Music", "Maps"] {
        let Ok(rd) = std::fs::read_dir(root.join(folder)) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(pkg) = Package::open(&path) else { continue };
            let name = path.display();
            for i in 0..pkg.exports.len() {
                match pkg.export_class_name(i).as_str() {
                    "Sound" => {
                        let a = load_sound(&pkg, i).unwrap_or_else(|e| panic!("{name} #{i}: {e}"));
                        if a.format == "mp3" {
                            let m = mp3_info(&a.data).unwrap_or_else(|e| panic!("{name} #{i}: {e}"));
                            // every byte belongs to a frame: the walk ends exactly at the end of the data
                            assert_eq!((m.leading_bytes, m.trailing_bytes), (0, 0), "{name} #{i}");
                            assert_eq!(m.sample_rate, 44100);
                            mp3 += 1;
                        } else {
                            assert_eq!(a.format, "wav", "{name} #{i}");
                            let w = wav_info(&a.data).unwrap_or_else(|e| panic!("{name} #{i}: {e}"));
                            assert_eq!((w.format_tag, w.bits_per_sample), (1, 16), "{name} #{i}");
                            assert_eq!(w.data_len % (2 * w.channels as usize), 0, "{name} #{i}: partial sample frame");
                            wav += 1;
                        }
                    }
                    "Music" => {
                        let a = load_music(&pkg, i).unwrap_or_else(|e| panic!("{name} #{i}: {e}"));
                        assert_eq!((a.format.as_str(), &a.data[..4]), ("it", &b"IMPM"[..]), "{name} #{i}");
                        music += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    // counts for the retail 1112fm install
    assert_eq!((wav, mp3, music), (549, 10080, 35));
}

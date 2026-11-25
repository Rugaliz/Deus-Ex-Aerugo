//! Saved games stay loadable: every reference save of every format version loads in this build, the level and the
//! player come back, and the game runs on. `DEUSEX_SAVE_FIXTURES` is a folder of version folders (`v1`, `v2`, ...),
//! each holding save slots (`SaveNNNN` with `save.dxs`, `info.txt`, `Current/`) as a player's save folder does; set
//! `DEUSEX_DIR` to the game folder. The slots are only read (loading copies nothing into them).

use dx_play::Session;
use std::path::PathBuf;

#[test]
fn every_reference_save_loads() {
    let (Some(game), Some(fixtures)) = (std::env::var_os("DEUSEX_DIR").map(PathBuf::from), std::env::var_os("DEUSEX_SAVE_FIXTURES").map(PathBuf::from)) else { return };
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&fixtures).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    versions.sort();
    assert!(!versions.is_empty(), "no version folders in {}", fixtures.display());
    let mut s = Session::open(&game, "DX").unwrap();
    let mut loaded = 0;
    let mut failed = Vec::new();
    for dir in versions {
        let mut slots: Vec<(i32, PathBuf)> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| Some((e.file_name().to_str()?.strip_prefix("Save")?.parse().ok()?, e.path())))
            .collect();
        slots.sort();
        s.vm.save_dir = Some(dir.clone());
        for (n, slot) in slots {
            let info = std::fs::read_to_string(slot.join("info.txt")).unwrap_or_default();
            let map = info.lines().find_map(|l| l.strip_prefix("MapName=")).unwrap_or("").trim().to_string();
            let what = format!("{} Save{n:04} ({map})", dir.file_name().unwrap().to_string_lossy());
            let issues_before = s.issues.len();
            if let Err(e) = s.load_game(n) {
                failed.push(format!("{what}: {e}"));
                continue;
            }
            if !s.map_name.eq_ignore_ascii_case(&map) {
                failed.push(format!("{what}: loaded {}", s.map_name));
                continue;
            }
            let at = s.player_location();
            for _ in 0..30 {
                s.step_bound(1.0 / 30.0);
            }
            let moved = s.player_location();
            let d = ((at[0] - moved[0]).powi(2) + (at[1] - moved[1]).powi(2) + (at[2] - moved[2]).powi(2)).sqrt();
            // standing players stay put (a save on a lift or in water may drift a little)
            if d > 200.0 {
                failed.push(format!("{what}: the player moved {d:.0} units in one second"));
            }
            let new_issues: Vec<String> = s.issues[issues_before..].iter().map(|i| format!("{} {}: {}", i.class, i.event, i.error)).collect();
            if !new_issues.is_empty() {
                eprintln!("{what}: {} script errors, first: {}", new_issues.len(), new_issues[0]);
            }
            loaded += 1;
        }
    }
    eprintln!("{loaded} reference saves loaded");
    assert!(failed.is_empty(), "{} saves failed:\n{}", failed.len(), failed.join("\n"));
}

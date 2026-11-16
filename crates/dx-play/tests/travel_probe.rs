//! A level change and the way back, compared with the original game line by line: the probe `DXProbe.TravelGame`
//! (source in the parity archive, `deusex_shin/probe/DXProbe/Classes/TravelGame.uc`; compiled package in `probe/`)
//! opens Liberty Island, gives the player state worth carrying, destroys a pawn, travels to UNATCO HQ and back, and
//! logs `TRAVEL|<step>|<what>|<value>` at each arrival. `probe/expected_travel.txt` is the original's log
//! (`scripts/probe.sh travel` in the archive). Set `DEUSEX_DIR`.

use dx_play::Session;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `DeusEx.ini` with `DXProbe` loaded and `TravelGame` the default game.
fn probe_ini(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let (mut section, mut last_edit) = (String::new(), None);
    for line in text.lines() {
        let s = line.trim();
        if s.starts_with('[') && s.ends_with(']') {
            section = s[1..s.len() - 1].to_string();
        }
        if section == "Engine.Engine" && s.starts_with("DefaultGame=") {
            out.push("DefaultGame=DXProbe.TravelGame".into());
            continue;
        }
        if section == "Editor.EditorEngine" && s.starts_with("EditPackages=") {
            last_edit = Some(out.len());
        }
        out.push(line.to_string());
    }
    out.insert(last_edit.expect("EditPackages") + 1, "EditPackages=DXProbe".into());
    out.join("\r\n")
}

/// A tree of links to the game (in a new folder) with the probe package, the ini and the route.
fn link_tree(game: &Path, dir: &Path) {
    std::fs::create_dir_all(dir.join("System")).unwrap();
    for e in std::fs::read_dir(game).unwrap().flatten() {
        if e.file_name() != "System" && e.file_name() != "Save" {
            std::os::unix::fs::symlink(e.path(), dir.join(e.file_name())).unwrap();
        }
    }
    for e in std::fs::read_dir(game.join("System")).unwrap().flatten() {
        let n = e.file_name();
        if n != "DeusEx.ini" && n != "DXProbe.ini" && n != "DXProbe.u" {
            std::os::unix::fs::symlink(e.path(), dir.join("System").join(n)).unwrap();
        }
    }
    let ini = String::from_utf8_lossy(&std::fs::read(game.join("System/DeusEx.ini")).unwrap()).into_owned();
    std::fs::write(dir.join("System/DeusEx.ini"), probe_ini(&ini)).unwrap();
    std::fs::copy(repo().join("probe/DXProbe.u"), dir.join("System/DXProbe.u")).unwrap();
    std::fs::write(
        dir.join("System/DXProbe.ini"),
        "[DXProbe.TravelGame]\r\nStartMap=01_NYC_UNATCOIsland\r\nToB=01_NYC_UNATCOHQ#Inside_UN_HQ\r\nBackToA=01_NYC_UNATCOIsland#tunnel_drop_nyc\r\nSettleTicks=30\r\n",
    )
    .unwrap();
}

/// Takes the tree down again: only its links (never what they point to) and the files written here.
fn unlink_tree(dir: &Path) {
    for sub in [dir.join("System"), dir.to_path_buf()] {
        for e in std::fs::read_dir(&sub).into_iter().flatten().flatten() {
            if e.path().symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    for f in ["System/DeusEx.ini", "System/DXProbe.ini", "System/DXProbe.u"] {
        let _ = std::fs::remove_file(dir.join(f));
    }
}

/// The `TRAVEL|` lines of a log; a value with a newline goes on in the next log line, which the original's log
/// (and so the expected file) does not show as part of it.
fn travel_lines<'a>(log: impl Iterator<Item = &'a str>) -> Vec<String> {
    log.flat_map(|l| l.split('\n').next().map(str::to_string)).filter(|l| l.starts_with("TRAVEL|")).collect()
}

#[test]
fn a_level_change_and_back_carries_and_keeps_what_the_original_does() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let tree = std::env::temp_dir().join(format!("dxtravel-{}", std::process::id()));
    link_tree(&game, &tree);
    let mut s = Session::open(&tree, "DX.dx").unwrap();
    let mut last = (String::new(), 0u32, 0u32);
    for frame in 0..3000 {
        if s.quit {
            break;
        }
        s.step_bound(1.0 / 60.0);
        if std::env::var_os("DX_TRAVEL_TRACE").is_some() {
            let fb = match s.vm.get_prop(s.player, "flagBase", 0) { Ok(ue_vm::Value::Obj(f)) => f, _ => 0 };
            let now = (s.map_name.clone(), s.player, fb);
            if now != last {
                let game = s.vm.get_prop(s.vm.level, "Game", 0);
                eprintln!("frame {frame}: map {} player {} flagBase {} url_map {:?} game {game:?} options {:?}", now.0, now.1, now.2, s.vm.url_map, s.vm.url_options);
                last = now;
            }
        }
    }
    let port = travel_lines(s.vm.log.iter().map(String::as_str));
    if port.is_empty() {
        let tail: Vec<&String> = s.vm.log.iter().rev().take(15).collect();
        let issues: Vec<String> = s.issues.iter().take(8).map(|i| format!("{} {}: {}", i.class, i.event, i.error)).collect();
        unlink_tree(&tree);
        let p = s.player;
        let fb = s.vm.get_prop(p, "flagBase", 0);
        let pl = s.vm.get_prop(p, "Player", 0);
        let cls = s.vm.class_of(p).map(|c| s.vm.reg.class(c).path.clone());
        let early: Vec<&String> = s.vm.log.iter().filter(|l| l.contains("TravelGame") || l.contains("Login") || l.contains("open")).take(10).collect();
        panic!("no TRAVEL lines: map {}, quit {}, player {p} {cls:?} flagBase {fb:?} Player {pl:?}, issues {issues:?}, early {early:?}, last log lines {tail:?}", s.map_name, s.quit);
    }
    let latin1 = |b: Vec<u8>| b.into_iter().map(char::from).collect::<String>();
    let expected_text = latin1(std::fs::read(repo().join("probe/expected_travel.txt")).unwrap());
    let original = travel_lines(expected_text.lines());
    if std::env::var_os("DX_TRAVEL_DUMP").is_some() {
        std::fs::write(tree.join("port_travel.txt"), port.join("\n")).unwrap();
        eprintln!("port lines in {}", tree.join("port_travel.txt").display());
    }
    let mut diffs = Vec::new();
    for i in 0..port.len().max(original.len()) {
        let (a, b) = (original.get(i).map(String::as_str).unwrap_or("<none>"), port.get(i).map(String::as_str).unwrap_or("<none>"));
        if a != b {
            diffs.push(format!("{i}: original {a:?}\n   port     {b:?}"));
        }
    }
    unlink_tree(&tree);
    assert!(diffs.is_empty(), "{} lines differ (of {} / {}):\n{}", diffs.len(), original.len(), port.len(), diffs.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
}

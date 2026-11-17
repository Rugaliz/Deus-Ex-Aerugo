//! A carried crate dropped against another, compared with the original game: the probe `DXProbe.CrateGame` (source
//! in the parity archive, `deusex_shin/probe/DXProbe/Classes/CrateGame.uc`; compiled package in `probe/`) stands a
//! crate in front of the player at four gaps, puts another in the player's hands, drops it (`DropDecoration`) and
//! logs `CRATE|<case>|<what>|<value>`: whether it settled inside the first. The tester saw dropped crates inside
//! each other: the original does the same (`SetLocation` does not test other actors, the drop's trace passes over the
//! lower crate, and a move that starts inside a cylinder only hits it toward its axis). `probe/expected_crates.txt` is
//! the original's log (`scripts/probe.sh crates`). Set `DEUSEX_DIR`.

use dx_play::Session;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `DeusEx.ini` with `DXProbe` loaded and `CrateGame` the default game.
fn probe_ini(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let (mut section, mut last_edit) = (String::new(), None);
    for line in text.lines() {
        let s = line.trim();
        if s.starts_with('[') && s.ends_with(']') {
            section = s[1..s.len() - 1].to_string();
        }
        if section == "Engine.Engine" && s.starts_with("DefaultGame=") {
            out.push("DefaultGame=DXProbe.CrateGame".into());
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
        "[DXProbe.CrateGame]\r\nStartMap=01_NYC_UNATCOIsland\r\n",
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

/// The `CRATE|` lines of a log; a value with a newline goes on in the next log line, which the original's log
/// (and so the expected file) does not show as part of it.
fn crate_lines<'a>(log: impl Iterator<Item = &'a str>) -> Vec<String> {
    log.flat_map(|l| l.split('\n').next().map(str::to_string)).filter(|l| l.starts_with("CRATE|")).collect()
}

#[test]
fn a_dropped_crate_settles_as_in_the_original() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let tree = std::env::temp_dir().join(format!("dxcrates-{}", std::process::id()));
    link_tree(&game, &tree);
    let mut s = Session::open(&tree, "DX.dx").unwrap();
    for _ in 0..3000 {
        if s.quit {
            break;
        }
        s.step_bound(1.0 / 60.0);
    }
    let port = crate_lines(s.vm.log.iter().map(String::as_str));
    unlink_tree(&tree);
    let latin1 = |b: Vec<u8>| b.into_iter().map(char::from).collect::<String>();
    let expected = latin1(std::fs::read(repo().join("probe/expected_crates.txt")).unwrap());
    let original = crate_lines(expected.lines());
    assert!(!port.is_empty(), "no CRATE lines (map {}, issues {:?})", s.map_name, s.issues.iter().take(5).map(|i| format!("{} {}: {}", i.class, i.event, i.error)).collect::<Vec<_>>());
    assert_eq!(port, original);
}

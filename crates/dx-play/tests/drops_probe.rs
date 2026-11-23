//! A dropped item resting in a slope, compared with the original game: the probe `DXProbe.DropGame` (source in the
//! parity archive, `deusex_shin/probe/DXProbe/Classes/DropGame.uc`; compiled package in `probe/`) puts the Dragon's
//! Tooth sword falling, at rest and spinning, where a save had it hanging in the air in 12_Vandenberg_Gas (its box 22
//! units into a steep slope), and once more 40 units higher, and logs `DROP|<case>|<tick>|<what>|<value>` after 10,
//! 60 and 300 ticks. Its path is not compared line by line: there, whether a slide along the slope gets through
//! depends on the rounding of the trace's end point (WORKLOG 3.241), so the original's ticks of uneven length make it
//! slide in fits and starts. What both must do is leave the spot: land (physics 0) or slide away. The port used to
//! hang there for good, spinning (its steps all have one length, so the blocked state came back every step).
//! `probe/expected_drops.txt` is the original's log (`scripts/probe.sh drops`). Set `DEUSEX_DIR`.

use dx_play::Session;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The probe's settings, as `scripts/probe.sh drops` writes them for the sword.
const DROP_INI: &str = "[DXProbe.DropGame]\r\nStartMap=12_Vandenberg_Gas\r\nItemClass=DeusEx.WeaponNanoSword\r\n\
Spot=(X=-2464.8408,Y=-680.5289,Z=-856.0274)\r\nTurn=(Pitch=35644,Yaw=39811,Roll=0)\r\n\
Spin=(Pitch=49256,Yaw=59068,Roll=0)\r\nLift=40\r\n";

/// `DeusEx.ini` with `DXProbe` loaded and `DropGame` the default game.
fn probe_ini(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let (mut section, mut last_edit) = (String::new(), None);
    for line in text.lines() {
        let s = line.trim();
        if s.starts_with('[') && s.ends_with(']') {
            section = s[1..s.len() - 1].to_string();
        }
        if section == "Engine.Engine" && s.starts_with("DefaultGame=") {
            out.push("DefaultGame=DXProbe.DropGame".into());
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

/// A tree of links to the game (in a new folder) with the probe package and the ini.
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
    std::fs::write(dir.join("System/DXProbe.ini"), DROP_INI).unwrap();
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

/// The `DROP|` lines of a log.
fn drop_lines<'a>(log: impl Iterator<Item = &'a str>) -> Vec<String> {
    log.flat_map(|l| l.split('\n').next().map(str::to_string)).filter(|l| l.starts_with("DROP|")).collect()
}

/// The value logged for `case`, `tick`, `what`.
fn value<'a>(lines: &'a [String], case: u32, tick: u32, what: &str) -> &'a str {
    let key = format!("DROP|{case}|{tick}|{what}|");
    lines.iter().find_map(|l| l.strip_prefix(&key)).unwrap_or_else(|| panic!("no {key} in {lines:?}"))
}

fn location(v: &str) -> [f32; 3] {
    let f: Vec<f32> = v.split(',').map(|x| x.trim().parse().unwrap()).collect();
    [f[0], f[1], f[2]]
}

/// Whether the item in `case` left the spot by tick 300: landed, or more than a unit from where it started.
fn left_the_spot(lines: &[String], case: u32) -> bool {
    let (a, b) = (location(value(lines, case, 0, "location")), location(value(lines, case, 300, "location")));
    let moved = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    value(lines, case, 300, "physics") == "0" || moved > 1.0
}

#[test]
fn an_item_resting_in_a_slope_does_not_hang_there() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let tree = std::env::temp_dir().join(format!("dxdrops-{}", std::process::id()));
    link_tree(&game, &tree);
    let mut s = Session::open(&tree, "DX.dx").unwrap();
    for _ in 0..3000 {
        if s.quit {
            break;
        }
        s.step_bound(1.0 / 60.0);
    }
    let port = drop_lines(s.vm.log.iter().map(String::as_str));
    unlink_tree(&tree);
    let latin1 = |b: Vec<u8>| b.into_iter().map(char::from).collect::<String>();
    let expected = latin1(std::fs::read(repo().join("probe/expected_drops.txt")).unwrap());
    let original = drop_lines(expected.lines());
    assert!(port.last().map(|l| l == "DROP|END").unwrap_or(false), "the probe did not finish: {port:?} (map {}, issues {:?})", s.map_name, s.issues.iter().take(5).map(|i| format!("{} {}: {}", i.class, i.event, i.error)).collect::<Vec<_>>());
    for case in [0, 1] {
        // the placement itself matches
        for what in ["spawned", "moved", "location", "physics", "rotation", "state"] {
            assert_eq!(value(&port, case, 0, what), value(&original, case, 0, what), "case {case} {what}");
        }
        assert!(left_the_spot(&original, case), "the original's case {case}");
        assert!(left_the_spot(&port, case), "case {case} hangs where it was put: {port:?}");
        assert_eq!(value(&port, case, 300, "state"), "Pickup");
    }
}

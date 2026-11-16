//! Maps of one mission visited again come back as they were left (`DDeusExGameEngine::Browse`, the `Current` save
//! folder). Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::travel::TravelRequest;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
    }
}

fn new_game() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.vm.save_dir = Some(std::env::temp_dir().join(format!("aerugo-revisit-{}", std::process::id())));
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    run(&mut s, 5);
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland");
    Some(s)
}

fn flag(s: &mut Session, name: &str) -> bool {
    let fb = match s.vm.get_prop(s.player, "flagBase", 0) { Ok(Value::Obj(f)) => f, _ => 0 };
    let n = s.vm.reg.names.intern(name);
    matches!(s.vm.call_by_name(fb, "GetBool", vec![Value::Name(n)]), Ok(Value::Bool(true)))
}

fn set_flag(s: &mut Session, name: &str) {
    let fb = match s.vm.get_prop(s.player, "flagBase", 0) { Ok(Value::Obj(f)) => f, _ => 0 };
    let n = s.vm.reg.names.intern(name);
    s.vm.call_by_name(fb, "SetBool", vec![Value::Name(n), Value::Bool(true), Value::Bool(true), Value::Int(0)]).unwrap();
}

fn of_class(s: &mut Session, class: &str) -> Vec<u32> {
    let c = s.vm.reg.class_by_name(class).unwrap();
    s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).collect()
}

/// Terrorists still standing (alive, not dying).
fn nsf_standing(s: &mut Session) -> usize {
    of_class(s, "Terrorist").into_iter().filter(|&t| s.vm.get_prop(t, "Health", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0) > 0).count()
}

fn travel(s: &mut Session, url: &str) {
    s.vm.travel_request = Some(TravelRequest { url: url.into(), items: true });
    run(s, 10);
}

/// The tester's report: after the statue (Mission01's timer kills the island's NSF once `StatueMissionComplete` is
/// set, then sets `MS_MissionComplete`) and a visit to UNATCO HQ, the NSF patrolled the island again because the map
/// was loaded afresh. The island must come back as it was left: no NSF standing, one player, the flags kept.
#[test]
fn liberty_island_stays_cleared_after_unatco() {
    let Some(mut s) = new_game() else { return };
    let before = nsf_standing(&mut s);
    assert!(before > 0, "a new game has NSF on the island");
    set_flag(&mut s, "StatueMissionComplete");
    run(&mut s, 60);
    assert!(flag(&mut s, "MS_MissionComplete"), "Mission01 ran its clean-up");
    assert_eq!(nsf_standing(&mut s), 0, "the clean-up killed the NSF");
    travel(&mut s, "01_NYC_UNATCOHQ");
    assert_eq!(s.map_name, "01_NYC_UNATCOHQ");
    assert!(s.current_levels.contains_key("01_nyc_unatcoisland"), "the island was kept");
    // the player's own travelling objects are not kept with the level
    let log_before = s.vm.log.len();
    travel(&mut s, "01_NYC_UNATCOIsland");
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland");
    run(&mut s, 30);
    assert_eq!(nsf_standing(&mut s), 0, "the NSF are back on the island ({before} at the start)");
    assert_eq!(of_class(&mut s, "JCDentonMale").len(), 1, "one player pawn");
    assert_eq!(of_class(&mut s, "AugmentationManager").len(), 1, "one augmentation manager");
    assert_eq!(of_class(&mut s, "SkillManager").len(), 1, "one skill manager");
    assert!(flag(&mut s, "MS_MissionComplete"), "the flags travelled");
    let new_log: Vec<String> = s.vm.log[log_before..].to_vec();
    // the return logs what any travel logs (`Login`, the inventory accepted, the mission script started), no errors
    assert!(!new_log.iter().any(|l| l.contains("rror") || l.contains("Accessed None")), "script log after the return: {new_log:?}");
    let issues: Vec<String> = s.issues.iter().filter(|i| i.class == "travel").map(|i| format!("{}: {}", i.event, i.error)).collect();
    assert!(issues.is_empty(), "travel issues: {issues:?}");
    // another mission forgets the kept levels
    travel(&mut s, "02_NYC_BatteryPark");
    assert_eq!(s.map_name, "02_NYC_BatteryPark");
    assert!(s.current_levels.is_empty(), "a new mission starts without kept levels");
}

/// Saved games carry the mission's kept levels (`SaveGame` copies the `Current` folder; loading copies it back).
#[test]
fn kept_levels_go_into_saved_games() {
    let Some(mut s) = new_game() else { return };
    set_flag(&mut s, "StatueMissionComplete");
    run(&mut s, 60);
    travel(&mut s, "01_NYC_UNATCOHQ");
    let slot = s.save_game(0, "revisit").unwrap();
    s.current_levels.clear();
    s.load_game(slot).unwrap();
    run(&mut s, 5);
    assert!(s.current_levels.contains_key("01_nyc_unatcoisland"), "the island came back with the saved game");
    travel(&mut s, "01_NYC_UNATCOIsland");
    run(&mut s, 30);
    assert_eq!(nsf_standing(&mut s), 0);
    let _ = std::fs::remove_dir_all(s.vm.save_dir.clone().unwrap());
}

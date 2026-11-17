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
    // the managers the player uses are the carried ones (`Login` made its own too, left unreferenced as in the
    // original: `ResetPlayerToDefaults` -> `InitializeSubSystems` before the travel import replaces them)
    for (field, class) in [("AugmentationSystem", "AugmentationManager"), ("SkillSystem", "SkillManager")] {
        let m = match s.vm.get_prop(s.player, field, 0) { Ok(Value::Obj(m)) => m, _ => 0 };
        assert!(m != 0 && s.vm.is_live(m) && of_class(&mut s, class).contains(&m), "{field}");
    }
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

/// Arriving inside a teleporter that leads elsewhere does not send the player on: `00_TrainingCombat#backtocombat`
/// is itself a teleporter to `00_TrainingFinal`, touched when `Login` puts the pawn there, and `ClientTravel` of a pawn
/// without its `Player` yet does nothing (`APlayerPawn::execClientTravel`). The port bounced the player straight back.
#[test]
fn arriving_on_a_teleporter_does_not_travel_again() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&game, "00_TrainingFinal").unwrap();
    run(&mut s, 5);
    travel(&mut s, "00_trainingCombat#backtocombat");
    run(&mut s, 30);
    assert_eq!(s.map_name, "00_TrainingCombat");
}

/// The tester's crash taking the smugglers' elevator up to the street: a scorch mark lay on a door there. A mover's
/// surfaces and nodes exist only while the level's moving-brush tracker does (its teardown, 1037b0b0, removes them and
/// the decals on them when the level is saved or left); the kept street came back with the decal's entry naming a
/// dynamic node of the old tracker, past the end of the new model, and drawing it panicked. Now the decal is on no
/// surface after the return, and the level draws.
#[test]
fn a_decal_on_a_door_does_not_outlive_the_level() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "02_NYC_Street").unwrap();
    s.vm.save_dir = Some(std::env::temp_dir().join(format!("aerugo-decal-{}", std::process::id())));
    run(&mut s, 5);
    let mut r = None;
    s.render_vk(&mut r, 640, 480).expect("vulkan render");
    let scorch = s.vm.reg.class_by_name("ScorchMark").unwrap();
    let level_surfs = s.vm.surf_info.len();
    // a scorch mark on the face of a door (a vertical node of a mover)
    let mut placed = None;
    'doors: for m in of_class(&mut s, "DeusExMover") {
        let Some((model, nodes, _)) = s.tracked_model(m) else { continue };
        let faces: Vec<([f32; 3], [f32; 3])> = nodes
            .iter()
            .filter_map(|&n| {
                let nd = &model.nodes[n];
                let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| model.points[model.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
                (pts.len() >= 3 && nd.plane[2].abs() < 0.3).then(|| (pts.iter().fold([0.0; 3], |a, p| [a[0] + p[0] / pts.len() as f32, a[1] + p[1] / pts.len() as f32, a[2] + p[2] / pts.len() as f32]), [nd.plane[0], nd.plane[1], nd.plane[2]]))
            })
            .collect();
        for (c, n) in faces {
            let at = [c[0] + n[0] * 8.0, c[1] + n[1] * 8.0, c[2] + n[2] * 8.0];
            let yaw = ((-n[1]).atan2(-n[0]) / std::f32::consts::PI * 32768.0) as i32 & 0xffff;
            let Ok(Some(d)) = s.vm.spawn(scorch, 0, None, Some(at), Some([0, yaw, 0])) else { continue };
            if s.vm.decals.get(&d).and_then(|p| p.surfs.first()).map(|e| e.surf >= level_surfs).unwrap_or(false) {
                placed = Some(d);
                break 'doors;
            }
            let _ = s.vm.destroy(d);
        }
    }
    let decal = placed.expect("a scorch mark on a door");
    s.render_vk(&mut r, 640, 480).expect("vulkan render");
    travel(&mut s, "02_NYC_Smug#ToSmugBackDoor");
    assert_eq!(s.map_name, "02_NYC_Smug");
    s.reload_vk_world(r.as_mut().unwrap()).unwrap();
    travel(&mut s, "02_NYC_Street#FromSmugBackDoor");
    assert_eq!(s.map_name, "02_NYC_Street");
    s.reload_vk_world(r.as_mut().unwrap()).unwrap();
    assert!(s.vm.decals.values().all(|p| p.surfs.iter().all(|e| e.surf < level_surfs)), "a decal kept a mover's surface");
    for _ in 0..5 {
        s.render_vk(&mut r, 640, 480).expect("vulkan render");
        run(&mut s, 1);
    }
    r.take().unwrap().destroy();
    eprintln!("decal {decal} on a door before; after the return, decals on level surfaces only");
}

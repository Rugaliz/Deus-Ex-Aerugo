//! How long decals stay (`Session::max_decals`, `AerugoSettings.MaxDecals`): the newest that many stay out of sight,
//! older ones go as `DeusExDecal.Timer` lets them. Set `DEUSEX_DIR` to the game folder.

use dx_play::{Input, Session};
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

/// 30 scorch marks on the park path of Liberty Island, in view and seen for a few frames, then the player goes
/// elsewhere for 40 s; the decals left, oldest first.
/// Returns the decals spawned, those left, and how many `DeusExDecal`s are left in all (the AI may fight meanwhile
/// and leave its own).
fn decals_left_after_looking_away(max_decals: usize) -> Option<(Vec<u32>, Vec<u32>, usize)> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    s.max_decals = max_decals;
    let p = s.player;
    s.teleport([-1600.0, 5704.0, -80.0], 0.0);
    for _ in 0..3 {
        s.step(&Input::default(), DT);
    }
    let loc = s.vm.location(p);
    let c = s.vm.reg.class_by_name("ScorchMark").unwrap();
    let cls = Value::Obj(s.vm.reg.obj_id(&s.vm.reg.class(c).path.clone()).0);
    let mut spawned = Vec::new();
    for i in 0..30 {
        let at = [loc[0] + 150.0 + 40.0 * i as f32, loc[1], loc[2]];
        if let Ok(Value::Obj(d)) = s.vm.call_by_name(p, "Spawn", vec![cls.clone(), Value::Obj(0), Value::Name(0), Value::Vector(at), Value::Rotator([16384, 0, 0])]) {
            if d != 0 && s.vm.decals.contains_key(&d) {
                spawned.push(d);
            }
        }
        // one a frame, so their order is known
        s.step(&Input::default(), DT);
        let cam = s.render_camera();
        s.mark_rendered(&cam);
    }
    assert_eq!(spawned.len(), 30, "the scorch marks attach");
    for _ in 0..60 {
        s.step(&Input::default(), DT);
        let cam = s.render_camera();
        s.mark_rendered(&cam);
    }
    // far away, on the pier (the decals out of sight; looking the other way is not enough here: the port counts a
    // decal seen when its surface's node is drawn, and the path's floor reaches past the camera)
    s.teleport([-4666.0, 10355.0, -240.0], 0.0);
    for _ in 0..(40.0 / DT) as usize {
        s.step(&Input::default(), DT);
        let cam = s.render_camera();
        s.mark_rendered(&cam);
    }
    let left = spawned.iter().copied().filter(|d| s.vm.is_live(*d) && s.vm.decals.contains_key(d)).collect();
    let dc = s.vm.reg.class_by_name("DeusExDecal").unwrap();
    let all = s.vm.decals.keys().filter(|&&d| s.vm.class_of(d).is_some_and(|c| s.vm.is_a(c, dc))).count();
    Some((spawned, left, all))
}

#[test]
fn unseen_decals_go_as_in_the_original() {
    let Some((_, left, _)) = decals_left_after_looking_away(0) else { return };
    assert!(left.is_empty(), "{} decals out of sight for 40 s are still there", left.len());
}

#[test]
fn the_newest_decals_stay_out_of_sight() {
    let Some((spawned, left, all)) = decals_left_after_looking_away(10) else { return };
    assert!(!left.is_empty(), "some of the newest stay");
    assert_eq!(left, spawned[spawned.len() - left.len()..], "the newest stay, the oldest go");
    assert!(all >= 10, "ten decals or more are left: {all}");
}

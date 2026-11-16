//! A level reached by travel is drawn as the same level opened directly: nothing the session worked out for the
//! level it left may carry over. Set `DEUSEX_DIR` to the game folder; pictures need Vulkan.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::travel::TravelRequest;

const DT: f32 = 1.0 / 30.0;

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
    }
}

/// The view at `at` (yaw degrees) of `s`, rendered with `r`, as RGBA.
fn view(s: &mut Session, r: &mut Option<dx_vk::renderer::Renderer>, at: [f32; 3], yaw: f32) -> (u32, u32, Vec<u8>) {
    s.teleport(at, yaw);
    // the eye settles at its standing height after a level change
    run(s, 60);
    let img = s.render_vk(r, 800, 600).expect("vulkan render");
    (img.width, img.height, img.rgba)
}

/// Mean absolute difference per channel inside the rectangle `x0..x1`, `y0..y1`.
fn diff(a: &(u32, u32, Vec<u8>), b: &(u32, u32, Vec<u8>), x0: u32, y0: u32, x1: u32, y1: u32) -> f64 {
    let mut sum = 0u64;
    let mut n = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * a.0 + x) * 4) as usize;
            for c in 0..3 {
                sum += (a.2[i + c] as i32 - b.2[i + c] as i32).unsigned_abs() as u64;
                n += 1;
            }
        }
    }
    sum as f64 / n as f64
}

/// The tester's "metal panel" at the pistol range: the session kept each mover brush's polygons by export number of
/// the map package, so after travelling from `00_Training` the counter's button cover of lane 1 (`shield1`) got the
/// polygons of the Training door with the same export number and stood as a brass door in the window.
#[test]
fn the_pistol_range_after_training_looks_as_opened_directly() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let booth = [65.0, -520.0, -90.0];
    let fresh = {
        let mut s = Session::open(&game, "00_TrainingCombat").unwrap();
        run(&mut s, 5);
        let mut r = None;
        let v = view(&mut s, &mut r, booth, 270.0);
        r.take().unwrap().destroy();
        v
    };
    let mut s = Session::open(&game, "00_Training").unwrap();
    run(&mut s, 5);
    let mut r = None;
    // a few frames drawn in Training first (the movers there are worked out)
    for _ in 0..3 {
        s.render_vk(&mut r, 800, 600).expect("vulkan render");
        run(&mut s, 1);
    }
    s.vm.travel_request = Some(TravelRequest { url: "00_TrainingCombat".into(), items: true });
    run(&mut s, 2);
    assert_eq!(s.map_name, "00_TrainingCombat");
    assert!(s.take_map_changed());
    s.reload_vk_world(r.as_mut().unwrap()).unwrap();
    run(&mut s, 3);
    let travelled = view(&mut s, &mut r, booth, 270.0);
    r.take().unwrap().destroy();
    if std::env::var_os("SHOTS").is_some() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("range_fresh.png"), dx_soft::png(fresh.0, fresh.1, &fresh.2)).unwrap();
        std::fs::write(dir.join("range_travelled.png"), dx_soft::png(travelled.0, travelled.1, &travelled.2)).unwrap();
    }
    // the two lane windows and the counter below them (the message window at the top differs in time)
    let d = diff(&fresh, &travelled, 60, 180, 740, 560);
    assert!(d < 4.0, "the booth differs after the travel (mean difference {d:.1} per channel)");
}

/// The movers the renderer gets through the visibility walk (brush export -> fragments), after one drawn frame.
fn drawn_movers(s: &mut Session, r: &mut Option<dx_vk::renderer::Renderer>) -> Vec<(usize, usize)> {
    s.render_vk(r, 640, 480).expect("vulkan render");
    let mut m: Vec<(usize, usize)> = r.as_ref().unwrap().mover_fragments.as_ref().map(|m| m.iter().map(|(e, f)| (*e, f.len())).collect()).unwrap_or_default();
    m.sort();
    m
}

/// A map loaded again in the same session (training restarted from the menu, a return to a level of the mission) is the
/// same package: a second copy was never found by path, so every mover's brush seemed to belong to another package and
/// the walk left all movers out (closed doors invisible, the user's report after restarting training).
#[test]
fn a_map_loaded_again_still_draws_its_movers() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut r = None;
    // training restarted as the menu does it (`StartTrainingMission`: kept levels deleted, a new game started)
    let mut s = Session::open(&game, "00_Training").unwrap();
    run(&mut s, 5);
    let before = drawn_movers(&mut s, &mut r);
    assert!(!before.is_empty());
    let p = s.player;
    s.vm.call_by_name(p, "StartTrainingMission", vec![]).unwrap();
    run(&mut s, 2);
    assert!(s.take_map_changed());
    s.reload_vk_world(r.as_mut().unwrap()).unwrap();
    run(&mut s, 3);
    assert_eq!(s.map_name, "00_Training");
    assert!(s.current_levels.is_empty(), "DeleteSaveGameFiles forgets the kept levels");
    assert_eq!(drawn_movers(&mut s, &mut r), before, "the movers after the restart");
    // a return to a kept level: every mover is in the tracker again (the walk sees them wherever the player looks)
    let mut s = Session::open(&game, "01_NYC_UNATCOIsland").unwrap();
    run(&mut s, 5);
    s.reload_vk_world(r.as_mut().unwrap()).unwrap();
    drawn_movers(&mut s, &mut r);
    let tracked = |s: &mut Session| {
        let c = s.vm.reg.class_by_name("DeusExMover").unwrap();
        let movers: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).collect();
        movers.iter().filter(|&&m| s.tracked_model(m).map(|t| !t.1.is_empty()).unwrap_or(false)).count()
    };
    let before = tracked(&mut s);
    assert!(before > 0);
    for url in ["01_NYC_UNATCOHQ#Inside_UN_HQ", "01_NYC_UNATCOIsland#tunnel_drop_nyc"] {
        s.vm.travel_request = Some(TravelRequest { url: url.into(), items: true });
        run(&mut s, 2);
        assert!(s.take_map_changed());
        s.reload_vk_world(r.as_mut().unwrap()).unwrap();
        run(&mut s, 3);
        drawn_movers(&mut s, &mut r);
    }
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland");
    assert_eq!(tracked(&mut s), before, "the island's movers in the tracker after the return");
    r.take().unwrap().destroy();
}

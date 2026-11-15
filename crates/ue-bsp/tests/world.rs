//! BSP queries against real maps. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_assets::files::{vector_prop, GameFiles};
use ue_assets::level::Level;
use ue_assets::model::Model;
use ue_bsp::Bsp;
use ue_package::ObjectRef;

const MAPS: [&str; 3] = ["00_Training", "02_NYC_Street", "01_NYC_UNATCOHQ"];

struct World {
    bsp: Bsp,
    /// Locations of the map's actors (almost all lie in free space).
    points: Vec<[f32; 3]>,
}

fn load(map: &str) -> Option<World> {
    let root = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let files = GameFiles::new(&root);
    let m = files.package(map).unwrap();
    let li = (0..m.pkg.exports.len()).find(|&i| m.pkg.export_class_name(i) == "Level").unwrap();
    let level = Level::load(&m.pkg, li).unwrap();
    let ObjectRef::Export(me) = ObjectRef::from_index(level.model) else { panic!() };
    let model = Model::load(&m.pkg, me).unwrap();
    let mut points = Vec::new();
    for &a in &level.actors {
        if let ObjectRef::Export(i) = ObjectRef::from_index(a) {
            let class = m.pkg.export_class_name(i);
            if matches!(class.as_str(), "PlayerStart" | "PatrolPoint" | "WayPoint" | "ScriptedPawn" | "Light" | "AmbientSound") || class.ends_with("Light") {
                if let Some(p) = vector_prop(&m.pkg, i, "Location") {
                    points.push(p);
                }
            }
        }
    }
    Some(World { bsp: Bsp::from_model(&model), points })
}

#[test]
fn actors_stand_in_empty_space_and_floors_are_hit() {
    for map in MAPS {
    let Some(w) = load(map) else { return };
    assert!(w.points.len() > 20);
    let outside = w.points.iter().filter(|p| w.bsp.point_region(**p).outside).count();
    assert!(outside as f32 > 0.9 * w.points.len() as f32, "{outside} of {} points in empty space", w.points.len());
    // a trace straight down from an actor reaches a surface with an upward-facing normal somewhere below
    let mut floors = 0;
    for p in w.points.iter().filter(|p| w.bsp.point_region(**p).outside) {
        let end = [p[0], p[1], p[2] - 2000.0];
        if let Some(h) = w.bsp.line_check(*p, end, [0.0; 3], 0) {
            assert!(h.time > 0.0 && h.time < 1.0);
            assert!(h.location[2] < p[2] + 0.01 && h.location[2] > end[2]);
            if h.normal[2] > 0.7 {
                floors += 1;
            }
        }
    }
    assert!(floors > 5, "{map}: only {floors} downward traces hit floors");
    }
}

#[test]
fn fast_line_check_agrees_with_zero_extent_trace() {
    for map in MAPS {
    let Some(w) = load(map) else { return };
    let pts: Vec<_> = w.points.iter().filter(|p| w.bsp.point_region(**p).outside).cloned().collect();
    let (mut agree, mut total) = (0, 0);
    for i in 0..pts.len().min(60) {
        for j in 0..pts.len().min(60) {
            if i == j {
                continue;
            }
            let (a, b) = (pts[i], pts[j]);
            let fast = w.bsp.fast_line_check(a, b);
            let slow = w.bsp.line_check(a, b, [0.0; 3], 0).is_none();
            total += 1;
            agree += (fast == slow) as usize;
        }
    }
    eprintln!("{map}: fast vs trace agree {agree}/{total}");
    assert!(total > 500);
    assert!(agree as f32 > 0.97 * total as f32, "{map}: agree {agree}/{total}");
    }
}

#[test]
fn swept_box_stops_in_free_space_and_is_never_clear_through_walls() {
    for map in MAPS {
    let Some(w) = load(map) else { return };
    let pts: Vec<_> = w.points.iter().filter(|p| w.bsp.point_region(**p).outside).cloned().collect();
    let ext = [17.0, 17.0, 39.0]; // roughly the player
    let (mut blocked, mut stopped_free, mut clear_but_overlaps) = (0, 0, 0);
    for i in 0..pts.len().min(50) {
        for j in 0..pts.len().min(50) {
            if i == j {
                continue;
            }
            let (a, b) = (pts[i], pts[j]);
            // only paths whose start box is free
            if w.bsp.point_check(a, ext, 0).is_some() {
                continue;
            }
            match w.bsp.line_check(a, b, ext, 0) {
                Some(h) => {
                    blocked += 1;
                    if w.bsp.point_check(h.location, ext, 0).is_none() {
                        stopped_free += 1;
                    }
                }
                None => {
                    // if the sweep says clear, the middle and end boxes must not be inside solid
                    for t in [0.25f32, 0.5, 0.75] {
                        let m = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
                        if w.bsp.point_check(m, ext, 0).is_some() {
                            clear_but_overlaps += 1;
                        }
                    }
                }
            }
        }
    }
    eprintln!("{map}: blocked {blocked}, stopped free {stopped_free}, clear-but-overlapping {clear_but_overlaps}");
    assert!(blocked > 50, "{map}: blocked {blocked}");
    assert!(stopped_free as f32 > 0.95 * blocked as f32, "{map}: stopped in free space {stopped_free}/{blocked}");
    assert!(clear_but_overlaps < 5, "{map}: {clear_but_overlaps} clear sweeps pass through solid");
    }
}

#[test]
fn a_player_sized_box_dropped_from_any_actor_lands_on_something() {
    for map in MAPS {
        let Some(w) = load(map) else { return };
        let ext = [17.0, 17.0, 39.0];
        let (mut hit, mut total) = (0, 0);
        for p in w.points.iter().filter(|p| w.bsp.point_region(**p).outside && w.bsp.point_check(**p, ext, 0).is_none()) {
            total += 1;
            let h = w.bsp.line_check(*p, [p[0], p[1], p[2] - 20000.0], ext, 0);
            if let Some(h) = h {
                hit += 1;
                assert!(h.location[2] <= p[2]);
                // the box where it stopped is free
                assert!(w.bsp.point_check(h.location, ext, 0).is_none(), "{map}: stops inside solid at {:?}", h.location);
            }
        }
        assert!(total > 20, "{map}: {total}");
        assert!(hit as f32 > 0.9 * total as f32, "{map}: only {hit}/{total} drops land");
    }
}

/// Surfaces with wavy poly flags in every map (probe): `cargo test -p ue-bsp --test world wavy -- --ignored --nocapture`.
#[test]
#[ignore]
fn wavy_surfaces_census() {
    let Some(root) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let files = GameFiles::new(&root);
    let mut maps: Vec<String> = std::fs::read_dir(root.join("Maps")).unwrap().filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".dx").map(String::from)).collect();
    maps.sort();
    for map in maps {
        let m = files.package(&map).unwrap();
        let li = (0..m.pkg.exports.len()).find(|&i| m.pkg.export_class_name(i) == "Level").unwrap();
        let level = Level::load(&m.pkg, li).unwrap();
        let ObjectRef::Export(me) = ObjectRef::from_index(level.model) else { continue };
        let model = Model::load(&m.pkg, me).unwrap();
        let count = |f: u32| model.surfs.iter().filter(|s| s.poly_flags & f != 0).count();
        let (big, small, water, cloud) = (count(0x1000), count(0x2000), count(0x4000), count(0x20000));
        if map == "01_NYC_UNATCOIsland" {
            println!("{map}: auto-pan surfaces {}", count(0x200) + count(0x400));
        }
        if big + small + water + cloud > 0 {
            println!("{map}: big {big} small {small} water {water} cloud {cloud}");
        }
    }
}

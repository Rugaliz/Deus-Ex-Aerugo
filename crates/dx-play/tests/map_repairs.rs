//! Aerugo repairs of the shipped maps' data (`ue_vm::Vm::repair_initial_inventories`). Set `DEUSEX_DIR` to the game
//! folder. Its own test binary.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

/// Battery Park's UNATCO troopers: the map gives them a pistol with boxes of rifle ammunition (so, once the pistol's
/// six rounds are gone, the knife: the original does the same, WORKLOG 3.245); they get their class's assault rifle
/// back, with that ammunition, and hold it.
#[test]
fn battery_park_troopers_carry_their_rifles() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "02_NYC_BatteryPark").unwrap();
    for _ in 0..10 {
        s.step_bound(1.0 / 60.0);
    }
    let tc = s.vm.reg.class_by_name("UNATCOTroop").unwrap();
    let troops: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a) == Some(tc)).collect();
    assert_eq!(troops.len(), 5);
    let class = |s: &Session, o: u32| s.vm.class_of(o).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
    for t in troops {
        let w = match s.vm.get_prop(t, "Weapon", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        assert_eq!(class(&s, w), "DeusEx.WeaponAssaultGun", "{} holds the rifle", s.vm.name_of(t));
        let ammo = match s.vm.get_prop(w, "AmmoType", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        assert_eq!(class(&s, ammo), "DeusEx.Ammo762mm");
        assert!(s.vm.get_prop(ammo, "AmmoAmount", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0) > 100);
    }
    assert!(s.vm.log.iter().any(|l| l.starts_with("Aerugo repair: 5 pawns")), "{:?}", s.vm.log.iter().filter(|l| l.contains("repair")).collect::<Vec<_>>());
}

/// Every map started fresh: which pawns the repair changes (to check it touches only the map's slips).
#[test]
#[ignore]
fn survey_inventory_repairs() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut maps: Vec<String> = std::fs::read_dir(g.join("Maps")).unwrap().flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".dx").map(str::to_string)).collect();
    maps.sort();
    for m in maps {
        let Ok(s) = Session::open(&g, &m) else { continue };
        for l in s.vm.log.iter().filter(|l| l.starts_with("Aerugo repair:")) {
            println!("{m}: {l}");
        }
    }
}

/// The last Hell's Kitchen visit (08_NYC_Street): the GOTY map has no music (`LevelInfo.Song` empty; the earlier visits
/// play `NYCStreets_Music`), and the game ships `NYCStreets2_Music`, which no map refers to. The level gets it, and the
/// player's music logic picks it up as for any level.
#[test]
fn hells_kitchen_has_music_on_the_last_visit() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "08_NYC_Street").unwrap();
    for _ in 0..90 {
        s.step_bound(1.0 / 30.0);
    }
    let path = |s: &Session, o: u32| if o == 0 { String::new() } else { s.vm.reg.obj_path(ue_registry::ObjId(o)).to_string() };
    let level = s.vm.level;
    let song = match s.vm.get_prop(level, "Song", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    assert_eq!(path(&s, song), "NYCStreets2_Music.NYCStreets2_Music");
    let p = s.player;
    let playing = match s.vm.get_prop(p, "Song", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    assert_eq!(path(&s, playing), "NYCStreets2_Music.NYCStreets2_Music", "the player's music");
}

//! Aerugo repairs of the original's story defects (docs/known-original-bugs.md in deusex_shin). Set `DEUSEX_DIR` to the
//! game folder and `DEUSEX_SAVE_FIXTURES` to the reference saves (`out/save-fixtures`). Its own test binary.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn session_with_saves() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let fixtures = PathBuf::from(std::env::var_os("DEUSEX_SAVE_FIXTURES")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.vm.save_dir = Some(fixtures.join("v1"));
    Some(s)
}

fn flag(s: &mut Session, name: &str) -> bool {
    let fb = match s.vm.get_prop(s.player, "FlagBase", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    let n = s.vm.reg.names.intern(name);
    matches!(s.vm.call_by_name(fb, "GetBool", vec![Value::Name(n)]), Ok(Value::Bool(true)))
}

fn travel(s: &mut Session, map: &str) {
    let p = s.player;
    s.vm.call_by_name(p, "ClientTravel", vec![Value::Str(map.into()), Value::Byte(2), Value::Bool(true)]).unwrap();
    for _ in 0..300 {
        s.step_bound(DT);
        if s.map_name.eq_ignore_ascii_case(map) {
            break;
        }
    }
    assert!(s.map_name.eq_ignore_ascii_case(map), "still in {}", s.map_name);
    for _ in 0..30 {
        s.step_bound(DT);
    }
}

fn carries(s: &Session, class: &str) -> bool {
    let mut item = match s.vm.get_prop(s.player, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    let mut n = 0;
    while item != 0 && n < 200 {
        if s.vm.class_of(item).is_some_and(|c| s.vm.reg.class(c).path.eq_ignore_ascii_case(class)) {
            return true;
        }
        item = match s.vm.get_prop(item, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        n += 1;
    }
    false
}

/// Ford Schick, rescued from the sewers in mission 2, waits at the Smuggler's in mission 8 and gives JC the augmentation
/// upgrade canister he talks about. In the original neither happens: the rescue flag ran out at the end of mission 2
/// (`Mission02`'s own long-lived setting waits on a flag nothing sets), and the gift names a class that does not exist.
/// From the user's play through: Save0014 (02_NYC_Warehouse, Ford rescued), then straight to 08_NYC_Smug.
#[test]
fn ford_schick_waits_at_the_smugglers_in_mission_8() {
    let Some(mut s) = session_with_saves() else { return };
    s.load_game(14).unwrap();
    assert!(flag(&mut s, "FordSchickRescued"), "the save has Ford rescued");
    travel(&mut s, "08_NYC_Smug");
    assert!(flag(&mut s, "FordSchickRescued"), "the rescue is remembered in mission 8");
    let fc = s.vm.reg.class_by_name("FordSchick").unwrap();
    let ford = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(fc)).expect("Ford is in the map");
    assert_eq!(s.vm.get_prop(ford, "bInWorld", 0).ok(), Some(Value::Bool(true)), "Ford came into the world");
    assert!(!carries(&s, "DeusEx.AugmentationUpgradeCannister"), "no canister yet");
    // stand before him (he sits, facing his `Rotation`) and talk to him as frobbing him does
    // (`DeusExPlayer.StartConversation`, IM_Frob)
    let t = s.vm.location(ford);
    let yaw = match s.vm.get_prop(ford, "Rotation", 0) { Ok(Value::Rotator(r)) => r[1] as f32 * std::f32::consts::TAU / 65536.0, _ => 0.0 };
    s.teleport([t[0] + 70.0 * yaw.cos(), t[1] + 70.0 * yaw.sin(), t[2] + 20.0], (yaw + std::f32::consts::PI).to_degrees());
    for _ in 0..90 {
        s.step_bound(DT);
    }
    // the play through's JC carries a full pack, and a full pack takes no gift (as in the original): make room
    let mut item = match s.vm.get_prop(s.player, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    while item != 0 {
        let next = match s.vm.get_prop(item, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        if s.vm.class_of(item).is_some_and(|c| matches!(s.vm.reg.class(c).path.as_str(), "DeusEx.WeaponGEPGun" | "DeusEx.WeaponFlamethrower")) {
            s.vm.call_by_name(item, "Destroy", vec![]).unwrap();
        }
        item = next;
    }
    let p = s.player;
    let started = s.vm.call_by_name(p, "StartConversation", vec![Value::Obj(ford), Value::Byte(1)]).unwrap();
    assert_eq!(started, Value::Bool(true), "the conversation starts");
    let mut talked = false;
    for _ in 0..30 * 120 {
        s.step_bound(DT);
        if s.vm.state_name(s.player) == Some("Conversation") {
            talked = true;
        } else if talked {
            break;
        }
    }
    assert!(talked, "Ford talked");
    assert!(carries(&s, "DeusEx.AugmentationUpgradeCannister"), "Ford gave the canister");
}

/// Castle Clinton: `BatteryParkSlaughter` (everyone later treats JC as a butcher) is meant for JC killing 2 or more of
/// the 5 terrorists, but the original sets it when any 2 are dead, the UNATCO troopers' kills included. Two killed by a
/// trooper: JC is not blamed; two more killed by JC: he is.
#[test]
fn castle_clinton_blames_jc_only_for_his_own_kills() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "02_NYC_BatteryPark").unwrap();
    for _ in 0..30 {
        s.step_bound(DT);
    }
    let tagged = |s: &Session, class: &str, tag: &str| -> Vec<u32> {
        let c = s.vm.reg.class_by_name(class).unwrap();
        s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a) == Some(c) && matches!(s.vm.get_prop(a, "Tag", 0), Ok(Value::Name(n)) if s.vm.reg.names.str(n) == tag)).collect()
    };
    let terrorists = tagged(&s, "Terrorist", "ClintonTerrorist");
    let guards = tagged(&s, "UNATCOTroop", "ClintonGuards");
    assert_eq!(terrorists.len(), 5, "Castle Clinton's terrorists");
    assert!(!guards.is_empty(), "Castle Clinton's troopers");
    let kill = |s: &mut Session, victim: u32, by: u32| {
        let at = s.vm.location(victim);
        let shot = s.vm.reg.names.intern("Shot");
        s.vm.call_by_name(victim, "TakeDamage", vec![Value::Int(1000), Value::Obj(by), Value::Vector(at), Value::Vector([0.0; 3]), Value::Name(shot)]).unwrap();
    };
    // the troopers kill two
    kill(&mut s, terrorists[0], guards[0]);
    kill(&mut s, terrorists[1], guards[0]);
    for _ in 0..30 * 5 {
        s.step_bound(DT);
    }
    assert!(!flag(&mut s, "BatteryParkSlaughter"), "JC is blamed for the troopers' kills");
    // JC kills two
    let p = s.player;
    kill(&mut s, terrorists[2], p);
    kill(&mut s, terrorists[3], p);
    for _ in 0..30 * 5 {
        s.step_bound(DT);
    }
    assert!(flag(&mut s, "BatteryParkSlaughter"), "JC killed two: the slaughter is his");
}

/// Paul in the 'Ton: JC leaving by the window kills Paul unless Paul got out through the lobby first; in the MJ12 lab
/// (mission 5) a saved Paul is there alive, a killed one lies dead. The original removed a saved Paul too when JC had
/// taken the window, so he was nowhere. The hotel's flags as the map's triggers set them, then the level change.
#[test]
fn paul_saved_before_the_window_is_alive_in_the_mj12_lab() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    for dead in [false, true] {
        let mut s = Session::open(&g, "04_NYC_Hotel").unwrap();
        for _ in 0..10 {
            s.step_bound(DT);
        }
        let fb = match s.vm.get_prop(s.player, "FlagBase", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        for (name, exp, on) in [("PlayerBailedOutWindow", 6, true), ("PaulDenton_Dead", 16, dead)] {
            if on {
                let n = s.vm.reg.names.intern(name);
                s.vm.call_by_name(fb, "SetBool", vec![Value::Name(n), Value::Bool(true), Value::Void, Value::Int(exp)]).unwrap();
            }
        }
        let map = std::fs::read_dir(g.join("Maps")).unwrap().flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).find(|f| f.eq_ignore_ascii_case("05_NYC_UNATCOMJ12Lab.dx")).unwrap();
        travel(&mut s, map.trim_end_matches(".dx"));
        let count = |s: &Session, class: &str| {
            let c = s.vm.reg.class_by_name(class).unwrap();
            s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a) == Some(c)).count()
        };
        let (alive, body) = (count(&s, "PaulDenton"), count(&s, "PaulDentonCarcass"));
        if dead {
            assert_eq!((alive, body), (0, 1), "killed by the window: only his body");
        } else {
            assert_eq!((alive, body), (1, 0), "out through the lobby before the window: Paul is alive in the lab");
        }
    }
}

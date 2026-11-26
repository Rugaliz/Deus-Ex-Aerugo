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

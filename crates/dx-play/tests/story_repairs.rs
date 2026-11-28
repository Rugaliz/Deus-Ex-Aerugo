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

/// Maggie Chow's flat: the display case with the Buddha statue is the secret way between her rooms and the hidden
/// corridor behind (a light switch and a trigger fire it). In the GOTY map both its keyframes are at rest, so it never
/// moved; Aerugo gives it the 140-unit sink of its leftover copy in the same map. The switch now opens the way, and the
/// statue standing on the case goes down with it.
#[test]
fn maggie_chows_display_case_opens() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "06_HongKong_WanChai_Street").unwrap();
    s.step_bound(DT);
    let tagged = |s: &Session, tag: &str| s.vm.live_actors().into_iter().filter(|&a| matches!(s.vm.get_prop(a, "Tag", 0), Ok(Value::Name(n)) if s.vm.reg.names.str(n) == tag)).collect::<Vec<u32>>();
    let case = tagged(&s, "Dispalycase")[0];
    let buddha = tagged(&s, "HKBuddha").into_iter().find(|&b| s.vm.get_prop(b, "Base", 0).ok() == Some(Value::Obj(case))).expect("the statue stands on the case");
    let switch = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).is_some_and(|c| s.vm.reg.class(c).path == "DeusEx.LightSwitch") && matches!(s.vm.get_prop(a, "Event", 0), Ok(Value::Name(n)) if s.vm.reg.names.str(n) == "Dispalycase")).expect("the case's light switch");
    let (case_at, buddha_at) = (s.vm.location(case), s.vm.location(buddha));
    let p = s.player;
    s.vm.call_by_name(switch, "Frob", vec![Value::Obj(p), Value::Obj(0)]).unwrap();
    for _ in 0..30 * 3 {
        s.step_bound(DT);
    }
    assert!((s.vm.location(case)[2] - (case_at[2] - 140.0)).abs() < 0.5, "the case sank: {:?} -> {:?}", case_at, s.vm.location(case));
    assert!((s.vm.location(buddha)[2] - (buddha_at[2] - 140.0)).abs() < 0.5, "the statue went down with it");
    assert!(s.vm.log.iter().any(|l| l.contains("moves again")));
}

/// Bob Page in the MJ12 lab under VersaLife: the map leaves him mortal, so he could be killed there though the story
/// has him speak to JC later (Vandenberg, the Ocean Lab, Area 51). He takes no harm.
#[test]
fn bob_page_cannot_be_killed_in_hong_kong() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "06_HongKong_MJ12lab").unwrap();
    s.step_bound(DT);
    let c = s.vm.reg.class_by_name("BobPage").unwrap();
    let page = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(c)).expect("Page is in the lab");
    let p = s.player;
    let at = s.vm.location(page);
    let shot = s.vm.reg.names.intern("Shot");
    s.vm.call_by_name(page, "TakeDamage", vec![Value::Int(1000), Value::Obj(p), Value::Vector(at), Value::Vector([0.0; 3]), Value::Name(shot)]).unwrap();
    for _ in 0..30 {
        s.step_bound(DT);
    }
    assert!(s.vm.live_actors().contains(&page) && s.vm.state_name(page) != Some("Dying"), "Page was killed");
    assert_eq!(s.vm.get_prop(page, "Health", 0).ok().and_then(|v| v.as_i32()), Some(100), "Page took harm");
}

/// The Universal Constructor's self-destruct: its keypad fires a dispatcher whose events toggle the hazards on (the
/// control rod, the shocker doors, the electricity); entering the code a second time fired them again and switched them
/// off. A second entry changes nothing.
#[test]
fn the_uc_self_destruct_runs_once() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "06_HongKong_Storage").unwrap();
    s.step_bound(DT);
    let is_tag = |s: &Session, a: u32, t: &str| matches!(s.vm.get_prop(a, "Tag", 0), Ok(Value::Name(n)) if s.vm.reg.names.str(n) == t);
    let keypad = s.vm.live_actors().into_iter().find(|&a| matches!(s.vm.get_prop(a, "Event", 0), Ok(Value::Name(n)) if s.vm.reg.names.str(n) == "Self_Destruct")).expect("the destruct keypad");
    let movers: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).is_some_and(|c| s.vm.reg.class(c).path == "DeusEx.DeusExMover") && (is_tag(&s, a, "Shocker") || is_tag(&s, a, "Control_Rod"))).collect();
    let sparks: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).is_some_and(|c| s.vm.reg.class(c).path == "DeusEx.ElectricityEmitter") && is_tag(&s, a, "Shocker")).collect();
    assert!(movers.len() >= 6 && sparks.len() >= 12, "the hazards: {} movers, {} emitters", movers.len(), sparks.len());
    let hazards = |s: &Session| -> Vec<String> {
        movers.iter().map(|&m| format!("{:?}", s.vm.get_prop(m, "KeyNum", 0).ok())).chain(sparks.iter().map(|&e| format!("{:?}", s.vm.get_prop(e, "bIsOn", 0).ok()))).collect()
    };
    let at_rest = hazards(&s);
    let p = s.player;
    let mut enter = |s: &mut Session| {
        s.vm.call_by_name(keypad, "RunEvents", vec![Value::Obj(p), Value::Bool(true)]).unwrap();
        for _ in 0..30 * 15 {
            s.step_bound(DT);
        }
    };
    enter(&mut s);
    let running = hazards(&s);
    assert_ne!(running, at_rest, "the self-destruct changed nothing");
    enter(&mut s);
    assert_eq!(hazards(&s), running, "the second entry switched the hazards back");
}

/// A survey for these repairs (not a check): the actors of `SURVEY_MAP` whose class, tag, event, flag or bind name
/// contains `SURVEY_FILTER`, with their trigger and order settings.
#[test]
#[ignore]
fn survey_map_actors() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let map = std::env::var("SURVEY_MAP").unwrap_or_else(|_| "06_HongKong_WanChai_Market".into());
    let filter = std::env::var("SURVEY_FILTER").unwrap_or_default().to_ascii_lowercase();
    let mut s = Session::open(&g, &map).unwrap();
    s.step_bound(DT);
    let show = |s: &Session, a: u32, p: &str| match s.vm.get_prop(a, p, 0) { Ok(Value::Name(n)) => s.vm.reg.names.str(n).to_string(), Ok(Value::Str(t)) => format!("{t:?}"), Ok(v) => format!("{v:?}"), _ => String::new() };
    for a in s.vm.live_actors() {
        let Some(c) = s.vm.class_of(a) else { continue };
        let path = s.vm.reg.class(c).path.clone();
        let line = format!("{path} {}", ["Tag", "Event", "flagName", "BindName", "Alliance", "AllianceName"].iter().map(|p| show(&s, a, p)).collect::<Vec<_>>().join(" ")).to_ascii_lowercase();
        if !line.contains(&filter) {
            continue;
        }
        let props: Vec<String> = ["Tag", "Event", "flagName", "flagValue", "bSetFlag", "bTrigger", "TriggerType", "ClassProximityType", "bTriggerOnceOnly", "bInitiallyActive", "CollisionRadius", "CollisionHeight", "BindName", "Alliance", "Orders", "OrderTag", "bInWorld", "Alliances", "AllianceName", "AllianceLevel", "bPermanent", "Alliance0", "bHateTrespass", "bInvincible", "Health", "bImportant", "imageClass", "textTag"].iter().filter_map(|p| s.vm.get_prop(a, p, 0).ok().map(|_| format!("{p}={}", show(&s, a, p)))).collect();
        let mut props = props;
        if path.ends_with("Dispatcher") {
            let ev: Vec<String> = (0..8).filter_map(|i| match s.vm.get_prop(a, "OutEvents", i) { Ok(Value::Name(n)) if n != 0 && s.vm.reg.names.str(n) != "None" => Some(format!("{}@{:?}", s.vm.reg.names.str(n), s.vm.get_prop(a, "OutDelays", i).ok().and_then(|v| v.as_f32()))), _ => None }).collect();
            props.push(format!("OutEvents={ev:?}"));
        }
        let (name, state, at) = (s.vm.name_of(a).to_string(), s.vm.state_name(a).map(str::to_string), s.vm.location(a));
        eprintln!("{name} {path} state {state:?} at {at:?}: {}", props.join(" "));
    }
}

/// A survey (not a check): the flags conversation events `DeusExConText.ConEventSetFlag<N>` set (`SURVEY_EVENTS`, comma
/// separated numbers, as `out/audit/flags_data.tsv` names them), with their expiration.
#[test]
#[ignore]
fn survey_con_flag() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "06_HongKong_WanChai_Market").unwrap();
    for id in std::env::var("SURVEY_EVENTS").unwrap_or_default().split(',') {
        let ev = s.vm.load_con_object(&format!("DeusExConText.ConEventSetFlag{id}"));
        let mut fr = match s.vm.get_prop(ev, "flagRef", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        while fr != 0 {
            let name = match s.vm.get_prop(fr, "flagName", 0) { Ok(Value::Name(n)) => s.vm.reg.names.str(n).to_string(), _ => String::new() };
            eprintln!("event {id}: {name} = {:?}, expiration {:?}", s.vm.get_prop(fr, "value", 0).ok(), s.vm.get_prop(fr, "expiration", 0).ok());
            fr = match s.vm.get_prop(fr, "nextFlagRef", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        }
    }
}

/// A once-only skill award (`SkillAwardTrigger`, `bTriggerOnceOnly`) fired by an event pays once: the original honours
/// the flag only for touches, so anything that fires it again (a button, a flag trigger touched again) paid again.
/// The 'Ton's `StayedWithPaul` award, triggered three times.
#[test]
fn a_once_only_skill_award_pays_once() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "04_NYC_Hotel").unwrap();
    for _ in 0..10 {
        s.step_bound(DT);
    }
    let c = s.vm.reg.class_by_name("SkillAwardTrigger").unwrap();
    let tag = s.vm.reg.names.intern("StayedWithPaul");
    let award = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(c) && s.vm.get_prop(a, "Tag", 0).ok() == Some(Value::Name(tag))).expect("the award");
    assert_eq!(s.vm.get_prop(award, "bTriggerOnceOnly", 0).ok(), Some(Value::Bool(true)));
    let points = |s: &Session| s.vm.get_prop(s.player, "SkillPointsTotal", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0);
    let p = s.player;
    let start = points(&s);
    for _ in 0..3 {
        s.vm.call_by_name(award, "Trigger", vec![Value::Obj(p), Value::Obj(p)]).unwrap();
    }
    let gained = points(&s) - start;
    let each = s.vm.get_prop(award, "skillPointsAdded", 0).ok().and_then(|v| v.as_i32()).unwrap_or(-1);
    assert!(gained > 0, "it paid at all");
    assert_eq!(each, 0, "spent");
    assert!(gained <= 500 && gained == points(&s) - start, "paid once: {gained}");
    let again = points(&s);
    s.vm.call_by_name(award, "Trigger", vec![Value::Obj(p), Value::Obj(p)]).unwrap();
    assert_eq!(points(&s), again, "a fourth time pays nothing");
}

/// Unconscious bodies can die: shot (or blown up, burnt), and drowned when they land in water. In the original an
/// unconscious body kept its "Unconscious" name and no killer when shot (the mission scripts count it alive), and only a
/// body that appeared in water drowned, not one that fell in.
#[test]
fn unconscious_bodies_can_be_killed_and_drown() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    for _ in 0..10 {
        s.step_bound(DT);
    }
    let tc = s.vm.reg.class_by_name("Terrorist").unwrap();
    let cc = s.vm.reg.class_by_name("TerroristCarcass").unwrap();
    let p = s.player;
    let text = |s: &Session, a: u32, n: &str| match s.vm.get_prop(a, n, 0) { Ok(Value::Str(t)) => t, _ => String::new() };
    let mut knock_out = |s: &mut Session| -> u32 {
        let t = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(tc)).unwrap();
        let at = s.vm.location(t);
        let ko = s.vm.reg.names.intern("KnockedOut");
        s.vm.call_by_name(t, "TakeDamage", vec![Value::Int(1000), Value::Obj(p), Value::Vector(at), Value::Vector([0.0; 3]), Value::Name(ko)]).unwrap();
        for _ in 0..90 {
            s.step_bound(DT);
        }
        let bodies: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a) == Some(cc)).collect();
        bodies.into_iter().find(|&a| { let l = s.vm.location(a); (l[0] - at[0]).abs() + (l[1] - at[1]).abs() < 200.0 }).expect("the body")
    };
    // shot
    let body = knock_out(&mut s);
    assert_eq!(text(&s, body, "itemName"), "Unconscious");
    let at = s.vm.location(body);
    let shot = s.vm.reg.names.intern("Shot");
    s.vm.call_by_name(body, "TakeDamage", vec![Value::Int(5), Value::Obj(p), Value::Vector(at), Value::Vector([0.0; 3]), Value::Name(shot)]).unwrap();
    assert_eq!(s.vm.get_prop(body, "bNotDead", 0).ok(), Some(Value::Bool(false)), "shot dead");
    assert_ne!(text(&s, body, "itemName"), "Unconscious");
    assert_eq!(text(&s, body, "KillerBindName"), "JCDenton");
    // drowned: the second body is moved into the bay
    let body = knock_out(&mut s);
    assert_eq!(text(&s, body, "itemName"), "Unconscious");
    let zi = s.vm.reg.class_by_name("ZoneInfo").unwrap();
    let water = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).is_some_and(|c| s.vm.is_a(c, zi)) && s.vm.get_prop(a, "bWaterZone", 0).ok() == Some(Value::Bool(true))).expect("a water zone");
    let spot = s.vm.location(water);
    s.vm.call_by_name(body, "SetLocation", vec![Value::Vector(spot)]).unwrap();
    for _ in 0..5 {
        s.step_bound(DT);
    }
    assert_eq!(s.vm.get_prop(body, "bNotDead", 0).ok(), Some(Value::Bool(false)), "drowned");
    assert_eq!(s.vm.get_prop(body, "Mesh", 0).ok(), s.vm.get_prop(body, "Mesh3", 0).ok(), "the body in water");
}

/// The hazmat suit protects against what its description says (fire, electricity, EMP besides gases, radiation and
/// poison): the original's `DXReduceDamage` gives the suit's share only to the latter.
#[test]
fn the_hazmat_suit_protects_against_fire_electricity_and_emp() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..10 {
        s.step_bound(DT);
    }
    let p = s.player;
    let reduce = |s: &mut Session, kind: &str| -> i32 {
        let n = s.vm.reg.names.intern(kind);
        let mut a = vec![Value::Int(40), Value::Name(n), Value::Vector([0.0; 3]), Value::Int(0), Value::Bool(true)];
        s.vm.call_by_name_out(p, "DXReduceDamage", &mut a).unwrap();
        a[3].as_i32().unwrap()
    };
    let kinds = ["Burned", "Flamed", "Shocked", "EMP", "PoisonGas"];
    let without: Vec<i32> = kinds.iter().map(|k| reduce(&mut s, k)).collect();
    s.exec("Summon HazMatSuit");
    for _ in 0..10 {
        s.step_bound(DT);
    }
    let c = s.vm.reg.class_by_name("HazMatSuit").unwrap();
    // into the inventory (`Inventory.GiveTo`)
    let lying = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(c)).expect("the summoned suit");
    s.vm.call_by_name(lying, "GiveTo", vec![Value::Obj(p)]).unwrap();
    for _ in 0..5 {
        s.step_bound(DT);
    }
    let mut suit = 0;
    let mut item = match s.vm.get_prop(p, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    while item != 0 {
        if s.vm.class_of(item) == Some(c) {
            suit = item;
        }
        item = match s.vm.get_prop(item, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    }
    assert_ne!(suit, 0, "the suit is carried");
    s.vm.call_by_name(suit, "Activate", vec![]).unwrap();
    for _ in 0..5 {
        s.step_bound(DT);
    }
    assert_eq!(s.vm.get_prop(suit, "bActive", 0).ok(), Some(Value::Bool(true)));
    let with: Vec<i32> = kinds.iter().map(|k| reduce(&mut s, k)).collect();
    eprintln!("without {without:?}, with {with:?}");
    for i in 0..4 {
        assert!(with[i] < without[i], "{} not reduced: {} -> {}", kinds[i], without[i], with[i]);
    }
    assert!(with[4] < without[4], "poison gas as before");
}

/// A decoration carried when a third-person conversation starts is set down, not thrown at the person spoken to (the
/// conversation puts away what the player holds through `DropDecoration`, which throws it forward).
#[test]
fn a_carried_object_is_set_down_when_a_conversation_starts() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..10 {
        s.step_bound(DT);
    }
    let pc = s.vm.reg.class_by_name("PaulDenton").unwrap();
    let paul = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(pc)).unwrap();
    let at = s.vm.location(paul);
    let yaw = match s.vm.get_prop(paul, "Rotation", 0) { Ok(Value::Rotator(r)) => r[1] as f32 * std::f32::consts::TAU / 65536.0, _ => 0.0 };
    s.teleport([at[0] + 200.0 * yaw.cos(), at[1] + 200.0 * yaw.sin(), at[2]], (yaw + std::f32::consts::PI).to_degrees());
    for _ in 0..10 {
        s.step_bound(DT);
    }
    let bc = s.vm.reg.class_by_name("Basketball").unwrap();
    let me = s.player_location();
    let ball = s.vm.spawn(bc, 0, None, Some([me[0], me[1], me[2] + 120.0]), None).unwrap().expect("the ball");
    let p = s.player;
    s.vm.set_prop(p, "CarriedDecoration", 0, Value::Obj(ball)).unwrap();
    s.vm.call_by_name(p, "PutCarriedDecorationInHand", vec![]).unwrap();
    for _ in 0..5 {
        s.step_bound(DT);
    }
    assert_eq!(s.vm.get_prop(p, "CarriedDecoration", 0).ok(), Some(Value::Obj(ball)), "carried");
    assert_eq!(s.vm.call_by_name(p, "StartConversation", vec![Value::Obj(paul), Value::Byte(1)]).unwrap(), Value::Bool(true));
    let mut fastest = 0.0f32;
    for _ in 0..20 {
        s.step_bound(DT);
        if let Ok(Value::Vector(v)) = s.vm.get_prop(ball, "Velocity", 0) {
            fastest = fastest.max((v[0] * v[0] + v[1] * v[1]).sqrt());
        }
    }
    assert_ne!(s.vm.get_prop(p, "CarriedDecoration", 0).ok(), Some(Value::Obj(ball)), "put away");
    eprintln!("the ball's fastest horizontal speed: {fastest}");
    assert!(fastest < 50.0, "thrown: {fastest}");
}

/// Aggressive Defense detonates missiles and grenades (its description; the game marks every other projectile
/// `bIgnoresNanoDefense`), not darts, throwing knives or flamethrower fire, which the original's single-player search
/// took too: with a dart near and a rocket farther, it picks the rocket.
#[test]
fn aggressive_defense_leaves_darts_and_knives_alone() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..10 {
        s.step_bound(DT);
    }
    let p = s.player;
    let ac = s.vm.reg.class_by_name("AugDefense").unwrap();
    let aug = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(ac) && s.vm.get_prop(a, "Player", 0).ok() == Some(Value::Obj(p))).expect("the player's AugDefense");
    let me = s.player_location();
    let spawn = |s: &mut Session, class: &str, dist: f32| -> u32 {
        let c = s.vm.reg.class_by_name(class).unwrap();
        let a = s.vm.spawn(c, 0, None, Some([me[0] + dist, me[1], me[2] + 40.0]), None).unwrap().expect(class);
        s.vm.set_prop(a, "Velocity", 0, Value::Vector([-800.0, 0.0, 0.0])).unwrap();
        a
    };
    let dart = spawn(&mut s, "Dart", 150.0);
    let rocket = spawn(&mut s, "Rocket", 400.0);
    let got = s.vm.call_by_name(aug, "FindNearestProjectile", vec![]).unwrap();
    assert_eq!(got, Value::Obj(rocket), "the rocket, not the dart ({dart})");
    s.vm.destroy(rocket).unwrap();
    let got = s.vm.call_by_name(aug, "FindNearestProjectile", vec![]).unwrap();
    assert_ne!(got, Value::Obj(dart), "never the dart");
}

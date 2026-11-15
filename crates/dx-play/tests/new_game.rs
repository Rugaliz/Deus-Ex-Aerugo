//! The path from the start of the game to a running game: the main menu, New Game, the character screen, the intro level
//! (skipped with the menu key) and level travel into the first mission. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
        let _ = s.ui_quads(800, 600);
    }
}

/// The texts of the visible buttons (for failure messages).
fn visible_buttons(s: &mut Session) -> Vec<String> {
    let root = s.vm.ui_root(s.player);
    let (mut stack, mut out) = (vec![root], Vec::new());
    while let Some(w) = stack.pop() {
        if let (Ok(Value::Str(b)), Ok(Value::Bool(true))) = (s.vm.get_prop(w, "buttonText", 0), s.vm.get_prop(w, "bIsVisible", 0)) {
            if !b.is_empty() {
                out.push(b);
            }
        }
        let mut c = match s.vm.get_prop(w, "firstChild", 0) { Ok(Value::Obj(c)) => c, _ => 0 };
        while c != 0 {
            stack.push(c);
            c = match s.vm.get_prop(c, "nextSibling", 0) { Ok(Value::Obj(n)) => n, _ => 0 };
        }
    }
    out
}

/// Clicks the visible button with this text (any depth below the root).
fn click_text(s: &mut Session, text: &str) -> bool {
    let root = s.vm.ui_root(s.player);
    let mut stack = vec![root];
    while let Some(w) = stack.pop() {
        let t = matches!(s.vm.get_prop(w, "buttonText", 0), Ok(Value::Str(ref b)) if b.replace('|', "").replace('&', "").eq_ignore_ascii_case(text));
        let visible = matches!(s.vm.get_prop(w, "bIsVisible", 0), Ok(Value::Bool(true)));
        if t && visible {
            let (ox, oy) = s.vm.ui_window_origin(w);
            let [_, _, ww, wh] = s.vm.ui_geometry(w);
            assert!(s.vm.ui_mouse_position(s.player, ox + ww / 2.0, oy + wh / 2.0));
            assert!(s.ui_key(1, true), "press on {text} not taken");
            assert!(s.ui_key(1, false));
            return true;
        }
        let mut c = match s.vm.get_prop(w, "firstChild", 0) { Ok(Value::Obj(c)) => c, _ => 0 };
        while c != 0 {
            stack.push(c);
            c = match s.vm.get_prop(c, "nextSibling", 0) { Ok(Value::Obj(n)) => n, _ => 0 };
        }
    }
    false
}

#[test]
fn new_game_from_the_menu_reaches_the_first_mission() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("ShowMainMenu");
    run(&mut s, 5);
    assert!(s.ui_wants_mouse(), "the main menu did not open");
    // a fresh configuration asks once whether to play the training first (`bAskedToTrain`)
    s.vm.set_prop(s.player, "bAskedToTrain", 0, Value::Bool(true)).unwrap();
    assert!(click_text(&mut s, "New Game"), "no New Game button");
    run(&mut s, 3);
    // character screen: the defaults are fine, "Start" begins
    let mut tried = Vec::new();
    for label in ["Medium", "Start Game"] {
        if label == "Start Game" {
            let root = s.vm.ui_root(s.player);
            let bar = s.vm.ui_find_child(root, "MenuUIActionButtonBarWindow");
            eprintln!("bar {:?} geometry {:?}", bar, s.vm.ui_geometry(bar));
            let mut c = match s.vm.get_prop(bar, "firstChild", 0) { Ok(Value::Obj(c)) => c, _ => 0 };
            while c != 0 {
                eprintln!("  button {:?} geom {:?} vis {:?} text {:?} pref {:?}", c, s.vm.ui_geometry(c), s.vm.get_prop(c, "bIsVisible", 0), s.vm.get_prop(c, "buttonText", 0), s.vm.call_by_name(c, "QueryPreferredSize", vec![Value::Float(0.0), Value::Float(0.0)]));
                c = match s.vm.get_prop(c, "nextSibling", 0) { Ok(Value::Obj(n)) => n, _ => 0 };
            }
            for i in 0..3u32 {
                eprintln!("  slot {i}: {:?} {:?}", s.vm.get_prop(bar, "actionButtons", i), s.vm.get_prop(bar, "buttonCount", 0));
            }
            let root = s.vm.ui_root(s.player);
            let edit = s.vm.ui_find_child(root, "EditWindow");
            eprintln!("edit window {edit}: {:?}; focus {:?}", s.vm.call_by_name(edit, "GetText", vec![]), s.vm.get_prop(root, "focusWindow", 0));
        }
        if click_text(&mut s, label) {
            tried.push(label);
            eprintln!("clicked {label}: travel request {:?}; log tail {:?}", s.vm.travel_request, s.vm.log.iter().rev().take(4).collect::<Vec<_>>());
            run(&mut s, 3);
        }
    }
    assert!(tried.contains(&"Start Game"), "the screens had no Start Game button (clicked {tried:?}; visible now: {:?})", visible_buttons(&mut s));
    run(&mut s, 5);
    let (buttons, issues) = (visible_buttons(&mut s), s.issues.iter().take(3).map(|i| format!("{} {}: {}", i.class, i.event, i.error)).collect::<Vec<_>>());
    assert_eq!(s.map_name, "00_Intro", "New Game should go to the intro first (visible buttons: {buttons:?}; issues: {issues:?})");
    // the menu key skips the intro and the game starts
    assert!(s.exec("ShowMainMenu"));
    run(&mut s, 10);
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland", "skipping the intro did not start the first mission");
    let health = match s.vm.get_prop(s.player, "Health", 0) {
        Ok(Value::Int(h)) => h,
        other => panic!("Health: {other:?}"),
    };
    assert!(health > 0, "the player is dead");
    assert!(s.issues.iter().filter(|i| i.class == "travel").count() == 0, "travel errors: {:?}", s.issues.iter().filter(|i| i.class == "travel").collect::<Vec<_>>());
}

/// Menu buttons click audibly: `XButtonWindow` plays its press and click sounds through `XWindow::PlaySound` (an
/// interface sound, source 0) at the window's sound volume.
#[test]
fn menu_buttons_play_their_sounds() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("ShowMainMenu");
    run(&mut s, 5);
    let _ = s.vm.take_sound_events();
    assert!(click_text(&mut s, "Settings"), "no Settings button");
    let ui: Vec<_> = s.vm.take_sound_events().into_iter().filter_map(|e| match e { ue_vm::ai::SoundEvent::Play(r) if r.source == 0 => Some(r), _ => None }).collect();
    assert!(!ui.is_empty(), "the button made no sound");
    assert!(ui.iter().all(|r| r.volume > 0.0), "silent button sounds {:?}", ui.iter().map(|r| r.volume).collect::<Vec<_>>());
}

/// Interface sounds as `XWindow::PlaySound` places them: one unit from the player's location, radius 100, volume
/// times 100/99; a point at the root's left edge turns the direction a quarter turn left of the view yaw, the right
/// edge a quarter turn right; a hidden window still plays (straight ahead).
#[test]
fn interface_sounds_are_placed_by_window_position() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("ShowMainMenu");
    run(&mut s, 5);
    let _ = s.vm.take_sound_events();
    assert!(click_text(&mut s, "Settings"), "no Settings button");
    let ui: Vec<_> = s.vm.take_sound_events().into_iter().filter_map(|e| match e { ue_vm::ai::SoundEvent::Play(r) if r.source == 0 => Some(r), _ => None }).collect();
    let p = s.player;
    let l = s.vm.location(p);
    assert!(!ui.is_empty());
    for r in &ui {
        let at = r.location.expect("an interface sound without a place");
        let d = ((at[0] - l[0]).powi(2) + (at[1] - l[1]).powi(2) + (at[2] - l[2]).powi(2)).sqrt();
        assert!((d - 1.0).abs() < 1e-3 && r.radius == 100.0, "placed {at:?} from {l:?}, radius {}", r.radius);
    }
    let sound = ui[0].sound;
    let root = match s.vm.get_prop(p, "rootWindow", 0) { Ok(Value::Obj(r)) => r, _ => panic!() };
    let width = s.vm.get_prop(root, "width", 0).unwrap().as_f32().unwrap();
    let yaw = match s.vm.get_prop(p, "ViewRotation", 0) { Ok(Value::Rotator(r)) => r[1], _ => 0 };
    let angle_at = |s: &mut Session, w: u32, x: f32| -> f32 {
        let _ = s.vm.take_sound_events();
        s.vm.call_by_name(w, "PlaySound", vec![Value::Obj(sound), Value::Float(0.5), Value::Float(1.0), Value::Float(x), Value::Float(0.0)]).unwrap();
        let r = s.vm.take_sound_events().into_iter().find_map(|e| match e { ue_vm::ai::SoundEvent::Play(r) => Some(r), _ => None }).expect("no sound");
        assert!((r.volume - 0.5 * 100.0 / 99.0).abs() < 1e-4, "volume {}", r.volume);
        let at = r.location.unwrap();
        (at[1] - l[1]).atan2(at[0] - l[0])
    };
    let base = (yaw & 0xffff) as f32 * std::f32::consts::TAU / 65536.0;
    let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    let left = wrap(angle_at(&mut s, root, 0.0) - base);
    let right = wrap(angle_at(&mut s, root, width) - base);
    let mid = wrap(angle_at(&mut s, root, width * 0.5) - base);
    eprintln!("turns from the view: left {left:.3}, middle {mid:.3}, right {right:.3} rad");
    let q = std::f32::consts::FRAC_PI_2;
    assert!((left + q).abs() < 1e-3 && (right - q).abs() < 1e-3 && mid.abs() < 1e-3);
    // a hidden window: plays, straight ahead
    let wclass = s.vm.reg.obj_id("Extension.Window").0;
    let hidden = s.vm.call_by_name(root, "NewChild", vec![Value::Obj(wclass)]).unwrap();
    if let Value::Obj(h) = hidden {
        s.vm.call_by_name(h, "Hide", vec![]).unwrap();
        assert!(wrap(angle_at(&mut s, h, 0.0) - base).abs() < 1e-3, "a hidden window's sound is panned");
    }
}

/// Typing sounds: a `ComputerWindow` plays its text sound for every printed character at the text cursor (so the pan
/// moves right along the line) and a `PlaySoundLater` sound in its turn; an edit window plays its type sound for each
/// character typed and its enter sound on Enter.
#[test]
fn typing_sounds_play_where_the_cursor_is() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("ShowMainMenu");
    run(&mut s, 5);
    let _ = s.vm.take_sound_events();
    assert!(click_text(&mut s, "Settings"));
    let sound = s.vm.take_sound_events().into_iter().find_map(|e| match e { ue_vm::ai::SoundEvent::Play(r) => Some(r.sound), _ => None }).unwrap();
    // the new screen's first button takes the focus at the root's next tick (with its focus sound)
    run(&mut s, 1);
    let _ = s.vm.take_sound_events();
    let p = s.player;
    let root = match s.vm.get_prop(p, "rootWindow", 0) { Ok(Value::Obj(r)) => r, _ => panic!() };
    // the turn from the player's view when the sound played: the windows tick before the level (`XGameEngineExt::Tick`),
    // so with the view of the previous step (the logo level moves the camera)
    let view = |s: &mut Session| (match s.vm.get_prop(p, "ViewRotation", 0) { Ok(Value::Rotator(r)) => r[1], _ => 0 }, s.vm.location(p));
    let turn = |(yaw, l): (i32, [f32; 3]), r: &ue_vm::ai::SoundRequest| {
        let base = (yaw & 0xffff) as f32 * std::f32::consts::TAU / 65536.0;
        let at = r.location.unwrap();
        ((at[1] - l[1]).atan2(at[0] - l[0]) - base + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
    };
    let cw_class = s.vm.reg.obj_id("Extension.ComputerWindow").0;
    let cw = match s.vm.call_by_name(root, "NewChild", vec![Value::Obj(cw_class)]).unwrap() { Value::Obj(w) => w, _ => panic!() };
    s.vm.call_by_name(cw, "SetTextSize", vec![Value::Int(40), Value::Int(5)]).unwrap();
    s.vm.call_by_name(cw, "SetTextSound", vec![Value::Obj(sound)]).unwrap();
    s.vm.call_by_name(cw, "Print", vec![Value::Str("abcdefgh".into()), Value::Bool(false)]).unwrap();
    s.vm.call_by_name(cw, "PlaySoundLater", vec![Value::Obj(sound)]).unwrap();
    let _ = s.vm.take_sound_events();
    let mut played = Vec::new();
    for _ in 0..60 {
        let before = view(&mut s);
        run(&mut s, 1);
        for e in s.vm.take_sound_events() {
            if let ue_vm::ai::SoundEvent::Play(r) = e {
                played.push(turn(before, &r));
            }
        }
    }
    eprintln!("computer window sounds (turn from the view, rad): {played:?}");
    assert_eq!(played.len(), 9, "8 characters and the queued sound");
    assert!(played[..8].windows(2).all(|w| w[1] > w[0]), "the pan does not follow the cursor");
    // the queued sound plays at the cursor after the last character
    assert!((played[8] - played[7]).abs() < 0.05);

    let ew_class = s.vm.reg.obj_id("Extension.EditWindow").0;
    s.vm.call_by_name(cw, "Destroy", vec![]).unwrap();
    // inside the top window (the open menu, a modal, has the keyboard)
    let top = match s.vm.call_by_name(root, "GetTopChild", vec![]).unwrap() { Value::Obj(w) if w != 0 => w, _ => root };
    let ew = match s.vm.call_by_name(top, "NewChild", vec![Value::Obj(ew_class)]).unwrap() { Value::Obj(w) => w, _ => panic!() };
    s.vm.call_by_name(ew, "SetSize", vec![Value::Float(200.0), Value::Float(20.0)]).unwrap();
    s.vm.call_by_name(ew, "SetEditSounds", vec![Value::Obj(sound), Value::Void, Value::Obj(sound), Value::Void]).unwrap();
    s.vm.call_by_name(top, "SetFocusWindow", vec![Value::Obj(ew)]).ok();
    run(&mut s, 2);
    let _ = s.vm.take_sound_events();
    for c in "hi".chars() {
        s.ui_char(c);
    }
    let typed = s.vm.take_sound_events().len();
    let enter = dx_input::key_code("Enter").unwrap();
    s.ui_key(enter, true);
    s.ui_key(enter, false);
    let entered = s.vm.take_sound_events().len();
    let text = s.vm.call_by_name(ew, "GetText", vec![]).unwrap();
    eprintln!("edit window: text {text:?}, {typed} type sounds, {entered} enter sounds");
    // Enter in a multi-line edit window starts a new line
    assert_eq!(text, Value::Str("hi\n".into()));
    assert_eq!((typed, entered), (2, 1));
}

/// Accelerator keys: a button whose text has `|&X` gets X as its key (`SetAcceleratorText`); with Alt held, a key the
/// screen's script leaves goes to that button (`XModalWindow::KeyPressed`), which presses itself. Alt+C on the New Game
/// screen is its Cancel button.
#[test]
fn alt_and_the_underlined_letter_press_a_button() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("ShowMainMenu");
    run(&mut s, 5);
    // a fresh configuration asks once whether to play the training first (`bAskedToTrain`)
    s.vm.set_prop(s.player, "bAskedToTrain", 0, Value::Bool(true)).unwrap();
    assert!(click_text(&mut s, "New Game"));
    run(&mut s, 5);
    // the difficulty first, then the screen with the action buttons
    assert!(click_text(&mut s, "Medium"));
    run(&mut s, 5);
    let root = s.vm.ui_root(s.player);
    let top = |s: &mut Session| match s.vm.call_by_name(root, "GetTopChild", vec![]).unwrap() { Value::Obj(w) => w, _ => 0 };
    let screen = top(&mut s);
    let class = |s: &Session, w: u32| s.vm.class_of(w).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
    eprintln!("screen {}", class(&s, screen));
    // the windows of this screen that have an accelerator key
    let mut keys = Vec::new();
    let mut stack = vec![screen];
    while let Some(w) = stack.pop() {
        let k = s.vm.get_prop(w, "acceleratorKey", 0).unwrap().as_i32().unwrap_or(0);
        if k != 0 {
            keys.push((w, char::from_u32(k as u32).unwrap(), s.vm.get_prop(w, "buttonText", 0).ok()));
        }
        let mut c = match s.vm.get_prop(w, "firstChild", 0) { Ok(Value::Obj(c)) => c, _ => 0 };
        while c != 0 {
            stack.push(c);
            c = match s.vm.get_prop(c, "nextSibling", 0) { Ok(Value::Obj(n)) => n, _ => 0 };
        }
    }
    eprintln!("accelerators: {keys:?}");
    let cancel = keys.iter().find(|k| matches!(&k.2, Some(Value::Str(t)) if t.to_ascii_lowercase().contains("cancel"))).expect("no accelerator for Cancel");
    assert_eq!(s.vm.call_by_name(screen, "IsCurrentModal", vec![]).unwrap(), Value::Bool(true));
    let alt = dx_input::key_code("Alt").unwrap();
    s.ui_key(alt, true);
    s.ui_char(cancel.1.to_ascii_lowercase());
    s.ui_key(alt, false);
    run(&mut s, 30);
    let now = top(&mut s);
    eprintln!("after Alt+C: {}", class(&s, now));
    assert_ne!(now, screen, "Alt+C did not cancel the New Game screen");
    // a style change (a new colour theme) reaches every window's StyleChanged without script errors
    let before = s.issues.len();
    s.vm.log.clear();
    s.vm.call_by_name(root, "ChangeStyle", vec![]).unwrap();
    run(&mut s, 2);
    assert_eq!(s.issues.len(), before, "script errors after ChangeStyle: {:?}", &s.issues[before..]);
}

/// The training's test subject lies unconscious (user report): the menu's `StartTrainingMission` travels, the
/// engine's `SetClientTravel` sends `PreTravel` to every actor, the player sets `PlayerTraveling`, and the mission
/// script's `FirstFrame` knocks the trooper out; he becomes a carcass that can be searched and carried.
#[test]
fn the_training_test_subject_is_knocked_out() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&g, "DX").unwrap();
    let p = s.player;
    s.vm.call_by_name(p, "StartTrainingMission", vec![]).unwrap();
    for _ in 0..240 {
        s.step_bound(1.0 / 30.0);
    }
    let carcass = s.vm.reg.class_by_name("UNATCOTroopCarcass").unwrap();
    let troop = s.vm.reg.class_by_name("UNATCOTroop").unwrap();
    let tag = |s: &mut dx_play::Session, a: u32| match s.vm.get_prop(a, "Tag", 0) {
        Ok(ue_vm::Value::Name(n)) => s.vm.reg.names.str(n).to_string(),
        _ => String::new(),
    };
    let actors = s.vm.live_actors();
    let mut body = false;
    for a in actors {
        let Some(c) = s.vm.class_of(a) else { continue };
        if tag(&mut s, a) != "Test_Subject" {
            continue;
        }
        assert!(!s.vm.is_a(c, troop) || s.vm.is_a(c, carcass), "the test subject is still a standing trooper");
        body |= s.vm.is_a(c, carcass);
    }
    assert!(body, "the test subject's body is there");
    // the travel carries only what differs from the defaults: `ResetSkills` put the pistol skill at its default 0, so
    // the fresh skill keeps the 1 its `PreBeginPlay` gives it (TRAINED, as in the original)
    let pistol = s.vm.reg.class_by_name("SkillWeaponPistol").unwrap();
    let p = s.player;
    let sys = match s.vm.get_prop(p, "SkillSystem", 0) { Ok(ue_vm::Value::Obj(o)) => o, _ => 0 };
    let mut sk = match s.vm.get_prop(sys, "FirstSkill", 0) { Ok(ue_vm::Value::Obj(o)) => o, _ => 0 };
    let mut levels = Vec::new();
    while sk != 0 && levels.len() < 64 {
        if s.vm.class_of(sk) == Some(pistol) {
            levels.push(s.vm.get_prop(sk, "CurrentLevel", 0).ok().and_then(|v| v.as_i32()).unwrap_or(-1));
        }
        sk = match s.vm.get_prop(sk, "next", 0) { Ok(ue_vm::Value::Obj(o)) => o, _ => 0 };
    }
    assert_eq!(levels, vec![1], "the pistol skill starts trained");
}

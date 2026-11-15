//! The window system through the game's own HUD scripts. Set `DEUSEX_DIR` to the game folder.
//! Expected sizes come from the scripts (`SetSize(541, 69)`, full alignment), not from the code under test.

use std::path::PathBuf;
use ue_config::Config;
use ue_registry::Registry;
use ue_vm::{Value, Vm};

fn world() -> Option<(Vm, u32)> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut reg = Registry::load_game(&g).unwrap();
    reg.set_config(Config::new(&g.join("System"), None));
    let mut vm = Vm::new(reg);
    vm.init_world().unwrap();
    let pi = vm.reg.add_package(&g.join("Maps").join("00_Training.dx"), "00_Training").unwrap();
    vm.load_level(pi).unwrap();
    let player = vm.login_player("JCDentonMale").unwrap();
    Some((vm, player))
}

#[test]
fn the_root_window_builds_the_hud_tree_and_lays_it_out() {
    let Some((mut vm, player)) = world() else { return };
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    assert_ne!(root, 0, "the player has no root window");
    assert_eq!(vm.ui_geometry(root), [0.0, 0.0, 640.0, 480.0]);
    assert!(vm.ui_find_child(root, "DeusExHUD") != 0, "no HUD in the root window");
    let hud = vm.ui_find_child(root, "DeusExHUD");
    // the root script aligns the HUD fully: it takes the size of the root
    assert_eq!(vm.ui_geometry(hud)[2..], [640.0, 480.0]);
    // the belt hard-codes its size in InitWindow
    let belt = vm.ui_find_child(hud, "HUDObjectBelt");
    assert_ne!(belt, 0);
    assert_eq!(vm.ui_geometry(belt)[2..], [541.0, 69.0]);
    // a larger screen is divided by a whole multiplier: 1920x1080 is laid out as 960x540
    vm.ui_set_screen(player, 1920.0, 1080.0);
    assert_eq!(vm.ui_geometry(root)[2..], [960.0, 540.0]);
    assert_eq!(vm.ui_geometry(hud)[2..], [960.0, 540.0]);
    // every window has been told it is ready
    assert_eq!(vm.get_prop(root, "initCount", 0).unwrap(), Value::Int(0));
}

#[test]
fn destroying_a_window_removes_it_from_the_tree() {
    let Some((mut vm, player)) = world() else { return };
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    let hud = vm.ui_find_child(root, "DeusExHUD");
    let belt = vm.ui_find_child(hud, "HUDObjectBelt");
    assert_ne!(belt, 0);
    vm.call_by_name(belt, "Destroy", vec![]).unwrap();
    assert_eq!(vm.ui_find_child(hud, "HUDObjectBelt"), 0);
    assert!(!vm.is_live(belt));
}

#[test]
fn painting_the_hud_produces_a_draw_list() {
    let Some((mut vm, player)) = world() else { return };
    let list = vm.ui_paint(player, 640.0, 480.0);
    let failures: Vec<_> = vm.log.iter().filter(|l| l.contains("window event")).cloned().collect();
    eprintln!("DRAW {} commands; failures {:?}", list.len(), failures);
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    let hud = vm.ui_find_child(root, "DeusExHUD");
    let belt = vm.ui_find_child(hud, "HUDObjectBelt");
    eprintln!("belt geometry {:?}", vm.ui_geometry(belt));
    for c in list.iter().take(12) {
        eprintln!("  {:?}", c);
    }
    assert!(!list.is_empty());
    assert!(failures.is_empty(), "{failures:?}");
    for c in &list {
        assert!(c.x >= -0.01 && c.y >= -0.01 && c.x + c.w <= 640.01 && c.y + c.h <= 480.01, "off screen: {c:?}");
    }
}

/// The main menu opens through the player's `ShowMainMenu`, takes the mouse, and clicking a button reaches the script
/// that belongs to it: "Exit" asks for confirmation in a message box.
#[test]
fn the_main_menu_takes_mouse_clicks() {
    let Some((mut vm, player)) = world() else { return };
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    assert!(!vm.ui_wants_mouse(player));
    assert!(vm.exec_command(player, "ShowMainMenu").unwrap());
    vm.ui_paint(player, 640.0, 480.0);
    assert!(vm.ui_wants_mouse(player), "an open menu must take the mouse");
    // find the Exit button by its text and click its middle
    let mut exit = 0;
    let menu = vm.ui_find_child(root, "MenuMain");
    assert_ne!(menu, 0, "no main menu window");
    let mut stack = vec![menu];
    while let Some(w) = stack.pop() {
        if matches!(vm.get_prop(w, "buttonText", 0), Ok(Value::Str(ref t)) if t == "Exit") {
            exit = w;
        }
        let mut c = match vm.get_prop(w, "firstChild", 0) { Ok(Value::Obj(c)) => c, _ => 0 };
        while c != 0 {
            stack.push(c);
            c = match vm.get_prop(c, "nextSibling", 0) { Ok(Value::Obj(n)) => n, _ => 0 };
        }
    }
    assert_ne!(exit, 0, "no Exit button");
    let (ox, oy) = vm.ui_window_origin(exit);
    let [_, _, w, h] = vm.ui_geometry(exit);
    assert!(vm.ui_mouse_position(player, ox + w / 2.0, oy + h / 2.0));
    assert!(vm.ui_process_key(player, 1, 1), "press not taken");
    assert!(vm.ui_process_key(player, 1, 3), "release not taken");
    vm.ui_paint(player, 640.0, 480.0);
    assert_ne!(vm.ui_find_child(root, "MenuUIMessageBoxWindow"), 0, "clicking Exit did not ask for confirmation");
}

/// Every menu screen of the game opens and draws something. (Several still log failed window events for widgets that are
/// not ported yet: scroll areas, sliders, lists; `DX_UI_PROBE=1` prints them.)
#[test]
fn every_menu_screen_opens_and_draws() {
    let Some((mut vm, player)) = world() else { return };
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    let screens = ["MenuSelectDifficulty", "MenuScreenNewGame", "MenuSettings", "MenuScreenOptions", "MenuScreenControls", "MenuScreenDisplay", "MenuScreenSound", "MenuScreenLoadGame", "MenuScreenSaveGame", "MenuScreenBrightness", "MenuScreenAdjustColors", "MenuScreenCustomizeKeys", "MenuScreenThemesLoad", "MenuScreenRGB"];
    for sc in screens {
        vm.log.clear();
        let c = vm.reg.class_by_name(sc).unwrap_or_else(|| panic!("no class {sc}"));
        let path = vm.reg.class(c).path.clone();
        let cls = Value::Obj(vm.reg.obj_id(&path).0);
        vm.call_by_name(root, "InvokeMenuScreen", vec![cls, Value::Bool(true)]).unwrap();
        // the save screen hides the root while it takes a snapshot of the 3D view: a tick shows it again
        vm.ui_tick(root, 0.05);
        vm.ui_tick(root, 0.05);
        let list = vm.ui_paint(player, 640.0, 480.0);
        if std::env::var_os("DX_UI_PROBE").is_some() {
            let seen: std::collections::BTreeSet<_> = vm.log.iter().cloned().collect();
            eprintln!("SCREEN {sc}: {} draws", list.len());
            for l in seen.iter().take(8) {
                eprintln!("   {l}");
            }
        }
        assert!(list.len() > 50, "{sc} drew only {} rectangles", list.len());
        vm.call_by_name(root, "ClearWindowStack", vec![]).unwrap();
    }
}

/// A slider of the sound menu sits at the position of its configured value, jumps to where the mouse presses and follows
/// the mouse while dragging.
#[test]
fn a_slider_follows_the_mouse() {
    let Some((mut vm, player)) = world() else { return };
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    let c = vm.reg.class_by_name("MenuScreenSound").expect("no MenuScreenSound");
    let path = vm.reg.class(c).path.clone();
    let cls = Value::Obj(vm.reg.obj_id(&path).0);
    vm.call_by_name(root, "InvokeMenuScreen", vec![cls, Value::Bool(true)]).unwrap();
    vm.ui_paint(player, 640.0, 480.0);
    assert!(vm.ui_wants_mouse(player));
    let scale = vm.ui_find_child(root, "ScaleWindow");
    assert_ne!(scale, 0, "no slider on the sound screen");
    let tick = |vm: &mut Vm| match vm.call_by_name(scale, "GetTickPosition", vec![]) {
        Ok(Value::Int(t)) => t,
        other => panic!("GetTickPosition: {other:?}"),
    };
    // the music volume of the shipped ini is 153 of 255 on an 11 tick scale
    assert_eq!(tick(&mut vm), 6, "slider not at the configured value");
    let (ox, oy) = vm.ui_window_origin(scale);
    let [_, _, w, h] = vm.ui_geometry(scale);
    assert!(vm.ui_mouse_position(player, ox + w - 3.0, oy + h / 2.0));
    assert!(vm.ui_process_key(player, 1, 1), "press not taken");
    assert_eq!(tick(&mut vm), 10, "pressing the right end did not move the thumb there");
    assert!(vm.ui_mouse_position(player, ox + 3.0, oy + h / 2.0));
    assert_eq!(tick(&mut vm), 0, "dragging to the left end did not follow");
    assert!(vm.ui_process_key(player, 1, 3));
    assert_eq!(tick(&mut vm), 0);
}

fn open_screen(vm: &mut Vm, player: u32, name: &str) -> u32 {
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    let c = vm.reg.class_by_name(name).unwrap_or_else(|| panic!("no class {name}"));
    let path = vm.reg.class(c).path.clone();
    let cls = Value::Obj(vm.reg.obj_id(&path).0);
    vm.call_by_name(root, "InvokeMenuScreen", vec![cls, Value::Bool(true)]).unwrap();
    vm.ui_tick(root, 0.05);
    vm.ui_paint(player, 640.0, 480.0);
    root
}

/// The key bindings list shows rows (one per action), a click selects the clicked row and the arrow keys move the selection.
#[test]
fn a_list_selects_with_mouse_and_keys() {
    let Some((mut vm, player)) = world() else { return };
    let root = open_screen(&mut vm, player, "MenuScreenCustomizeKeys");
    let list = vm.ui_find_child(root, "ListWindow");
    assert_ne!(list, 0, "no list on the key screen");
    let int = |vm: &mut Vm, f: &str, args: Vec<Value>| match vm.call_by_name(list, f, args) {
        Ok(Value::Int(i)) => i,
        other => panic!("{f}: {other:?}"),
    };
    let rows = int(&mut vm, "GetNumRows", vec![]);
    assert!(rows > 20, "only {rows} rows");
    // click the third row
    let (ox, oy) = vm.ui_window_origin(list);
    let line = match vm.get_prop(list, "lineSize", 0) {
        Ok(Value::Float(f)) => f,
        other => panic!("lineSize: {other:?}"),
    };
    assert!(line > 4.0, "line size {line}");
    assert!(vm.ui_mouse_position(player, ox + 20.0, oy + line * 2.5));
    assert!(vm.ui_process_key(player, 1, 1));
    assert!(vm.ui_process_key(player, 1, 3));
    let sel = int(&mut vm, "GetSelectedRow", vec![]);
    assert_eq!(int(&mut vm, "RowIdToIndex", vec![Value::Int(sel)]), 2, "the click did not select the third row");
    assert_eq!(int(&mut vm, "GetNumSelectedRows", vec![]), 1);
    // the down arrow (virtual key 0x28) moves to the fourth row
    assert!(vm.ui_process_key(player, 0x28, 1));
    assert!(vm.ui_process_key(player, 0x28, 3));
    let sel = int(&mut vm, "GetSelectedRow", vec![]);
    assert_eq!(int(&mut vm, "RowIdToIndex", vec![Value::Int(sel)]), 3, "the down arrow did not move the selection");
}

/// Typing into the real-name field of the new game screen changes its text, backspace removes it again.
#[test]
fn an_edit_window_takes_typing() {
    let Some((mut vm, player)) = world() else { return };
    let root = open_screen(&mut vm, player, "MenuScreenNewGame");
    let edit = vm.ui_find_child(root, "EditWindow");
    assert_ne!(edit, 0, "no edit window");
    // the real name is the second edit window: find them all
    let mut edits = Vec::new();
    let mut stack = vec![root];
    let edit_class = vm.reg.class_by_name("EditWindow").unwrap();
    while let Some(w) = stack.pop() {
        if vm.class_of(w).map(|k| vm.is_a(k, edit_class)).unwrap_or(false) {
            edits.push(w);
        }
        let mut c = match vm.get_prop(w, "firstChild", 0) { Ok(Value::Obj(c)) => c, _ => 0 };
        while c != 0 {
            stack.push(c);
            c = match vm.get_prop(c, "nextSibling", 0) { Ok(Value::Obj(n)) => n, _ => 0 };
        }
    }
    assert!(!edits.is_empty());
    let text = |vm: &mut Vm, w: u32| match vm.call_by_name(w, "GetText", vec![]) {
        Ok(Value::Str(s)) => s,
        other => panic!("GetText: {other:?}"),
    };
    // the focused one (the screen focuses the name field)
    let focus = match vm.get_prop(root, "focusWindow", 0) { Ok(Value::Obj(f)) => f, _ => 0 };
    assert!(edits.contains(&focus), "the name field has no focus");
    let before = text(&mut vm, focus);
    assert!(!before.is_empty());
    // the whole name is selected: typing replaces it
    assert!(vm.ui_process_char(player, 'Z'));
    assert_eq!(text(&mut vm, focus), "Z");
    assert!(vm.ui_process_char(player, 'q'));
    assert_eq!(text(&mut vm, focus), "Zq");
    // backspace (virtual key 8)
    assert!(vm.ui_process_key(player, 8, 1));
    assert!(vm.ui_process_key(player, 8, 3));
    assert_eq!(text(&mut vm, focus), "Z");
}

/// A word-wrapped multi-line edit moves its caret by the rows the text wraps into (`MoveInsertionPoint` over the row
/// data): down keeps the x, end of row stops before the consumed space, the caret's y steps by one row.
#[test]
fn a_wrapped_edit_moves_its_caret_by_rows() {
    let Some((mut vm, player)) = world() else { return };
    let root = open_screen(&mut vm, player, "MenuScreenNewGame");
    let edit = match vm.get_prop(root, "focusWindow", 0) { Ok(Value::Obj(f)) => f, _ => 0 };
    assert_ne!(edit, 0);
    vm.call_by_name(edit, "EnableSingleLineEditing", vec![Value::Bool(false)]).unwrap();
    vm.call_by_name(edit, "SetWordWrap", vec![Value::Bool(true)]).unwrap();
    vm.call_by_name(edit, "SetMaxSize", vec![Value::Int(400)]).unwrap();
    vm.call_by_name(edit, "SetSize", vec![Value::Float(70.0), Value::Float(200.0)]).unwrap();
    let text = "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj kkkk llll mmmm nnnn oooo pppp qqqq rrrr ssss tttt";
    vm.call_by_name(edit, "SetText", vec![Value::Str(text.into())]).unwrap();
    let mv = |vm: &mut Vm, how: u8| {
        vm.call_by_name(edit, "MoveInsertionPoint", vec![Value::Byte(how), Value::Bool(false)]).unwrap();
        match vm.get_prop(edit, "insertPos", 0) { Ok(Value::Int(p)) => p as usize, other => panic!("{other:?}") }
    };
    let f = |vm: &mut Vm, n: &str| match vm.get_prop(edit, n, 0) { Ok(Value::Float(v)) => v, other => panic!("{n}: {other:?}") };
    assert_eq!(mv(&mut vm, 10), 0);
    let y0 = f(&mut vm, "insertY");
    // the second row starts after a space
    let p = mv(&mut vm, 1);
    assert!(p > 0 && p < text.len() && text.as_bytes()[p - 1] == b' ', "down went to {p}");
    let lh = f(&mut vm, "lineHeight");
    let v = match vm.get_prop(edit, "vSpace", 0) { Ok(Value::Float(v)) => v, _ => 0.0 };
    assert_eq!(f(&mut vm, "insertY"), y0 + lh + v);
    // the row's end is the consumed space before the next row
    let e = mv(&mut vm, 7);
    assert_eq!(text.as_bytes()[e], b' ', "end of row at {e} (row 1 at {p})");
    // start of row comes back to the row start; word right moves to the next word on the row or the row end
    assert_eq!(mv(&mut vm, 6), p);
    let wr = mv(&mut vm, 5);
    assert!(wr > p && wr <= e, "word right went to {wr}");
    // up goes back to row 0, at the column of the preferred x (0)
    assert_eq!(mv(&mut vm, 6), p);
    assert_eq!(mv(&mut vm, 0), 0);
    assert_eq!(mv(&mut vm, 11), text.len());
}

/// `XComputerWindow::Init` makes a full-size text window and a hidden input edit window, and the window is selectable.
#[test]
fn a_computer_window_has_its_text_and_input_windows() {
    let Some((mut vm, player)) = world() else { return };
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    let class = vm.reg.obj_id("Extension.ComputerWindow").0;
    let cw = match vm.call_by_name(root, "NewChild", vec![Value::Obj(class)]).unwrap() { Value::Obj(w) => w, other => panic!("{other:?}") };
    let obj = |vm: &Vm, n: &str| match vm.get_prop(cw, n, 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    let flag = |vm: &Vm, w: u32, n: &str| matches!(vm.get_prop(w, n, 0), Ok(Value::Bool(true)));
    let (text, input) = (obj(&vm, "textWindow"), obj(&vm, "inputWindow"));
    assert!(text != 0 && input != 0, "text {text}, input {input}");
    let edit = vm.reg.class_by_name("EditWindow").unwrap();
    assert!(vm.class_of(input).is_some_and(|c| vm.is_a(c, edit)));
    assert!(flag(&vm, text, "bIsVisible") && !flag(&vm, input, "bIsVisible"));
    assert!(flag(&vm, cw, "bIsSelectable"));
    assert_eq!(obj(&vm, "player"), player);
}

/// The in-game screens (inventory, skills, augmentations, goals, ...) open and draw. `DX_UI_PROBE=1` prints the failed
/// window events of each: the shopping list of missing widgets.
#[test]
fn every_persona_screen_opens_and_draws() {
    let Some((mut vm, player)) = world() else { return };
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    let screens = ["PersonaScreenInventory", "PersonaScreenGoals", "PersonaScreenAugmentations", "PersonaScreenSkills", "PersonaScreenHealth", "PersonaScreenImages", "PersonaScreenConversations", "PersonaScreenLogs"];
    for sc in screens {
        vm.log.clear();
        let c = vm.reg.class_by_name(sc).unwrap_or_else(|| panic!("no class {sc}"));
        let path = vm.reg.class(c).path.clone();
        let cls = Value::Obj(vm.reg.obj_id(&path).0);
        let r = vm.call_by_name(root, "InvokeUIScreen", vec![cls, Value::Bool(true)]);
        vm.ui_tick(root, 0.05);
        vm.ui_tick(root, 0.05);
        let list = vm.ui_paint(player, 640.0, 480.0);
        if std::env::var_os("DX_UI_PROBE").is_some() {
            let seen: std::collections::BTreeSet<_> = vm.log.iter().cloned().collect();
            eprintln!("SCREEN {sc}: {:?} {} draws", r.is_ok(), list.len());
            for l in seen.iter().take(12) {
                eprintln!("   {l}");
            }
        }
        assert!(r.is_ok(), "{sc} did not open");
        assert!(list.len() > 50, "{sc} drew only {} rectangles", list.len());
        vm.call_by_name(root, "ClearWindowStack", vec![]).unwrap();
    }
}

/// A radio box keeps exactly one of its toggle buttons on (the music tool window of the game has one).
#[test]
fn a_radio_box_keeps_one_toggle_on() {
    let Some((mut vm, player)) = world() else { return };
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    let c = vm.reg.class_by_name("PlayMusicWindow").expect("no PlayMusicWindow");
    let path = vm.reg.class(c).path.clone();
    let cls = Value::Obj(vm.reg.obj_id(&path).0);
    vm.call_by_name(root, "InvokeUIScreen", vec![cls, Value::Bool(true)]).unwrap();
    vm.ui_paint(player, 640.0, 480.0);
    let win = vm.ui_find_child(root, "PlayMusicWindow");
    assert_ne!(win, 0);
    let obj = |vm: &mut Vm, o: u32, n: &str| match vm.get_prop(o, n, 0) { Ok(Value::Obj(r)) => r, other => panic!("{n}: {other:?}") };
    let radio = obj(&mut vm, win, "radSongType");
    let (ambient, combat) = (obj(&mut vm, win, "btnAmbient"), obj(&mut vm, win, "btnCombat"));
    let on = |vm: &mut Vm, o: u32| vm.get_prop(o, "bButtonPressed", 0).unwrap().as_bool().unwrap();
    let selected = |vm: &mut Vm| match vm.call_by_name(radio, "GetEnabledToggle", vec![]) { Ok(Value::Obj(r)) => r, other => panic!("{other:?}") };
    assert_eq!(selected(&mut vm), ambient, "the first toggle set on at creation is not the selection");
    vm.call_by_name(combat, "SetToggle", vec![Value::Bool(true)]).unwrap();
    assert_eq!(selected(&mut vm), combat);
    assert!(on(&mut vm, combat) && !on(&mut vm, ambient), "the previous toggle stayed on");
    // the one that is on cannot be switched off
    vm.call_by_name(combat, "SetToggle", vec![Value::Bool(false)]).unwrap();
    assert!(on(&mut vm, combat), "the selected toggle was switched off");
    assert_eq!(selected(&mut vm), combat);
}

/// `PopGC(-1)` pops one level, as `PopGC()` does (`execPopGC` treats -1 as "no argument").
#[test]
fn pop_gc_with_minus_one_pops_one_level() {
    let Some((mut vm, player)) = world() else { return };
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    let gc = match vm.call_by_name(root, "GetGC", vec![]).unwrap() { Value::Obj(g) => g, other => panic!("{other:?}") };
    let push = |vm: &mut Vm| vm.call_by_name(gc, "PushGC", vec![]).unwrap();
    push(&mut vm);
    let second = push(&mut vm);
    vm.call_by_name(gc, "PopGC", vec![Value::Int(-1)]).unwrap();
    assert_eq!(push(&mut vm), second);
}

/// The sounds played since the last call.
fn played(vm: &mut Vm) -> Vec<u32> {
    vm.take_sound_events().into_iter().filter_map(|e| match e { ue_vm::ai::SoundEvent::Play(r) => Some(r.sound), _ => None }).collect()
}

fn obj_prop(vm: &Vm, o: u32, n: &str) -> u32 {
    match vm.get_prop(o, n, 0) { Ok(Value::Obj(r)) => r, _ => 0 }
}

fn int_prop(vm: &Vm, o: u32, n: &str) -> i32 {
    match vm.get_prop(o, n, 0) { Ok(Value::Int(i)) => i, other => panic!("{n}: {other:?}") }
}

/// The menu list of the key bindings screen (`SetListSounds(Menu_Press, Menu_Focus)`): moving the focus plays the move
/// sound (`MoveToRow`), Enter the activate sound; PageDown moves by the rows that show in the scroll area (`GetPageSize`
/// takes the clip height), not to the end of the list (which is as tall as all its rows).
#[test]
fn a_menu_list_plays_its_sounds_and_pages_by_what_shows() {
    let Some((mut vm, player)) = world() else { return };
    let root = open_screen(&mut vm, player, "MenuScreenCustomizeKeys");
    let list = vm.ui_find_child(root, "ListWindow");
    assert_ne!(list, 0);
    let (activate, mv) = (obj_prop(&vm, list, "activateSound"), obj_prop(&vm, list, "moveSound"));
    assert!(activate != 0 && mv != 0 && activate != mv, "list sounds {activate} {mv}");
    let int = |vm: &mut Vm, f: &str, args: Vec<Value>| match vm.call_by_name(list, f, args) { Ok(Value::Int(i)) => i, other => panic!("{f}: {other:?}") };
    let focus_index = |vm: &mut Vm| {
        let id = int(vm, "GetFocusRow", vec![]);
        int(vm, "RowIdToIndex", vec![Value::Int(id)])
    };
    let line = vm.ui_float(list, "lineSize");
    let (ox, oy) = vm.ui_window_origin(list);
    let _ = played(&mut vm);
    assert!(vm.ui_mouse_position(player, ox + 20.0, oy + line * 2.5));
    assert!(vm.ui_process_key(player, 1, 1));
    assert!(vm.ui_process_key(player, 1, 3));
    assert_eq!(focus_index(&mut vm), 2);
    assert!(played(&mut vm).contains(&mv), "a click on another row played no move sound");
    // a press on the focused row again moves nothing: no sound (after the double-click time)
    vm.ui_tick(root, 1.0);
    let _ = played(&mut vm);
    assert!(vm.ui_process_key(player, 1, 1));
    assert!(vm.ui_process_key(player, 1, 3));
    assert!(!played(&mut vm).contains(&mv), "the move sound played without a focus change");
    assert!(vm.ui_process_key(player, 0x28, 1));
    assert!(vm.ui_process_key(player, 0x28, 3));
    assert_eq!(focus_index(&mut vm), 3);
    assert!(played(&mut vm).contains(&mv), "the down arrow played no move sound");
    // a page is the rows that fit in the visible part (the scroll area's clip window)
    vm.ui_paint(player, 640.0, 480.0);
    let parent = obj_prop(&vm, list, "winParent");
    let visible = vm.ui_geometry(parent)[3];
    let page = ((visible / line) as i32).max(1);
    let rows = int(&mut vm, "GetNumRows", vec![]);
    assert!(page + 3 < rows - 1, "the whole list shows: page {page} of {rows} rows");
    assert!(vm.ui_process_key(player, 0x22, 1));
    assert!(vm.ui_process_key(player, 0x22, 3));
    assert_eq!(focus_index(&mut vm), 3 + page, "PageDown did not move one visible page");
    let _ = played(&mut vm);
    assert!(vm.ui_process_key(player, 0x0d, 1));
    assert!(vm.ui_process_key(player, 0x0d, 3));
    assert!(played(&mut vm).contains(&activate), "Enter played no activate sound");
}

/// Type-ahead (`FindRowByKey`): the same letter again cycles through the rows starting with it, different letters match
/// the whole typed string from the focus row, upper case folds and the underscore is a hot key.
#[test]
fn a_list_type_ahead_cycles_and_matches_strings() {
    let Some((mut vm, player)) = world() else { return };
    let root = open_screen(&mut vm, player, "MenuScreenCustomizeKeys");
    let list = vm.ui_find_child(root, "ListWindow");
    assert_ne!(list, 0);
    let call = |vm: &mut Vm, f: &str, args: Vec<Value>| vm.call_by_name(list, f, args).unwrap();
    call(&mut vm, "EnableAutoSort", vec![Value::Bool(false)]);
    call(&mut vm, "DeleteAllRows", vec![]);
    for r in ["alpha", "b_c", "apple", "avocado", "beta"] {
        call(&mut vm, "AddRow", vec![Value::Str(r.into())]);
    }
    call(&mut vm, "EnableHotKeys", vec![Value::Bool(true)]);
    call(&mut vm, "SetHotKeyColumn", vec![Value::Int(0)]);
    call(&mut vm, "SetFocusWindow", vec![Value::Obj(list)]);
    let focus = |vm: &mut Vm| {
        let id = match vm.call_by_name(list, "GetFocusRow", vec![]) { Ok(Value::Int(i)) => i, other => panic!("{other:?}") };
        match vm.call_by_name(list, "GetField", vec![Value::Int(id), Value::Int(0)]) { Ok(Value::Str(s)) => s, other => panic!("{other:?}") }
    };
    let key = |vm: &mut Vm, c: char| assert!(vm.ui_process_char(player, c));
    // no focus row: a single letter starts the search after row 0
    key(&mut vm, 'a');
    assert_eq!(focus(&mut vm), "apple");
    key(&mut vm, 'a');
    assert_eq!(focus(&mut vm), "avocado", "a repeated letter did not cycle");
    key(&mut vm, 'a');
    assert_eq!(focus(&mut vm), "alpha", "the cycle did not wrap round");
    // the hot key string runs out after a second
    vm.ui_tick(root, 1.1);
    key(&mut vm, 'a');
    assert_eq!(focus(&mut vm), "apple");
    key(&mut vm, 'v');
    assert_eq!(focus(&mut vm), "avocado", "\"av\" did not match from the focus row");
    vm.ui_tick(root, 1.1);
    key(&mut vm, 'B');
    assert_eq!(focus(&mut vm), "beta", "upper case did not fold");
    key(&mut vm, '_');
    assert_eq!(focus(&mut vm), "b_c", "the underscore is not a hot key");
}

/// The music volume slider (`SetScaleSounds(Menu_Press, None, Menu_Slider)`): a press beside the thumb jumps there with
/// the set sound, dragging plays the drag sound per position change, the release after a change the set sound.
#[test]
fn a_slider_plays_its_scale_sounds() {
    let Some((mut vm, player)) = world() else { return };
    let root = open_screen(&mut vm, player, "MenuScreenSound");
    let scale = vm.ui_find_child(root, "ScaleWindow");
    assert_ne!(scale, 0);
    let (set, drag) = (obj_prop(&vm, scale, "setSound"), obj_prop(&vm, scale, "dragSound"));
    assert!(set != 0 && drag != 0, "scale sounds {set} {drag}");
    let (ox, oy) = vm.ui_window_origin(scale);
    let [_, _, w, h] = vm.ui_geometry(scale);
    let _ = played(&mut vm);
    assert!(vm.ui_mouse_position(player, ox + w - 3.0, oy + h / 2.0));
    assert!(vm.ui_process_key(player, 1, 1));
    assert_eq!(played(&mut vm), vec![set], "the jump played no set sound");
    assert!(vm.ui_mouse_position(player, ox + 3.0, oy + h / 2.0));
    assert_eq!(played(&mut vm), vec![drag], "dragging played no drag sound");
    // no position change, no sound
    assert!(vm.ui_mouse_position(player, ox + 2.0, oy + h / 2.0));
    assert!(played(&mut vm).is_empty());
    assert!(vm.ui_process_key(player, 1, 3));
    assert_eq!(played(&mut vm), vec![set], "the release after a change played no set sound");
}

/// Word selection in an edit window (`ComputeSelectArea` with `bSelectWords`): a double click selects the word, dragging
/// then extends by whole words, a triple click is a plain click again, Ctrl+click selects a word; holding the button
/// keeps selecting towards the cursor every 0.1 s (`Tick`).
#[test]
fn an_edit_window_selects_words() {
    let Some((mut vm, player)) = world() else { return };
    let root = open_screen(&mut vm, player, "MenuScreenNewGame");
    let edit = obj_prop(&vm, root, "focusWindow");
    assert_ne!(edit, 0);
    vm.call_by_name(edit, "SetText", vec![Value::Str("ab cd ef".into())]).unwrap();
    vm.ui_paint(player, 640.0, 480.0);
    let (ox, oy) = vm.ui_window_origin(edit);
    let [_, _, w, h] = vm.ui_geometry(edit);
    let sel = |vm: &Vm| (int_prop(vm, edit, "selectStart"), int_prop(vm, edit, "selectEnd"), int_prop(vm, edit, "insertPos"));
    let click = |vm: &mut Vm, x: f32| {
        assert!(vm.ui_mouse_position(player, x, oy + h / 2.0));
        assert!(vm.ui_process_key(player, 1, 1));
        assert!(vm.ui_process_key(player, 1, 3));
    };
    let (left, right) = (ox + 4.0, ox + w - 2.0);
    vm.ui_tick(root, 1.0);
    click(&mut vm, left);
    assert_eq!(sel(&vm), (-1, -1, 0), "a single click selected something (no selection is -1, -1: 3.192)");
    // the double click: the first word, the insertion point at its end
    assert!(vm.ui_mouse_position(player, left, oy + h / 2.0));
    assert!(vm.ui_process_key(player, 1, 1));
    assert_eq!(sel(&vm), (0, 2, 2), "the double click did not select the word");
    // dragging to the end grows by words
    assert!(vm.ui_mouse_position(player, right, oy + h / 2.0));
    assert_eq!(sel(&vm), (0, 8, 8));
    assert!(vm.ui_process_key(player, 1, 3));
    // the third click of the series is a plain click
    click(&mut vm, left);
    assert_eq!(sel(&vm), (-1, -1, 0), "the triple click selected words");
    // Ctrl+click on the last word
    vm.ui_tick(root, 1.0);
    assert!(vm.ui_process_key(player, 0x11, 1));
    click(&mut vm, right);
    assert!(vm.ui_process_key(player, 0x11, 3));
    assert_eq!(sel(&vm), (6, 8, 8), "Ctrl+click did not select the word");
    // a plain press at the start, the cursor goes to the end without a move event: the next drag step selects to it
    vm.ui_tick(root, 1.0);
    assert!(vm.ui_mouse_position(player, left, oy + h / 2.0));
    assert!(vm.ui_process_key(player, 1, 1));
    assert_eq!(sel(&vm), (-1, -1, 0));
    let _ = vm.set_prop(root, "mouseX", 0, Value::Float(right.trunc()));
    vm.ui_tick(root, 0.05);
    assert_eq!(sel(&vm), (-1, -1, 0), "the drag step came before 0.1 s");
    vm.ui_tick(root, 0.06);
    assert_eq!(sel(&vm), (0, 8, 8), "holding the button did not extend the selection");
    assert!(vm.ui_process_key(player, 1, 3));
}

/// `XComputerWindow::Tick`: the first showing invokes `ComputerStart` once; the game's pause holds the terminal
/// (processing stops on the first paused tick and resumes when the pause ends).
#[test]
fn a_computer_window_waits_while_the_game_is_paused() {
    let Some((mut vm, player)) = world() else { return };
    let root = vm.ui_set_screen(player, 640.0, 480.0);
    let class = vm.reg.obj_id("Extension.ComputerWindow").0;
    let cw = match vm.call_by_name(root, "NewChild", vec![Value::Obj(class), Value::Bool(false)]).unwrap() { Value::Obj(w) => w, other => panic!("{other:?}") };
    let flag = |vm: &Vm, n: &str| matches!(vm.get_prop(cw, n, 0), Ok(Value::Bool(true)));
    vm.call_by_name(cw, "Show", vec![]).unwrap();
    assert!(flag(&vm, "bInvokeComputerStart"), "showing did not ask for ComputerStart");
    // the first tick after creation does nothing but clear `bFirstTick` (on in the class defaults)
    assert!(flag(&vm, "bFirstTick"));
    vm.ui_tick(root, 0.05);
    assert!(!flag(&vm, "bFirstTick") && flag(&vm, "bInvokeComputerStart"));
    vm.ui_tick(root, 0.05);
    assert!(!flag(&vm, "bInvokeComputerStart") && flag(&vm, "bComputerStartInvoked"));
    vm.call_by_name(cw, "Hide", vec![]).unwrap();
    vm.call_by_name(cw, "Show", vec![]).unwrap();
    assert!(!flag(&vm, "bInvokeComputerStart"), "ComputerStart asked for twice");
    let level = obj_prop(&vm, player, "Level");
    let _ = vm.set_prop(level, "Pauser", 0, Value::Str("someone".into()));
    vm.call_by_name(cw, "Print", vec![Value::Str("abc".into()), Value::Bool(false)]).unwrap();
    vm.ui_tick(root, 0.05);
    assert!(flag(&vm, "bGamePaused") && flag(&vm, "bPauseProcessing"));
    let before = vm.ui_computer_cells();
    vm.ui_tick(root, 0.5);
    assert_eq!(vm.ui_computer_cells(), before, "the terminal printed during the pause");
    let _ = vm.set_prop(level, "Pauser", 0, Value::Str(String::new()));
    vm.ui_tick(root, 0.5);
    assert!(!flag(&vm, "bGamePaused") && !flag(&vm, "bPauseProcessing"));
    assert!(vm.ui_computer_cells() > before, "the terminal did not resume");
}

/// A resizeable border window (`EnableResizing`, no game script uses it) moves with the right button dragged
/// (`XBorderWindow::MouseButtonPressed`/`MouseMoved`: all four edges follow the cursor).
#[test]
fn a_resizeable_border_window_moves_with_the_right_button() {
    let Some((mut vm, player)) = world() else { return };
    let root = open_screen(&mut vm, player, "MenuScreenSound");
    let screen = vm.ui_find_child(root, "MenuScreenSound");
    assert_ne!(screen, 0);
    let class = vm.reg.obj_id("Extension.BorderWindow").0;
    let bw = match vm.call_by_name(screen, "NewChild", vec![Value::Obj(class)]).unwrap() { Value::Obj(w) => w, other => panic!("{other:?}") };
    vm.call_by_name(bw, "SetConfiguration", vec![Value::Float(20.0), Value::Float(30.0), Value::Float(100.0), Value::Float(80.0)]).unwrap();
    vm.call_by_name(bw, "EnableResizing", vec![Value::Bool(true)]).unwrap();
    vm.ui_tick(root, 0.05);
    vm.ui_paint(player, 640.0, 480.0);
    assert_eq!(vm.ui_geometry(bw), [20.0, 30.0, 100.0, 80.0]);
    let (ox, oy) = vm.ui_window_origin(bw);
    assert!(vm.ui_mouse_position(player, ox + 50.0, oy + 40.0));
    assert!(vm.ui_process_key(player, 2, 1), "the right press was not taken");
    assert!(vm.ui_mouse_position(player, ox + 60.0, oy + 45.0));
    assert!(vm.ui_process_key(player, 2, 3));
    vm.ui_tick(root, 0.05);
    vm.ui_paint(player, 640.0, 480.0);
    assert_eq!(vm.ui_geometry(bw), [30.0, 35.0, 100.0, 80.0], "the drag did not move the window");
}

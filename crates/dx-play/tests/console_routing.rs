//! `ConsoleCommand` routed as the engine does (`ue_vm::console`, WORKLOG 3.139): the pawn's viewport chain, the
//! engine's, the console's. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

fn session() -> Option<Session> {
    let g = std::env::var_os("DEUSEX_DIR").map(PathBuf::from)?;
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    for _ in 0..3 {
        s.step_bound(1.0 / 30.0);
    }
    Some(s)
}

fn cmd(s: &mut Session, who: u32, line: &str) -> Value {
    s.vm.call_by_name(who, "ConsoleCommand", vec![Value::Str(line.into())]).unwrap()
}

fn text(v: Value) -> String {
    match v {
        Value::Str(t) => t,
        other => panic!("{other:?}"),
    }
}

#[test]
fn player_commands_go_through_the_viewport() {
    let Some(mut s) = session() else { return };
    let p = s.player;
    // `UViewport::Exec`'s RMODE: 1..19 only; leading spaces and any case
    assert_eq!(text(cmd(&mut s, p, "RMODE 6")), "");
    assert_eq!(s.vm.ip(p, "RendMap"), 6);
    cmd(&mut s, p, "RMODE 25");
    assert_eq!(s.vm.ip(p, "RendMap"), 6);
    cmd(&mut s, p, "  rmode 5");
    assert_eq!(s.vm.ip(p, "RendMap"), 5);
    // a longer word is not RMODE
    cmd(&mut s, p, "RMODEX 3");
    assert_eq!(s.vm.ip(p, "RendMap"), 5);
    // SHOWACTORS / HIDEACTORS: `SHOW_Actors` (8) of the pawn's ShowFlags
    let f = s.vm.ip(p, "ShowFlags");
    cmd(&mut s, p, "HIDEACTORS");
    assert_eq!(s.vm.ip(p, "ShowFlags"), f & !8);
    cmd(&mut s, p, "SHOWACTORS");
    assert_eq!(s.vm.ip(p, "ShowFlags"), f | 8);
    // output comes back: a standalone game's ping, the input system's key names (key 13 is Enter)
    assert_eq!(text(cmd(&mut s, p, "GETPING")), "0");
    assert_eq!(text(cmd(&mut s, p, "KEYNAME 13")), "Enter");
    // the engine's GET at the end of the chain
    assert!(!text(cmd(&mut s, p, "get DeusExPlayer bAlwaysRun")).is_empty());
}

#[test]
fn an_actors_command_skips_the_viewport() {
    let Some(mut s) = session() else { return };
    let p = s.player;
    cmd(&mut s, p, "RMODE 5");
    // any actor that is not a pawn with a viewport: the engine takes the line, RMODE is the viewport's
    let level = s.vm.level;
    assert_eq!(text(cmd(&mut s, level, "RMODE 6")), "");
    assert_eq!(s.vm.ip(p, "RendMap"), 5);
    // nor GETPING, the viewport's too
    assert_eq!(text(cmd(&mut s, level, "GETPING")), "");
    // EXIT is the engine's: the engine says so and the session ends after the frame
    assert_eq!(text(cmd(&mut s, level, "EXIT")), "Closing by request");
    s.step_bound(1.0 / 30.0);
    assert!(s.quit);
}

#[test]
fn exec_functions_read_their_parameters_as_import_text_does() {
    let Some(mut s) = session() else { return };
    let p = s.player;
    let console = {
        let vp = s.vm.op(p, "Player");
        s.vm.op(vp, "Console")
    };
    assert!(console != 0, "the viewport has a console");
    // the console's exec functions come before the pawn's: `TimeDemo(bool bEnabled, optional bool bSaveToFile)`
    assert!(!s.vm.bp(console, "bTimeDemo"));
    assert_eq!(text(cmd(&mut s, p, "TimeDemo True")), "");
    assert!(s.vm.bp(console, "bTimeDemo"));
    assert!(!s.vm.bp(console, "bSaveTimeDemoToFile"), "a missing optional bool stays False");
    // a word that is not a bool: the call is skipped and Core's BadProperty text is the output
    assert_eq!(text(cmd(&mut s, p, "TimeDemo maybe")), "'TimeDemo': Bad or missing property 'bEnabled'");
    assert!(s.vm.bp(console, "bTimeDemo"));
    assert_eq!(text(cmd(&mut s, p, "TimeDemo 0")), "");
    assert!(!s.vm.bp(console, "bTimeDemo"));
    // `Console.ConsoleCommand` says whether anything took the line
    assert_eq!(cmd(&mut s, console, "RMODE 6"), Value::Bool(true));
    assert_eq!(s.vm.ip(p, "RendMap"), 6);
    assert_eq!(cmd(&mut s, console, "NoSuchCommandAnywhere"), Value::Bool(false));
}

/// The console sees keys and typed characters before the window system (`UEngine::InputEvent` / `UEngine::Key`): in
/// its default state it takes nothing; after `Talk` it is typing and takes characters into `TypedStr`; Escape empties
/// the line, a second Escape leaves typing.
#[test]
fn the_console_types_after_talk() {
    let Some(mut s) = session() else { return };
    assert!(!s.console_key_type('a'));
    assert!(!s.console_key_event(0x41, 1, 0.0));
    s.exec("Talk");
    // the character of the key that ran `Talk` is dropped (`bNoStuff`)
    for c in ['t', 'h', 'i'] {
        assert!(s.console_key_type(c));
    }
    let viewport = s.vm.op(s.player, "Player");
    let console = s.vm.op(viewport, "Console");
    assert_eq!(s.vm.get_prop(console, "TypedStr", 0).unwrap(), Value::Str("Say hi".into()));
    s.console_key_event(0x1B, 1, 0.0);
    assert_eq!(s.vm.get_prop(console, "TypedStr", 0).unwrap(), Value::Str(String::new()));
    s.console_key_event(0x1B, 1, 0.0);
    assert!(!s.console_key_type('x'));
}

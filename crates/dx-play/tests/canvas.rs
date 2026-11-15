//! The engine frame's canvas (`UGameEngine::Draw`, `docs/canvas.md`): the console ticks and draws through the
//! `Canvas` text and tile natives over the windows. Set `DEUSEX_DIR` to the game folder.

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

fn console(s: &mut Session) -> u32 {
    let v = s.vm.op(s.player, "Player");
    s.vm.op(v, "Console")
}

fn path(s: &Session, tex: u32) -> String {
    s.vm.reg.obj_path(ue_registry::ObjId(tex)).to_string()
}

#[test]
fn the_console_ticks_and_draws_the_typing_line() {
    let Some(mut s) = session() else { return };
    let c = console(&mut s);
    // `UConsole::PreRender` ticks the console each frame; `PostRender` gets the frame's size
    let t0 = s.vm.fp(c, "MsgTickTime");
    s.step_bound(1.0 / 30.0);
    assert!(s.vm.fp(c, "MsgTickTime") > t0);
    assert_eq!((s.vm.fp(c, "FrameX"), s.vm.fp(c, "FrameY")), (800.0, 600.0));
    assert!(s.vm.canvas_list.is_empty(), "nothing over the HUD");
    s.exec("Talk");
    for ch in ['t', 'h', 'i'] {
        s.console_key_type(ch);
    }
    s.step_bound(1.0 / 30.0);
    // `DrawSingleView`: "(> Say hi_" in the medium font, white, 2 pixels from the left, a pixel above the bottom
    let l = &s.vm.canvas_list;
    assert_eq!(l.len(), 10);
    assert!(path(&s, l[0].tex).starts_with("Engine.MedFont"));
    assert_eq!((l[0].x, l[0].y, l[0].h), (2.0, 600.0 - 11.0 - 1.0, 11.0));
    assert!(l.windows(2).all(|p| p[1].x == p[0].x + p[0].w), "each glyph after the last");
    assert!(l.iter().all(|d| d.color == [1.0, 1.0, 1.0] && d.poly_flags & 0x802 == 0x802));
    s.console_key_event(0x1B, 1, 0.0);
    s.console_key_event(0x1B, 1, 0.0);
    s.step_bound(1.0 / 30.0);
    assert!(s.vm.canvas_list.is_empty());
}

#[test]
fn the_full_console_slides_down_over_the_screen() {
    let Some(mut s) = session() else { return };
    let c = console(&mut s);
    s.exec("Talk");
    s.console_key_type('x');
    s.console_key_type('~');
    // the console slides by the time since the last frame: a second to come all the way down
    s.step_bound(0.5);
    let half = s.vm.ip(c, "ConsoleLines");
    assert!(half > 0 && half < 600, "{half}");
    for _ in 0..40 {
        s.step_bound(1.0 / 30.0);
    }
    assert_eq!(s.vm.ip(c, "ConsoleLines"), 600);
    let bg = s.vm.canvas_list[0].clone();
    assert_eq!(path(&s, bg.tex), "Engine.ConsoleBack");
    assert_eq!((bg.x, bg.y, bg.w, bg.h), (0.0, 0.0, 800.0, 600.0));
    assert_eq!(bg.color, [128.0 / 255.0; 3]);
}

#[test]
fn a_paused_game_says_so_but_not_under_a_menu() {
    let Some(mut s) = session() else { return };
    s.exec("Pause");
    s.step_bound(1.0 / 30.0);
    assert_ne!(s.vm.get_prop(s.vm.level, "Pauser", 0).unwrap(), Value::Str(String::new()));
    // `DrawLevelAction`: the paused message in the big font, centred
    let l = &s.vm.canvas_list;
    assert!(!l.is_empty());
    assert!(path(&s, l[0].tex).starts_with("Engine.BigFont"));
    let (left, right) = (l[0].x, l.last().map(|d| d.x + d.w).unwrap());
    assert!(((left + right) / 2.0 - 400.0).abs() <= 1.0, "{left} {right}");
    s.exec("Pause");
    s.exec("ShowMainMenu");
    s.step_bound(1.0 / 30.0);
    assert!(s.vm.canvas_list.is_empty());
}

/// Pictures of the typing line and the full console (needs a Vulkan device; `out/screenshots/console_*.png`).
#[test]
#[ignore]
fn shot_of_the_console() {
    let Some(mut s) = session() else { return };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    let mut r = None;
    s.exec("Talk");
    for ch in ['t', 'h', 'e', 'l', 'l', 'o'] {
        s.console_key_type(ch);
    }
    s.step_bound(1.0 / 30.0);
    let img = s.render_vk(&mut r, 800, 600).expect("vulkan render");
    std::fs::write(dir.join("console_typing.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    s.console_key_type('~');
    for _ in 0..40 {
        s.step_bound(1.0 / 30.0);
    }
    let img = s.render_vk(&mut r, 800, 600).expect("vulkan render");
    std::fs::write(dir.join("console_full.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
}

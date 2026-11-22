//! Resolutions and aspect ratios: the game's Display menu lists and switches resolutions through the console commands
//! the port answers (`display.rs`), and the game draws at 4:3 to 32:9. Set `DEUSEX_DIR` to the game folder.
use dx_play::display::resolution_list;
use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 60.0;

fn obj(s: &mut Session, o: u32, name: &str) -> u32 {
    match s.vm.get_prop(o, name, 0) {
        Ok(Value::Obj(r)) => r,
        _ => 0,
    }
}

fn class_name(s: &Session, o: u32) -> String {
    s.vm.class_of(o).map(|c| s.vm.reg.class(c).path.rsplit('.').next().unwrap_or("").to_string()).unwrap_or_default()
}

fn find_window(s: &mut Session, w: u32, class: &str) -> u32 {
    let mut c = obj(s, w, "firstChild");
    while c != 0 {
        if class_name(s, c) == class {
            return c;
        }
        let f = find_window(s, c, class);
        if f != 0 {
            return f;
        }
        c = obj(s, c, "nextSibling");
    }
    0
}

fn new_game() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(DT);
    }
    Some(s)
}

/// The Display menu's resolution choice lists what `GetRes` answers (for a 5120x1440 display) and selects the current
/// resolution; saving the menu asks for the new one with `SetRes`.
#[test]
fn the_display_menu_lists_and_sets_resolutions() {
    let Some(mut s) = new_game() else { return };
    let modes = resolution_list((5120, 1440), &[(5120, 1440), (2560, 1440)]);
    {
        let mut d = s.display.borrow_mut();
        d.native = (5120, 1440);
        d.modes = modes.clone();
        d.fullscreen = true;
        d.resolution = (3840, 1080);
    }
    assert!(s.open_menu_screen("MenuScreenDisplay"));
    s.step_bound(DT);
    let root = s.vm.ui_root(s.player);
    let choice = find_window(&mut s, root, "MenuChoice_Resolution");
    assert_ne!(choice, 0, "the resolution choice");
    let listed: Vec<String> = (0..40).filter_map(|i| match s.vm.get_prop(choice, "enumText", i) {
        Ok(Value::Str(t)) if !t.is_empty() => Some(t),
        _ => None,
    }).collect();
    let want: Vec<String> = modes.iter().map(|(w, h)| format!("{w}x{h}")).collect();
    assert_eq!(listed, want);
    assert_eq!(s.vm.get_prop(choice, "CurrentRes", 0).ok(), Some(Value::Str("3840x1080".into())));
    // pick 2560x1440 and save: the menu sends SetRes
    let idx = want.iter().position(|r| r == "2560x1440").unwrap() as i32;
    s.vm.call_by_name(choice, "SetValue", vec![Value::Int(idx)]).expect("SetValue");
    s.vm.call_by_name(choice, "SaveSetting", vec![]).expect("SaveSetting");
    assert_eq!(s.display.borrow().pending, Some(((2560, 1440), None)));
}

/// The HUD and the Display menu at 4:3, 16:9, 21:9 and 32:9 (`../out/screenshots/s13_res_*.png`), for a human to look
/// at; needs a Vulkan device.
#[test]
#[ignore]
fn shots_at_every_aspect_ratio() {
    let Some(mut s) = new_game() else { return };
    s.display.borrow_mut().modes = resolution_list((3840, 2160), &[]);
    let mut r = None;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    for (w, h) in [(1024, 768), (1920, 1080), (2560, 1080), (3840, 1080)] {
        s.display.borrow_mut().resolution = (w, h);
        let img = s.render_vk(&mut r, w, h).expect("render");
        std::fs::write(dir.join(format!("s13_res_{w}x{h}.png")), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    }
    assert!(s.open_menu_screen("MenuScreenDisplay"));
    s.step_bound(DT);
    let img = s.render_vk(&mut r, 3840, 1080).expect("render");
    std::fs::write(dir.join("s13_res_menu_3840x1080.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    r.take().unwrap().destroy();
}

/// The port's menus (`Aerugo`, `Vm::apply_port_menus`): the Display screen lists brightness, full screen, resolution,
/// vertical sync, frame rate limit, mouse DPI, field of view and the Advanced Graphics button (its screen: the picture
/// options), the Sound screen no sample format, channel or 3D hardware choice,
/// the Controls screen the half-step sensitivity slider. What the choices save is what the host reads
/// (`Session::aerugo_settings`, `follow_settings`): vertical sync, frame limit, mouse DPI (the mouse scale), output rate,
/// the sensitivity, and the brightness the Brightness screen saves.
#[test]
fn the_port_menus_set_what_the_host_reads() {
    let Some(mut s) = new_game() else { return };
    let choices = |s: &mut Session, screen: &str| -> Vec<(String, u32)> {
        assert!(s.open_menu_screen(screen), "{screen}");
        s.step_bound(DT);
        let root = s.vm.ui_root(s.player);
        let screen_win = find_window(s, root, screen);
        assert_ne!(screen_win, 0, "{screen} window");
        let mut out = Vec::new();
        let mut stack = vec![obj(s, screen_win, "firstChild")];
        while let Some(mut c) = stack.pop() {
            while c != 0 {
                let n = class_name(s, c);
                if n.starts_with("MenuChoice_") {
                    out.push((n, c));
                }
                stack.push(obj(s, c, "firstChild"));
                c = obj(s, c, "nextSibling");
            }
        }
        out.sort_by_key(|(_, c)| s.vm.ui_window_origin(*c).1 as i32);
        out
    };
    let names = |v: &[(String, u32)]| v.iter().map(|(n, _)| n.trim_start_matches("MenuChoice_").to_string()).collect::<Vec<_>>();
    let display = choices(&mut s, "MenuScreenDisplay");
    assert_eq!(names(&display), ["Brightness", "FullScreen", "Resolution", "AerugoVSync", "AerugoFrameLimit", "AerugoMouseDPI", "AerugoFOV", "AerugoGraphics"]);
    let set = |s: &mut Session, c: u32, v: i32| {
        s.vm.call_by_name(c, "SetValue", vec![Value::Int(v)]).unwrap();
        s.vm.call_by_name(c, "SaveSetting", vec![]).unwrap();
    };
    set(&mut s, display[3].1, 3);
    set(&mut s, display[4].1, 1);
    set(&mut s, display[5].1, 4);
    let st = s.aerugo_settings();
    assert_eq!((st.vsync, st.frame_limit, st.mouse_dpi), (3, -1, 1600));
    s.follow_settings();
    assert_eq!(s.mouse_scale, 0.25, "1600 DPI moves a quarter of a 400 DPI count");
    // the field of view: the game's own DefaultFOV, and the class default a scope restores
    set(&mut s, display[6].1, 3);
    assert_eq!(s.vm.get_prop(s.player, "DefaultFOV", 0).ok(), Some(Value::Float(90.0)));
    assert_eq!(s.vm.default_value_of("JCDentonMale", "DefaultFOV", 0), Some(Value::Float(90.0)));
    s.exec("ShowMainMenu");
    let sound = choices(&mut s, "MenuScreenSound");
    assert_eq!(names(&sound), ["MusicVolume", "SoundVolume", "SpeechVolume", "AerugoLoudness", "AerugoOutputRate", "ReverseStereo", "SurroundSound"]);
    set(&mut s, sound[4].1, 2);
    assert_eq!(s.aerugo_settings().output_rate, 48000);
    // the loudness control: on by default, the original's mix one choice away
    assert_eq!(s.aerugo_settings().loudness, 1);
    set(&mut s, sound[3].1, 0);
    assert_eq!(s.aerugo_settings().loudness, 0);
    set(&mut s, sound[3].1, 1);
    assert_eq!(s.aerugo_settings().loudness, 1);
    s.exec("ShowMainMenu");
    let controls = choices(&mut s, "MenuScreenControls");
    let (_, speed) = controls.iter().find(|(n, _)| n == "MenuChoice_AerugoMouseSpeed").expect("the sensitivity slider").clone();
    s.vm.call_by_name(speed, "SetValue", vec![Value::Float(9.0)]).unwrap();
    s.vm.call_by_name(speed, "ScalePositionChanged", vec![Value::Obj(0), Value::Int(8), Value::Float(9.0), Value::Bool(true)]).unwrap();
    assert_eq!(s.vm.get_prop(s.player, "MouseSensitivity", 0).ok(), Some(Value::Float(4.5)));
    // the brightness the Brightness screen saves (`set ini:Engine.Engine.ViewportManager Brightness`)
    s.exec("set ini:Engine.Engine.ViewportManager Brightness 0.8");
    s.follow_settings();
    assert!((s.brightness - 0.8).abs() < 1e-4, "brightness {}", s.brightness);
    // the Advanced Graphics screen, through its button: the picture options the renderer gets (the root's window
    // stack holds six screens: emptied first, as the menus' back buttons would)
    let root = s.vm.ui_root(s.player);
    s.vm.call_by_name(root, "ClearWindowStack", vec![]).unwrap();
    s.exec("ShowMainMenu");
    let display = choices(&mut s, "MenuScreenDisplay");
    s.vm.call_by_name(display[7].1, "ButtonActivated", vec![Value::Obj(0)]).unwrap();
    s.step_bound(DT);
    let root = s.vm.ui_root(s.player);
    assert_ne!(find_window(&mut s, root, "MenuScreenAerugoGraphics"), 0, "the button opens the screen");
    s.vm.call_by_name(root, "ClearWindowStack", vec![]).unwrap();
    s.exec("ShowMainMenu");
    let gfx = choices(&mut s, "MenuScreenAerugoGraphics");
    assert_eq!(names(&gfx), ["AerugoTextureFilter", "AerugoAntiAliasing", "AerugoColorDepth", "AerugoLightmaps", "AerugoMeshLighting", "AerugoRTShadows", "AerugoRTAO", "AerugoMaxDecals"]);
    s.settings_quality = true;
    let enhanced = dx_vk::Quality { filter: 5, msaa: 4, deep_color: true, smooth_lightmaps: true, pixel_lighting: true, rt_shadows: true, rt_ao: true };
    assert_eq!(s.quality(), enhanced, "the defaults");
    for (i, v) in [(0, 1), (1, 3), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0)] {
        set(&mut s, gfx[i].1, v);
    }
    assert_eq!(s.quality(), dx_vk::Quality { filter: 1, msaa: 8, deep_color: false, smooth_lightmaps: false, pixel_lighting: false, rt_shadows: false, rt_ao: false });
    // kept decals: 1000 by default, "Original" (0) and 5000 from the menu
    s.follow_settings();
    assert_eq!(s.max_decals, 1000, "the default");
    for (v, n) in [(0, 0), (5, 5000)] {
        set(&mut s, gfx[7].1, v);
        s.follow_settings();
        assert_eq!(s.max_decals, n);
    }
}

/// The game's main menu shows the port's version line (`Aerugo <tag or commit>`) beside the original's version text,
/// at the bottom left of the same row; without it (tests, probes) the menu is the original's.
#[test]
fn the_main_menu_shows_the_port_version_beside_the_original() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let texts = |s: &mut Session| -> Vec<(String, f32, f32)> {
        let root = s.vm.ui_root(s.player);
        let menu = find_window(s, root, "MenuMain");
        assert_ne!(menu, 0, "the main menu is open");
        let mut out = Vec::new();
        let mut c = obj(s, menu, "firstChild");
        while c != 0 {
            if class_name(s, c) == "TextWindow" {
                let t = match s.vm.call_by_name(c, "GetText", vec![]) { Ok(Value::Str(t)) => t, _ => String::new() };
                let f = |n: &str| s.vm.get_prop(c, n, 0).ok().and_then(|v| v.as_f32()).unwrap_or(-1.0);
                out.push((t, f("x"), f("width")));
            }
            c = obj(s, c, "nextSibling");
        }
        out
    };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("ShowMainMenu");
    s.step_bound(DT);
    let plain = texts(&mut s);
    assert_eq!(plain.len(), 1, "only the original's version text: {plain:?}");
    let mut s = Session::open(&g, "DX").unwrap();
    s.vm.port_version = Some(dx_play::port_version());
    s.exec("ShowMainMenu");
    s.step_bound(DT);
    let both = texts(&mut s);
    let original = both.iter().find(|t| t.0.contains("v1.112fm")).expect("the original's version text").clone();
    let port = both.iter().find(|t| t.0.starts_with("Aerugo ")).expect("the port's version line").clone();
    eprintln!("{both:?}");
    assert_eq!(original, plain[0], "the original's text and place unchanged");
    assert!(port.1 + port.2 <= original.1, "the port's line overlaps the original's: {port:?} {original:?}");
}

/// The render thread (`dx_vk::render_thread`): frames handed to it are the frames drawn on one thread. The same game
/// state, ten frames each way, the last one read back; the thread ends cleanly and gives the renderer back.
#[test]
fn the_render_thread_draws_what_one_thread_draws() {
    let picture = |threaded: bool| -> Option<Vec<u8>> {
        let mut s = new_game()?;
        let renderer = s.make_vk_renderer(&[]).unwrap();
        let mut rt = dx_vk::render_thread::RenderThread::new(renderer, threaded);
        assert_eq!(rt.threaded(), threaded);
        let mut prep = Vec::new();
        for _ in 0..10 {
            s.render_vk_bench_thread(&mut rt, (640, 360), &mut prep).unwrap();
        }
        let (_, _, rgba) = rt.renderer().unwrap().read_last_frame().unwrap().unwrap();
        rt.into_inner().unwrap().destroy();
        Some(rgba)
    };
    let Some(one) = picture(false) else { return };
    let two = picture(true).unwrap();
    let differ = one.iter().zip(&two).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "{differ} bytes differ between the frame drawn on one thread and on the render thread");
}

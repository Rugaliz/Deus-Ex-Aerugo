//! The game: `deusex [<game-dir>] [<Map>] [--windowed] [--size WxH]`, started on the logo and the main menu without a
//! map. Development options: `--headless [--frames N] [--shot out.png] [--forward]` (no window: N frames, optionally
//! walking forward, and a screenshot), `--bench` (frame timing), `--import-save <file.dxs>`; `--frames N` also ends a
//! windowed run after N frames (smoke tests with `SDL_VIDEODRIVER=dummy`).
//!
//! Key bindings are the game's own (`dx_input`), with modern defaults when there is no `Input.ini` in the per-user
//! configuration directory.

// Windows release builds start without a console window (errors are shown in a message box; run from a console, the
// output goes to that console)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod bench;
mod sdl_input;

use dx_play::{Input, Session};
use std::path::Path;
use std::time::{Duration, Instant};

const W: u32 = 640;
const H: u32 = 360;

/// Reports an error that ends the run: on the console, and on Windows also in a message box (a game started from the
/// file manager has no console to show it).
fn fatal(msg: &str, code: i32) -> ! {
    eprintln!("{msg}");
    #[cfg(windows)]
    {
        let _ = sdl3::messagebox::show_simple_message_box(sdl3::messagebox::MessageBoxFlag::ERROR, "Deus Ex", msg, None);
    }
    std::process::exit(code)
}

#[cfg(windows)]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process: u32) -> i32;
    }
    // SAFETY: plain Win32 call; ATTACH_PARENT_PROCESS
    unsafe { AttachConsole(u32::MAX) };
}

fn main() {
    #[cfg(windows)]
    attach_parent_console();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--write-default-bindings") {
        // writes the modern default layout to the per-user config directory (never overwrites an existing file)
        let Some(dir) = dx_play::config_dir() else { return eprintln!("no configuration directory") };
        let path = dir.join("Input.ini");
        if path.exists() {
            return eprintln!("{} exists already", path.display());
        }
        std::fs::create_dir_all(&dir).expect("create the configuration directory");
        std::fs::write(&path, dx_play::dx_input::DEFAULT_INI).expect("write the bindings");
        return println!("wrote {}", path.display());
    }
    // `deusex [<game-dir>] [<Map>]`: dropped into the game folder it needs no arguments (it starts on the menu map `DX`)
    let is_game = |p: &str| Path::new(p).join("System").is_dir();
    let first = args.get(1).filter(|a| !a.starts_with('-'));
    // without a folder argument: the working directory, else the folder the executable is in (started from a shortcut)
    let here = || {
        let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_string_lossy().into_owned()));
        if is_game(".") { ".".to_string() } else { exe_dir.filter(|d| is_game(d)).unwrap_or_else(|| ".".into()) }
    };
    let (game, map) = match first {
        Some(a) if is_game(a) => (a.clone(), args.get(2).filter(|m| !m.starts_with('-')).cloned().unwrap_or_else(|| "DX".into())),
        Some(a) => (here(), a.clone()),
        None => (here(), "DX".to_string()),
    };
    if !is_game(&game) {
        fatal("usage: deusex [<game-dir>] [<Map>] [--headless] [--frames N] [--shot out.png] [--soft] [--import-save <file.dxs>]\nrun it inside the Deus Ex folder (the one with System/ and Maps/) or pass that folder", 2);
    }
    let (game, map) = (&game, &map);
    let flag = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    // `--export-textures <folder>`: every texture of the game as PNG, named as the mods folder expects
    if let Some(out) = value("--export-textures") {
        match dx_play::mods::export_textures(Path::new(game), Path::new(&out)) {
            Ok(n) => return println!("{n} textures written to {out} (as Textures/<Package>/<Group>/<Name>.png)"),
            Err(e) => fatal(&format!("error: {e}"), 1),
        }
    }
    let mut session = match Session::open(Path::new(game), map) {
        Ok(s) => s,
        Err(e) => fatal(&format!("error: {e}"), 1),
    };
    // started without a map: like the original, the game plays the logo cutscene of the `DX` level (a click or the menu key
    // brings up the main menu); headless runs (screenshots, tests) and `--no-logo` go straight to the menu
    if map == "DX" && !flag("--no-menu") && (flag("--headless") || flag("--no-logo")) {
        session.exec("ShowMainMenu");
    }
    // `--load N`: start in saved game N of the player's save folder (`Save<NNNN>`)
    if let Some(n) = value("--load").and_then(|v| v.parse::<i32>().ok()) {
        if let Err(e) = session.load_game(n) {
            fatal(&format!("error: cannot load saved game {n}: {e}"), 1);
        }
    }
    // continue a game saved by the original (`Save/<slot>/<map>.dxs`)
    if let Some(file) = value("--import-save") {
        match session.import_original_save(Path::new(&file)) {
            Ok(r) => eprintln!("imported {file}: {} flags, {} actors back in their states", r.flags, r.states),
            Err(e) => {
                eprintln!("error: cannot import {file}: {e}");
                std::process::exit(1);
            }
        }
    }
    session.show_markers = flag("--markers");
    session.lightmaps = !flag("--nolight");
    // the player's picture options in a window; headless pictures draw as the original unless `--enhanced`
    session.settings_quality = !flag("--headless") || flag("--enhanced");
    if let Some(b) = value("--brightness").and_then(|v| v.parse().ok()) {
        session.brightness = b;
        session.brightness_fixed = true;
    }
    if let Some(at) = value("--at") {
        let v: Vec<f32> = at.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if v.len() == 3 {
            session.teleport([v[0], v[1], v[2]], value("--yaw").and_then(|y| y.parse().ok()).unwrap_or(0.0));
        }
    }
    if let Some(p) = value("--pitch").and_then(|v| v.parse::<f32>().ok()) {
        session.set_pitch(p);
    }
    if flag("--bench") {
        if let Some(cmd) = value("--exec") {
            session.exec(&cmd);
        }
        let size = value("--size").and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))).unwrap_or((1920, 1080));
        let seconds: f64 = value("--seconds").and_then(|v| v.parse().ok()).unwrap_or(20.0);
        let turn: f32 = value("--turn").and_then(|v| v.parse().ok()).unwrap_or(18.0);
        if let Err(e) = bench::run(&mut session, size, seconds, turn, value("--csv"), value("--profile")) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    if flag("--headless") {
        let frames: u64 = value("--frames").and_then(|v| v.parse().ok()).unwrap_or(120);
        let input = Input { forward: if flag("--forward") { 1.0 } else { 0.0 }, run: true, ..Default::default() };
        let offline = value("--audio-out").map(|out| (out, std::sync::Arc::new(std::sync::Mutex::new(dx_audio::Mixer::new(44100)))));
        if let Some((_, mixer)) = &offline {
            session.attach_audio(mixer.clone());
        }
        let mut rendered: Vec<f32> = Vec::new();
        if let Some(cmd) = value("--exec") {
            // `--exec "ShowMainMenu"`: a console command before the run (for screenshots of screens)
            session.exec(&cmd);
        }
        if let Some(screen) = value("--screen") {
            // `--screen MenuScreenSound`: opens a menu screen by class name (for screenshots)
            if !session.open_menu_screen(&screen) {
                eprintln!("could not open menu screen {screen}");
            }
        }
        let start = session.player_location();
        let t0 = Instant::now();
        let shot_size: (u32, u32) = value("--size").and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))).unwrap_or((W * 2, H * 2));
        let mut vk_renderer = None;
        for i in 0..frames {
            // a Vulkan shot: the renderer is made two steps before the end, so the last steps make the per-frame
            // lightmaps and fog maps it draws (they are only made once a renderer exists)
            if flag("--vk") && value("--shot").is_some() && i + 2 == frames {
                session.render_vk(&mut vk_renderer, shot_size.0, shot_size.1).expect("vulkan render");
            }
            // `--use`: press the use key once, early on
            let input = Input { use_key: flag("--use") && i == 3, ..input };
            session.step(&input, 1.0 / 30.0);
            if let Some((_, mixer)) = &offline {
                let mut chunk = vec![0.0f32; 2 * 1470];
                mixer.lock().unwrap().mix(&mut chunk);
                rendered.extend_from_slice(&chunk);
            }
        }
        if let Some((out, _)) = &offline {
            std::fs::write(out, dx_audio::encode_wav(44100, 2, &rendered)).expect("write audio");
            let rms = (rendered.iter().map(|x| x * x).sum::<f32>() / rendered.len().max(1) as f32).sqrt();
            println!("audio -> {out}: {:.1} s, rms {rms:.3}, {} sounds played, music {:?}", rendered.len() as f32 / 2.0 / 44100.0, session.audio.as_ref().map(|a| a.sounds_played).unwrap_or(0), session.audio.as_ref().and_then(|a| a.music_title.clone()));
        }
        let end = session.player_location();
        println!("{frames} frames in {:?}; player {start:?} -> {end:?}; {} script errors", t0.elapsed(), session.issues.len());
        for i in session.issues.iter().take(8) {
            println!("  error: {} {}: {}", i.class, i.event, i.error);
        }
        if flag("--log") {
            for l in session.vm.log.iter().take(40) {
                println!("  log: {l}");
            }
        }
        if let Some(out) = value("--shot") {
            let (sw, sh) = shot_size;
            let img = if flag("--vk") {
                let mut r = vk_renderer.take();
                let img = session.render_vk(&mut r, sw, sh).expect("vulkan render");
                r.unwrap().destroy();
                img
            } else {
                session.render(sw, sh)
            };
            std::fs::write(&out, dx_soft::png(img.width, img.height, &img.rgba)).expect("write screenshot");
            println!("screenshot -> {out}; {} actor meshes drawn", session.meshes_drawn);
        }
        return;
    }
    let max_frames: Option<u64> = value("--frames").and_then(|v| v.parse().ok());
    if let Some(cmd) = value("--exec") {
        // `--exec "SetRes 1280x720"`: a console command before the game starts
        session.exec(&cmd);
    }
    // `--windowed` or `--size WxH`: a window instead of fullscreen
    let size = value("--size").and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))));
    let windowed = flag("--windowed") || size.is_some();
    let result = if flag("--vk") || !flag("--soft") { run_window_vk(&mut session, map, max_frames, windowed, size) } else { run_window(&mut session, map, max_frames) };
    if let Err(e) = result {
        fatal(&format!("error: {e}"), 1);
    }
}

fn run_window(session: &mut Session, map: &str, max_frames: Option<u64>) -> Result<(), String> {
    use sdl3::event::Event;
    use sdl3::keyboard::Scancode;
    use sdl3::pixels::PixelFormat;
    use sdl3::render::FRect;
    let sdl = sdl3::init().map_err(|e| e.to_string())?;
    let video = sdl.video().map_err(|e| e.to_string())?;
    let window = video.window(&format!("Deus Ex Aerugo (software) - {map}"), W * 2, H * 2).position_centered().resizable().build().map_err(|e| e.to_string())?;
    sdl.mouse().set_relative_mouse_mode(&window, true);
    let mut canvas = window.into_canvas();
    let creator = canvas.texture_creator();
    let mut texture = creator.create_texture_streaming(PixelFormat::RGBA32, W, H).map_err(|e| e.to_string())?;
    let _audio = start_audio(&sdl, session);
    let mut events = sdl.event_pump().map_err(|e| e.to_string())?;
    let mut pad = sdl_input::SdlInput::new(&sdl);
    let mut last = Instant::now();
    let mut title_at = Instant::now();
    let (mut frames_since, mut render_ms) = (0u32, 0.0f64);
    'running: loop {
        if max_frames.map(|m| session.frames >= m).unwrap_or(false) {
            break;
        }
        for e in events.poll_iter() {
            match e {
                Event::Quit { .. } => break 'running,
                Event::KeyDown { scancode: Some(Scancode::Escape), .. } => break 'running,
                Event::KeyDown { scancode: Some(Scancode::ScrollLock), .. } => session.show_markers = !session.show_markers,
                Event::Window { win_event: sdl3::event::WindowEvent::FocusLost, .. } => session.input.reset(),
                e => {
                    pad.event(session, (W as f32 * 2.0, H as f32 * 2.0), &e);
                }
            }
        }
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        session.advance(dt);
        let t = Instant::now();
        let img = session.render(W, H);
        render_ms += t.elapsed().as_secs_f64() * 1000.0;
        texture.update(None, &img.rgba, (W * 4) as usize).map_err(|e| e.to_string())?;
        canvas.clear();
        let (ow, oh) = canvas.output_size().map_err(|e| e.to_string())?;
        canvas.copy(&texture, None, FRect::new(0.0, 0.0, ow as f32, oh as f32)).map_err(|e| e.to_string())?;
        canvas.present();
        frames_since += 1;
        if title_at.elapsed() > Duration::from_millis(500) {
            let p = session.player_location();
            let (on_ground, errors, meshes) = (session.on_ground(), session.issues.len(), session.meshes_drawn);
            let title = format!(
                "Deus Ex Aerugo (software) - {map} | {:.0} fps (render {:.0} ms, {} meshes) | pos {:.0} {:.0} {:.0} | {} | script errors {}",
                frames_since as f64 / title_at.elapsed().as_secs_f64(),
                render_ms / frames_since.max(1) as f64,
                meshes,
                p[0],
                p[1],
                p[2],
                if on_ground { "ground" } else { "air" },
                errors
            );
            let _ = canvas.window_mut().set_title(&title);
            title_at = Instant::now();
            frames_since = 0;
            render_ms = 0.0;
        }
    }
    Ok(())
}

/// The window path with the Vulkan renderer (swapchain). `--shot` is not supported here (use `--headless --vk --shot`).
/// Starts fullscreen at the display's own resolution unless a window is asked for (`windowed`, `size`) or the player
/// chose otherwise in the Display menu before (remembered in the port's `DeusEx.ini`).
fn run_window_vk(session: &mut Session, map: &str, max_frames: Option<u64>, windowed: bool, size: Option<(u32, u32)>) -> Result<(), String> {
    use sdl3::event::Event;
    use sdl3::keyboard::{Mod, Scancode};
    use std::ffi::CString;
    let sdl = sdl3::init().map_err(|e| e.to_string())?;
    let video = sdl.video().map_err(|e| e.to_string())?;
    // the display: its own resolution (in pixels) and video modes
    let primary = video.get_primary_display().map_err(|e| e.to_string())?;
    let px = |w: i32, h: i32, density: f32| (((w as f32) * density).round() as u32, ((h as f32) * density).round() as u32);
    let native = primary.get_mode().map(|m| px(m.w, m.h, m.pixel_density)).unwrap_or((W * 2, H * 2));
    let modes: Vec<(u32, u32)> = primary.get_fullscreen_modes().unwrap_or_default().iter().map(|m| px(m.w, m.h, m.pixel_density)).collect();
    let (saved_full, saved_window, saved_start) = session.saved_display();
    let fullscreen = !windowed && saved_start.unwrap_or(true);
    let window_size = size.or(saved_window).unwrap_or((W * 2, H * 2));
    let full_res = saved_full.unwrap_or(native);
    {
        let mut d = session.display.borrow_mut();
        d.native = native;
        d.modes = dx_play::display::resolution_list(native, &modes);
        d.fullscreen = fullscreen;
        d.resolution = if fullscreen { full_res } else { window_size };
        d.last_fullscreen = Some(full_res);
        d.last_window = Some(window_size);
    }
    println!("display {}x{}; {} at {}x{}", native.0, native.1, if fullscreen { "fullscreen" } else { "window" }, session.display.borrow().resolution.0, session.display.borrow().resolution.1);
    let mut builder = if fullscreen { video.window(&format!("Deus Ex Aerugo - {map}"), native.0, native.1) } else { video.window(&format!("Deus Ex Aerugo - {map}"), window_size.0, window_size.1) };
    builder.position_centered().resizable().vulkan();
    if fullscreen {
        builder.fullscreen();
    }
    let mut window = builder.build().map_err(|e| e.to_string())?;
    sdl.mouse().set_relative_mouse_mode(&window, true);
    let ext_names = window.vulkan_instance_extensions().map_err(|e| e.to_string())?;
    let ext_c: Vec<CString> = ext_names.iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
    let ext_ptrs: Vec<*const std::ffi::c_char> = ext_c.iter().map(|c| c.as_ptr()).collect();
    let mut renderer = session.make_vk_renderer(&ext_ptrs)?;
    println!("Vulkan device: {}", renderer.cx.device_name);
    let surface = unsafe {
        let mut surf: sdl3::sys::vulkan::VkSurfaceKHR = std::mem::zeroed();
        use dx_vk::ash::vk::Handle;
        let ok = sdl3::sys::vulkan::SDL_Vulkan_CreateSurface(window.raw(), renderer.cx.instance.handle().as_raw() as usize as _, std::ptr::null(), &mut surf);
        if !ok {
            return Err(format!("cannot create a Vulkan surface: {}", sdl3::get_error()));
        }
        dx_vk::ash::vk::SurfaceKHR::from_raw(surf as usize as u64)
    };
    session.follow_settings();
    let mut settings = session.aerugo_settings();
    let vsync = dx_vk::present::VSync::from_setting(settings.vsync);
    renderer.attach_window(surface, window.size_in_pixels(), vsync).map_err(|e| e.to_string())?;
    println!("vertical sync {vsync:?}");
    let mut settings_at = Instant::now();
    // the frame rate limit: the time the next frame may start
    let mut next_frame = Instant::now();
    renderer.render_size = if fullscreen { Some(full_res) } else { None };
    let _audio = start_audio(&sdl, session);
    let mut events = sdl.event_pump().map_err(|e| e.to_string())?;
    let mut pad = sdl_input::SdlInput::new(&sdl);
    video.text_input().start(&window);
    sdl.mouse().show_cursor(false);
    let mut ui_mouse = false;
    let mut last = Instant::now();
    let mut title_at = Instant::now();
    let mut frames_since = 0u32;
    // with `--frames`: the frame rate after the first 60 frames, printed at the end
    let (mut drawn_frames, mut measure_from) = (0u64, None);
    'running: loop {
        if max_frames.map(|m| session.frames >= m).unwrap_or(false) {
            break;
        }
        drawn_frames += 1;
        if drawn_frames == 60 {
            measure_from = Some(Instant::now());
        }
        for e in events.poll_iter() {
            match e {
                Event::Quit { .. } => break 'running,
                // debug toggle on a key the bindings do not use
                Event::KeyDown { scancode: Some(Scancode::ScrollLock), .. } => session.lightmaps = !session.lightmaps,
                // Alt+Enter: fullscreen <-> window, each at its remembered resolution
                Event::KeyDown { scancode: Some(Scancode::Return), keymod, .. } if keymod.intersects(Mod::LALTMOD | Mod::RALTMOD) => {
                    session.display.borrow_mut().toggle_fullscreen();
                }
                Event::Window { win_event: sdl3::event::WindowEvent::FocusLost, .. } => session.input.reset(),
                e => {
                    let sz = window.size();
                    pad.event(session, (sz.0 as f32, sz.1 as f32), &e);
                }
            }
        }
        if session.ui_wants_mouse() != ui_mouse {
            ui_mouse = !ui_mouse;
            sdl.mouse().set_relative_mouse_mode(&window, !ui_mouse);
        }
        // a script moved the window system's cursor: the pointer follows
        if ui_mouse {
            let sz = window.size();
            if let Some((x, y)) = session.ui_cursor_warp(sz.0 as f32, sz.1 as f32) {
                sdl.mouse().warp_mouse_in_window(&window, x, y);
            }
        }
        if session.quit {
            break 'running;
        }
        // a resolution change from the Display menu (`SetRes`) or Alt+Enter
        let pending = session.display.borrow_mut().pending.take();
        if let Some(((w, h), to_full)) = pending {
            let mut d = session.display.borrow().clone();
            let full = to_full.unwrap_or(d.fullscreen);
            if full {
                if !d.fullscreen {
                    let _ = window.set_fullscreen(true);
                }
                renderer.render_size = Some((w, h));
            } else {
                if d.fullscreen {
                    let _ = window.set_fullscreen(false);
                }
                let _ = window.set_size(w, h);
                renderer.render_size = None;
            }
            d.fullscreen = full;
            d.resolution = (w, h);
            if full {
                d.last_fullscreen = Some((w, h));
            } else {
                d.last_window = Some((w, h));
            }
            session.save_display(&d);
            *session.display.borrow_mut() = d;
            println!("resolution {w}x{h} {}", if full { "fullscreen" } else { "window" });
        }
        // in a window the resolution is the window's size (the player may drag its edges)
        if !session.display.borrow().fullscreen {
            let mut d = session.display.borrow_mut();
            d.resolution = window.size_in_pixels();
            d.last_window = Some(d.resolution);
        }
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        session.advance(dt);
        if session.take_map_changed() {
            session.reload_vk_world(&mut renderer)?;
        }
        renderer.world.use_lightmaps = session.lightmaps && renderer.world.lightmap_info.is_some();
        renderer.world.time = session.level_time();
        renderer.time = session.viewport_time();
        renderer.ui.flash = session.screen_flash();
        session.render_vk_window(&mut renderer, window.size_in_pixels())?;
        frames_since += 1;
        // settings the menus change while playing (a few times a second)
        if settings_at.elapsed() > Duration::from_millis(250) {
            settings_at = Instant::now();
            session.follow_settings();
            let now = session.aerugo_settings();
            if now.vsync != settings.vsync {
                renderer.set_vsync(dx_vk::present::VSync::from_setting(now.vsync)).map_err(|e| e.to_string())?;
            }
            session.follow_quality(&mut renderer)?;
            settings = now;
        }
        // the frame rate limit: Auto is 3 below the display's refresh rate (inside a FreeSync/G-Sync range)
        let limit = match settings.frame_limit {
            -1 => window.get_display().ok().and_then(|d| d.get_mode().ok()).map(|m| m.refresh_rate - 3.0).filter(|r| *r > 20.0).unwrap_or(0.0),
            n if n > 0 => n as f32,
            _ => 0.0,
        };
        if limit > 0.0 {
            let period = Duration::from_secs_f64(1.0 / limit as f64);
            next_frame = (next_frame + period).max(Instant::now() - period);
            // sleep most of the wait, spin the last moment (the scheduler is coarser than a frame)
            while let Some(left) = next_frame.checked_duration_since(Instant::now()) {
                if left > Duration::from_micros(1500) {
                    std::thread::sleep(left - Duration::from_micros(1000));
                } else {
                    std::hint::spin_loop();
                }
            }
        } else {
            next_frame = Instant::now();
        }
        if title_at.elapsed() > Duration::from_millis(500) {
            let p = session.player_location();
            let title = format!(
                "Deus Ex Aerugo - {map} | {:.0} fps | {} meshes | pos {:.0} {:.0} {:.0} | script errors {}",
                frames_since as f64 / title_at.elapsed().as_secs_f64(),
                session.meshes_drawn,
                p[0],
                p[1],
                p[2],
                session.issues.len()
            );
            let _ = window.clone().set_title(&title);
            title_at = Instant::now();
            frames_since = 0;
        }
    }
    if let (Some(_), Some(t)) = (max_frames, measure_from) {
        let n = drawn_frames - 60;
        let s = t.elapsed().as_secs_f64();
        println!("{n} frames in {s:.2} s: {:.0} fps (GPU {:.2} ms)", n as f64 / s, renderer.gpu_ms.unwrap_or(0.0));
    }
    // `XGameEngineExt::Destroy`: the windows go before the engine
    session.vm.ui_destroy_all_windows();
    renderer.destroy();
    Ok(())
}

/// Opens the sound device and connects the session to it (`--no-audio` skips it; a missing device is only reported).
fn start_audio(sdl: &sdl3::Sdl, session: &mut Session) -> Option<dx_audio::AudioEngine> {
    if std::env::args().any(|a| a == "--no-audio") {
        return None;
    }
    let wanted = session.aerugo_settings().output_rate.max(0) as u32;
    match sdl.audio().map_err(|e| e.to_string()).and_then(|a| {
        let rate = dx_audio::AudioEngine::output_rate(&a, wanted);
        println!("sound output {rate} Hz");
        dx_audio::AudioEngine::start(&a, rate)
    }) {
        Ok(engine) => {
            session.attach_audio(engine.mixer.clone());
            Some(engine)
        }
        Err(e) => {
            eprintln!("no audio: {e}");
            None
        }
    }
}

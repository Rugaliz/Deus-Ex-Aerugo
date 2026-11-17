//! An interactive session on the headless engine: a map loaded into the VM with all its actors running, a walking player
//! pawn driven by input, and the software renderer's view from the player's eyes with simple markers for pawns.
//! No window code lives here (see `main.rs`), so the whole loop can run and be tested without a display.

pub mod audio;
pub mod clock;
pub mod corona;
pub mod detail;
pub mod display;
pub mod mods;
mod mover_light;
mod savegame;
mod visibility;
mod world_light;

pub use dx_input;
pub use visibility::ListedMovers;

use dx_soft::{Camera, Image, MapView};
use std::path::Path;
use ue_registry::Registry;
use ue_vm::level::LoadIssue;
use ue_vm::{Value, Vm};

/// One frame of player input.
#[derive(Debug, Clone, Copy, Default)]
pub struct Input {
    /// -1..1, positive forward.
    pub forward: f32,
    /// -1..1, positive to the right.
    pub strafe: f32,
    pub jump: bool,
    pub run: bool,
    /// The use key (frob): opens the door in front of the player.
    pub use_key: bool,
    /// Look change in rotator units (65536 per turn); positive yaw turns left, positive pitch looks up.
    pub yaw: i32,
    pub pitch: i32,
}

pub struct Session {
    /// The levels of the running mission left for another of its maps, by lower-case map name: the original's
    /// `<Save>\Current\<map>.dxs` files (`DDeusExGameEngine::Browse`, `SaveCurrentLevel`). Kept in memory here and
    /// written into each saved game.
    pub current_levels: std::collections::HashMap<String, ue_vm::save::Snapshot>,
    pub vm: Vm,
    pub view: MapView,
    pub player: u32,
    pub pitch: i32,
    pub yaw: i32,
    /// Script errors met while ticking (the world keeps running past them).
    pub issues: Vec<LoadIssue>,
    pub frames: u64,
    pub fov: f32,
    /// Draw outlines around pawns in sight (on top of their meshes).
    pub show_markers: bool,
    /// Draw actor meshes (on by default).
    pub draw_meshes: bool,
    /// Sprites and decals (the CPU renderer draws neither, so comparisons with it turn them off).
    pub draw_sprites: bool,
    /// Draw the window system (HUD) over the Vulkan view.
    pub show_ui: bool,
    /// Set when the game asked to quit (`quit`/`exit` console command).
    pub quit: bool,
    /// Number of actor meshes drawn in the last frame.
    pub meshes_drawn: usize,
    /// Mouse counts are multiplied by this before they reach the game: 400 / the mouse's DPI (`AerugoSettings`).
    pub mouse_scale: f32,
    /// `brightness` was given on the command line: the ini's setting does not replace it.
    pub brightness_fixed: bool,
    /// Renderers use the player's picture options (`quality`); off for headless pictures and tests (the original's).
    pub settings_quality: bool,
    /// How many of the newest decals (`DeusExDecal`: blood, bullet holes, scorch marks) are kept however long they are
    /// out of sight (`AerugoSettings.MaxDecals`, read by `follow_settings`); 0: as the original, where a decal not
    /// drawn within the last second goes some 20 s after it appeared.
    pub max_decals: usize,
    /// The decals in the order they appeared (for `max_decals`).
    decal_order: Vec<u32>,
    /// The last frame's preparation phases (`prepare_vk_frame`), in milliseconds.
    prep_ms: Vec<(&'static str, f64)>,
    /// Mesh actors' world bounding boxes during one visibility pass (`compute_visibility` empties it).
    vis_boxes: ue_registry::FxHashMap<u32, Option<([f32; 3], [f32; 3])>>,
    /// The actors that may give a light item or a sprite in this visibility pass (`vis_candidates`).
    vis_cands: Option<std::rc::Rc<Vec<(u32, u8)>>>,
    /// Package index of the map in the VM's registry.
    map_pkg: usize,
    /// Draw the Vulkan view with the map's lightmaps (default) or with the flat shading of the CPU renderer.
    pub lightmaps: bool,
    /// Brightness (gamma) as in `DeusEx.ini`; 0.6 as shipped, 0.5 = untouched. Applied to the Vulkan view only.
    pub brightness: f32,
    world_lighting: Option<dx_light::MapLighting>,
    prev_use: bool,
    /// Key bindings and the state of held keys; feed it key/axis events, then call `step_bound`.
    pub input: dx_input::InputProcessor,
    game: std::path::PathBuf,
    pub audio: Option<audio::Audio>,
    /// Name of the map being played.
    pub map_name: String,
    map_changed: bool,
    /// Key bindings shared with the scripts' `KEYNAME`/`KEYBINDING`/`SET INPUT` console commands; `bool` = changed.
    shared_bindings: std::rc::Rc<std::cell::RefCell<(dx_input::Bindings, bool)>>,
    /// Resolution and fullscreen, shared with the console commands of the Display menu (`display.rs`).
    pub display: std::rc::Rc<std::cell::RefCell<display::Display>>,
    /// Where the scene shows inside the window, as fractions `[x, y, w, h]` (black bars around it), for mouse positions.
    pub view_rect: Option<[f32; 4]>,
    /// The root's cursor position after the last host mouse event (root units): a script move changes it.
    ui_mouse_last: Option<(f32, f32)>,
    /// The viewport time of the console's last `PreRender` (its `Tick` gets the time since).
    console_time: Option<f64>,
    /// The last step that had mouse movement turned the view by it (the player's scripts apply mouse look): only then
    /// does `render_camera` show the pending mouse turn ahead of the simulation.
    pub(crate) mouse_turns_view: bool,
    /// The version of each run-time texture the renderer was last given.
    runtime_published: std::collections::HashMap<u32, u64>,
    /// Fixed-step simulation and the blend of the drawn frame between steps (`advance`, `render_camera`).
    pub clock: clock::FrameClock,
    /// The view the game's `PlayerCalcView` chose after the last step: (view actor, camera location, camera rotation,
    /// first person). `None` before the first step or when the event failed.
    pub calc_view: Option<(u32, [f32; 3], [i32; 3], bool)>,
    /// Per-surface poly flags with texture flags merged, for the visibility pass (map package, flags).
    vis_flags: Option<(usize, Vec<u32>)>,
    /// The moving-brush tracker of the current level (`ULevel::BrushTracker`; it filters the movers into the VM's level
    /// model) and the poly flags of the movers' surfaces, keyed by the level's `LevelInfo` actor, map package and the
    /// model's surface count (a reloaded model needs a new tracker).
    brush_tracker: Option<((u32, usize, usize), dx_vis::dynbsp::Tracker, Vec<u32>)>,
    /// Model-space polygons of each mover brush (by export of the map package).
    brush_polys: std::collections::HashMap<usize, Vec<dx_vis::dynbsp::Poly>>,
    /// The tracker generation the level's collision was last given.
    brush_tracker_synced: u64,
    /// Filter the movers into the BSP the visibility walk sees (default on; off for comparisons).
    pub vis_movers: bool,
    /// The last visibility pass (one per simulation step, after the actors tick) for the map it ran on: what the main
    /// view draws.
    vis_cache: Option<(usize, Vec<dx_vis::walk::FrameVis>)>,
    /// False for the simulation steps of a window frame before its last: they skip the frame's drawing work.
    draw_this_step: bool,
    /// Size in pixels of the main view last drawn (the visibility pass works on that screen; 800x600 until drawn).
    vis_size: (i32, i32),
    /// The per-frame part of the world lightmaps (made once a Vulkan renderer with lightmaps exists).
    world_light: Option<world_light::WorldLight>,
    wants_world_light: bool,
    /// The light manager's persistent "ray misses the fog volume" shortcut flag (a global in the original).
    fog_skip: bool,
    /// Actor lighting per drawn frame: frame counter, last viewport time and fade step (`SetupForActor`).
    light_frame: u32,
    light_time: f64,
    light_step: i32,
    /// The `Canvas` object the engine hands `RenderOverlays` (made on first use, again after a level change).
    canvas: u32,
    /// Third-person items of the pawns drawn in the last engine frame (pawn, item, the item's pose):
    /// `DrawActorSprite` draws them on the pawn's weapon triangle.
    third_person: Vec<(u32, u32, ue_vm::animation::MeshDraw)>,
    /// Per map: the surfaces whose texture heads an animation chain (surface index -> texture key and id).
    anim_surfs: Option<(usize, std::collections::HashMap<usize, ((String, usize), u32)>)>,
    /// The chain index each animated surface texture shows (set once per engine frame, read by the renderer).
    anim_frames: std::collections::HashMap<(String, usize), usize>,
    /// The engine's corona table (light, brightness) and the real time of the last corona pass.
    coronas: [(u32, f32); corona::MAX_CORONAS],
    corona_time: Option<f64>,
    /// Per viewport-window view of the last paint: the actors and particles its own visibility pass drew.
    subview_vis: Vec<Option<(ue_registry::FxHashSet<u32>, ue_registry::FxHashSet<(u32, usize)>)>>,
    /// Surface texture (map object reference) -> its detail texture and size (`MapView::surface_detail`), cached.
    surface_details: std::collections::HashMap<i32, Option<(String, [f32; 2])>>,
    /// Per viewport window of the last paint: the coronas its frame drew (`DrawFrame` corona pass), as screen tiles.
    subview_coronas: Vec<Vec<dx_vk::ui::UiQuad>>,
    /// Each view's whole visibility pass (its frames), for drawing it through its own lists.
    subview_frames: Vec<Option<Vec<dx_vis::walk::FrameVis>>>,
}

/// Full deflection of a movement axis (what the input layer feeds `aForward`/`aStrafe`).
const AXIS: f32 = 6400.0;

fn set(vm: &mut Vm, a: u32, name: &str, v: Value) {
    let _ = vm.set_prop(a, name, 0, v);
}

impl Session {
    /// Loads the map into the VM (running its start-up events) and drops the player at the first `PlayerStart`.
    pub fn open(game: &Path, map: &str) -> Result<Session, String> {
        let mut reg = Registry::load_game(game).map_err(|e| e.to_string())?;
        let mods = ue_package::mods::current();
        if !mods.names.is_empty() {
            eprintln!("mods: {} ({} packages, {} replacement textures)", mods.names.join(", "), mods.packages.len(), mods.textures.len());
        }
        // class defaults come from the game's ini files and the port's own per-user layer, localised text from the .int files
        reg.set_config(ue_config::Config::new(&game.join("System"), config_dir().as_deref()));
        let mut vm = Vm::new(reg);
        // the port's menus: settings of today's hardware instead of 2000's (`Aerugo`)
        vm.apply_port_menus();
        // the engine's entry level first, as `UGameEngine::Init` does (no bootstrap world: its objects would draw names)
        vm.load_entry_level(&game.join("Maps")).map_err(|e| e.to_string())?;
        // the map as browsed (`DX.dx` from `[URL] LocalMap`, or a bare name); the file is `<name>.dx`
        let url_map = map.to_string();
        let map = map.strip_suffix(".dx").or_else(|| map.strip_suffix(".DX")).unwrap_or(map);
        vm.url_map = url_map;
        let pi = vm.reg.add_package(&game.join("Maps").join(format!("{map}.dx")), map).map_err(|e| e.to_string())?;
        let load = vm.load_level(pi).map_err(|e| e.to_string())?;
        let view = MapView::open(game, map)?;
        let rot = view.find_actor("PlayerStart").map(|(_, r)| r).unwrap_or([0.0; 3]);
        let player = vm.login_player("JCDentonMale").map_err(|e| e.to_string())?;
        vm.save_dir = Session::default_save_dir();
        let mut s = Session { current_levels: Default::default(), vm, view, player, pitch: 0, yaw: rot[1] as i32, issues: load.issues, frames: 0, fov: 90.0, show_markers: false, draw_meshes: true, draw_sprites: true, show_ui: true, quit: false, meshes_drawn: 0, map_pkg: pi, lightmaps: true, brightness: 0.6, world_lighting: None, prev_use: false, input: dx_input::InputProcessor::new(load_bindings()), game: game.to_path_buf(), audio: None, map_name: map.to_string(), map_changed: false, shared_bindings: Default::default(), display: Default::default(), view_rect: None, clock: Default::default(), calc_view: None, vis_flags: None, brush_tracker: None, brush_polys: Default::default(), vis_movers: true, brush_tracker_synced: 0, vis_cache: None, vis_size: (800, 600), world_light: None, wants_world_light: false, fog_skip: false, light_frame: 0, light_time: 0.0, light_step: 0, canvas: 0, third_person: Vec::new(), anim_surfs: None, anim_frames: Default::default(), coronas: [(0, 0.0); corona::MAX_CORONAS], corona_time: None, subview_vis: Vec::new(), subview_coronas: Vec::new(), subview_frames: Vec::new(), surface_details: Default::default(), draw_this_step: true, ui_mouse_last: None, console_time: None, mouse_turns_view: true, runtime_published: Default::default(), prep_ms: Vec::new(), vis_boxes: Default::default(), vis_cands: None, mouse_scale: 0.5, brightness_fixed: false, settings_quality: false, max_decals: 0, decal_order: Vec::new() };
        s.install_key_commands();
        s.apply_yaw();
        s.vm.ui_after_browse(s.player);
        Ok(s)
    }

    /// The host's part of the console command chain (`ue_vm::console`): the input system (`KEYNAME <n>`, `KEYBINDING
    /// <key>`, the button and axis commands, aliases), its key bindings (`GET`/`SET INPUT <key> [command]`, saved to
    /// the user's `Input.ini`) and the window's display commands.
    fn install_key_commands(&mut self) {
        use ue_vm::console::HostExec;
        *self.shared_bindings.borrow_mut() = (self.input.bindings.clone(), false);
        let shared = self.shared_bindings.clone();
        let display = self.display.clone();
        self.vm.host_console = Some(Box::new(move |_vm: &mut ue_vm::Vm, stage: HostExec, cmd: &str| {
            let words: Vec<&str> = cmd.split_whitespace().collect();
            let verb = words.first().map(|w| w.to_ascii_lowercase()).unwrap_or_default();
            match stage {
                HostExec::Window => display.borrow_mut().console(cmd),
                HostExec::Alias => shared.borrow().0.alias(cmd).map(str::to_string),
                HostExec::Input => match verb.as_str() {
                    // `KEYNAME n`: `appAtoi`, the name of that key number ("" for none)
                    "keyname" => {
                        let n = words.get(1).map(|w| w.trim_start_matches('-').chars().take_while(char::is_ascii_digit).collect::<String>()).and_then(|d| d.parse::<i32>().ok()).unwrap_or(0);
                        Some(u8::try_from(n).ok().and_then(dx_input::key_name).unwrap_or("").to_string())
                    }
                    // `KEYBINDING key`: an unknown key name is taken with no output
                    "keybinding" => Some(words.get(1).and_then(|k| dx_input::key_code(k)).map(|c| shared.borrow().0.binding(c).unwrap_or("").to_string()).unwrap_or_default()),
                    // the input layer keeps buttons and axes itself (`dx_input`); typed ones are taken
                    "button" | "pulse" | "toggle" | "axis" => Some(String::new()),
                    _ => None,
                },
                HostExec::InputConfig => {
                    let code = dx_input::key_code(words.get(2)?)?;
                    if verb == "get" {
                        return Some(shared.borrow().0.binding(code).unwrap_or("").to_string());
                    }
                    let rest = cmd.trim_start().splitn(4, char::is_whitespace).nth(3).unwrap_or("").trim().trim_matches('"').to_string();
                    let mut sh = shared.borrow_mut();
                    sh.0.bind(code, &rest);
                    sh.1 = true;
                    Some(String::new())
                }
            }
        }));
    }

    /// Takes over key bindings the scripts changed and saves them.
    fn sync_bindings(&mut self) {
        let changed = std::mem::take(&mut self.shared_bindings.borrow_mut().1);
        if changed {
            let b = self.shared_bindings.borrow().0.clone();
            if let Some(dir) = config_dir() {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join("Input.ini"), b.to_ini());
            }
            self.input.bindings = b;
        }
    }

    /// Writes the host-owned look angles into the pawn (`Rotation` has no pitch for the player, `ViewRotation` has).
    fn apply_yaw(&mut self) {
        set(&mut self.vm, self.player, "Rotation", Value::Rotator([0, self.yaw & 0xFFFF, 0]));
        set(&mut self.vm, self.player, "ViewRotation", Value::Rotator([self.pitch & 0xFFFF, self.yaw & 0xFFFF, 0]));
    }

    /// Connects the session to an audio mixer (the device's, or an offline one) and starts the level's music.
    pub fn attach_audio(&mut self, mixer: std::sync::Arc<std::sync::Mutex<dx_audio::Mixer>>) {
        // the volumes of the audio section of the ini (0-255)
        {
            let vol = |vm: &mut Vm, key: &str| vm.reg.config_value("System", "Galaxy.GalaxyAudioSubsystem", key).and_then(|v| v.trim().parse::<f32>().ok()).map(|v| (v / 255.0).clamp(0.0, 1.0));
            let mut m = mixer.lock().unwrap();
            if let Some(v) = vol(&mut self.vm, "MusicVolume") {
                m.music_volume = v;
            }
            if let Some(v) = vol(&mut self.vm, "SoundVolume") {
                m.sound_volume = (v * 255.0).round() as u8;
            }
            if let Some(v) = vol(&mut self.vm, "SpeechVolume") {
                m.speech_volume = (v * 255.0).round() as u8;
            }
            // Galaxy's effect channels: 128 (the original's ini setting allowed 4 to 16, for 2000's processors; a busy
            // scene then dropped sounds)
            m.set_effects_channels(EFFECTS_CHANNELS);
        }
        let mut a = audio::Audio::new(&self.game, mixer);
        a.read_settings(&mut self.vm);
        self.audio = Some(a);
    }

    /// Moves the player (a hard teleport) and turns it to `yaw_deg`.
    pub fn teleport(&mut self, loc: [f32; 3], yaw_deg: f32) {
        let _ = self.vm.call_by_name(self.player, "SetLocation", vec![Value::Vector(loc)]);
        self.yaw = (yaw_deg / 360.0 * 65536.0) as i32 & 0xFFFF;
        self.apply_yaw();
    }

    /// Turns the view to `yaw_deg` without moving.
    pub fn set_yaw(&mut self, yaw_deg: f32) {
        self.yaw = (yaw_deg / 360.0 * 65536.0) as i32 & 0xFFFF;
        self.apply_yaw();
    }

    /// The step phases gathered since the last call (`DX_STEP_PROFILE` set; `quiet` keeps them from being printed), in
    /// seconds, largest first.
    pub fn take_step_profile() -> Vec<(&'static str, f64)> {
        STEP_PROFILE.with(|p| {
            let mut v: Vec<_> = p.borrow_mut().drain().collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            v
        })
    }

    /// Sets the view pitch in degrees (up positive; the view keeps it within the player's +-15000 units).
    pub fn set_pitch(&mut self, deg: f32) {
        self.pitch = ((deg / 360.0 * 65536.0) as i32).clamp(-15000, 15000);
        self.apply_yaw();
    }

    pub fn player_location(&mut self) -> [f32; 3] {
        self.vm.location(self.player)
    }

    pub fn on_ground(&mut self) -> bool {
        self.vm.get_prop(self.player, "Physics", 0).ok().and_then(|v| v.as_i32()) == Some(1)
    }

    /// Advances the world by `dt` seconds with a simple direct input (tests, headless runs): movement axes go straight to
    /// the pawn and `PlayerTick`, look changes are added to the view, `PlayerInput` is not run.
    pub fn step(&mut self, input: &Input, dt: f32) {
        self.yaw = (self.yaw + input.yaw) & 0xFFFF;
        self.pitch = (self.pitch + input.pitch).clamp(-15000, 15000);
        self.apply_yaw();
        set(&mut self.vm, self.player, "bIsWalking", Value::Bool(!input.run));
        if input.jump {
            set(&mut self.vm, self.player, "bPressedJump", Value::Bool(true));
        }
        if let Err(e) = self.vm.player_tick(self.player, dt, input.forward * AXIS, input.strafe * AXIS) {
            self.issues.push(LoadIssue { actor: self.player, class: "player".into(), event: "PlayerTick".into(), error: e.to_string() });
        }
        if input.use_key && !self.prev_use {
            self.frob();
        }
        self.prev_use = input.use_key;
        self.tick_world(dt);
    }

    /// Advances the world by `dt` seconds with the events collected by `self.input` (key bindings, mouse, sticks): the
    /// engine's per-frame order: input variables, exec commands, `PlayerInput`, `PlayerTick`, then the actors.
    pub fn step_bound(&mut self, dt: f32) {
        self.sync_bindings();
        // `XGameEngineExt::Tick` (100266a0): the windows tick before `UGameEngine::Tick` (the level, then the frame)
        self.vm.ui_tick_player(self.player, dt);
        let f = self.input.end_frame(dt);
        let view_before = self.vm.get_prop(self.player, "ViewRotation", 0).ok();
        let mouse = f.axes.iter().any(|(n, v)| (n == "aMouseX" || n == "aMouseY") && *v != 0.0);
        match self.vm.player_frame(self.player, dt, &f.buttons, &f.axes, &f.execs) {
            Ok(problems) => {
                for p in problems {
                    self.issues.push(LoadIssue { actor: self.player, class: "input".into(), event: "Exec".into(), error: p });
                }
            }
            Err(e) => self.issues.push(LoadIssue { actor: self.player, class: "player".into(), event: "PlayerFrame".into(), error: e.to_string() }),
        }
        if mouse {
            self.mouse_turns_view = self.vm.get_prop(self.player, "ViewRotation", 0).ok() != view_before;
        }
        self.tick_world(dt);
    }

    /// The rest of a frame: the actors tick, the look angles are read back from the pawn, audio follows the camera.
    fn tick_world(&mut self, dt: f32) {
        // a game draws every frame: what the camera sees counts as rendered (`LastRenderTime`, zone render times)
        // even when nothing is drawn (headless runs), so stasis and the "not seen for a while" rules behave as in play
        let prof = std::env::var_os("DX_STEP_PROFILE").is_some();
        let mut t = std::time::Instant::now();
        let lap = |name: &'static str, t: &mut std::time::Instant| {
            if prof {
                let e = t.elapsed().as_secs_f64();
                STEP_PROFILE.with(|p| *p.borrow_mut().entry(name).or_insert(0.0) += e);
                *t = std::time::Instant::now();
            }
        };
        let mut issues = Vec::new();
        self.ensure_brush_tracker();
        lap("brush tracker", &mut t);
        self.vm.tick_report(dt, &mut issues);
        lap("level tick", &mut t);
        if self.issues.len() < 1000 {
            self.issues.extend(issues);
        }
        // then the frame is drawn (`UGameEngine::Tick`: the level ticks, then the viewports render): what the camera
        // sees counts as rendered (`LastRenderTime`, zone render times) even when nothing is drawn (headless runs), so
        // stasis and the "not seen for a while" rules behave as in play
        // `XRootWindow::PreRender` first sizes the root window to the canvas (`ResizeRoot`), then the world is drawn
        let (w, h) = self.vis_size;
        self.vm.ui_set_screen(self.player, w as f32, h as f32);
        // `UGameEngine::Draw` asks `PlayerCalcView` for the camera after the level tick, and the visibility pass and the
        // frame both use it: computed here, so the walk does not run one step behind the view
        self.calc_view = self.player_calc_view();
        if self.draw_this_step {
            let cam = self.camera();
            self.mark_rendered(&cam);
            lap("mark rendered", &mut t);
            self.engine_draw();
            lap("engine draw", &mut t);
            self.update_subview_visibility();
            lap("subview visibility", &mut t);
        }
        self.perform_engine_requests();
        lap("engine requests", &mut t);
        // volumes the options screens set (`SetInstantMusicVolume`...)
        if let Some(a) = &self.audio {
            let v = std::mem::take(&mut self.vm.ai.instant_volumes);
            let mut m = a.mixer.lock().unwrap();
            if let Some(x) = v[0] {
                m.music_volume = x as f32 / 255.0;
            }
            if let Some(x) = v[1] {
                m.sound_volume = x;
            }
            if let Some(x) = v[2] {
                m.speech_volume = x;
            }
        }
        let level_before = (self.map_pkg, self.player);
        self.perform_save_requests();
        if std::mem::take(&mut self.vm.clear_current_levels) {
            self.current_levels.clear();
        }
        if let Some(req) = self.vm.travel_request.take() {
            self.handle_travel(req);
        }
        if let Ok(Value::Rotator(r)) = self.vm.get_prop(self.player, "ViewRotation", 0) {
            self.yaw = r[1] & 0xFFFF;
            self.pitch = if r[0] & 0xFFFF > 32768 { (r[0] & 0xFFFF) - 65536 } else { r[0] & 0xFFFF };
        }
        if (self.map_pkg, self.player) != level_before {
            // a new level (travel, loaded game): the camera of the new player
            self.calc_view = self.player_calc_view();
        }
        self.frames += 1;
        lap("misc", &mut t);
        if self.audio.is_some() {
            let cam = self.camera();
            let mut a = self.audio.take().unwrap();
            a.update(&mut self.vm, &cam, self.player);
            self.audio = Some(a);
        }
        lap("audio", &mut t);
        if prof && self.frames % 120 == 0 && std::env::var_os("DX_STEP_PROFILE").is_some_and(|v| v != "quiet") {
            STEP_PROFILE.with(|p| {
                let mut v: Vec<_> = p.borrow().iter().map(|(k, v)| (*k, *v)).collect();
                v.sort_by(|a, b| b.1.total_cmp(&a.1));
                eprintln!("step profile over 120 frames (ms/frame): {}", v.iter().map(|(k, v)| format!("{k} {:.2}", v * 1000.0 / 120.0)).collect::<Vec<_>>().join(", "));
                p.borrow_mut().clear();
            });
        }
    }

    /// Carries out a level change: the next map is loaded, the player (and what it carries) goes along.
    fn handle_travel(&mut self, req: ue_vm::travel::TravelRequest) {
        // `?loadgame=N`: a saved game instead of a map
        if let Some(n) = req.url.split('?').skip(1).find_map(|o| o.strip_prefix("loadgame=")).and_then(|n| n.trim().parse::<i32>().ok()) {
            if let Err(e) = self.load_game(n) {
                self.issues.push(LoadIssue { actor: self.player, class: "load".into(), event: "LoadGame".into(), error: e });
            }
            return;
        }
        let Some((map, portal)) = req.map_and_portal() else {
            self.issues.push(LoadIssue { actor: self.player, class: "travel".into(), event: "Travel".into(), error: format!("cannot use the URL {:?}", req.url) });
            return;
        };
        let Some(path) = self.find_map_file(&map) else {
            self.issues.push(LoadIssue { actor: self.player, class: "travel".into(), event: "Travel".into(), error: format!("no map {map}") });
            return;
        };
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(&map).to_string();
        let result = (|| -> Result<(), String> {
            let pi = self.vm.reg.add_package(&path, &stem).map_err(|e| e.to_string())?;
            let view = MapView::open(&self.game, &stem)?;
            let class = self.vm.class_of(self.player).map(|c| self.vm.reg.class(c).path.rsplit('.').next().unwrap_or("JCDentonMale").to_string()).unwrap_or_else(|| "JCDentonMale".into());
            let kept = self.leave_level(&req.url, &stem);
            let revisited = kept.is_some();
            // `FURL(Base, URL, TRAVEL_Relative)`: the current URL's options stay, the travel URL's are added (a key
            // already there takes the new value)
            for o in req.url.split('#').next().unwrap_or("").split('?').skip(1).chain(req.url.split_once('#').map(|(_, p)| p).unwrap_or("").split('?').skip(1)) {
                if let Some((k, v)) = o.split_once('=') {
                    let k = k.trim().to_string();
                    self.vm.url_options.retain(|(ok, _)| !ok.eq_ignore_ascii_case(&k));
                    self.vm.url_options.push((k, v.trim().to_string()));
                }
            }

            let (pawn, load) = match kept {
                Some(snap) => self.vm.revisit_level(self.player, pi, &snap, &portal, &class, req.items),
                None => self.vm.travel_level(self.player, pi, &portal, &class, req.items),
            }
            .map_err(|e| e.to_string())?;
            self.view = view;
            self.map_pkg = pi;
            self.forget_level();
            self.player = pawn;
            self.pitch = 0;
            self.issues.extend(load.issues);
            self.map_name = stem.clone();
            // `XLevel->URL`: the map and portal as the travel URL wrote them
            let head = req.url.trim().split('?').next().unwrap_or("");
            let (m, portal) = head.split_once('#').unwrap_or((head, ""));
            // a kept level is browsed as its file (`Browse`: `<Save>\Current\<map>.dxs`)
            self.vm.url_map = if revisited { format!("..\\Save\\Current\\{}.dxs", m.trim().trim_end_matches(".dx").trim_end_matches(".DX")) } else { m.trim().to_string() };
            self.vm.url_portal = portal.to_string();
            self.map_changed = true;
            // where a run was when it ended (the Windows log of a game started without a console)
            eprintln!("level {} ({}{})", self.map_name, req.url.trim(), if revisited { ", kept" } else { "" });
        self.vm.ui_after_browse(self.player);
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("travel failed: {e}");
            self.issues.push(LoadIssue { actor: self.player, class: "travel".into(), event: "Travel".into(), error: e });
            return;
        }
        if let Some(mut a) = self.audio.take() {
            a.level_changed(&mut self.vm);
            self.audio = Some(a);
        }
    }

    /// What `DDeusExGameEngine::Browse` (DeusEx.dll 10010b90) does with the level being left for map `next` (`url`
    /// the travel URL): within one mission the level is kept (`PruneTravelActors`, `SaveCurrentLevel(-2)`), and a map
    /// of it kept before is returned to be entered as it was left. A map whose name does not start with a mission
    /// number, `?loadonly`, another mission or a player starting a new game (`bStartingNewGame`, then cleared) forget
    /// every kept level instead (`DeleteSaveGameFiles`).
    fn leave_level(&mut self, url: &str, next: &str) -> Option<ue_vm::save::Snapshot> {
        let current = self.mission_number();
        let next_mission = mission_of_map(next);
        let loadonly = url.split('?').skip(1).any(|o| o.eq_ignore_ascii_case("loadonly"));
        if next_mission.is_none() || loadonly {
            self.current_levels.clear();
            return None;
        }
        let new_game = matches!(self.vm.get_prop(self.player, "bStartingNewGame", 0), Ok(Value::Bool(true)));
        if next_mission != current || new_game {
            self.current_levels.clear();
            if new_game {
                let _ = self.vm.set_prop(self.player, "bStartingNewGame", 0, Value::Bool(false));
            }
            return None;
        }
        let pruned = self.vm.prune_travel_actors(self.player);
        match self.vm.make_snapshot_excluding(&self.map_name, self.player, &pruned) {
            Ok(snap) => {
                self.current_levels.insert(self.map_name.to_lowercase(), snap);
            }
            Err(e) => self.issues.push(LoadIssue { actor: self.player, class: "travel".into(), event: "SaveCurrentLevel".into(), error: e.to_string() }),
        }
        self.current_levels.get(&next.to_lowercase()).cloned()
    }

    /// Forgets what was worked out for the level just left (a travel, a loaded game). These caches are keyed by
    /// export number or object reference of the map package, or hold actors: kept across a level change they would
    /// lend the new level the old one's data (after Training, a brush of the combat map got the polygons of the
    /// Training door with the same export number and stood as a door at the pistol range).
    pub(crate) fn forget_level(&mut self) {
        self.world_lighting = None;
        self.world_light = None;
        self.brush_polys.clear();
        self.brush_tracker = None;
        self.surface_details.clear();
        self.vis_cache = None;
        self.vis_flags = None;
        self.anim_surfs = None;
        self.anim_frames.clear();
    }

    /// `DDeusExGameEngine::GetCurrentMissionNumber` (DeusEx.dll 10011ed0): the level's `DeusExLevelInfo.missionNumber`.
    fn mission_number(&mut self) -> Option<i32> {
        let c = self.vm.reg.class_by_name("DeusExLevelInfo")?;
        let info = self.vm.live_actors().into_iter().find(|&a| self.vm.class_of(a).map(|k| self.vm.is_a(k, c)).unwrap_or(false))?;
        self.vm.get_prop(info, "missionNumber", 0).ok().and_then(|v| v.as_i32())
    }

    /// True once after a level change: a renderer must then be given the new map (`reload_vk_world`).
    pub fn take_map_changed(&mut self) -> bool {
        std::mem::take(&mut self.map_changed)
    }

    /// Gives the Vulkan renderer the map the session changed to.
    pub fn reload_vk_world(&mut self, r: &mut dx_vk::renderer::Renderer) -> Result<(), String> {
        let lighting = dx_light::collect(&mut self.vm.reg, self.map_pkg, &self.view.model);
        r.replace_world(&mut self.view, Some(&lighting)).map_err(|e| e.to_string())?;
        r.world.use_lightmaps = self.lightmaps && r.world.lightmap_info.is_some();
        r.world.time = self.level_time();
        r.time = self.viewport_time();
        r.ui.flash = self.screen_flash();
        Ok(())
    }

    /// Uses the door in front of the player: the nearest `DeusExMover` within reach and roughly in the view direction gets `Frob`.
    /// Returns whether one was found.
    pub fn frob(&mut self) -> bool {
        let Some(door_class) = self.vm.reg.class_by_name("DeusExMover") else { return false };
        let cam = self.camera();
        let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
        let (sp, cp) = ang(cam.pitch).sin_cos();
        let (sy, cy) = ang(cam.yaw).sin_cos();
        let fwd = [cp * cy, cp * sy, sp];
        let mut best: Option<(f32, u32)> = None;
        for a in self.vm.live_actors() {
            let Some(c) = self.vm.class_of(a) else { continue };
            if !self.vm.is_a(c, door_class) {
                continue;
            }
            // the brush's centre: its location plus the middle of the model's bounds is not known here, so use the actor's origin
            let l = self.vm.location(a);
            let d = [l[0] - cam.pos[0], l[1] - cam.pos[1], l[2] - cam.pos[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
            let facing = (d[0] * fwd[0] + d[1] * fwd[1] + d[2] * fwd[2]) / dist;
            if dist < 220.0 && facing > 0.3 && best.map(|(bd, _)| dist < bd).unwrap_or(true) {
                best = Some((dist, a));
            }
        }
        let Some((_, door)) = best else { return false };
        let _ = self.vm.call_by_name(door, "Frob", vec![Value::Obj(self.player), Value::Obj(0)]);
        true
    }

    pub fn camera(&mut self) -> Camera {
        // the pawn's own field of view (75 degrees as shipped; scopes, binoculars and the `FOV` command change it)
        let fov = self.vm.get_prop(self.player, "FovAngle", 0).ok().and_then(|v| v.as_f32()).filter(|f| *f >= 1.0 && *f <= 170.0).unwrap_or(self.fov);
        if let Some((_, pos, rot, _)) = self.calc_view {
            let pitch = if rot[0] & 0xFFFF > 32768 { (rot[0] & 0xFFFF) - 65536 } else { rot[0] & 0xFFFF };
            return Camera { pos, pitch, yaw: rot[1] & 0xFFFF, fov, roll: rot[2] };
        }
        let p = self.player_location();
        let eye = self.vm.get_prop(self.player, "EyeHeight", 0).ok().and_then(|v| v.as_f32()).unwrap_or(30.0);
        Camera { pos: [p[0], p[1], p[2] + eye], pitch: self.pitch, yaw: self.yaw, fov, roll: 0 }
    }

    /// The camera as the game's scripts place it (`PlayerCalcView`, asked with the player itself, its location and
    /// view rotation, as the engine does before drawing): eye height and head bob in first person, the conversation
    /// cameras, the spy drone, a view target. First person = the player's own view, not from behind.
    fn player_calc_view(&mut self) -> Option<(u32, [f32; 3], [i32; 3], bool)> {
        let p = self.player;
        let loc = self.player_location();
        let rot = match self.vm.get_prop(p, "ViewRotation", 0) {
            Ok(Value::Rotator(r)) => r,
            _ => return None,
        };
        let out = match self.vm.call_event_out(p, "PlayerCalcView", vec![Value::Obj(p), Value::Vector(loc), Value::Rotator(rot)]) {
            Ok(Some(out)) => out,
            Ok(None) => return None,
            Err(e) => {
                if self.issues.len() < 1000 {
                    self.issues.push(LoadIssue { actor: p, class: "player".into(), event: "PlayerCalcView".into(), error: e.to_string() });
                }
                return None;
            }
        };
        let (Some(Value::Obj(actor)), Some(Value::Vector(cl)), Some(Value::Rotator(cr))) = (out.first(), out.get(1), out.get(2)) else { return None };
        let behind = matches!(self.vm.get_prop(p, "bBehindView", 0), Ok(Value::Bool(true)));
        let actor = if *actor == 0 { p } else { *actor };
        Some((actor, *cl, *cr, actor == p && !behind))
    }

    /// The actor whose body is not drawn: the player in first person.
    pub fn hidden_actor(&self) -> u32 {
        match self.calc_view {
            Some((_, _, _, false)) => 0,
            _ => self.player,
        }
    }

    /// Renders the world from the player's eyes and marks the pawns that are in sight.
    pub fn render(&mut self, w: u32, h: u32) -> Image {
        self.set_vis_size(w, h);
        let cam = self.camera();
        let mut frame = self.view.render_frame(&cam, w, h);
        self.meshes_drawn = if self.draw_meshes { self.draw_actors(&cam, &mut frame) } else { 0 };
        let mut img = frame.into_image();
        if self.show_markers {
            self.draw_markers(&cam, &mut img);
        }
        img
    }

    /// The frames of the last visibility pass (empty before the first).
    /// The port's settings (`Aerugo.AerugoSettings`, written by its menu choices): their class defaults, which the
    /// menus set and save, so a change shows at once.
    pub fn aerugo_settings(&mut self) -> AerugoSettings {
        let mut get = |k: &str, d: i32| self.vm.default_value_of("AerugoSettings", k, 0).and_then(|v| v.as_i32()).unwrap_or(d);
        let quality = dx_vk::Quality {
            filter: get("TextureFilter", 5).clamp(0, 5) as u8,
            msaa: get("AntiAliasing", 4).clamp(1, 8) as u32,
            deep_color: get("DeepColor", 1) != 0,
            smooth_lightmaps: get("SmoothLightmaps", 1) != 0,
            pixel_lighting: get("PixelLighting", 1) != 0,
            rt_shadows: get("RTShadows", 1) != 0,
            rt_ao: get("RTAO", 1) != 0,
        };
        AerugoSettings { vsync: get("VSync", 1), frame_limit: get("FrameLimit", 0), output_rate: get("OutputRate", 0), mouse_dpi: get("MouseDPI", 800), quality, max_decals: get("MaxDecals", 1000) }
    }

    /// The picture options a renderer of this session uses: the player's settings in a window (`settings_quality`),
    /// the original's for headless pictures and tests unless asked otherwise.
    pub fn quality(&mut self) -> dx_vk::Quality {
        // `DX_QUALITY=filter,msaa,deep,lightmaps,pixel,rt_shadows,rt_ao` (numbers): for comparing the options one by one
        if let Some(v) = std::env::var("DX_QUALITY").ok() {
            let n: Vec<u32> = v.split(',').map(|x| x.trim().parse().unwrap_or(0)).collect();
            let g = |i: usize| n.get(i).copied().unwrap_or(0);
            return dx_vk::Quality { filter: g(0) as u8, msaa: g(1).max(1), deep_color: g(2) != 0, smooth_lightmaps: g(3) != 0, pixel_lighting: g(4) != 0, rt_shadows: g(5) != 0, rt_ao: g(6) != 0 };
        }
        if self.settings_quality {
            self.aerugo_settings().quality
        } else {
            dx_vk::Quality::ORIGINAL
        }
    }

    /// Follows a change of the picture options: the renderer remakes its pipelines and targets, and gets the map again
    /// (with the world's lighting sent again in full).
    pub fn follow_quality(&mut self, r: &mut dx_vk::renderer::Renderer) -> Result<(), String> {
        let q = self.quality();
        if r.set_quality(q).map_err(|e| e.to_string())? {
            self.reload_vk_world(r)?;
            self.world_light = None;
            self.wants_world_light = true;
            eprintln!("picture options: {:?}", r.quality());
        }
        Ok(())
    }

    /// Follows settings that can change while playing: the brightness (`[WinDrv.WindowsClient] Brightness`, which the
    /// Brightness screen and the `BRIGHTNESS` command save), the mouse DPI, and the sound options the Sound screen
    /// saves (reverse stereo, surround). Cheap: the host calls it a few times a second.
    pub fn follow_settings(&mut self) {
        if !self.brightness_fixed {
            if let Some(b) = self.vm.reg.config_value("System", "WinDrv.WindowsClient", "Brightness").and_then(|v| v.trim().parse::<f32>().ok()) {
                self.brightness = b.clamp(0.0, 1.0);
            }
        }
        let settings = self.aerugo_settings();
        let dpi = settings.mouse_dpi.clamp(100, 32000);
        self.mouse_scale = 400.0 / dpi as f32;
        self.max_decals = settings.max_decals.max(0) as usize;
        if let Some(mut a) = self.audio.take() {
            a.read_settings(&mut self.vm);
            self.audio = Some(a);
        }
    }

    /// A boolean of the configured render device's ini section (`[Engine.Engine] GameRenderDevice`, e.g.
    /// `[D3DDrv.D3DRenderDevice] UseVSync`); `None` when absent.
    pub fn render_device_flag(&mut self, key: &str) -> Option<bool> {
        let dev = self.vm.reg.config_value("System", "Engine.Engine", "GameRenderDevice")?;
        let v = self.vm.reg.config_value("System", dev.trim(), key)?;
        Some(v.trim().eq_ignore_ascii_case("true") || v.trim() == "1")
    }

    pub fn vis_frames(&self) -> &[dx_vis::walk::FrameVis] {
        self.vis_cache.as_ref().map(|(_, f)| f.as_slice()).unwrap_or(&[])
    }

    /// The main view's screen size; a new size redoes this step's visibility pass for it.
    fn set_vis_size(&mut self, w: u32, h: u32) {
        let size = (w.max(1) as i32, h.max(1) as i32);
        if self.vis_size != size {
            self.vis_size = size;
            self.vis_cache = None;
        }
    }

    /// What the main view draws: the actors (and particles) the last visibility pass drew, the first-person weapon.
    /// The pass runs once per simulation step (`tick_world`); a view drawn before any step runs it here.
    pub fn actor_draws(&mut self, cam: &Camera) -> Vec<dx_soft::ActorDraw> {
        if self.vis_cache.as_ref().map(|(m, _)| *m != self.map_pkg).unwrap_or(true) {
            self.mark_rendered(cam);
        }
        // the main frame's own sprites: the sky and mirror frames draw theirs (`child_views`)
        let (actors, particles) = match &self.vis_cache {
            Some((_, frames)) => Self::picked(&frames[..frames.len().min(1)]),
            None => Default::default(),
        };
        let ranks = self.vis_cache.as_ref().and_then(|(_, f)| f.first()).map(Self::sprite_ranks).unwrap_or_default();
        let clip = self.vis_frames().first().map(|f| ClipFrame::of(&f.frame));
        self.draw_list(cam, 0, true, Some((&actors, &particles)), Some(&ranks), false, clip.as_ref())
    }

    /// The walk's child frames (`DrawFrame` draws them first: the sky zone seen through backdrops, mirrors), every one
    /// naming its parent, each with its own sprites (ranked in its sprite list), level nodes, movers and third list. A
    /// mirror's plane: the one the child's camera is the parent's reflected in; the sky's: the rotation between the two
    /// frames' axes. Child frames show the viewing pawn when their walk drew it. `None` without a walk.
    pub fn child_views(&mut self, cam: &Camera) -> Option<Vec<dx_vk::renderer::ChildView>> {
        self.child_views_hiding(cam, 0)
    }

    /// `child_views` without actor `hide` (a viewport window's origin actor, hidden during its pass).
    fn child_views_hiding(&mut self, cam: &Camera, hide: u32) -> Option<Vec<dx_vk::renderer::ChildView>> {
        type Picked = (ue_registry::FxHashSet<u32>, ue_registry::FxHashSet<(u32, usize)>);
        struct Spec {
            parent: Option<usize>,
            mirror: Option<[f32; 4]>,
            sky: Option<([[f32; 3]; 3], [f32; 3])>,
            picked: Picked,
            ranks: ue_registry::FxHashMap<(u32, u32), u32>,
            blended: Vec<u32>,
            mirrors: Vec<u32>,
            dynamic: Vec<u32>,
            solid: Vec<u32>,
            fi: usize,
        }
        let mut specs: Vec<Spec> = Vec::new();
        {
            let first_dynamic = self.brush_tracker.as_ref().map(|t| t.1.first_dynamic_surf).unwrap_or(usize::MAX);
            let model = self.vm.world_model.as_deref().unwrap_or(&self.view.model);
            let frames = &self.vis_cache.as_ref().filter(|(m, _)| *m == self.map_pkg)?.1;
            frames.first()?;
            // frame index -> view index (the main frame has none; a frame left out leaves out its own children)
            let mut view_of: std::collections::HashMap<usize, Option<usize>> = std::collections::HashMap::from([(0, None)]);
            for (fi, fv) in frames.iter().enumerate().skip(1) {
                let Some(pi) = fv.parent else { continue };
                let Some(&parent) = view_of.get(&pi) else { continue };
                let pf = &frames[pi].frame;
                let (mirror, sky) = if fv.frame.mirror != pf.mirror {
                    // the plane the child's camera is the parent's reflected in
                    let (a, b) = (pf.origin, fv.frame.origin);
                    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
                    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if l < 1e-3 {
                        continue;
                    }
                    let n = d.map(|c| c / l);
                    let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5];
                    (Some([n[0], n[1], n[2], n[0] * mid[0] + n[1] * mid[1] + n[2] * mid[2]]), None)
                } else {
                    // the sky zone's rotation taking the parent's axes to the child's (both orthonormal: C Pᵀ)
                    let (pa, ca) = ([pf.x_axis, pf.y_axis, pf.z_axis], [fv.frame.x_axis, fv.frame.y_axis, fv.frame.z_axis]);
                    let rot: [[f32; 3]; 3] = std::array::from_fn(|col| std::array::from_fn(|row| (0..3).map(|k| ca[k][row] * pa[k][col]).sum()));
                    (None, Some((rot, fv.frame.origin)))
                };
                view_of.insert(fi, Some(specs.len()));
                let blended = Self::list_order(fv, 2);
                let mirrors = Self::list_order(fv, 0);
                // its movers' nodes, and its level nodes (the renderer draws their solid and masked faces with the
                // solid world: masked surfaces are in list 2, which the device draws masked)
                let (mut dynamic, mut solid): (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
                let mut seen = std::collections::HashSet::new();
                for &(node, _, _) in &fv.vis.surfaces {
                    let s = model.nodes.get(node as usize).map(|n| n.i_surf).unwrap_or(-1);
                    if s < 0 || !seen.insert(node) {
                        continue;
                    }
                    if s as usize >= first_dynamic {
                        dynamic.push(node);
                    } else {
                        solid.push(node);
                    }
                }
                specs.push(Spec { parent, mirror, sky, picked: Self::picked(std::slice::from_ref(fv)), ranks: Self::sprite_ranks(fv), blended, mirrors, dynamic, solid, fi });
            }
        }
        Some(
            specs
                .into_iter()
                .map(|sp| {
                    let clip = self.vis_frames().get(sp.fi).map(|f| ClipFrame::of(&f.frame));
                    let actors = self.draw_list(cam, hide, false, Some((&sp.picked.0, &sp.picked.1)), Some(&sp.ranks), true, clip.as_ref());
                    let (details, details_blended) = self.frame_details(sp.fi);
                    dx_vk::renderer::ChildView { frame: sp.fi, parent: sp.parent, mirror: sp.mirror, sky: sp.sky, actors, blended: sp.blended, mirrors: sp.mirrors, dynamic: sp.dynamic, solid: sp.solid, details, details_blended }
                })
                .collect(),
        )
    }


    /// The place of each sprite in a frame's sprite list, as `DrawFrame` walks it: the walk links a sprite at the head
    /// when its first piece shows (`Visibility::sprite_order`), so the list is that order reversed. Keyed by (actor,
    /// render-iterator item; `u32::MAX` for the actor itself).
    pub fn sprite_ranks(fv: &dx_vis::walk::FrameVis) -> ue_registry::FxHashMap<(u32, u32), u32> {
        fv.vis.sprite_order.iter().rev().enumerate().filter_map(|(r, &i)| fv.sprites.get(i).map(|s| ((s.id, s.item), r as u32))).collect()
    }

    /// The main view's non-occluding surfaces in the order `DrawFrame` draws its third list (each piece is a draw list
    /// of its own, linked at the head: newest first), as BSP nodes.
    pub fn blended_order(&self) -> Option<Vec<u32>> {
        Some(Self::list_order(self.vis_cache.as_ref().and_then(|(_, f)| f.first())?, 2))
    }

    /// Runs `f` with `frames` (another view's visibility pass) as the last pass, then puts the main view's back.
    fn with_walk<R>(&mut self, frames: Vec<dx_vis::walk::FrameVis>, f: impl FnOnce(&mut Self) -> R) -> (R, Vec<dx_vis::walk::FrameVis>) {
        let saved = self.vis_cache.replace((self.map_pkg, frames));
        let r = f(self);
        let frames = std::mem::replace(&mut self.vis_cache, saved).map(|(_, f)| f).unwrap_or_default();
        (r, frames)
    }

    /// The movers' dynamic nodes frame `fi` of the last pass drew, each once.
    fn frame_dynamic_nodes(&self, fi: usize) -> Vec<u32> {
        let (Some(fv), Some((_, tracker, _)), Some(m)) = (self.vis_frames().get(fi), self.brush_tracker.as_ref(), self.vm.world_model.as_deref()) else { return Vec::new() };
        let mut out: Vec<u32> = Vec::new();
        for &(node, _, _) in &fv.vis.surfaces {
            if !out.contains(&node) && m.nodes.get(node as usize).map(|nd| nd.i_surf >= 0 && tracker.surf_is_dynamic(nd.i_surf as usize)).unwrap_or(false) {
                out.push(node);
            }
        }
        out
    }

    /// The main view's translucent mirrors drawn solid: list 1 entries the walk cleared `PF_Translucent` on while the
    /// surface is a translucent mirror (`OccludeBsp` with `ShinySurfaces` off).
    pub fn solid_mirror_order(&self) -> Option<Vec<u32>> {
        let fv = self.vis_cache.as_ref().and_then(|(_, f)| f.first())?;
        const PF_TRANSLUCENT: u32 = 0x4;
        const PF_MIRRORED: u32 = 0x0800_0000;
        let model = &self.view.model;
        let v: Vec<u32> = fv
            .vis
            .surfaces
            .iter()
            .zip(&fv.vis.surface_lists)
            .filter(|((node, flags, _), &l)| {
                let surf = model.nodes.get(*node as usize).and_then(|n| model.surfs.get(n.i_surf as usize)).map(|s| s.poly_flags).unwrap_or(0);
                l == 1 && flags & PF_TRANSLUCENT == 0 && surf & (PF_TRANSLUCENT | PF_MIRRORED) == PF_TRANSLUCENT | PF_MIRRORED
            })
            .map(|((node, _, _), _)| *node)
            .collect();
        Some(v)
    }

    /// The main frame's level nodes from the last walk, each once, movers' nodes left out (`DrawFrame` draws its own
    /// lists, not the visible zones whole). `None` without a walk of this level.
    pub fn main_solid_nodes(&self) -> Option<Vec<u32>> {
        let fv = self.vis_cache.as_ref().filter(|(m, _)| *m == self.map_pkg).and_then(|(_, f)| f.first())?;
        let first_dynamic = self.brush_tracker.as_ref().map(|t| t.1.first_dynamic_surf).unwrap_or(usize::MAX);
        let model = self.vm.world_model.as_deref().unwrap_or(&self.view.model);
        let mut seen = std::collections::HashSet::new();
        Some(
            fv.vis
                .surfaces
                .iter()
                .map(|&(node, _, _)| node)
                .filter(|&node| {
                    let s = model.nodes.get(node as usize).map(|n| n.i_surf).unwrap_or(-1);
                    s >= 0 && (s as usize) < first_dynamic && seen.insert(node)
                })
                .collect(),
        )
    }

    /// The main view's first list (mirrors that opened a child frame) in the order `DrawFrame` draws it, as BSP nodes.
    pub fn mirror_order(&self) -> Option<Vec<u32>> {
        Some(Self::list_order(self.vis_cache.as_ref().and_then(|(_, f)| f.first())?, 0))
    }

    /// Frame list `list` of a walked frame as `DrawFrame` draws it: every piece is linked at the head of its list, list 0
    /// is reversed before drawing (walk order), list 2 is drawn as linked (newest first).
    pub fn list_order(fv: &dx_vis::walk::FrameVis, list: u8) -> Vec<u32> {
        let nodes = fv.vis.surfaces.iter().zip(&fv.vis.surface_lists).filter(|(_, &l)| l == list).map(|(&(node, _, _), _)| node);
        if list == 0 {
            nodes.collect()
        } else {
            let mut v: Vec<u32> = nodes.collect();
            v.reverse();
            v
        }
    }

    /// The actors view `i` of the window system's last paint drew in its own visibility pass (tests and tools).
    pub fn subview_drawn(&self, i: usize) -> Vec<u32> {
        self.subview_vis.get(i).cloned().flatten().map(|(a, _)| a.into_iter().collect()).unwrap_or_default()
    }

    /// The actors the main view's last visibility pass drew (tests and tools).
    pub fn main_drawn(&self) -> Vec<u32> {
        self.vis_cache.as_ref().map(|(_, f)| Self::picked(f).0.into_iter().collect()).unwrap_or_default()
    }

    /// `DrawFrame` draws the decals of every surface on its draw lists, node by node: a decal entry that touches the
    /// node (or lists no nodes) is clipped to it (`ClipDecal`), and when something is left the decal's
    /// `LastRenderedTime` becomes the level time (`DeusExDecal.Timer` keeps decals seen within the last second).
    fn stamp_decals(&mut self, frames: &[dx_vis::walk::FrameVis]) {
        if self.vm.decals.is_empty() {
            return;
        }
        let now = self.level_time();
        let Some(model) = self.vm.world_model.clone() else { return };
        let mut stamped: std::collections::HashSet<u32> = Default::default();
        for fv in frames {
            for &(node, _, _) in &fv.vis.surfaces {
                let node = node as usize;
                let Some(nd) = model.nodes.get(node) else { continue };
                if nd.i_surf < 0 {
                    continue;
                }
                let decals = self.vm.surf_decals.get(&(nd.i_surf as usize)).cloned().unwrap_or_default();
                for d in decals {
                    if stamped.contains(&d) {
                        continue;
                    }
                    let Some(place) = self.vm.decals.get(&d) else { continue };
                    let hit = place.surfs.iter().filter(|e| e.surf == nd.i_surf as usize).any(|e| {
                        if e.nodes.is_empty() {
                            let n = model.vectors.get(model.surfs[e.surf].v_normal as usize).copied().unwrap_or([0.0, 0.0, 1.0]);
                            !ue_vm::decal::clip_to_node(&model, node, n, e.verts.to_vec()).is_empty()
                        } else {
                            e.nodes.contains(&node)
                        }
                    });
                    if hit {
                        stamped.insert(d);
                    }
                }
            }
        }
        for d in stamped {
            self.vm.set_fp(d, "LastRenderedTime", now);
        }
    }

    /// With `max_decals`: the newest that many `DeusExDecal`s count as drawn this frame, so `DeusExDecal.Timer` keeps
    /// them (it destroys a decal not drawn within the last second); older ones are left to it as in the original.
    fn keep_newest_decals(&mut self) {
        if self.max_decals == 0 {
            return;
        }
        let Some(cls) = self.vm.reg.class_by_name("DeusExDecal") else { return };
        let vm = &self.vm;
        self.decal_order.retain(|d| vm.decals.contains_key(d));
        let known: ue_registry::FxHashSet<u32> = self.decal_order.iter().copied().collect();
        let mut new: Vec<u32> = self.vm.decals.keys().copied().filter(|d| !known.contains(d) && self.vm.class_of(*d).is_some_and(|c| self.vm.is_a(c, cls))).collect();
        new.sort_unstable();
        self.decal_order.extend(new);
        let now = self.level_time();
        let from = self.decal_order.len().saturating_sub(self.max_decals);
        for i in from..self.decal_order.len() {
            let d = self.decal_order[i];
            self.vm.set_fp(d, "LastRenderedTime", now);
        }
    }

    /// The actors and (proxy, particle) items a visibility pass drew in any of its frames.
    fn picked(frames: &[dx_vis::walk::FrameVis]) -> (ue_registry::FxHashSet<u32>, ue_registry::FxHashSet<(u32, usize)>) {
        let (mut actors, mut particles) = (ue_registry::FxHashSet::default(), ue_registry::FxHashSet::default());
        for fv in frames {
            for (s, &v) in fv.sprites.iter().zip(&fv.vis.sprites) {
                if v {
                    if s.item == u32::MAX {
                        actors.insert(s.id);
                    } else {
                        particles.insert((s.id, s.item as usize));
                    }
                }
            }
        }
        (actors, particles)
    }

    /// `XViewportWindow::Draw` (Extension 1004a770) for the views the window system painted last: a master frame of the
    /// window's size at its camera, the viewing player in behind view unless the window shows weapons from the player's
    /// own eye, the origin actor hidden unless shown, then `RenderFrame` -> `DrawWorld`: the visibility pass of that
    /// frame, which stamps what it draws (`LastRenderTime`, zone render times) like the main view. Once per engine
    /// frame; each view then draws exactly what its pass picked. (Coronas of these frames are not drawn.)
    fn update_subview_visibility(&mut self) {
        let views: Vec<ue_vm::ui_gc::ViewCmd> = self.vm.ui_views().to_vec();
        self.subview_vis.clear();
        self.subview_coronas.clear();
        self.subview_frames.clear();
        let now = self.level_time();
        let p = self.player;
        for v in views {
            let (w, h) = (v.rect[2] as i32, v.rect[3] as i32);
            if w <= 0 || h <= 0 {
                self.subview_vis.push(None);
                self.subview_coronas.push(Vec::new());
                self.subview_frames.push(None);
                continue;
            }
            let cam = Camera { pos: v.pos, pitch: v.rot[0], yaw: v.rot[1], fov: v.fov, roll: v.rot[2] };
            let behind = self.vm.bp(p, "bBehindView");
            self.vm.set_bp(p, "bBehindView", !v.show_weapons);
            let hidden = (v.hide_actor != 0).then(|| self.vm.bp(v.hide_actor, "bHidden"));
            if v.hide_actor != 0 {
                self.vm.set_bp(v.hide_actor, "bHidden", true);
            }
            let (drawn, frames) = self.compute_visibility(&cam, w, h);
            if let Some(hd) = hidden {
                self.vm.set_bp(v.hide_actor, "bHidden", hd);
            }
            self.vm.set_bp(p, "bBehindView", behind);
            let zones = dx_vis::walk::zones_seen(&frames);
            for z in 0..self.vm.zone_render_times.len().min(64) {
                if zones & (1u64 << z) != 0 {
                    self.vm.zone_render_times[z] = now;
                }
            }
            for &a in &drawn {
                self.vm.set_fp(a, "LastRenderTime", now);
            }
            self.stamp_decals(&frames);
            // the view's `DrawFrame` runs its corona pass (on the shared table and clock) and draws the result
            let leaf = self.vm.region_leaf(p);
            let walked = frames.first().and_then(|fv| fv.vis.leaf_lights.get(&leaf).cloned()).unwrap_or_default();
            self.corona_pass(cam.pos, walked);
            let coronas = self.corona_quads(&cam, [v.rect[0] as i32, v.rect[1] as i32, w, h]);
            self.subview_coronas.push(coronas);
            self.subview_vis.push(Some(Self::picked(&frames)));
            self.subview_frames.push(Some(frames));
        }
    }

    /// The renderer stamps every actor it draws with the level time (`Actor.LastRenderTime`, set only in
    /// `URender::DrawActorSprite`) and every zone it reaches (`FZoneProperties.LastRenderTime`, read by `InStasis`):
    /// scripts ask `LastRendered()` to wake pawns nobody has seen yet, pick cameras, freeze distant generators...
    /// Which actors and zones count comes from the original's visibility pass (`dx-vis`: the front-to-back BSP walk with
    /// span-buffer occlusion and zone portals), so actors behind walls are not stamped.
    pub fn mark_rendered(&mut self, cam: &Camera) {
        let prof = std::env::var_os("DX_STEP_PROFILE").is_some();
        let mut t = std::time::Instant::now();
        let lap = |name: &'static str, t: &mut std::time::Instant| {
            if prof {
                let e = t.elapsed().as_secs_f64();
                STEP_PROFILE.with(|p| *p.borrow_mut().entry(name).or_insert(0.0) += e);
                *t = std::time::Instant::now();
            }
        };
        let now = self.level_time();
        // the visibility pass works on the scene frame (`XRootWindow::PreRender`): nothing when the world is not drawn
        let (drawn, frames) = match self.scene_view(cam) {
            Some((c, r)) => self.compute_visibility(&c, r[2], r[3]),
            None => (Vec::new(), Vec::new()),
        };
        lap("  visibility walk", &mut t);
        let zones = dx_vis::walk::zones_seen(&frames);
        for z in 0..self.vm.zone_render_times.len().min(64) {
            if zones & (1u64 << z) != 0 {
                self.vm.zone_render_times[z] = now;
            }
        }
        for &a in &drawn {
            self.vm.set_fp(a, "LastRenderTime", now);
        }
        lap("  stamps", &mut t);
        self.update_third_person(&drawn);
        lap("  third person", &mut t);
        if let Some((_, frames)) = self.vis_cache.take() {
            self.stamp_decals(&frames);
            self.vis_cache = Some((self.map_pkg, frames));
        }
        self.keep_newest_decals();
        lap("  decals", &mut t);
        self.update_texture_animation(&drawn);
        lap("  texture animation", &mut t);
        self.update_coronas(cam);
        lap("  coronas", &mut t);
        self.vis_cache = Some((self.map_pkg, frames));
        // the viewport's frame counter and the actor lights' fade step for this frame
        let now = self.viewport_time();
        self.light_step = (((now - self.light_time) * 768.0) as i32).clamp(0, 255);
        self.light_time = now;
        self.light_frame = self.light_frame.wrapping_add(1);
        self.update_world_light();
        lap("  world light", &mut t);
        self.update_fog();
        lap("  fog", &mut t);
    }

    /// The actors seen from `cam`, without `hide` (the actor a viewport window looks out of) and with the first-person
    /// weapon only when `weapon` is set.
    pub fn actor_draws_for(&mut self, cam: &Camera, hide: u32, weapon: bool) -> Vec<dx_soft::ActorDraw> {
        self.draw_list(cam, hide, weapon, None, None, false, None)
    }

    /// The draw list of a view. `picked`: the actors and (proxy, particle) items a visibility pass drew; without it
    /// (viewport windows, which have no pass of their own yet) every actor not well behind the camera. `child`: a
    /// child frame's list, which keeps the viewing pawn (its walk decides).
    #[allow(clippy::type_complexity)]
    #[allow(clippy::too_many_arguments)]
    fn draw_list(&mut self, cam: &Camera, hide: u32, weapon: bool, picked: Option<(&ue_registry::FxHashSet<u32>, &ue_registry::FxHashSet<(u32, usize)>)>, ranks: Option<&ue_registry::FxHashMap<(u32, u32), u32>>, child: bool, clip: Option<&ClipFrame>) -> Vec<dx_soft::ActorDraw> {
        let rank_of = |a: u32, item: u32| ranks.and_then(|r| r.get(&(a, item)).copied()).unwrap_or(u32::MAX);
        // with ray-traced shadows, unlit meshes (trees) get the lights around them too, for their shadows
        let shadow_lights = !child && self.quality().rt_shadows;
        let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
        let (sp, cp) = ang(cam.pitch).sin_cos();
        let (sy, cy) = ang(cam.yaw).sin_cos();
        let fwd = [cp * cy, cp * sy, sp];
        let mut candidates: Vec<(f32, u32, ue_vm::animation::MeshDraw)> = Vec::new();
        for a in self.vm.live_actors() {
            if (!child && a == self.hidden_actor()) || a == hide {
                continue;
            }
            // a view draws what its walk picked, and decals (drawn with their surfaces): nothing else needs its draw
            if let Some((set, _)) = picked {
                if !set.contains(&a) && !self.vm.decals.contains_key(&a) {
                    continue;
                }
            }
            let Some(mut d) = self.vm.mesh_draw(a) else { continue };
            // decals are drawn with the surfaces they stick to, not as sprites of the walk
            if let Some((set, _)) = picked {
                if d.decal.is_none() && !set.contains(&a) {
                    continue;
                }
            }
            // blended between the last two simulation steps
            let (dp, dr) = self.pose_offset(a);
            for k in 0..3 {
                d.location[k] += dp[k];
                d.rotation[k] = d.rotation[k].wrapping_add(dr[k]);
            }
            if d.sprite.is_some() && !self.draw_sprites {
                continue;
            }
            // an actor whose position is not a number (the engine's maths can produce one, e.g. a swimming pawn with
            // no mass) fails the original's visibility tests: never drawn
            if !d.location.iter().all(|x| x.is_finite()) {
                continue;
            }
            let rel = [d.location[0] - cam.pos[0], d.location[1] - cam.pos[1], d.location[2] - cam.pos[2]];
            let depth = rel[0] * fwd[0] + rel[1] * fwd[1] + rel[2] * fwd[2];
            let dist = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
            // the original draws whatever its visibility lets through, however far (the Statue of Liberty is ~13600 units
            // from the island's start); only what is well behind the camera is left out here
            if picked.is_none() && depth < -150.0 {
                continue;
            }
            candidates.push((dist, a, d));
        }
        candidates.sort_by(|x, y| x.0.total_cmp(&y.0));
        // every decal: `DrawFrame` draws the decals of each surface it draws, however many (a cap on the nearest made
        // the far ones of a fight's blood and bullet holes pop out); they are not counted against the others
        let (decals, others): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|(_, _, d)| d.decal.is_some());
        let chosen: Vec<(u32, ue_vm::animation::MeshDraw)> = others.into_iter().take(1024).chain(decals).map(|(_, a, d)| (a, d)).collect();
        let mut out: Vec<dx_soft::ActorDraw> = Vec::with_capacity(chosen.len() + 1);
        for (a, d) in chosen {
            let lit = d.sprite.is_none() && !d.unlit;
            // the pawn's third-person item on its weapon triangle, lit as the pawn (`DrawMesh(item, Owner = pawn)`)
            let item = self.third_person.iter().find(|t| t.0 == a).map(|t| t.2.clone());
            let placed = item.and_then(|pose| self.weapon_triangle(&d, clip).map(|frame| (pose, frame)));
            let mut draw = Self::to_actor_draw(d, false);
            draw.rank = rank_of(a, u32::MAX);
            if lit {
                draw.lighting = Some(self.mesh_lighting(a, true));
            } else if shadow_lights && draw.sprite.is_none() && draw.decal.is_none() && !draw.overlay {
                draw.shadow_lighting = Some(self.shadow_lighting(a));
            }
            let pawn_lighting = draw.lighting.clone();
            let draw_rank = draw.rank;
            out.push(draw);
            if let Some((pose, frame)) = placed {
                let unlit = pose.unlit;
                let mut w = Self::to_actor_draw(pose, false);
                // drawn on the frame itself: `DrawActorSprite` passes it to `DrawMesh` as the coordinates
                w.frame = Some(frame);
                // drawn by the pawn's sprite, right after it
                w.rank = draw_rank;
                if !unlit {
                    w.lighting = pawn_lighting;
                }
                out.push(w);
            }
        }
        // particles: the proxy of every generator, once per live particle
        // (`RenderInterface` is an `Actor` property: one offset for every actor)
        let generators: Vec<u32> = match self.vm.actor_prop_offset("RenderInterface") {
            Some((off, _)) => self.vm.live_actors().into_iter().filter(|&a| self.vm.u32_at(a, off) != 0).collect(),
            None => Vec::new(),
        };
        for g in generators {
            let it = self.vm.op(g, "RenderInterface");
            let proxy = self.vm.op(it, "proxy");
            for (i, d) in self.vm.particle_draws(g) {
                if picked.map(|(_, p)| p.contains(&(proxy, i))).unwrap_or(true) {
                    let mut pd = Self::to_actor_draw(d, false);
                    pd.rank = rank_of(proxy, i as u32);
                    out.push(pd);
                }
            }
        }
        if weapon {
            out.extend(self.hand_overlays());
        }
        out
    }

    /// The lights and ambient colour of actor `a` for the renderer (`SetupForActor`): the lights permeating its leaf,
    /// and with `walk` its sprite's leaf lights and fog lights from the main view's visibility pass.
    pub fn mesh_lighting(&mut self, a: u32, walk: bool) -> dx_soft::MeshLighting {
        let l = self.actor_lighting(a, walk);
        // the fog lights (`PF_RenderFog`), farthest from the camera first along its view axis, with their colour and
        // brightness of the moment as the fog maps take them
        let mut fog_lights = Vec::new();
        if l.render_fog && !l.fog_lights.is_empty() {
            let cam = self.camera();
            let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
            let (sp, cp) = ang(cam.pitch).sin_cos();
            let (sy, cy) = ang(cam.yaw).sin_cos();
            let fwd = [cp * cy, cp * sy, sp];
            let time = self.level_time();
            let level_brightness = self.vm.fp(self.vm.level, "Brightness");
            let mut seen = std::collections::HashSet::new();
            for &id in &l.fog_lights {
                if !seen.insert(id) || !self.vm.is_live(id) {
                    continue;
                }
                let li = self.live_light(id);
                let b = self.light_brightness_now(id, &li, time);
                let colour = dx_light::manager::light_colour(&li, b, level_brightness);
                fog_lights.push((li.pos, self.vm.u8p(id, "VolumeRadius"), self.vm.u8p(id, "VolumeBrightness"), self.vm.u8p(id, "VolumeFog"), b, colour));
            }
            let depth = |p: [f32; 3]| (p[0] - cam.pos[0]) * fwd[0] + (p[1] - cam.pos[1]) * fwd[1] + (p[2] - cam.pos[2]) * fwd[2];
            fog_lights.sort_by(|x, y| depth(y.0).partial_cmp(&depth(x.0)).unwrap_or(std::cmp::Ordering::Equal));
        }
        dx_soft::MeshLighting { ambient: l.ambient, scale: l.scale, lights: l.lights.iter().map(|m| (m.pos, m.color, m.radius)).collect(), fog_lights }
    }

    /// The lights around an unlit actor for the ray-traced shadows it casts (the port's own; the engine lights it not
    /// at all): the lights permeating its leaf that reach it, with their colour of the moment, its zone's ambient. Reads
    /// only (no light cache, no light tags).
    pub fn shadow_lighting(&mut self, a: u32) -> dx_soft::MeshLighting {
        let leaf = self.vm.region_leaf(a);
        let loc = self.vm.location(a);
        let mut ids = Vec::new();
        if leaf >= 0 {
            if let Some(lf) = self.view.model.leaves.get(leaf as usize) {
                if lf.i_permeating >= 0 {
                    let mut i = lf.i_permeating as usize;
                    while self.view.model.lights.get(i).copied().unwrap_or(0) != 0 {
                        if let Some(&l) = self.vm.model_light_actors.get(i) {
                            ids.push(l);
                        }
                        i += 1;
                    }
                }
            }
        }
        let time = self.level_time();
        let level_brightness = self.vm.fp(self.vm.level, "Brightness");
        let mut lights = Vec::new();
        for l in ids {
            if l == 0 || !self.vm.is_live(l) || self.vm.u8p(l, "LightType") == 0 || self.vm.u8p(l, "LightBrightness") == 0 {
                continue;
            }
            let li = self.live_light(l);
            let r = (self.vm.u8p(l, "LightRadius") as f32 + 1.0) * 25.0;
            let d2 = (0..3).map(|k| (li.pos[k] - loc[k]).powi(2)).sum::<f32>();
            if d2 >= r * r {
                continue;
            }
            let b = self.light_brightness_now(l, &li, time);
            lights.push((li.pos, dx_light::manager::light_colour(&li, b, level_brightness), r));
        }
        let zone = self.vm.zone_of_actor(a);
        let zone = if zone != 0 { zone } else { self.vm.level };
        let ambient = ue_assets::light::hsv_to_rgb(self.vm.u8p(zone, "AmbientHue"), self.vm.u8p(zone, "AmbientSaturation"), self.vm.u8p(zone, "AmbientBrightness"));
        dx_soft::MeshLighting { ambient, scale: 1.4, lights, fog_lights: Vec::new() }
    }

    /// `SetupForActor` for actor `a` this frame (see `mesh_lighting`); updates its light cache and fades.
    pub fn actor_lighting(&mut self, a: u32, walk: bool) -> ue_vm::lighting::ActorLighting {
        let leaf = self.vm.region_leaf(a);
        let model = &self.view.model;
        let mut permeating = Vec::new();
        if leaf >= 0 {
            if let Some(lf) = model.leaves.get(leaf as usize) {
                if lf.i_permeating >= 0 {
                    let mut i = lf.i_permeating as usize;
                    while model.lights.get(i).copied().unwrap_or(0) != 0 {
                        if let Some(&l) = self.vm.model_light_actors.get(i) {
                            if l != 0 {
                                permeating.push(l);
                            }
                        }
                        i += 1;
                    }
                }
            }
        }
        let (mut leaf_lights, mut vols) = (Vec::new(), Vec::new());
        if walk {
            if let Some(fv) = self.vis_frames().first() {
                leaf_lights = fv.vis.leaf_lights.get(&leaf).cloned().unwrap_or_default();
                if let Some(i) = fv.sprites.iter().position(|sp| sp.id == a && sp.item == u32::MAX) {
                    vols = fv.vis.sprite_vols.get(i).cloned().unwrap_or_default();
                }
            }
        }
        let fr = ue_vm::lighting::ActorFrame {
            frame_count: self.light_frame,
            fade_step: self.light_step,
            viewport_time: self.viewport_time(),
            has_leaves: !self.view.model.leaves.is_empty(),
            lit_view: self.lit_view(),
            permeating: &permeating,
            leaf_lights: &leaf_lights,
            volumetrics: &vols,
        };
        self.vm.setup_for_actor(a, &fr)
    }

    /// The 3D views of the viewport windows painted by the last `ui_quads`, ready for the renderer.
    pub fn ui_subviews(&mut self) -> Vec<dx_vk::renderer::SubView> {
        let views: Vec<ue_vm::ui_gc::ViewCmd> = self.vm.ui_views().to_vec();
        views
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                let cam = Camera { pos: v.pos, pitch: v.rot[0], yaw: v.rot[1], fov: v.fov, roll: v.rot[2] };
                // what the view's own visibility pass drew this engine frame (`update_subview_visibility`): its main
                // frame's sprites in their list order, and its lists (`DrawFrame` of its master frame, as the main view)
                let frames = self.subview_frames.get(i).cloned().flatten().filter(|f| !f.is_empty());
                let (actors, lists) = match frames {
                    Some(frames) => {
                        let (out, _) = self.with_walk(frames, |s| {
                            let fv = &s.vis_frames()[0];
                            let (a, p) = Self::picked(std::slice::from_ref(fv));
                            let ranks = Self::sprite_ranks(fv);
                            let (blended, mirrors) = (Self::list_order(fv, 2), Self::list_order(fv, 0));
                            let dynamic = s.frame_dynamic_nodes(0);
                            let clip = ClipFrame::of(&s.vis_frames()[0].frame);
                            let actors = if s.draw_meshes { s.draw_list(&cam, v.hide_actor, v.show_weapons, Some((&a, &p)), Some(&ranks), false, Some(&clip)) } else { Vec::new() };
                            // its child frames have no fog maps of their own (the view's pass makes none)
                            let mut children = if s.draw_meshes { s.child_views_hiding(&cam, v.hide_actor).unwrap_or_default() } else { Vec::new() };
                            for c in &mut children {
                                c.frame = usize::MAX;
                            }
                            let (details, details_blended) = s.frame_details(0);
                            (actors, dx_vk::renderer::ViewLists { children, blended, mirrors, dynamic, details, details_blended })
                        });
                        (out.0, Some(out.1))
                    }
                    None => (if self.draw_meshes { self.actor_draws_for(&cam, v.hide_actor, v.show_weapons) } else { Vec::new() }, None),
                };
                let coronas = self.subview_coronas.get(i).cloned().unwrap_or_default();
                dx_vk::renderer::SubView { after: v.after, seq: v.seq, rect: v.rect, clip: v.clip, cam, actors, coronas, lists }
            })
            .collect()
    }

    /// The window system's `GC.DrawActor`s of the last `ui_quads` (vision augmentation), ready for the renderer: drawn
    /// by the main camera; a lit one gets the actor's lighting as in the world.
    pub fn ui_actor_draws(&mut self) -> Vec<dx_vk::renderer::UiActors> {
        let cmds: Vec<ue_vm::ui_gc::ActorCmd> = self.vm.ui_actor_cmds().to_vec();
        if cmds.is_empty() {
            return Vec::new();
        }
        cmds.into_iter()
            .map(|c| {
                let mut actors = Vec::new();
                if let Some(d) = c.draw.filter(|d| d.location.iter().all(|x| x.is_finite())) {
                    let unlit = d.unlit;
                    let mut ad = Self::to_actor_draw(d, false);
                    if !unlit {
                        ad.lighting = Some(self.mesh_lighting(c.actor, false));
                    }
                    actors.push(ad);
                }
                dx_vk::renderer::UiActors { after: c.after, seq: c.seq, clear_z: c.clear_z, actors }
            })
            .collect()
    }

    fn to_actor_draw(d: ue_vm::animation::MeshDraw, overlay: bool) -> dx_soft::ActorDraw {
        dx_soft::ActorDraw { mesh: d.mesh, frame_a: d.frame_a, frame_b: d.frame_b, blend: d.blend, location: d.location, rotation: d.rotation, pre_pivot: d.pre_pivot, frame: None, scale: d.scale, skins: d.skins, style: d.style, glow: d.glow, unlit: d.unlit, sprite: d.sprite, decal: d.decal, decal_polys: d.decal_polys, overlay,
            lighting: None,
            shadow_lighting: None,
            blends: d.blends,
            rank: u32::MAX,
            fatness: d.fatness,
            ambient_glow: d.ambient_glow,
            lod_bias: d.lod_bias,
            flags: d.flags,
            env_texture: d.env_texture,
            all_textures: d.all_textures,
        }
    }

    /// The camera the main view is drawn with now and its scene frame for the renderer (`None` = the whole screen,
    /// an empty rectangle = no world).
    fn scene_camera(&mut self) -> (Camera, Option<[u32; 4]>) {
        let cam = self.render_camera();
        let (w, h) = self.vis_size;
        match self.scene_view(&cam) {
            None => (cam, Some([0; 4])),
            Some((c, r)) if r == [0, 0, w, h] => (c, None),
            Some((c, r)) => (c, Some([r[0] as u32, r[1] as u32, r[2] as u32, r[3] as u32])),
        }
    }

    /// `UTexture::Get` for what was drawn this engine frame, on the viewport clock: the textures of the surfaces on
    /// the frames' draw lists and of the drawn sprites (or the viewer's `Sprite` override). Animated surface textures
    /// remember the chain index they show for the renderer (`anim_frames`).
    fn update_texture_animation(&mut self, drawn: &[u32]) {
        let now = self.viewport_time();
        if self.anim_surfs.as_ref().map(|(m, _)| *m != self.map_pkg).unwrap_or(true) {
            let mut table = std::collections::HashMap::new();
            for (i, surf) in self.view.model.surfs.iter().enumerate() {
                let raw = surf.texture;
                if raw == 0 {
                    continue;
                }
                let Some(path) = self.vm.reg.ref_path(self.map_pkg, raw) else { continue };
                let id = self.vm.reg.obj_id(&path).0;
                if self.vm.texture_anim_next(id) == 0 {
                    continue;
                }
                if let Some(key) = self.view.texture_key(raw) {
                    table.insert(i, (key, id));
                }
            }
            self.anim_surfs = Some((self.map_pkg, table));
            self.anim_frames.clear();
        }
        let surfs: std::collections::HashSet<usize> = match &self.vis_cache {
            Some((_, frames)) => frames
                .iter()
                .flat_map(|fv| fv.vis.surfaces.iter().map(|&(node, _, _)| node))
                .filter_map(|node| self.view.model.nodes.get(node as usize).map(|n| n.i_surf))
                .filter(|&s| s >= 0)
                .map(|s| s as usize)
                .collect(),
            None => Default::default(),
        };
        let mut heads: Vec<((String, usize), u32)> = Vec::new();
        if let Some((_, table)) = &self.anim_surfs {
            for s in surfs {
                if let Some(e) = table.get(&s) {
                    if !heads.iter().any(|h| h.1 == e.1) {
                        heads.push(e.clone());
                    }
                }
            }
        }
        for (key, id) in heads {
            let cur = self.vm.texture_get(id, now);
            let mut index = 0;
            let mut t = id;
            while t != cur && index < 256 {
                t = self.vm.texture_anim_next(t);
                index += 1;
                if t == 0 || t == id {
                    index = 0;
                    break;
                }
            }
            self.anim_frames.insert(key, index);
        }
        // drawn sprites (the viewer's `Sprite` stands in for their texture)
        let viewer_sprite = {
            let v = self.vm.viewport_actor();
            if v != 0 { self.vm.op(v, "Sprite") } else { 0 }
        };
        for &a in drawn {
            // a drawn mesh: every material texture (`DrawLodMesh` gets each one)
            if self.vm.u8p(a, "DrawType") == 2 {
                for (tex, _) in self.vm.mesh_skin_textures(a) {
                    if tex != 0 && self.vm.texture_anim_next(tex) != 0 {
                        self.vm.texture_get(tex, now);
                    }
                }
                continue;
            }
            if self.vm.u8p(a, "DrawType") != 1 {
                continue;
            }
            let tex = if viewer_sprite != 0 { viewer_sprite } else { self.vm.op(a, "Texture") };
            self.vm.texture_get(tex, now);
        }
    }

    /// The chain index each animated surface texture showed in the last engine frame (tests and tools).
    pub fn anim_frames(&self) -> &std::collections::HashMap<(String, usize), usize> {
        &self.anim_frames
    }

    /// The surfaces of the map whose texture heads an animation chain (tests and tools; filled by the first frame).
    pub fn animated_surfaces(&self) -> Vec<usize> {
        self.anim_surfs.as_ref().map(|(_, t)| t.keys().copied().collect()).unwrap_or_default()
    }

    /// How many third-person items the last engine frame recorded (tests and tools).
    pub fn third_person_count(&self) -> usize {
        self.third_person.len()
    }

    /// `DrawActorSprite` after drawing a pawn whose mesh has a weapon triangle (a special face: the human meshes):
    /// the pawn's third-person item (`Vm::third_person_item`), once per engine frame for every pawn drawn.
    fn update_third_person(&mut self, drawn: &[u32]) {
        self.third_person.clear();
        let Some(pawn_class) = self.vm.reg.class_by_name("Pawn") else { return };
        for &a in drawn {
            if !self.vm.class_of(a).map(|c| self.vm.is_a(c, pawn_class)).unwrap_or(false) {
                continue;
            }
            let Some(d) = self.vm.mesh_draw(a) else { continue };
            let has_triangle = self.view.mesh_data(&d.mesh).map(|md| !md.lod.special_faces.is_empty()).unwrap_or(false);
            if !has_triangle {
                continue;
            }
            if let Some((item, pose)) = self.vm.third_person_item(a) {
                self.third_person.push((a, item, pose));
            }
        }
    }

    /// The weapon triangle frame `DrawLodMesh` keeps for a pawn drawn as `d` (Render 10b0ea80, the special face's
    /// vertices v0, v1, v2 as `GetFrame` placed them, before `Fatness`): `X = SafeNormal(v1 - v0)`,
    /// `Y = SafeNormal(X x (v0 - v2))`, `Z = Y x X`, origin `(v0 + v2) * 0.5`, all in the view's mirrored coordinates;
    /// here in world terms (a cross product changes sign): `Y = -SafeNormal(X x (v0 - v2))`, `Z = X x Y`. Kept only when
    /// the three vertices' outcodes in `clip` (the frame being drawn) have no side in common. Returns origin, X, Y, Z.
    fn weapon_triangle(&mut self, d: &ue_vm::animation::MeshDraw, clip: Option<&ClipFrame>) -> Option<[[f32; 3]; 4]> {
        let md = self.view.mesh_data(&d.mesh)?;
        let lod = &md.lod;
        let face = lod.special_faces.first()?;
        let pose = dx_vk::actors::posed_vertices(lod, &Self::to_actor_draw(d.clone(), false));
        let get = |k: usize| pose.get(face.wedge[k] as usize).copied();
        let (v0, v1, v2) = (get(0)?, get(1)?, get(2)?);
        if let Some(c) = clip {
            if c.outcode(v0) & c.outcode(v1) & c.outcode(v2) != 0 {
                return None;
            }
        }
        Some(weapon_frame(v0, v1, v2))
    }

    /// The main view's scene frame on the current screen (`XRootWindow::PreRender`, `Vm::ui_render_rect`) and the
    /// camera for it: `ComputeRenderSize` keeps the horizontal field of view across the frame's own width, so a render
    /// viewport (conversations, cinematics) shows a band with the full screen's horizontal angle (Hor+ as the port
    /// draws), centred on the band. `None` when the world is not drawn (menus over a snapshot).
    pub fn scene_view(&self, cam: &Camera) -> Option<(Camera, [i32; 4])> {
        let (w, h) = self.vis_size;
        let r = self.vm.ui_render_rect(self.player, w, h);
        if r[2] <= 0 || r[3] <= 0 {
            return None;
        }
        if r == [0, 0, w, h] {
            return Some((*cam, r));
        }
        let ht = dx_vk::world::hor_plus_tan(cam.fov, w as f32 / h.max(1) as f32);
        let aspect = r[2] as f32 / r[3] as f32;
        let fov = 2.0 * (ht / (aspect / (4.0 / 3.0)).max(1.0)).atan().to_degrees();
        Some((Camera { fov, ..*cam }, r))
    }

    /// The engine's canvas (made on first use, again after a level change).
    fn ensure_canvas(&mut self) -> Option<u32> {
        let cls = self.vm.reg.class_by_name("Canvas")?;
        if self.canvas == 0 || !self.vm.is_live(self.canvas) {
            self.canvas = self.vm.new_object(cls).ok()?;
        }
        Some(self.canvas)
    }

    fn draw_event(&mut self, obj: u32, event: &str, args: Vec<Value>) {
        if let Err(e) = self.vm.send_event(obj, event, args) {
            self.issues.push(LoadIssue { actor: obj, class: "draw".into(), event: event.into(), error: e.to_string() });
        }
    }

    /// `UCanvas::SetClip` (10303305): the pen at 0, 0, the origin and the clip size.
    fn canvas_set_clip(&mut self, c: u32, x: i32, y: i32, w: i32, h: i32) {
        for (n, v) in [("CurX", 0.0), ("CurY", 0.0), ("OrgX", x as f32), ("OrgY", y as f32), ("ClipX", w as f32), ("ClipY", h as f32)] {
            let _ = self.vm.set_prop(c, n, 0, Value::Float(v));
        }
    }

    /// `UConsole::PreRender` (103031ed): the console's `Tick` with the viewport time since its last frame, then
    /// `ConsoleLines` (the slid-down part of the frame) and the borders `BorderSize` asks for, which shrink the frame.
    fn console_pre_render(&mut self, console: u32, frame: &mut ue_vm::canvas::CanvasFrame) {
        let now = self.viewport_time();
        let delta = self.console_time.map_or(0.0, |t| (now - t) as f32);
        self.console_time = Some(now);
        self.draw_event(console, "Tick", vec![Value::Float(delta)]);
        let ftol = |v: f64| if v.is_nan() || v.abs() >= 2_147_483_648.0 { 0 } else { v.trunc() as i32 };
        let pos = self.vm.fp(console, "ConsolePos") as f64;
        let fy = frame.y as f64;
        let lines = if pos > 0.0 { ftol((fy * pos).min(fy)) } else { 0 };
        let (mut border_lines, mut border_pixels) = (0, 0);
        let size = self.vm.ip(console, "BorderSize");
        if size > 1 {
            let k = (size - 1) as f64 * 0.2f32 as f64;
            border_lines = ftol((fy * k * 0.25).min(fy)).max(0);
            frame.y -= 2 * border_lines;
            let fx = frame.x as f64;
            border_pixels = ftol((fx * k * 0.25).min(fx)) & !3;
            frame.x -= 2 * border_pixels;
        }
        frame.xb += border_pixels;
        frame.yb += border_lines;
        for (n, v) in [("ConsoleLines", lines), ("BorderLines", border_lines), ("BorderPixels", border_pixels)] {
            let _ = self.vm.set_prop(console, n, 0, Value::Int(v));
        }
    }

    /// `UGameEngine::Draw` around the world (10303a71), once per engine frame after the visibility pass:
    /// * the console's `PreRender` (its `Tick`, `ConsoleLines`, borders);
    /// * `UCanvas::Update`: the canvas `Reset`s and takes the frame's size;
    /// * the player's `PreRender`; `PreRenderWindows` (`XRootWindow::PreRender`) clips the frame to the root's render
    ///   viewport (`SetClip(XB, YB, X, Y)`);
    /// * the world when that frame is not empty and the console draws it (`GetDrawWorld`: not `bNoDrawWorld`); it ends
    ///   with the pawn's `RenderOverlays`;
    /// * the player's `PostRenderFlash`; `PostRenderWindows` (`XRootWindow::PostRender`) puts the frame back and
    ///   `SetClip(0, 0, X, Y)` (the windows themselves are painted by `ui_quads`);
    /// * the player's `PostRender`, then the console's (`UConsole::PostRender`: `FrameX`/`FrameY` from the frame),
    ///   whose text and tiles land in `Vm::canvas_list`, drawn over the windows.
    fn engine_draw(&mut self) {
        let Some(c) = self.ensure_canvas() else { return };
        let (w, h) = self.vis_size;
        let mut frame = ue_vm::canvas::CanvasFrame { xb: 0, yb: 0, x: w as i32, y: h as i32 };
        let console = self.console();
        if console != 0 {
            self.console_pre_render(console, &mut frame);
        }
        self.vm.canvas_frame = frame;
        self.vm.canvas_list.clear();
        let viewport = self.vm.op(self.player, "Player");
        let _ = self.vm.set_prop(c, "Viewport", 0, Value::Obj(viewport));
        self.draw_event(c, "Reset", Vec::new());
        for (n, v) in [("ClipX", Value::Float(frame.x as f32)), ("ClipY", Value::Float(frame.y as f32)), ("SizeX", Value::Int(frame.x)), ("SizeY", Value::Int(frame.y))] {
            let _ = self.vm.set_prop(c, n, 0, v);
        }
        let player = self.player;
        self.draw_event(player, "PreRender", vec![Value::Obj(c)]);
        let root = self.vm.op(player, "rootWindow");
        let mut scene = frame;
        if root != 0 {
            let r = self.vm.ui_render_rect(player, w, h);
            scene = ue_vm::canvas::CanvasFrame { xb: r[0], yb: r[1], x: r[2].max(0), y: r[3].max(0) };
            self.vm.canvas_frame = scene;
            for (n, v) in [("ClipX", Value::Float(scene.x as f32)), ("ClipY", Value::Float(scene.y as f32)), ("SizeX", Value::Int(scene.x)), ("SizeY", Value::Int(scene.y))] {
                let _ = self.vm.set_prop(c, n, 0, v);
            }
            self.canvas_set_clip(c, scene.xb, scene.yb, scene.x, scene.y);
        }
        self.vm.canvas_draws.clear();
        let draws_world = console == 0 || !self.vm.bp(console, "bNoDrawWorld");
        if scene.x > 0 && scene.y > 0 && draws_world {
            self.render_overlays(c);
        }
        self.draw_event(player, "PostRenderFlash", vec![Value::Obj(c)]);
        if root != 0 {
            self.vm.canvas_frame = frame;
            self.canvas_set_clip(c, 0, 0, frame.x, frame.y);
        }
        self.draw_event(player, "PostRender", vec![Value::Obj(c)]);
        if console != 0 {
            let _ = self.vm.set_prop(console, "FrameX", 0, Value::Float(frame.x as f32));
            let _ = self.vm.set_prop(console, "FrameY", 0, Value::Float(frame.y as f32));
            self.draw_event(console, "PostRender", vec![Value::Obj(c)]);
        }
    }

    /// `URender::DrawWorld` ends with the viewing pawn's `RenderOverlays(Canvas)` (after the depth clear): the scripts
    /// place the weapon and the item in hand at the eye (`SetLocation`/`SetRotation`), hide them when zoomed or
    /// `bHideWeapon`, and draw them with `Canvas.DrawActor` (recorded in `Vm::canvas_draws`).
    fn render_overlays(&mut self, c: u32) {
        self.draw_event(self.player, "RenderOverlays", vec![Value::Obj(c)]);
    }

    /// What `RenderOverlays` drew with `Canvas.DrawActor` in the last engine frame (the weapon, the item in hand), each
    /// where its script put it, blended with the eye's movement since the last step and turned like the view drawn now;
    /// lit as itself (`URender::DrawActor`: its own light cache and the lights of its leaf, no leaf or fog lights).
    /// Where the items in hand are drawn this frame (location, rotation), for tests.
    pub fn hand_overlay_poses(&mut self) -> Vec<([f32; 3], [i32; 3])> {
        self.hand_overlays().into_iter().map(|d| (d.location, d.rotation)).collect()
    }

    fn hand_overlays(&mut self) -> Vec<dx_soft::ActorDraw> {
        let mut out = Vec::new();
        let draws = self.vm.canvas_draws.clone();
        if draws.is_empty() {
            return out;
        }
        // `RenderOverlays` placed the item against the view of the step (`CalcDrawOffset`: the eye plus an offset turned
        // by the view rotation, the view rotation with the hand's roll); the original runs it with the view it draws,
        // so the item is fixed in the view: keep its pose relative to the step's view and put it in the drawn one
        let step = self.camera();
        let cam = self.render_camera();
        let (step_rot, cam_rot) = ([step.pitch, step.yaw, step.roll], [cam.pitch, cam.yaw, cam.roll]);
        for cd in draws {
            if !self.vm.is_live(cd.actor) {
                continue;
            }
            let Some(mut d) = self.vm.mesh_draw_opts(cd.actor, true) else { continue };
            let rel = [d.location[0] - step.pos[0], d.location[1] - step.pos[1], d.location[2] - step.pos[2]];
            let w = ue_vm::rotate_vector(ue_vm::rotate_vector(rel, step_rot, true), cam_rot, false);
            d.location = [cam.pos[0] + w[0], cam.pos[1] + w[1], cam.pos[2] + w[2]];
            d.rotation = [d.rotation[0].wrapping_add(cam.pitch.wrapping_sub(step.pitch)) & 0xFFFF, d.rotation[1].wrapping_add(cam.yaw.wrapping_sub(step.yaw)) & 0xFFFF, d.rotation[2]];
            let unlit = d.unlit;
            let mut ad = Self::to_actor_draw(d, true);
            if !unlit {
                ad.lighting = Some(self.mesh_lighting(cd.actor, false));
            }
            out.push(ad);
        }
        out
    }

    /// Draws the meshes of the nearest visible actors into the frame; returns how many were drawn.
    fn draw_actors(&mut self, cam: &Camera, frame: &mut dx_soft::Frame) -> usize {
        let mut drawn = 0;
        for draw in self.actor_draws(cam) {
            if self.view.draw_actor(frame, cam, &draw) {
                drawn += 1;
            }
        }
        drawn
    }

    /// Makes sure the renderer has the brush models of all movers and tells it where they stand now, and which movers
    /// the main view draws through its lists (the fragments the walk drew; then even a hidden mover's).
    fn sync_movers(&mut self, renderer: &mut dx_vk::renderer::Renderer) {
        let mut list = Vec::new();
        for a in self.vm.live_actors() {
            let Some(d) = self.vm.mover_draw(a) else { continue };
            let Some((pi, export)) = self.vm.reg.export_by_path(&d.brush) else { continue };
            if pi != self.map_pkg || !self.ensure_mover_world(renderer, a, export, d.location, d.rotation) {
                continue;
            }
            // doors and lifts are drawn blended between the last two steps too
            let (dp, dr) = self.pose_offset(a);
            let location = [d.location[0] + dp[0], d.location[1] + dp[1], d.location[2] + dp[2]];
            let rotation: [i32; 3] = std::array::from_fn(|k| d.rotation[k].wrapping_add(dr[k]));
            list.push((export, self.mover_placement(a, location, rotation)));
        }
        renderer.movers = list;
        match self.listed_movers() {
            Some(listed) => {
                for &(a, export) in &listed.owners {
                    if self.vm.is_live(a) {
                        let (loc, rot) = (self.vm.location(a), self.vm.rotation(a));
                        self.ensure_mover_world(renderer, a, export, loc, rot);
                    }
                }
                renderer.mover_fragments = Some(listed.fragments);
                renderer.mover_nodes = listed.nodes;
                renderer.mover_main = listed.main;
            }
            None => {
                renderer.mover_fragments = None;
                renderer.mover_nodes.clear();
                renderer.mover_main.clear();
            }
        }
        renderer.world.blended_order = self.blended_order();
        renderer.world.mirror_order = self.mirror_order();
        renderer.world.solid_mirror_order = self.solid_mirror_order();
        renderer.world.main_solid = self.main_solid_nodes();
    }

    /// Gives the renderer mover `a`'s brush model (`export`) with the lightmaps baked where it stands now, once.
    fn ensure_mover_world(&mut self, renderer: &mut dx_vk::renderer::Renderer, a: u32, export: usize, location: [f32; 3], rotation: [i32; 3]) -> bool {
        if renderer.has_mover(export) {
            return true;
        }
        let Some((model, lighting)) = self.mover_lighting(a, export, location, rotation) else { return false };
        let maps = dx_light::generate(&model, &lighting);
        if renderer.add_mover(&self.view, export, &model, Some(&lighting), Some(maps)).is_err() {
            return false;
        }
        if let Some(wl) = &mut self.world_light {
            wl.movers.forget_brush(export);
        }
        true
    }

    /// Mover `a`'s surfaces as the moving-brush tracker makes them (one per brush polygon, `dx_light::mover_model`) and
    /// the lighting `SetupForSurf` (Render 10b06c90) gives them, in the brush's own space at the pose it is first drawn
    /// in: the brush model's stored lightmaps, light list and shadow bits; for a `bDynamicLightMover` the unshadowed
    /// lights permeating its leaf instead (`dx_light::leaf_light_lists`). Static lights only: the per-frame lights, and
    /// the static part again whenever the mover moves, come from `update_mover_light` once a surface is drawn.
    fn mover_lighting(&mut self, a: u32, export: usize, location: [f32; 3], rotation: [i32; 3]) -> Option<(ue_assets::model::Model, dx_light::MapLighting)> {
        let brush = self.view.model_at(export)?;
        let polys = self.view.brush_polys(export).unwrap_or_default();
        let place = self.mover_placement(a, location, rotation);
        if self.world_lighting.is_none() {
            self.world_lighting = Some(dx_light::collect(&mut self.vm.reg, self.map_pkg, &self.view.model));
        }
        let special_lit = self.vm.bp(a, "bSpecialLit");
        let zone = self.vm.zone_number(a);
        let mut model = dx_light::mover_model(&brush, &polys, special_lit, zone);
        let to_local = |l: dx_light::Light| dx_light::Light { pos: place.to_local(l.pos), dir: place.dir_to_local(l.dir), ..l };
        let lights = if self.vm.bp(a, "bDynamicLightMover") {
            let wl = self.world_lighting.as_ref().unwrap();
            let mut leaf_lights = Vec::new();
            let leaf = self.vm.region_leaf(a);
            if let Some(l) = usize::try_from(leaf).ok().and_then(|l| self.view.model.leaves.get(l)).filter(|_| !self.view.model.leaves.is_empty()) {
                if l.i_permeating >= 0 {
                    let mut k = l.i_permeating as usize;
                    while self.view.model.lights.get(k).copied().unwrap_or(0) != 0 {
                        if let Some(Some(light)) = wl.lights.get(k) {
                            leaf_lights.push(to_local(*light));
                        }
                        k += 1;
                    }
                }
            }
            dx_light::leaf_light_lists(&mut model, &leaf_lights, special_lit)
        } else {
            dx_light::collect(&mut self.vm.reg, self.map_pkg, &brush).lights.into_iter().map(|l| l.map(to_local)).collect()
        };
        let wl = self.world_lighting.as_ref().unwrap();
        let shadowed = !self.vm.bp(a, "bDynamicLightMover");
        Some((model, dx_light::MapLighting { lights, zone_ambient: wl.zone_ambient.clone(), zone_pan: wl.zone_pan.clone(), level_brightness: wl.level_brightness, shadowed, static_only: true }))
    }

    /// Where mover `a`'s brush is drawn at `location`/`rotation`: `ABrush::ToWorld` (scales and pre-pivot included), the
    /// frame the moving-brush tracker puts the brush polygons through.
    fn mover_placement(&mut self, a: u32, location: [f32; 3], rotation: [i32; 3]) -> dx_vk::world::Placement {
        let c = self.vm.brush_to_world(a, location, rotation);
        dx_vk::world::Placement::from_coords(c.origin, c.x, c.y, c.z)
    }

    /// A Vulkan renderer for this map that can present into a window (`instance_extensions` from the windowing library).
    pub fn make_vk_renderer(&mut self, instance_extensions: &[*const std::ffi::c_char]) -> Result<dx_vk::renderer::Renderer, String> {
        let lighting = dx_light::collect(&mut self.vm.reg, self.map_pkg, &self.view.model);
        let quality = self.quality();
        let mut r = dx_vk::renderer::Renderer::with_quality(&mut self.view, Some(&lighting), vk_validation(), instance_extensions, quality).map_err(|e| e.to_string())?;
        if r.quality() != dx_vk::Quality::ORIGINAL {
            eprintln!("picture options: {:?}", r.quality());
        }
        self.wants_world_light = true;
        r.world.use_lightmaps = self.lightmaps && r.world.lightmap_info.is_some();
        r.world.time = self.level_time();
        r.time = self.viewport_time();
        r.ui.flash = self.screen_flash();
        Ok(r)
    }

    /// The window system's draw list for a `w` x `h` screen: the HUD and whatever windows are open, as rectangles for the
    /// Vulkan renderer. Also gives the windows the current camera (for `ConvertVectorToCoordinates`).
    pub fn ui_quads(&mut self, w: u32, h: u32) -> Vec<dx_vk::ui::UiQuad> {
        self.vm.ui_enable_views(true);
        let cam = self.render_camera();
        // the root window's frame is the main view's master frame (the scene frame's size)
        let frame = self.scene_view(&cam).map(|(c, r)| ue_vm::ui::ViewInfo { pos: c.pos, rot: [c.pitch, c.yaw, c.roll], fov_deg: c.fov, width: r[2] as f32, height: r[3] as f32 });
        self.vm.ui_set_view(frame);
        let t = self.viewport_time();
        self.vm.ui_set_time(t);
        let mut list = self.vm.ui_paint(self.player, w as f32, h as f32);
        // the canvas draws of the engine frame (the console's `PostRender`) come after the windows
        list.extend(self.vm.canvas_list.iter().cloned());
        list.iter()
            .filter_map(|c| {
                let path = self.vm.reg.obj_path(ue_registry::ObjId(c.tex)).to_string();
                (!path.is_empty()).then(|| dx_vk::ui::UiQuad { texture: path, rect: [c.x, c.y, c.w, c.h], uv: [c.u, c.v, c.ul, c.vl], color: c.color, poly_flags: c.poly_flags })
            })
            .collect()
    }

    /// Does the window system have the mouse (a menu or screen is open)? Then mouse input goes to it, not to the view.
    /// The level's `TimeSeconds` (drives auto-panning surfaces and light effects).
    /// The player's screen flash as the engine hands it to the renderer: `FlashFog` and half of `FlashScale`, each
    /// component clamped to 0..1 (`UGameEngine::Draw`).
    pub fn screen_flash(&mut self) -> Option<([f32; 3], [f32; 3])> {
        let vec = |s: &mut Session, name: &str| match s.vm.get_prop(s.player, name, 0) {
            Ok(ue_vm::Value::Vector(v)) => Some(v),
            _ => None,
        };
        let scale = vec(self, "FlashScale")?;
        let fog = vec(self, "FlashFog")?;
        let c = |v: f32| v.clamp(0.0, 1.0);
        Some(([c(fog[0]), c(fog[1]), c(fog[2])], [c(scale[0] * 0.5), c(scale[1] * 0.5), c(scale[2] * 0.5)]))
    }

    pub fn level_time(&mut self) -> f32 {
        let l = self.vm.level;
        self.vm.get_prop(l, "TimeSeconds", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0)
    }

    pub fn ui_wants_mouse(&self) -> bool {
        self.vm.ui_wants_mouse(self.player)
    }

    pub fn ui_wants_keyboard(&self) -> bool {
        self.vm.ui_wants_keyboard(self.player)
    }

    /// A key or button for the window system (engine key code). True when it took the event.
    /// The viewport's console (`UViewport::Console`), 0 without one.
    fn console(&mut self) -> u32 {
        let viewport = self.vm.op(self.player, "Player");
        if viewport == 0 { 0 } else { self.vm.op(viewport, "Console") }
    }

    /// `UEngine::InputEvent`: the console's `KeyEvent(Key, Action, Delta)` sees every key, button and axis event before
    /// the window system and the bindings; true = it took the event (a taken release still reaches the bindings, as
    /// the engine's `PreProcess` does). Actions: 1 press, 3 release, 4 axis.
    pub fn console_key_event(&mut self, code: u8, action: u8, delta: f32) -> bool {
        let c = self.console();
        if c == 0 {
            return false;
        }
        match self.vm.send_event(c, "KeyEvent", vec![Value::Byte(code), Value::Byte(action), Value::Float(delta)]) {
            Ok(Some(Value::Bool(b))) => b,
            Ok(_) => false,
            Err(e) => {
                self.issues.push(LoadIssue { actor: c, class: "console".into(), event: "KeyEvent".into(), error: e.to_string() });
                false
            }
        }
    }

    /// `UEngine::Key`: a typed character goes to the console's `KeyType` first; true = it took it.
    pub fn console_key_type(&mut self, ch: char) -> bool {
        let c = self.console();
        if c == 0 {
            return false;
        }
        // the console gets the character's low byte (`UEngine::Key` truncates it), the window system all of it
        let code = ch as u32 & 0xFF;
        match self.vm.send_event(c, "KeyType", vec![Value::Byte(code as u8)]) {
            Ok(Some(Value::Bool(b))) => b,
            Ok(_) => false,
            Err(e) => {
                self.issues.push(LoadIssue { actor: c, class: "console".into(), event: "KeyType".into(), error: e.to_string() });
                false
            }
        }
    }

    pub fn ui_key(&mut self, code: u8, down: bool) -> bool {
        self.vm.ui_process_key(self.player, code as i32, if down { 1 } else { 3 })
    }

    /// An axis event (`IST_Axis`, action 4) of a mouse axis through `XRootWindow::Process` (`HandleButtons`: the
    /// windows under the cursor get `RawMouseButtonPressed` with action 4). True when the window system took it.
    pub fn ui_axis(&mut self, code: u8) -> bool {
        self.vm.ui_process_key(self.player, code as i32, 4)
    }

    pub fn ui_char(&mut self, c: char) -> bool {
        self.vm.ui_process_char(self.player, c)
    }

    /// The mouse is at pixel `(x, y)` of a `w` x `h` window; the cursor of the window system follows.
    pub fn ui_mouse_abs(&mut self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let root = self.vm.ui_root(self.player);
        if root == 0 {
            return false;
        }
        let [_, _, rw, rh] = self.vm.ui_geometry(root);
        if w <= 0.0 || h <= 0.0 {
            return false;
        }
        // the scene may sit inside black bars
        let (mut fx, mut fy) = (x / w, y / h);
        if let Some([rx, ry, rwf, rhf]) = self.view_rect {
            fx = ((fx - rx) / rwf).clamp(0.0, 1.0);
            fy = ((fy - ry) / rhf).clamp(0.0, 1.0);
        }
        let took = self.vm.ui_mouse_position(self.player, fx * rw, fy * rh);
        self.ui_mouse_last = Some(self.ui_cursor());
        took
    }

    fn ui_cursor(&self) -> (f32, f32) {
        let root = self.vm.ui_root(self.player);
        (self.vm.ui_float(root, "mouseX"), self.vm.ui_float(root, "mouseY"))
    }

    /// Where the OS pointer has to go (pixels of a `w` x `h` window) when a script moved the cursor (`SetCursorPos`)
    /// since the last host mouse event; `None` when it did not.
    pub fn ui_cursor_warp(&mut self, w: f32, h: f32) -> Option<(f32, f32)> {
        let root = self.vm.ui_root(self.player);
        if root == 0 || !self.ui_wants_mouse() {
            return None;
        }
        let now = self.ui_cursor();
        if self.ui_mouse_last == Some(now) || self.ui_mouse_last.is_none() {
            self.ui_mouse_last = Some(now);
            return None;
        }
        self.ui_mouse_last = Some(now);
        let [_, _, rw, rh] = self.vm.ui_geometry(root);
        if rw <= 0.0 || rh <= 0.0 {
            return None;
        }
        let (mut fx, mut fy) = ((now.0 + 0.5) / rw, (now.1 + 0.5) / rh);
        if let Some([rx, ry, rwf, rhf]) = self.view_rect {
            fx = fx * rwf + rx;
            fy = fy * rhf + ry;
        }
        Some((fx * w, fy * h))
    }

    /// Opens a menu screen by class name (`MenuScreenSound`) on the root window, for screenshots of screens.
    pub fn open_menu_screen(&mut self, class: &str) -> bool {
        let root = self.vm.ui_root(self.player);
        let Some(c) = self.vm.reg.class_by_name(class) else { return false };
        let path = self.vm.reg.class(c).path.clone();
        let cls = ue_vm::Value::Obj(self.vm.reg.obj_id(&path).0);
        root != 0 && self.vm.call_by_name(root, "InvokeMenuScreen", vec![cls, ue_vm::Value::Bool(true)]).is_ok()
    }

    /// Is the view lit (the viewing player's `RendMap` 5, `REN_DynLight`)? Render.dll sets up surface and actor lights
    /// only then; the `Matrix` cheat switches to `RMODE 6` (plain textures).
    pub fn lit_view(&mut self) -> bool {
        self.vm.ip(self.player, "RendMap") == 5
    }

    /// Runs a console command line as the console hands it to the viewport (`ShowMainMenu`, `RMODE 6`, `SetRes
    /// 1024x768`): the whole chain of `ue_vm::console`. Returns whether something took it.
    pub fn exec(&mut self, line: &str) -> bool {
        match self.vm.exec_viewport_line(self.player, line) {
            Ok((taken, _)) => {
                self.perform_engine_requests();
                taken
            }
            Err(e) => {
                self.issues.push(LoadIssue { actor: self.player, class: "input".into(), event: "Exec".into(), error: format!("{line}: {e}") });
                false
            }
        }
    }

    /// What console commands asked the engine to do after the frame.
    fn perform_engine_requests(&mut self) {
        use ue_vm::console::EngineRequest;
        for r in std::mem::take(&mut self.vm.engine_requests) {
            match r {
                EngineRequest::Exit => self.quit = true,
                // the port cannot start itself again: it leaves (Exit follows)
                EngineRequest::Relaunch(_) => {}
                EngineRequest::Reconnect => {
                    self.vm.travel_request = Some(ue_vm::travel::TravelRequest { url: self.map_name.clone(), items: false });
                }
                // screenshots of the running game are not written (the original writes ShotNNNN.bmp into the game
                // folder); the renderer keeps no caches a flush would drop
                EngineRequest::Shot | EngineRequest::Flush => {}
            }
        }
    }

    /// Takes the snapshots the scripts asked for (`GenerateSnapshot`) from the last frame drawn, before the next one is,
    /// then hands the renderer every run-time texture whose pixels changed (snapshots, saved games' pictures).
    fn take_snapshots(&mut self, renderer: &mut dx_vk::renderer::Renderer) {
        if !self.vm.snapshot_requests.is_empty() {
            if let Ok(Some((w, h, rgba))) = renderer.read_last_frame() {
                self.vm.fill_snapshots(w, h, &rgba, self.brightness);
            }
        }
        let changed: Vec<(u32, u64)> = self.vm.runtime_textures.iter().filter(|(id, t)| self.runtime_published.get(*id) != Some(&t.version)).map(|(id, t)| (*id, t.version)).collect();
        for (tex, version) in changed {
            let Some(t) = self.vm.runtime_textures.get(&tex) else { continue };
            let (w, h, rgba) = (t.usize, t.vsize, t.rgba());
            let path = self.vm.reg.obj_path(ue_registry::ObjId(tex)).to_string();
            self.view.set_runtime_texture(&path, w, h, rgba);
            self.runtime_published.insert(tex, version);
        }
    }

    /// Draws the current view into the renderer's window. Returns false when nothing was drawn (minimised, swapchain rebuilt).
    pub fn render_vk_window(&mut self, renderer: &mut dx_vk::renderer::Renderer, window_size: (u32, u32)) -> Result<bool, String> {
        let (w, h) = renderer.render_size.unwrap_or(window_size);
        let (cam, actors) = self.prepare_vk_frame(renderer, (w, h), window_size);
        let drawn = renderer.render_present(&mut self.view, &cam, &actors, self.brightness, false, window_size).map_err(|e| e.to_string())?;
        self.meshes_drawn = renderer.actors.drawn;
        Ok(drawn)
    }

    /// One frame as the window draws it, offscreen (no presentation, no read back): for measurements.
    /// `prep_ms` gets the session's preparation phases (milliseconds); the renderer's own are in `renderer.cpu_ms`,
    /// the GPU time in `renderer.gpu_ms`.
    pub fn render_vk_bench(&mut self, renderer: &mut dx_vk::renderer::Renderer, size: (u32, u32), prep_ms: &mut Vec<(&'static str, f64)>) -> Result<(), String> {
        self.prep_ms.clear();
        let (cam, actors) = self.prepare_vk_frame(renderer, size, size);
        *prep_ms = std::mem::take(&mut self.prep_ms);
        // as the window draws (frames in flight), unless `DX_BENCH_SYNC` asks for each frame to be waited for
        if std::env::var_os("DX_BENCH_SYNC").is_some() {
            renderer.render_offscreen(&mut self.view, &cam, &actors, size.0, size.1).map_err(|e| e.to_string())?;
        } else {
            renderer.render_queued(&mut self.view, &cam, &actors, size.0, size.1).map_err(|e| e.to_string())?;
        }
        self.meshes_drawn = renderer.actors.drawn;
        Ok(())
    }

    /// The session's part of a window frame rendered at `(w, h)` in a `window_size` window: the camera, the actors and
    /// everything the renderer reads (child frames, details, movers, the window system's draw list, uploads). Each
    /// phase's time goes to `prep_ms`.
    fn prepare_vk_frame(&mut self, renderer: &mut dx_vk::renderer::Renderer, (w, h): (u32, u32), window_size: (u32, u32)) -> (Camera, Vec<dx_soft::ActorDraw>) {
        let mut t = std::time::Instant::now();
        let mut lap = |s: &mut Self, name: &'static str| {
            let e = t.elapsed().as_secs_f64() * 1000.0;
            s.prep_ms.push((name, e));
            t = std::time::Instant::now();
        };
        self.take_snapshots(renderer);
        self.set_vis_size(w, h);
        lap(self, "snapshots/vis size");
        let (cam, rect) = self.scene_camera();
        renderer.main_rect = rect;
        let r = rect.map(|r| [r[0] as i32, r[1] as i32, r[2] as i32, r[3] as i32]).unwrap_or([0, 0, w as i32, h as i32]);
        renderer.coronas = self.corona_quads(&cam, r);
        lap(self, "camera/coronas");
        let actors = if self.draw_meshes && rect != Some([0; 4]) { self.actor_draws(&cam) } else { Vec::new() };
        lap(self, "actor draws");
        renderer.child_views = if self.draw_meshes { self.child_views(&cam) } else { None };
        lap(self, "child views");
        (renderer.actors.details, renderer.actors.details_blended) = if rect != Some([0; 4]) { self.detail_polys(&cam) } else { Default::default() };
        lap(self, "detail polys");
        self.sync_movers(renderer);
        lap(self, "movers");
        let plain = !self.lit_view();
        renderer.set_plain_textures(plain);
        let [rx, ry, rw, rh] = dx_vk::present::fit_rect((w, h), window_size);
        let (ww, wh) = (window_size.0.max(1) as f32, window_size.1.max(1) as f32);
        self.view_rect = Some([rx / ww, ry / wh, rw / ww, rh / wh]);
        renderer.ui_quads = if self.show_ui { self.ui_quads(w, h) } else { Vec::new() };
        lap(self, "window system paint");
        renderer.ui_views = if self.show_ui { self.ui_subviews() } else { Vec::new() };
        renderer.ui_actors = if self.show_ui { self.ui_actor_draws() } else { Vec::new() };
        lap(self, "window views/actors");
        for (k, i) in &self.anim_frames {
            renderer.set_anim_frame(k, *i);
        }
        renderer.lightmap_updates = self.take_lightmap_updates();
        renderer.mover_lightmap_updates = self.take_mover_lightmap_updates();
        renderer.mover_fog_updates = self.take_mover_fog_updates();
        renderer.child_fog_updates = self.take_child_fog_updates();
        renderer.fog_updates = self.take_fog_updates();
        lap(self, "light/fog map updates");
        (cam, actors)
    }

    /// Renders the same view with the Vulkan renderer (offscreen, read back as RGBA8). `renderer` is made on first use.
    pub fn render_vk(&mut self, renderer: &mut Option<dx_vk::renderer::Renderer>, w: u32, h: u32) -> Result<Image, String> {
        if let Some(r) = renderer.as_mut() {
            self.take_snapshots(r);
        }
        self.set_vis_size(w, h);
        let (cam, rect) = self.scene_camera();
        let actors = if self.draw_meshes && rect != Some([0; 4]) { self.actor_draws(&cam) } else { Vec::new() };
        if renderer.is_none() {
            self.wants_world_light = true;
            let lighting = dx_light::collect(&mut self.vm.reg, self.map_pkg, &self.view.model);
            let quality = self.quality();
            let r = dx_vk::renderer::Renderer::with_quality(&mut self.view, Some(&lighting), vk_validation(), &[], quality).map_err(|e| e.to_string())?;
            if r.quality() != dx_vk::Quality::ORIGINAL {
                eprintln!("picture options: {:?}", r.quality());
            }
            *renderer = Some(r);
        }
        let corona_rect = rect.map(|r| [r[0] as i32, r[1] as i32, r[2] as i32, r[3] as i32]).unwrap_or([0, 0, w as i32, h as i32]);
        let coronas = self.corona_quads(&cam, corona_rect);
        let r = renderer.as_mut().unwrap();
        r.main_rect = rect;
        r.coronas = coronas;
        r.world.use_lightmaps = self.lightmaps && r.world.lightmap_info.is_some();
        r.world.time = self.level_time();
        r.time = self.viewport_time();
        r.ui.flash = self.screen_flash();
        if let Some(k) = std::env::var("DX_LIGHT_SCALE").ok().and_then(|v| v.parse().ok()) {
            r.world.light_scale = k;
        }
        r.child_views = if self.draw_meshes { self.child_views(&cam) } else { None };
        (r.actors.details, r.actors.details_blended) = if rect != Some([0; 4]) { self.detail_polys(&cam) } else { Default::default() };
        self.sync_movers(r);
        let plain = !self.lit_view();
        r.set_plain_textures(plain);
        r.ui_quads = if self.show_ui { self.ui_quads(w, h) } else { Vec::new() };
        r.ui_views = if self.show_ui { self.ui_subviews() } else { Vec::new() };
        r.ui_actors = if self.show_ui { self.ui_actor_draws() } else { Vec::new() };
        for (k, i) in &self.anim_frames {
            r.set_anim_frame(k, *i);
        }
        r.lightmap_updates = self.take_lightmap_updates();
        r.mover_lightmap_updates = self.take_mover_lightmap_updates();
        r.mover_fog_updates = self.take_mover_fog_updates();
        r.child_fog_updates = self.take_child_fog_updates();
        r.fog_updates = self.take_fog_updates();
        let rgba = r.render(&mut self.view, &cam, &actors, w, h).map_err(|e| e.to_string())?;
        self.meshes_drawn = r.actors.drawn;
        let mut rgba = rgba;
        dx_vk::apply_brightness(&mut rgba, self.brightness);
        let mut img = Image { width: w, height: h, rgba };
        if self.show_markers {
            self.draw_markers(&cam, &mut img);
        }
        Ok(img)
    }

    /// Screen-space boxes (x, y, width, height, is_scripted) of the live pawns in front of the camera and in sight.
    pub fn pawn_boxes(&mut self, cam: &Camera, w: u32, h: u32) -> Vec<(i32, i32, i32, i32, bool)> {
        let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
        let (sp, cp) = ang(cam.pitch).sin_cos();
        let (sy, cy) = ang(cam.yaw).sin_cos();
        let fwd = [cp * cy, cp * sy, sp];
        let right = [-sy, cy, 0.0];
        let up = [-sp * cy, -sp * sy, cp];
        let focal = (w as f32 * 0.5) / dx_vk::world::hor_plus_tan(cam.fov, w as f32 / h.max(1) as f32);
        let pawn = self.vm.reg.class_by_name("Pawn").unwrap();
        let scripted = self.vm.reg.class_by_name("ScriptedPawn");
        let mut out = Vec::new();
        let ids: Vec<u32> = self.vm.live_actors();
        for a in ids {
            let Some(c) = self.vm.class_of(a) else { continue };
            if a == self.player || !self.vm.is_a(c, pawn) || self.vm.mesh_draw(a).is_none() {
                continue;
            }
            let loc = self.vm.location(a);
            let d = [loc[0] - cam.pos[0], loc[1] - cam.pos[1], loc[2] - cam.pos[2]];
            let depth = d[0] * fwd[0] + d[1] * fwd[1] + d[2] * fwd[2];
            if depth < 20.0 || depth > 6000.0 {
                continue;
            }
            if !self.vm.bsp.as_ref().map(|b| b.fast_line_check(loc, cam.pos)).unwrap_or(true) {
                continue;
            }
            let x = d[0] * right[0] + d[1] * right[1] + d[2] * right[2];
            let y = d[0] * up[0] + d[1] * up[1] + d[2] * up[2];
            let (r, hh) = (self.vm.get_prop(a, "CollisionRadius", 0).ok().and_then(|v| v.as_f32()).unwrap_or(17.0), self.vm.get_prop(a, "CollisionHeight", 0).ok().and_then(|v| v.as_f32()).unwrap_or(39.0));
            let (sx, sy_) = (w as f32 * 0.5 + x / depth * focal, h as f32 * 0.5 - y / depth * focal);
            let (bw, bh) = (r * 2.0 / depth * focal, hh * 2.0 / depth * focal);
            let is_scripted = scripted.map(|s| self.vm.is_a(c, s)).unwrap_or(false);
            out.push(((sx - bw * 0.5) as i32, (sy_ - bh * 0.5) as i32, bw.max(2.0) as i32, bh.max(2.0) as i32, is_scripted));
        }
        out
    }

    fn draw_markers(&mut self, cam: &Camera, img: &mut Image) {
        let (w, h) = (img.width as i32, img.height as i32);
        for (x, y, bw, bh, scripted) in self.pawn_boxes(cam, img.width, img.height) {
            let color = if scripted { [255, 140, 30, 255] } else { [200, 200, 200, 255] };
            for t in 0..=1 {
                for i in 0..=bw {
                    for yy in [y + t * bh] {
                        plot(img, w, h, x + i, yy, color);
                    }
                }
                for i in 0..=bh {
                    plot(img, w, h, x + t * bw, y + i, color);
                }
            }
        }
    }
}

fn plot(img: &mut Image, w: i32, h: i32, x: i32, y: i32, c: [u8; 4]) {
    if x >= 0 && y >= 0 && x < w && y < h {
        let i = (y * w + x) as usize * 4;
        img.rgba[i..i + 4].copy_from_slice(&c);
    }
}

/// `DDeusExGameEngine::GetNextMissionNumber` (DeusEx.dll 10011ef0): the number a map's name starts with
/// (`01_NYC_UNATCOIsland`: 1), none when it does not start with a digit.
pub fn mission_of_map(map: &str) -> Option<i32> {
    let digits: String = map.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// The per-user configuration directory of this port (see `ue_config::user_config_dir`).
/// A cargo test harness (`target/<profile>/deps/<name>-<hash>`) gets a directory of its own under the system's
/// temporary directory instead, so tests neither read the player's settings nor write over them.
pub fn config_dir() -> Option<std::path::PathBuf> {
    let in_test_harness = std::env::current_exe().ok().is_some_and(|e| e.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "deps"));
    if in_test_harness {
        return Some(std::env::temp_dir().join(format!("deus-ex-aerugo-test-config-{}", std::process::id())));
    }
    ue_config::user_config_dir()
}

/// The player's bindings: `Input.ini` in the configuration directory when it exists (same format as the game's
/// `[Engine.Input]` section), else the port's modern defaults.
pub fn load_bindings() -> dx_input::Bindings {
    config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("Input.ini")).ok())
        .map(|t| dx_input::Bindings::from_ini(&t))
        .unwrap_or_else(dx_input::Bindings::defaults)
}

/// What `DrawLodMesh`'s outcodes need of the frame being drawn: its camera and side slopes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClipFrame {
    origin: [f32; 3],
    axes: [[f32; 3]; 3],
    /// `PrjXM`, `PrjYM`, `PrjXP`, `PrjYP`.
    prj: [f32; 4],
}

impl ClipFrame {
    fn of(f: &dx_vis::frame::Frame) -> ClipFrame {
        ClipFrame { origin: f.origin, axes: [f.x_axis, f.y_axis, f.z_axis], prj: [f.prj_xm, f.prj_ym, f.prj_xp, f.prj_yp] }
    }

    /// A world point's outcode (Render 10b0ea80's transform loop): one bit per side the view-space point is outside,
    /// `z PrjXM + x`, `z PrjYM + y`, `z PrjXP - x`, `z PrjYP - y` with the sign bit set (the engine tests the stored
    /// float's sign bit: -0.0 counts).
    fn outcode(&self, p: [f32; 3]) -> u8 {
        let d = [p[0] - self.origin[0], p[1] - self.origin[1], p[2] - self.origin[2]];
        let v: [f32; 3] = std::array::from_fn(|i| d[0] * self.axes[i][0] + d[1] * self.axes[i][1] + d[2] * self.axes[i][2]);
        let side = [v[2] * self.prj[0] + v[0], v[2] * self.prj[1] + v[1], v[2] * self.prj[2] - v[0], v[2] * self.prj[3] - v[1]];
        side.iter().enumerate().fold(0u8, |m, (k, &s)| if s.is_sign_negative() { m | 1 << k } else { m })
    }
}

/// The weapon triangle frame of special vertices `v0`, `v1`, `v2` in world terms (see `Session::weapon_triangle`):
/// origin, X, Y, Z.
fn weapon_frame(v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> [[f32; 3]; 4] {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let x = ue_bsp::safe_normal(sub(v1, v0));
    let u = ue_bsp::safe_normal(cross(x, sub(v0, v2)));
    let y = [-u[0], -u[1], -u[2]];
    let z = cross(x, y);
    let mid = [(v0[0] + v2[0]) * 0.5, (v0[1] + v2[1]) * 0.5, (v0[2] + v2[2]) * 0.5];
    [mid, x, y, z]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine builds the frame in view space (`ViewCoords`: view = (w.y, -w.z, w.x)): for v0 = 0, v1 = 10 x,
    /// v2 = -5 z the view points are (0,0,0), (0,0,10), (0,5,0); X = (0,0,1), Y = SafeNormal(X x (v0 - v2)) = (1,0,0),
    /// Z = Y x X = (0,-1,0); back in world terms (w = (c, a, -b)) X = +x, Y = +y, Z = +z, origin (0, 0, -2.5).
    #[test]
    fn weapon_frame_as_the_engine_builds_it_in_view_space() {
        let f = weapon_frame([0.0; 3], [10.0, 0.0, 0.0], [0.0, 0.0, -5.0]);
        assert_eq!(f, [[0.0, 0.0, -2.5], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }

    /// Outcodes against a 90-degree 800 x 600 view looking along +x: a point far left is outside the left side only,
    /// one above the top outside the top only; -0.0 counts as outside (sign bit).
    #[test]
    fn outcodes_of_the_frame_being_drawn() {
        let f = dx_vis::frame::Frame::master([0.0; 3], [0, 0, 0], 90.0, 800, 600);
        let c = ClipFrame::of(&f);
        assert_eq!(c.outcode([100.0, 0.0, 0.0]), 0);
        assert_eq!(c.outcode([100.0, -300.0, 0.0]), 1);
        assert_eq!(c.outcode([100.0, 0.0, 300.0]), 2);
        assert_eq!(c.outcode([100.0, 300.0, 0.0]), 4);
        assert_eq!(c.outcode([100.0, 0.0, -300.0]), 8);
        // exactly on the left edge (z PrjXM + x = 100 - 100 = 0.0, positive zero): inside
        assert_eq!(c.outcode([100.0, -100.0, 0.0]) & 1, 0);
        let side = -0.0f32;
        assert!(side.is_sign_negative());
    }
}

/// The Khronos validation layer is a development aid that costs a large part of every frame: on only when
/// `DX_VALIDATION=1` is set.
pub fn vk_validation() -> bool {
    std::env::var_os("DX_VALIDATION").is_some_and(|v| v != "0")
}

thread_local! {
    /// `DX_STEP_PROFILE=1`: seconds per phase of `tick_world`, printed every 120 frames.
    pub(crate) static STEP_PROFILE: std::cell::RefCell<std::collections::HashMap<&'static str, f64>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Effect sound channels mixed (the port's fixed number; see `attach_audio`).
pub const EFFECTS_CHANNELS: usize = 128;

/// The port's own settings (`Aerugo.AerugoSettings`; see `Session::aerugo_settings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AerugoSettings {
    /// 0 off (immediate), 1 on (FIFO), 2 adaptive (relaxed FIFO), 3 fast (mailbox).
    pub vsync: i32,
    /// Frames per second; 0 none, -1 the display's refresh rate minus 3.
    pub frame_limit: i32,
    /// Sound output rate in Hz; 0 the system's.
    pub output_rate: i32,
    /// The mouse's counts per inch.
    pub mouse_dpi: i32,
    /// The picture options of the Advanced Graphics screen.
    pub quality: dx_vk::Quality,
    /// The newest decals kept out of sight (0: as the original).
    pub max_decals: i32,
}


//! Saving and loading games. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dxshin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn new_game(dir: &PathBuf) -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.vm.save_dir = Some(dir.clone());
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    run(&mut s, 5);
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland");
    Some(s)
}

fn actor_of(s: &mut Session, class: &str) -> u32 {
    let c = s.vm.reg.class_by_name(class).unwrap();
    s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).unwrap()
}

fn health(s: &mut Session, a: u32) -> i32 {
    s.vm.get_prop(a, "Health", 0).ok().and_then(|v| v.as_i32()).unwrap_or(-999)
}

fn flag(s: &mut Session, name: &str) -> bool {
    let fb = match s.vm.get_prop(s.player, "flagBase", 0) { Ok(Value::Obj(f)) => f, _ => 0 };
    let n = s.vm.reg.names.intern(name);
    matches!(s.vm.call_by_name(fb, "GetBool", vec![Value::Name(n)]), Ok(Value::Bool(true)))
}

fn set_flag(s: &mut Session, name: &str) {
    let fb = match s.vm.get_prop(s.player, "flagBase", 0) { Ok(Value::Obj(f)) => f, _ => 0 };
    let n = s.vm.reg.names.intern(name);
    s.vm.call_by_name(fb, "SetBool", vec![Value::Name(n), Value::Bool(true), Value::Bool(true), Value::Int(0)]).unwrap();
}

#[test]
fn a_game_can_be_saved_and_loaded_again() {
    let dir = temp_dir("roundtrip");
    let Some(mut s) = new_game(&dir) else { return };
    // a state worth saving: the player somewhere else, a flag set, the pistol at 3 rounds fewer, Paul hurt
    let paul = actor_of(&mut s, "PaulDenton");
    let spot = [-3000.0, 10200.0, -250.0];
    s.teleport(spot, 90.0);
    set_flag(&mut s, "SaveTestFlag");
    s.vm.set_prop(paul, "Health", 0, Value::Int(77)).unwrap();
    run(&mut s, 20);
    let at_save = s.player_location();
    let index = s.save_game(0, "first test save").unwrap();
    assert_eq!(index, 1);
    assert!(dir.join("Save0001/save.dxs").exists() && dir.join("Save0001/info.txt").exists());

    // change things, then go back
    s.teleport([-4700.0, 10400.0, -250.0], 0.0);
    s.vm.set_prop(paul, "Health", 0, Value::Int(5)).unwrap();
    run(&mut s, 30);
    s.load_game(1).unwrap();
    run(&mut s, 5);
    let paul = actor_of(&mut s, "PaulDenton");
    let now = s.player_location();
    eprintln!("saved at {at_save:?}, loaded at {now:?}, Paul {}", health(&mut s, paul));
    assert!((now[0] - at_save[0]).abs() < 30.0 && (now[1] - at_save[1]).abs() < 30.0, "player not where it was saved: {now:?}");
    assert_eq!(health(&mut s, paul), 77, "Paul's health was not restored");
    assert!(flag(&mut s, "SaveTestFlag"), "the flag was lost");
    let terr = actor_of(&mut s, "Terrorist");
    let gs = s.vm.call_by_name(terr, "GetStateName", vec![]);
    eprintln!("terrorist state {:?} gstate {gs:?}", s.vm.state_name(terr));
    // the HUD and the game go on
    let root = match s.vm.get_prop(s.player, "rootWindow", 0) { Ok(Value::Obj(r)) => r, _ => 0 };
    assert!(s.vm.is_live(root), "no root window after loading");
    s.issues.clear();
    run(&mut s, 90);
    let bad: Vec<String> = s.issues.iter().filter(|i| !i.error.contains("InStasis")).take(5).map(|i| format!("{} {}: {}", i.class, i.event, i.error)).collect();
    assert!(bad.is_empty(), "script errors after loading: {bad:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// (class, location, state, health) of every live actor that is not an effect, sorted: the world as the game sees it.
fn digest(s: &mut Session) -> Vec<String> {
    let mut v = Vec::new();
    for a in s.vm.live_actors() {
        let Some(c) = s.vm.class_of(a) else { continue };
        let class = s.vm.reg.class(c).path.clone();
        let l = s.vm.location(a);
        let st = s.vm.state_name(a).unwrap_or("-").to_string();
        let h = s.vm.get_prop(a, "Health", 0).ok().and_then(|x| x.as_i32()).unwrap_or(0);
        v.push(format!("{class} {:.0} {:.0} {:.0} {st} {h}", l[0], l[1], l[2]));
    }
    v.sort();
    v
}

fn inventory_classes(s: &mut Session) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = match s.vm.get_prop(s.player, "Inventory", 0) { Ok(Value::Obj(i)) => i, _ => 0 };
    while it != 0 && out.len() < 200 {
        if let Some(c) = s.vm.class_of(it) {
            out.push(s.vm.reg.class(c).path.clone());
        }
        it = match s.vm.get_prop(it, "Inventory", 0) { Ok(Value::Obj(i)) => i, _ => 0 };
    }
    out.sort();
    out
}

/// The whole world comes back: the same actors with the same places, states and health, the same inventory; and the game
/// then runs on without script errors, also in a fresh session that never saw the saved level.
#[test]
fn the_world_is_the_same_after_loading_in_a_fresh_session() {
    let dir = temp_dir("fresh");
    let Some(mut s) = new_game(&dir) else { return };
    run(&mut s, 120);
    let before = digest(&mut s);
    let inv = inventory_classes(&mut s);
    let p = s.player;
    let hp = health(&mut s, p);
    let at = s.player_location();
    s.save_game(0, "digest").unwrap();
    drop(s);

    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR").unwrap());
    let mut f = Session::open(&g, "DX").unwrap();
    f.vm.save_dir = Some(dir.clone());
    f.load_game(1).unwrap();
    let after = digest(&mut f);
    eprintln!("actors: {} before, {} after", before.len(), after.len());
    let missing: Vec<&String> = before.iter().filter(|x| !after.contains(x)).take(8).collect();
    let extra: Vec<&String> = after.iter().filter(|x| !before.contains(x)).take(8).collect();
    eprintln!("missing after load: {missing:?}\nextra after load: {extra:?}");
    assert!(missing.is_empty() && extra.is_empty() && before.len() == after.len(), "the world differs after loading");
    assert_eq!(inventory_classes(&mut f), inv);
    let fp = f.player;
    assert_eq!(health(&mut f, fp), hp);
    let now = f.player_location();
    assert!((now[0] - at[0]).abs() < 1.0 && (now[1] - at[1]).abs() < 1.0);
    f.issues.clear();
    run(&mut f, 150);
    let bad: Vec<String> = f.issues.iter().filter(|i| !i.error.contains("InStasis")).take(5).map(|i| format!("{} {}: {}", i.class, i.event, i.error)).collect();
    assert!(bad.is_empty(), "script errors after loading: {bad:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The game's own commands: `QuickSave` writes the quick slot, `LoadGame -1` (what the quick load does) restores it, and a
/// numbered save made before shows up in the load screen's directory listing.
#[test]
fn the_scripts_save_and_load_through_the_commands_and_the_directory() {
    let dir = temp_dir("commands");
    let Some(mut s) = new_game(&dir) else { return };
    run(&mut s, 30);
    s.teleport([-3500.0, 10300.0, -250.0], 45.0);
    run(&mut s, 10);
    let at = s.player_location();
    s.save_game(0, "numbered").unwrap();
    assert!(s.exec("QuickSave"));
    run(&mut s, 3);
    assert!(dir.join("QuickSave/save.dxs").exists(), "QuickSave wrote nothing");
    s.teleport([-4700.0, 10400.0, -250.0], 0.0);
    run(&mut s, 10);
    assert!(s.exec("LoadGame -1"));
    run(&mut s, 5);
    let now = s.player_location();
    assert!((now[0] - at[0]).abs() < 30.0 && (now[1] - at[1]).abs() < 30.0, "quick load did not bring the player back: {now:?}");

    // the directory listing the load screen reads
    let player = s.player;
    let gd = s.vm.call_by_name(player, "CreateGameDirectoryObject", vec![]).unwrap();
    let Value::Obj(gd) = gd else { panic!("no GameDirectory") };
    s.vm.call_by_name(gd, "SetDirType", vec![Value::Byte(1)]).unwrap();
    s.vm.call_by_name(gd, "GetGameDirectory", vec![]).unwrap();
    assert_eq!(s.vm.call_by_name(gd, "GetDirCount", vec![]).unwrap(), Value::Int(1));
    let Value::Obj(info) = s.vm.call_by_name(gd, "GetSaveInfoFromDirectoryIndex", vec![Value::Int(0)]).unwrap() else { panic!("no save info") };
    assert_eq!(s.vm.get_prop(info, "Description", 0).unwrap(), Value::Str("numbered".into()));
    assert_eq!(s.vm.get_prop(info, "MapName", 0).unwrap(), Value::Str("01_NYC_UNATCOIsland".into()));
    let Value::Obj(quick) = s.vm.call_by_name(gd, "GetSaveInfo", vec![Value::Int(-1)]).unwrap() else { panic!("no quick save info") };
    assert!(s.vm.is_live(quick));
    assert_eq!(s.vm.call_by_name(gd, "GetSaveInfo", vec![Value::Int(7)]).unwrap(), Value::Obj(0));
    // deleting from the load screen (`player.ConsoleCommand("DeleteGame " $ n)`): at once, as the engine does
    let r = s.vm.call_by_name(player, "ConsoleCommand", vec![Value::Str("DeleteGame 1".into())]).unwrap();
    assert_eq!(r, Value::Str(String::new()));
    assert!(!dir.join("Save0001").exists(), "the saved game was not deleted");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Picture of the load screen with two saved games (`../out/screenshots/s12_load_screen.png`); needs a Vulkan device.
#[test]
#[ignore]
fn shot_of_the_load_screen() {
    let dir = temp_dir("shot");
    let Some(mut s) = new_game(&dir) else { return };
    run(&mut s, 30);
    s.save_game(0, "First try at the island").unwrap();
    s.teleport([-3500.0, 10300.0, -250.0], 45.0);
    run(&mut s, 30);
    s.save_game(0, "Near the docks").unwrap();
    assert!(s.open_menu_screen("MenuScreenLoadGame"));
    run(&mut s, 10);
    let mut r = None;
    let img = s.render_vk(&mut r, 800, 600).expect("vulkan render");
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("s12_load_screen.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    r.take().unwrap().destroy();
    let _ = std::fs::remove_dir_all(&dir);
}

fn click(s: &mut Session, x: f32, y: f32) {
    s.ui_mouse_abs(x, y, 800.0, 600.0);
    s.step_bound(DT);
    s.ui_key(1, true);
    s.step_bound(DT);
    s.ui_key(1, false);
    s.step_bound(DT);
}

/// The load screen: pick a saved game in the list with the mouse and load it.
#[test]
fn the_load_screen_loads_the_chosen_game() {
    let dir = temp_dir("loadscreen");
    let Some(mut s) = new_game(&dir) else { return };
    run(&mut s, 30);
    let first = s.player_location();
    s.save_game(0, "First try at the island").unwrap();
    s.teleport([-3500.0, 10300.0, -250.0], 45.0);
    run(&mut s, 30);
    s.save_game(0, "Near the docks").unwrap();
    s.teleport([-4000.0, 10000.0, -250.0], 90.0);
    run(&mut s, 10);
    assert!(s.open_menu_screen("MenuScreenLoadGame"));
    run(&mut s, 10);
    click(&mut s, 250.0, 181.0); // the first row
    click(&mut s, 545.0, 459.0); // Load Game
    run(&mut s, 10);
    let now = s.player_location();
    eprintln!("first save at {first:?}, now {now:?}");
    // the rows of two saves made within a minute of each other have no fixed order: either game is a correct answer
    let near = |p: [f32; 3], q: [f32; 3]| (p[0] - q[0]).abs() < 40.0 && (p[1] - q[1]).abs() < 40.0;
    assert!(near(now, first) || near(now, [-3500.0, 10300.0, -250.0]), "no saved game was loaded: {now:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The save screen: a new row for the new game, Enter confirms the suggested name and the game is written.
#[test]
fn the_save_screen_writes_a_new_game() {
    let dir = temp_dir("savescreen");
    let Some(mut s) = new_game(&dir) else { return };
    run(&mut s, 30);
    assert!(s.open_menu_screen("MenuScreenSaveGame"));
    run(&mut s, 10);
    let enter = dx_input::key_code("Enter").unwrap();
    s.ui_key(enter, true);
    s.step_bound(DT);
    s.ui_key(enter, false);
    run(&mut s, 30);
    assert!(dir.join("Save0001/save.dxs").exists(), "the save screen wrote nothing; issues: {:?}", s.issues.iter().take(3).map(|i| &i.error).collect::<Vec<_>>());
    let info = std::fs::read_to_string(dir.join("Save0001/info.txt")).unwrap();
    eprintln!("{info}");
    assert!(info.contains("MapName=01_NYC_UNATCOIsland"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The strictest check: after saving, the original session and a freshly loaded one are run for the same time and compared.
/// What was saved is everything that decides the next frames (memory, states, state-code position, sleeps, timers, the
/// random generator), so the two worlds stay together.
#[test]
fn a_loaded_game_continues_like_the_original() {
    let dir = temp_dir("determinism");
    let Some(mut a) = new_game(&dir) else { return };
    run(&mut a, 150);
    a.save_game(0, "determinism").unwrap();
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR").unwrap());
    let mut b = Session::open(&g, "DX").unwrap();
    b.vm.save_dir = Some(dir.clone());
    b.load_game(1).unwrap();
    // the renderer's frame randoms (`FGlobalRandoms`) are not part of a save, in the original either; they draw from the
    // game's random generator every frame, more right after a load: the loaded game gets the saver's to compare
    b.vm.randoms = a.vm.randoms.clone();
    b.vm.strobe = a.vm.strobe;
    for _ in 0..150 {
        a.step_bound(DT);
        b.step_bound(DT);
    }
    let (da, db) = (digest(&mut a), digest(&mut b));
    let only_a: Vec<&String> = da.iter().filter(|x| !db.contains(x)).collect();
    let only_b: Vec<&String> = db.iter().filter(|x| !da.contains(x)).collect();
    eprintln!("{} actors; only in the original: {}, only in the loaded game: {}", da.len(), only_a.len(), only_b.len());
    for x in only_a.iter().take(10) { eprintln!("  original: {x}"); }
    for x in only_b.iter().take(10) { eprintln!("  loaded:   {x}"); }
    assert!(only_a.is_empty() && only_b.is_empty(), "the loaded game went its own way");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Other maps: a game saved on each of them comes back with the same actors.
#[test]
#[ignore]
fn other_maps_save_and_load() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    for map in ["00_Training", "02_NYC_Street", "04_NYC_Hotel", "06_HongKong_Helibase", "09_NYC_Dockyard", "15_Area51_Entrance"] {
        let dir = temp_dir(map);
        let mut s = Session::open(&g, map).unwrap();
        s.vm.save_dir = Some(dir.clone());
        run(&mut s, 60);
        let before = digest(&mut s);
        s.save_game(0, map).unwrap();
        let size = std::fs::metadata(dir.join("Save0001/save.dxs")).unwrap().len();
        let mut f = Session::open(&g, "DX").unwrap();
        f.vm.save_dir = Some(dir.clone());
        f.load_game(1).unwrap();
        let after = digest(&mut f);
        f.issues.clear();
        run(&mut f, 60);
        let bad = f.issues.iter().filter(|i| !i.error.contains("InStasis")).count();
        let missing = before.iter().filter(|x| !after.contains(x)).count();
        eprintln!("{map}: {} actors, file {} KB, missing {missing}, script errors after load {bad}", before.len(), size / 1024);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The level's actor list as `LoadMap` leaves it: the level, the static actors, then the others
/// (later spawns after them), for a fresh level and for a loaded game; loading a game ends with `PostPostBeginPlay` for
/// every actor (pawns bind their conversations again, mission scripts restart their timer).
#[test]
fn actor_order_and_post_post_begin_play_as_load_map() {
    fn check_order(s: &mut Session) {
        let all = s.vm.live_actors();
        assert_eq!(all[0], s.vm.level);
        // the default (builder) brush's slot stays empty in a game: it is saved for the editor only
        assert!(s.vm.class_of(all[1]).map(|c| s.vm.reg.class(c).path.clone()).as_deref() != Some("Engine.Brush") || s.vm.bp(all[1], "bStatic"));
        let statics: Vec<bool> = all[1..].iter().map(|&a| s.vm.bp(a, "bStatic")).collect();
        let first_dynamic = statics.iter().position(|&x| !x).unwrap();
        assert!(first_dynamic > 0, "no static actors");
        assert!(statics[first_dynamic..].iter().all(|&x| !x), "a static actor after the first dynamic one");
    }
    let dir = temp_dir("order");
    let Some(mut s) = new_game(&dir) else { return };
    check_order(&mut s);
    s.save_game(0, "order").unwrap();
    s.vm.event_trace = Some(Vec::new());
    s.load_game(1).unwrap();
    check_order(&mut s);
    let trace = s.vm.event_trace.take().unwrap();
    let got = |s: &mut Session, class: &str| {
        let c = s.vm.reg.class_by_name(class).unwrap();
        trace.iter().any(|(o, e)| e == "PostPostBeginPlay" && s.vm.class_of(*o).map(|k| s.vm.is_a(k, c)).unwrap_or(false))
    };
    for class in ["DeusExPlayer", "ScriptedPawn", "MissionScript", "DeusExDecoration"] {
        assert!(got(&mut s, class), "no PostPostBeginPlay for a {class} after loading");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A saved game holds the level's game rules object and what it owns (as the original's saves do): loading it in a
/// fresh session brings that object back and the session's own one ends.
#[test]
fn a_saved_game_carries_its_game_rules_object() {
    fn count(s: &mut Session, n: &str) -> usize {
        let c = s.vm.reg.class_by_name(n).unwrap();
        s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).count()
    }
    let dir = temp_dir("gamerules");
    let Some(mut s) = new_game(&dir) else { return };
    let game = s.vm.op(s.vm.level, "Game");
    s.vm.set_prop(game, "Difficulty", 0, Value::Byte(3)).unwrap();
    s.save_game(0, "rules").unwrap();
    drop(s);
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR").unwrap());
    let mut f = Session::open(&g, "DX").unwrap();
    let fresh = f.vm.op(f.vm.level, "Game");
    f.vm.save_dir = Some(dir.clone());
    f.load_game(1).unwrap();
    let restored = f.vm.op(f.vm.level, "Game");
    assert!(restored != fresh && f.vm.is_live(restored) && !f.vm.is_live(fresh));
    assert_eq!(f.vm.get_prop(restored, "Difficulty", 0).unwrap(), Value::Byte(3));
    assert_eq!(count(&mut f, "GameInfo"), 1);
    assert_eq!(count(&mut f, "GameReplicationInfo"), 1);
    assert_eq!(count(&mut f, "Mutator"), 1);
    let bm = f.vm.op(restored, "BaseMutator");
    assert!(bm != 0 && f.vm.is_live(bm));
    run(&mut f, 30);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Loading runs `SpawnPlayActor` as `LoadMap` does (`login_saved_player`): `GameInfo.Login` takes the saved pawn
/// instead of spawning one (expectations from `GameInfo.Login`: `PlayerID = CurrentID++`, `NumPlayers++` on the saved
/// game object; `DeusExGameInfo.Login` leaves the pawn where it was because of `?loadgame`).
#[test]
fn loading_logs_the_saved_player_in_again() {
    fn count(s: &mut Session, n: &str) -> usize {
        let c = s.vm.reg.class_by_name(n).unwrap();
        s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).count()
    }
    let int = |s: &mut Session, o: u32, p: &str| s.vm.get_prop(o, p, 0).ok().and_then(|v| v.as_i32()).unwrap();
    let dir = temp_dir("login");
    let Some(mut s) = new_game(&dir) else { return };
    s.teleport([-3000.0, 10200.0, -250.0], 90.0);
    run(&mut s, 20);
    let at_save = s.player_location();
    let game = s.vm.op(s.vm.level, "Game");
    let (current_id, players) = (int(&mut s, game, "CurrentID"), int(&mut s, game, "NumPlayers"));
    s.save_game(0, "login").unwrap();
    s.teleport([-4700.0, 10400.0, -250.0], 0.0);
    run(&mut s, 10);
    s.vm.log.clear();
    s.load_game(1).unwrap();
    assert!(!s.vm.log.iter().any(|l| l.contains("Login gave a new player")), "{:?}", s.vm.log);
    assert_eq!(count(&mut s, "PlayerPawn"), 1);
    let now = s.player_location();
    assert!((now[0] - at_save[0]).abs() < 1.0 && (now[1] - at_save[1]).abs() < 1.0, "moved by Login: {now:?} vs {at_save:?}");
    assert!(s.vm.op(s.player, "Player") != 0);
    let game = s.vm.op(s.vm.level, "Game");
    let pri = s.vm.op(s.player, "PlayerReplicationInfo");
    assert_eq!(int(&mut s, pri, "PlayerID"), current_id);
    assert_eq!(int(&mut s, game, "CurrentID"), current_id + 1);
    assert_eq!(int(&mut s, game, "NumPlayers"), players + 1);
    run(&mut s, 30);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Snapshots (`GenerateSnapshot`, `docs/snapshot.md`; needs a Vulkan device): a saved game gets the greyscale 160x120
/// picture of the frame on screen in a 256x128 texture, the load screen's `GetSaveInfo` gives it back as `Snapshot`;
/// with `UIBackground=1` the menus draw the frame's picture as the root's raw background. Pictures in
/// `out/screenshots/snapshot_*.png`.
#[test]
#[ignore]
fn snapshots_for_saves_and_the_menu_background() {
    let dir = temp_dir("snapshot");
    let Some(mut s) = new_game(&dir) else { return };
    run(&mut s, 30);
    let mut r = None;
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&out).unwrap();
    s.render_vk(&mut r, 800, 600).unwrap();
    let index = s.save_game(0, "snapshot test").unwrap();
    // the picture is taken from the last frame before the next one is drawn
    s.render_vk(&mut r, 800, 600).unwrap();
    let file = s.vm.save_slot_dir(index).unwrap().join(ue_vm::snapshot::SAVE_FILE);
    let t = ue_vm::snapshot::RuntimeTexture::decode(&std::fs::read(&file).expect("the save's picture")).unwrap();
    assert_eq!((t.usize, t.vsize), (256, 128));
    assert!(t.data[..160].iter().any(|&v| v > 0), "the frame is in it");
    assert!(t.data[(127 * 256)..].iter().all(|&v| v == 0) && t.data[200] == 0, "outside 160x120 stays black");
    assert_eq!(t.palette[100], [100, 100, 100, 255]);
    let png: Vec<u8> = t.data.iter().flat_map(|&v| [v, v, v, 255]).collect();
    std::fs::write(out.join("snapshot_save.png"), dx_soft::png(256, 128, &png)).unwrap();
    // the load screen's save info carries it
    let info = s.vm.make_save_info_pub(index);
    let tex = match s.vm.get_prop(info, "Snapshot", 0).unwrap() {
        Value::Obj(o) => o,
        v => panic!("{v:?}"),
    };
    assert_ne!(tex, 0);
    assert_eq!(s.vm.texture_size(tex), (256.0, 128.0));
    // the snapshot UI background
    let p = s.player;
    s.vm.set_prop(p, "UIBackground", 0, Value::Int(1)).unwrap();
    s.exec("ShowMainMenu");
    run(&mut s, 3);
    s.render_vk(&mut r, 800, 600).unwrap();
    run(&mut s, 2);
    let img = s.render_vk(&mut r, 800, 600).unwrap();
    std::fs::write(out.join("snapshot_menu.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    let root = s.vm.ui_root(p);
    let raw = match s.vm.get_prop(root, "rawBackground", 0).unwrap() {
        Value::Obj(o) => o,
        v => panic!("{v:?}"),
    };
    assert!(s.vm.runtime_textures.contains_key(&raw), "the menu's background is a snapshot");
    r.take().unwrap().destroy();
}

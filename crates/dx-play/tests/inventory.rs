//! The in-game inventory screen with real items. Set `DEUSEX_DIR` to the game folder.

use dx_input::key_code;
use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
    }
}

fn new_game() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    run(&mut s, 5);
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland");
    Some(s)
}

fn shot(s: &mut Session, r: &mut Option<dx_vk::renderer::Renderer>, name: &str) {
    let img = s.render_vk(r, 800, 600).expect("vulkan render");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
}

fn inventory_names(s: &mut Session) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = match s.vm.get_prop(s.player, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    while cur != 0 && out.len() < 100 {
        out.push(s.vm.class_of(cur).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default());
        cur = match s.vm.get_prop(cur, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    }
    out
}

/// Pictures of the inventory with items and with an item selected; needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_inventory() {
    let Some(mut s) = new_game() else { return };
    eprintln!("inventory: {:?}", inventory_names(&mut s));
    for c in ["Medkit", "WeaponStealthPistol", "Lockpick", "Multitool", "BioelectricCell"] {
        s.exec(&format!("Summon {c}"));
    }
    run(&mut s, 10);
    eprintln!("inventory: {:?}", inventory_names(&mut s));
    s.vm.log.clear();
    s.issues.clear();
    assert!(s.exec("ShowInventoryWindow"));
    run(&mut s, 5);
    let mut r = None;
    shot(&mut s, &mut r, "s10_inv_items.png");
    for i in s.issues.iter().take(10) {
        eprintln!("issue {} {}: {}", i.class, i.event, i.error);
    }
    for l in s.vm.log.iter().take(20) {
        eprintln!("log {l}");
    }
    click(&mut s, 150.0, 150.0);
    run(&mut s, 3);
    shot(&mut s, &mut r, "s10_inv_selected.png");
    for l in s.vm.log.iter().take(20) {
        eprintln!("log {l}");
    }
    // Equip the pistol (third item), then use the medkit
    click(&mut s, 255.0, 150.0);
    run(&mut s, 3);
    shot(&mut s, &mut r, "s10_inv_pistol.png");
    click(&mut s, 150.0, 450.0);
    run(&mut s, 3);
    eprintln!("inHand: {:?}", s.vm.get_prop(s.player, "inHand", 0));
    shot(&mut s, &mut r, "s10_inv_equipped.png");
    for i in s.issues.iter().take(10) {
        eprintln!("issue {} {}: {}", i.class, i.event, i.error);
    }
    for l in s.vm.log.iter().take(20) {
        eprintln!("log {l}");
    }
    r.take().unwrap().destroy();
}

/// A left click at a pixel of the 800x600 window.
fn click(s: &mut Session, x: f32, y: f32) {
    s.ui_mouse_abs(x, y, 800.0, 600.0);
    s.ui_key(key_code("LeftMouse").unwrap(), true);
    s.step_bound(DT);
    s.ui_key(key_code("LeftMouse").unwrap(), false);
    s.step_bound(DT);
}

#[allow(dead_code)]
fn dump_tree(s: &mut Session, w: u32, depth: usize) {
    let class = s.vm.class_of(w).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
    let text = match s.vm.get_prop(w, "text", 0) { Ok(Value::Str(t)) => format!("{t:?}"), _ => String::new() };
    eprintln!("{}{class} {:?} {text}", "  ".repeat(depth), s.vm.ui_geometry(w));
    let mut c = match s.vm.get_prop(w, "firstChild", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    while c != 0 {
        dump_tree(s, c, depth + 1);
        c = match s.vm.get_prop(c, "nextSibling", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    }
}

fn geti(s: &mut Session, o: u32, name: &str) -> i32 {
    s.vm.get_prop(o, name, 0).ok().and_then(|v| v.as_i32()).unwrap_or(-1)
}

/// The inventory item of this class (the first in the chain), 0 when the player has none.
fn item_of(s: &mut Session, class: &str) -> u32 {
    let c = s.vm.reg.class_by_name(class).unwrap();
    let mut cur = match s.vm.get_prop(s.player, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    while cur != 0 {
        if s.vm.class_of(cur).map(|k| s.vm.is_a(k, c)).unwrap_or(false) {
            return cur;
        }
        cur = match s.vm.get_prop(cur, "Inventory", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    }
    0
}

const MEDKIT: (f32, f32) = (150.0, 150.0);
const PISTOL: (f32, f32) = (255.0, 150.0);
const EQUIP: (f32, f32) = (150.0, 449.0);
const USE: (f32, f32) = (197.0, 449.0);
const DROP: (f32, f32) = (254.0, 449.0);

#[test]
fn a_medkit_is_used_from_the_inventory_screen() {
    let Some(mut s) = new_game() else { return };
    let kit = item_of(&mut s, "MedKit");
    assert_ne!(kit, 0, "no medkit");
    let before = geti(&mut s, kit, "NumCopies");
    // hurt the player a little, then use the kit from the screen
    s.vm.set_prop(s.player, "HealthTorso", 0, Value::Int(40)).unwrap();
    assert!(s.exec("ShowInventoryWindow"));
    run(&mut s, 5);
    s.ui_quads(800, 600);
    click(&mut s, MEDKIT.0, MEDKIT.1);
    click(&mut s, USE.0, USE.1);
    run(&mut s, 10);
    let p = s.player;
    let torso = geti(&mut s, p, "HealthTorso");
    let kit = item_of(&mut s, "MedKit");
    let after = if kit == 0 { 0 } else { geti(&mut s, kit, "NumCopies") };
    eprintln!("torso {torso}, medkits {before} -> {after}");
    assert!(torso > 40, "the medkit did not heal ({torso})");
    assert_eq!(after, before - 1, "the medkit was not used up");
}

#[test]
fn a_weapon_is_equipped_and_dropped_from_the_inventory_screen() {
    let Some(mut s) = new_game() else { return };
    assert!(s.exec("ShowInventoryWindow"));
    run(&mut s, 5);
    s.ui_quads(800, 600);
    click(&mut s, PISTOL.0, PISTOL.1);
    click(&mut s, EQUIP.0, EQUIP.1);
    for _ in 0..4 {
        run(&mut s, 5);
        eprintln!("inHand {:?} pending {:?}", s.vm.get_prop(s.player, "inHand", 0), s.vm.get_prop(s.player, "inHandPending", 0));
    }
    for i in s.issues.iter().take(10) {
        eprintln!("issue {} {}: {}", i.class, i.event, i.error);
    }
    for l in s.vm.log.iter().take(20) {
        eprintln!("log {l}");
    }
    // the screen pauses the game: the weapon is only pending; it comes into the hand (`PlayerTick`) once the screen
    // is closed
    let pistol = item_of(&mut s, "WeaponPistol");
    let pending = match s.vm.get_prop(s.player, "inHandPending", 0) { Ok(Value::Obj(h)) => h, _ => 0 };
    assert_eq!(pending, pistol, "the pistol was not chosen");
    s.ui_key(key_code("Escape").unwrap(), true);
    run(&mut s, 1);
    s.ui_key(key_code("Escape").unwrap(), false);
    run(&mut s, 60);
    let hand = match s.vm.get_prop(s.player, "inHand", 0) { Ok(Value::Obj(h)) => h, _ => 0 };
    assert_eq!(hand, pistol, "the pistol was not equipped");
    assert!(s.exec("ShowInventoryWindow"));
    run(&mut s, 5);
    s.ui_quads(800, 600);
    click(&mut s, PISTOL.0, PISTOL.1);
    // drop it (the button now says Unequip; Drop is on its right)
    click(&mut s, DROP.0, DROP.1);
    run(&mut s, 10);
    assert_eq!(item_of(&mut s, "WeaponPistol"), 0, "the pistol is still in the inventory after Drop");
    let hand = match s.vm.get_prop(s.player, "inHand", 0) { Ok(Value::Obj(h)) => h, _ => 0 };
    assert_eq!(hand, 0, "a dropped weapon is still in hand");
}

fn skill_level(s: &mut Session, class: &str) -> i32 {
    let c = s.vm.reg.class_by_name(class).unwrap();
    let sys = match s.vm.get_prop(s.player, "SkillSystem", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    let mut cur = match s.vm.get_prop(sys, "FirstSkill", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    while cur != 0 {
        if s.vm.class_of(cur).map(|k| s.vm.is_a(k, c)).unwrap_or(false) {
            return geti(s, cur, "CurrentLevel");
        }
        cur = match s.vm.get_prop(cur, "next", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    }
    -1
}

#[test]
fn a_skill_is_upgraded_from_the_skills_screen() {
    let Some(mut s) = new_game() else { return };
    assert!(s.exec("ShowSkillsWindow"));
    run(&mut s, 5);
    s.ui_quads(800, 600);
    let p = s.player;
    let (before, points) = (skill_level(&mut s, "SkillLockpicking"), geti(&mut s, p, "SkillPointsAvail"));
    // the Lockpicking row, then the Upgrade button below the list
    click(&mut s, 200.0, 318.0);
    click(&mut s, 136.0, 450.0);
    run(&mut s, 5);
    let (after, left) = (skill_level(&mut s, "SkillLockpicking"), geti(&mut s, p, "SkillPointsAvail"));
    eprintln!("lockpicking {before} -> {after}, points {points} -> {left}");
    assert_eq!(after, before + 1, "the skill did not rise");
    assert!(left < points, "no skill points were spent");
}

/// Pictures of the augmentation, goals and logs screens with some content; needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_the_other_screens() {
    let Some(mut s) = new_game() else { return };
    let p0 = s.player;
    s.vm.set_prop(p0, "bCheatsEnabled", 0, Value::Bool(true)).unwrap();
    let sys = match s.vm.get_prop(p0, "AugmentationSystem", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    assert_ne!(sys, 0, "no augmentation system");
    for a in ["AugBallistic", "AugCloak", "AugHealing"] {
        let c = s.vm.reg.class_by_name(a).unwrap();
        let path = s.vm.reg.class(c).path.clone();
        let cls = Value::Obj(s.vm.reg.obj_id(&path).0);
        let r = s.vm.call_by_name(sys, "GivePlayerAugmentation", vec![cls]);
        eprintln!("give {a}: {r:?}");
    }
    let p = s.player;
    s.vm.call_by_name(p, "AddNote", vec![Value::Str("Check the generator room.".into()), Value::Bool(true), Value::Bool(false)]).unwrap();
    run(&mut s, 5);
    let mut r = None;
    for (cmd, name) in [("ShowAugmentationsWindow", "augs"), ("ShowGoalsWindow", "goals"), ("ShowLogsWindow", "logs"), ("ShowConversationsWindow", "convs"), ("ShowImagesWindow", "images")] {
        s.exec(cmd);
        run(&mut s, 5);
        shot(&mut s, &mut r, &format!("s10_screen_{name}.png"));
        let root = s.vm.ui_root(s.player);
        s.vm.call_by_name(root, "ClearWindowStack", vec![]).unwrap();
        run(&mut s, 2);
    }
    for l in s.vm.log.iter().rev().take(10) {
        eprintln!("log {l}");
    }
    r.take().unwrap().destroy();
}

#[test]
fn an_augmentation_is_activated_from_the_augmentations_screen() {
    let Some(mut s) = new_game() else { return };
    let p = s.player;
    // a console command with a class argument (a cheat, so cheats must be on)
    s.vm.set_prop(p, "bCheatsEnabled", 0, Value::Bool(true)).unwrap();
    assert!(s.exec("AugAdd AugBallistic"));
    let sys = match s.vm.get_prop(p, "AugmentationSystem", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    let cls = class_value(&mut s, "AugBallistic");
    let aug = match s.vm.call_by_name(sys, "FindAugmentation", vec![cls]) { Ok(Value::Obj(o)) => o, _ => 0 };
    assert_ne!(aug, 0, "the augmentation was not installed");
    assert!(s.exec("ShowAugmentationsWindow"));
    run(&mut s, 5);
    s.ui_quads(800, 600);
    let active = |s: &mut Session| matches!(s.vm.get_prop(aug, "bIsActive", 0), Ok(Value::Bool(true)));
    assert!(!active(&mut s));
    // the first subdermal slot (where the ballistic protection sits), then the Activate button
    click(&mut s, 150.0, 320.0);
    click(&mut s, 147.0, 512.0);
    run(&mut s, 10);
    assert!(active(&mut s), "the augmentation did not switch on");
}

fn class_value(s: &mut Session, name: &str) -> Value {
    let c = s.vm.reg.class_by_name(name).unwrap();
    let path = s.vm.reg.class(c).path.clone();
    Value::Obj(s.vm.reg.obj_id(&path).0)
}

/// The biggest quads the inventory screen draws (texture, colour, blend): for comparing with the original's look.
#[test]
#[ignore]
fn survey_inventory_quads() {
    let Some(mut s) = new_game() else { return };
    assert!(s.exec("ShowInventoryWindow"));
    run(&mut s, 5);
    let mut q = s.ui_quads(800, 600);
    q.sort_by(|a, b| (b.rect[2] * b.rect[3]).partial_cmp(&(a.rect[2] * a.rect[3])).unwrap());
    for x in q.iter().take(12) {
        eprintln!("{:?} {} color {:?} flags {:#x} uv {:?}", x.rect, x.texture, x.color, x.poly_flags, x.uv);
    }
}

/// Mean colour of the inventory panel textures (to explain the panel brightness against the original).
#[test]
#[ignore]
fn survey_panel_texture_colours() {
    let Some(s) = new_game() else { return };
    for name in ["InventoryBackground_1", "InventoryBackground_2", "InventoryBorder_1", "MaskTexture"] {
        let path = format!("DeusExUI.UserInterface.{name}");
        let Some(m) = s.view.texture_mips_by_path(&path) else { eprintln!("{name}: none"); continue };
        let (w, h, px) = &m[0];
        let n = (*w * *h) as usize;
        let mean = |c: usize| px.chunks(4).take(n).map(|p| p[c] as f32).sum::<f32>() / n as f32;
        eprintln!("{name}: {w}x{h} mean rgba {:.1} {:.1} {:.1} {:.1}", mean(0), mean(1), mean(2), mean(3));
    }
}

/// An item lying in the world is picked up by looking at it and pressing the use key.
#[test]
fn an_item_on_the_ground_is_picked_up_with_the_use_key() {
    let Some(mut s) = new_game() else { return };
    // the nearest pickup lying in the level (not owned by anyone)
    let inv = s.vm.reg.class_by_name("DeusExPickup").unwrap();
    let p = s.player_location();
    let cands: Vec<u32> = s
        .vm
        .live_actors()
        .into_iter()
        .filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, inv)).unwrap_or(false))
        .filter(|&a| matches!(s.vm.get_prop(a, "Owner", 0), Ok(Value::Obj(0))))
        .collect();
    let mut items: Vec<(f32, u32)> = Vec::new();
    for a in cands {
        let l = s.vm.location(a);
        items.push((((l[0] - p[0]).powi(2) + (l[1] - p[1]).powi(2)).sqrt(), a));
    }
    items.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let item = items.iter().map(|x| x.1).find(|&a| {
        let l = s.vm.location(a);
        let l = l;
        (0..8).any(|k| {
            let ang = k as f32 * std::f32::consts::TAU / 8.0;
            let spot = [l[0] + 60.0 * ang.cos(), l[1] + 60.0 * ang.sin(), l[2] + 30.0];
            s.vm.bsp.as_ref().unwrap().fast_line_check(spot, l)
        })
    }).expect("no reachable item");
    let class = s.vm.class_of(item).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
    let l = s.vm.location(item);
    let (spot, yaw) = (0..8)
        .find_map(|k| {
            let ang = k as f32 * std::f32::consts::TAU / 8.0;
            let spot = [l[0] + 60.0 * ang.cos(), l[1] + 60.0 * ang.sin(), l[2] + 30.0];
            s.vm.bsp.as_ref().unwrap().fast_line_check(spot, l).then_some((spot, (ang + std::f32::consts::PI).to_degrees()))
        })
        .unwrap();
    s.teleport(spot, yaw);
    run(&mut s, 10);
    // look down at it from the eye
    let eye = s.player_location();
    let eye_z = eye[2] + s.vm.get_prop(s.player, "BaseEyeHeight", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0);
    let d = ((l[0] - eye[0]).powi(2) + (l[1] - eye[1]).powi(2)).sqrt();
    let pitch = ((l[2] - eye_z).atan2(d) / std::f32::consts::TAU * 65536.0) as i32;
    let r = match s.vm.get_prop(s.player, "ViewRotation", 0) { Ok(Value::Rotator(r)) => r, _ => [0; 3] };
    s.vm.set_prop(s.player, "ViewRotation", 0, Value::Rotator([pitch & 0xFFFF, r[1], 0])).unwrap();
    run(&mut s, 10);
    let ft = match s.vm.get_prop(s.player, "FrobTarget", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    eprintln!("item {class} at distance {d:.0}, frob target is the item: {}", ft == item);
    s.input.key(key_code("F").unwrap(), true);
    run(&mut s, 2);
    s.input.key(key_code("F").unwrap(), false);
    run(&mut s, 5);
    let owner = match s.vm.get_prop(item, "Owner", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    let p = s.player;
    assert!(owner == p || !s.vm.is_live(item), "the {class} was not picked up (owner {owner})");
}

/// Tools that are not weapons (lockpick, multitool) are drawn in the player's hand like weapons (`DeusExPlayer`'s
/// `RenderOverlays` draws the item in hand when it is not a weapon); they were invisible.
#[test]
fn a_lockpick_and_a_multitool_are_drawn_in_hand() {
    let Some(mut s) = new_game() else { return };
    let p = s.player;
    for (class, mesh) in [("Lockpick", "DeusExItems.LockpickPOV"), ("Multitool", "DeusExItems.MultitoolPOV")] {
        let c = s.vm.reg.class_by_name(class).unwrap();
        let loc = s.player_location();
        let item = s.vm.spawn(c, 0, None, Some(loc), None).unwrap().unwrap();
        s.vm.call_by_name(item, "GiveTo", vec![Value::Obj(p)]).unwrap();
        s.vm.call_by_name(p, "PutInHand", vec![Value::Obj(item)]).unwrap();
        run(&mut s, 60);
        let cam = s.camera();
        let overlay: Vec<String> = s.actor_draws(&cam).into_iter().filter(|d| d.overlay).map(|d| d.mesh).collect();
        assert_eq!(overlay, vec![mesh.to_string()], "{class}");
    }
}

/// A screen pauses the game (`ShowMenu` -> `SetPause` sets `Level.Pauser`): while it is up the NPCs stand still and
/// `PlayerTick` does not run; closing it lets the world go on.
#[test]
fn a_screen_pauses_the_world() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 60);
    let pawn = s.vm.reg.class_by_name("ScriptedPawn").unwrap();
    let npcs: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, pawn)).unwrap_or(false)).collect();
    let locs = |s: &mut Session| npcs.iter().map(|&a| s.vm.location(a)).collect::<Vec<_>>();
    assert!(s.exec("ShowInventoryWindow"));
    run(&mut s, 5);
    let pauser = s.vm.get_prop(s.vm.level, "Pauser", 0).unwrap();
    eprintln!("Pauser {pauser:?}");
    assert!(matches!(&pauser, Value::Str(p) if !p.is_empty()), "the screen did not pause the game");
    let before = locs(&mut s);
    run(&mut s, 60);
    assert_eq!(before, locs(&mut s), "NPCs moved while paused");
    // close the screen
    s.ui_key(key_code("Escape").unwrap(), true);
    run(&mut s, 1);
    s.ui_key(key_code("Escape").unwrap(), false);
    run(&mut s, 5);
    let pauser = s.vm.get_prop(s.vm.level, "Pauser", 0).unwrap();
    eprintln!("Pauser after closing {pauser:?}");
    assert!(matches!(&pauser, Value::Str(p) if p.is_empty()), "still paused");
    let resumed = locs(&mut s);
    run(&mut s, 90);
    assert_ne!(resumed, locs(&mut s), "nothing moves after closing the screen");
}

/// Keyboard navigation (`MoveFocusDown`, the root window's arrow keys): in the main menu the focus moves from button
/// to button down the column and wraps around to where it started.
#[test]
fn arrow_keys_move_the_focus_through_the_main_menu() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 10);
    assert!(s.exec("ShowMainMenu"));
    run(&mut s, 10);
    let root = s.vm.get_prop(s.player, "RootWindow", 0).ok().and_then(|v| match v { Value::Obj(o) => Some(o), _ => None }).unwrap_or(0);
    assert_ne!(root, 0);
    let focus = |s: &mut Session| match s.vm.get_prop(root, "focusWindow", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    let down = key_code("Down").unwrap();
    let mut seen = Vec::new();
    for _ in 0..12 {
        s.ui_key(down, true);
        run(&mut s, 1);
        s.ui_key(down, false);
        run(&mut s, 1);
        let f = focus(&mut s);
        let class = s.vm.class_of(f).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
        eprintln!("focus {f} {class} at {:?}", s.vm.ui_window_origin(f));
        seen.push(f);
    }
    let distinct: std::collections::HashSet<u32> = seen.iter().copied().collect();
    assert!(distinct.len() >= 3, "the focus did not move through the buttons: {seen:?}");
    assert!(seen.iter().skip(1).any(|&f| f == seen[0]), "the focus did not wrap around");
}

/// The same views as the original's reference set (`tools/oracle/screens.sh`): the main menu, the Training map's HUD,
/// the inventory, every tab of the in-game screens, the in-game menu, as `s27_ours_*.png`; needs a Vulkan device.
#[test]
#[ignore]
fn shots_like_the_oracle() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    run(&mut s, 60);
    let mut r = None;
    shot(&mut s, &mut r, "s27_ours_menu.png");
    let p = s.player;
    s.vm.call_by_name(p, "StartTrainingMission", vec![]).unwrap();
    run(&mut s, 90);
    // the window loop gives the renderer the new map after a travel
    if s.take_map_changed() {
        if let Some(rr) = r.as_mut() {
            s.reload_vk_world(rr).unwrap();
        }
    }
    shot(&mut s, &mut r, "s27_ours_hud.png");
    assert!(s.exec("ShowInventoryWindow"));
    run(&mut s, 10);
    shot(&mut s, &mut r, "s27_ours_inventory.png");
    // the tabs of the in-game screens, at the oracle's click points (view coordinates)
    for (name, x) in [("Health", 203.0), ("Augs", 255.0), ("Skills", 306.0), ("Goals", 375.0), ("Conversations", 471.0), ("Images", 552.0), ("Logs", 605.0), ("Inventory", 138.0)] {
        click(&mut s, x, 75.0);
        run(&mut s, 10);
        shot(&mut s, &mut r, &format!("s27_ours_screen_{name}.png"));
    }
    r.take().unwrap().destroy();
}

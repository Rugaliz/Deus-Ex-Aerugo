//! Weapon basics beyond firing the pistol: reloading, melee, thrown weapons. Set `DEUSEX_DIR` to the game folder.

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

fn geti(s: &mut Session, o: u32, n: &str) -> i32 {
    s.vm.get_prop(o, n, 0).ok().and_then(|v| v.as_i32()).unwrap_or(-1)
}

fn in_hand(s: &mut Session) -> u32 {
    match s.vm.get_prop(s.player, "inHand", 0) { Ok(Value::Obj(h)) => h, _ => 0 }
}

fn state_of(s: &mut Session, o: u32) -> String {
    s.vm.state_name(o).unwrap_or("").to_string()
}

fn fire(s: &mut Session, frames: usize) {
    s.input.key(key_code("LeftMouse").unwrap(), true);
    run(s, 2);
    s.input.key(key_code("LeftMouse").unwrap(), false);
    run(s, frames);
}

#[test]
fn the_pistol_reloads() {
    let Some(mut s) = new_game() else { return };
    assert!(s.exec("ActivateBelt 1"));
    run(&mut s, 60);
    let w = in_hand(&mut s);
    assert_ne!(w, 0);
    let (clip, reload_count) = (geti(&mut s, w, "ClipCount"), geti(&mut s, w, "ReloadCount"));
    let ammo = |s: &mut Session| {
        let at = match s.vm.get_prop(w, "AmmoType", 0) { Ok(Value::Obj(a)) => a, _ => 0 };
        geti(s, at, "AmmoAmount")
    };
    eprintln!("clip {clip}/{reload_count}, ammo {}", ammo(&mut s));
    // fire three shots
    for _ in 0..3 {
        fire(&mut s, 25);
    }
    let after = geti(&mut s, w, "ClipCount");
    eprintln!("after shots: clip {after}, state {}", state_of(&mut s, w));
    assert!(after > clip, "ClipCount counts shots fired");
    // reload
    s.vm.log.clear();
    assert!(s.exec("ReloadWeapon"));
    run(&mut s, 5);
    eprintln!("state during reload: {}", state_of(&mut s, w));
    run(&mut s, 120);
    let done = geti(&mut s, w, "ClipCount");
    eprintln!("after reload: clip {done}, state {}", state_of(&mut s, w));
    for l in s.vm.log.iter().filter(|l| l.contains("Accessed") || l.contains("not implemented") || l.contains("failed")).take(10) {
        eprintln!("log {l}");
    }
    assert_eq!(done, 0, "the clip was not refilled");
}

fn terrorist_and_spot_at(s: &mut Session, dist: f32) -> (u32, [f32; 3], f32) {
    let tc = s.vm.reg.class_by_name("Terrorist").unwrap();
    let target = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, tc)).unwrap_or(false)).unwrap();
    let t = s.vm.location(target);
    let (spot, yaw) = (0..16)
        .find_map(|k| {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            let p = [t[0] + dist * a.cos(), t[1] + dist * a.sin(), t[2]];
            s.vm.bsp.as_ref().unwrap().fast_line_check(p, t).then_some((p, (a + std::f32::consts::PI).to_degrees()))
        })
        .expect("no spot with a line of sight");
    (target, spot, yaw)
}

#[test]
fn the_riot_prod_hurts_what_it_touches() {
    let Some(mut s) = new_game() else { return };
    let (target, spot, yaw) = terrorist_and_spot_at(&mut s, 45.0);
    s.teleport(spot, yaw);
    let health = |s: &mut Session| geti(s, target, "Health");
    let before = health(&mut s);
    assert!(s.exec("ActivateBelt 2"));
    run(&mut s, 60);
    let w = in_hand(&mut s);
    eprintln!("in hand: {:?}", s.vm.class_of(w).map(|c| s.vm.reg.class(c).path.clone()));
    s.vm.log.clear();
    s.issues.clear();
    fire(&mut s, 40);
    run(&mut s, 30);
    for i in s.issues.iter().take(5) {
        eprintln!("issue {} {}: {}", i.class, i.event, i.error);
    }
    for l in s.vm.log.iter().filter(|l| l.contains("Accessed") || l.contains("not implemented") || l.contains("failed")).take(10) {
        eprintln!("log {l}");
    }
    let after = health(&mut s);
    eprintln!("health {before} -> {after}, target state {:?}", s.vm.state_name(target));
    assert!(after < before || s.vm.state_name(target) != Some("Wandering"), "the prod did nothing");
}

fn give(s: &mut Session, class: &str) -> u32 {
    let c = s.vm.reg.class_by_name(class).unwrap();
    let path = s.vm.reg.class(c).path.clone();
    let cls = Value::Obj(s.vm.reg.obj_id(&path).0);
    let p = s.player;
    let it = match s.vm.call_by_name(p, "Spawn", vec![cls]) { Ok(Value::Obj(o)) if o != 0 => o, other => panic!("spawn {class}: {other:?}") };
    s.vm.call_by_name(it, "GiveTo", vec![Value::Obj(p)]).unwrap();
    // a weapon gets its ammunition as the game's pickup code does
    let _ = s.vm.call_by_name(it, "GiveAmmo", vec![Value::Obj(p)]);
    it
}

fn count_class(s: &mut Session, name: &str) -> usize {
    let Some(c) = s.vm.reg.class_by_name(name) else { return 0 };
    s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).count()
}

#[test]
fn a_thrown_grenade_flies_and_explodes() {
    let Some(mut s) = new_game() else { return };
    let (target, spot, yaw) = terrorist_and_spot_at(&mut s, 250.0);
    s.teleport(spot, yaw);
    let lam = give(&mut s, "WeaponLAM");
    let p = s.player;
    s.vm.call_by_name(p, "PutInHand", vec![Value::Obj(lam)]).unwrap();
    run(&mut s, 60);
    assert_eq!(in_hand(&mut s), lam, "the grenade is not in hand");
    let before = geti(&mut s, target, "Health");
    s.vm.log.clear();
    s.issues.clear();
    s.input.key(key_code("LeftMouse").unwrap(), true);
    run(&mut s, 3);
    s.input.key(key_code("LeftMouse").unwrap(), false);
    let mut seen = 0;
    for f in 0..240 {
        run(&mut s, 1);
        let n = count_class(&mut s, "ThrownProjectile");
        if n > 0 && seen == 0 {
            seen = f;
            eprintln!("projectile exists at frame {f}");
        }
    }
    for i in s.issues.iter().take(5) {
        eprintln!("issue {} {}: {}", i.class, i.event, i.error);
    }
    for l in s.vm.log.iter().filter(|l| l.contains("Accessed") || l.contains("not implemented") || l.contains("failed")).take(10) {
        eprintln!("log {l}");
    }
    let after = if s.vm.is_live(target) { geti(&mut s, target, "Health") } else { -999 };
    eprintln!("target health {before} -> {after}; projectiles left {}", count_class(&mut s, "ThrownProjectile"));
    assert!(seen > 0 || after != before, "no grenade was thrown");
}

fn shot(s: &mut Session, r: &mut Option<dx_vk::renderer::Renderer>, name: &str) {
    let img = s.render_vk(r, 800, 600).expect("vulkan render");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
}

/// Pictures of several weapons in hand, then the pistol while reloading; needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_weapons_in_hand() {
    let Some(mut s) = new_game() else { return };
    let mut r = None;
    let p = s.player;
    for (class, name) in [("WeaponProd", "prod"), ("WeaponCrowbar", "crowbar"), ("WeaponAssaultGun", "assault"), ("WeaponAssaultShotgun", "shotgun"), ("WeaponSword", "sword"), ("WeaponLAM", "lam"), ("WeaponStealthPistol", "stealth")] {
        let w = give(&mut s, class);
        s.vm.call_by_name(p, "PutInHand", vec![Value::Obj(w)]).unwrap();
        run(&mut s, 45);
        shot(&mut s, &mut r, &format!("s10_weapon_{name}.png"));
    }
    assert!(s.exec("ActivateBelt 1"));
    run(&mut s, 60);
    fire(&mut s, 25);
    assert!(s.exec("ReloadWeapon"));
    for i in 0..4 {
        run(&mut s, 8);
        shot(&mut s, &mut r, &format!("s10_reload_{i}.png"));
    }
    r.take().unwrap().destroy();
}

/// `URender::DrawWorld` -> `RenderOverlays(Canvas)` -> `Canvas.DrawActor`: the weapon's own script puts it at the eye
/// (`SetLocation(Owner.Location + CalcDrawOffset())`), the canvas draw stamps it and hides it again; nothing is drawn while
/// zoomed (`DesiredFOV != DefaultFOV`); with the root window's rendering off the scene frame is empty (no world, no
/// weapon, nobody stamped), and a render viewport gives a band of the screen.
#[test]
fn render_overlays_draw_the_weapon_through_the_canvas() {
    let Some(mut s) = new_game() else { return };
    let p = s.player;
    let w = give(&mut s, "WeaponPistol");
    s.vm.call_by_name(p, "PutInHand", vec![Value::Obj(w)]).unwrap();
    run(&mut s, 45);
    let draws: Vec<u32> = s.vm.canvas_draws.iter().map(|d| d.actor).collect();
    assert_eq!(draws, vec![w], "the weapon is drawn through the canvas");
    let off = match s.vm.call_by_name(w, "CalcDrawOffset", vec![]).unwrap() { Value::Vector(v) => v, v => panic!("{v:?}") };
    let (pl, wl) = (s.vm.location(p), s.vm.location(w));
    for k in 0..3 {
        assert!((wl[k] - (pl[k] + off[k])).abs() < 0.5, "weapon at {wl:?}, eye + offset {:?}", [pl[0] + off[0], pl[1] + off[1], pl[2] + off[2]]);
    }
    assert!(s.vm.bp(w, "bHidden"));
    let now = s.vm.fp(s.vm.level, "TimeSeconds");
    assert_eq!(s.vm.fp(w, "LastRenderTime"), now);
    // zoomed in: the weapon's script draws nothing
    let default_fov = s.vm.fp(p, "DefaultFOV");
    s.vm.set_prop(p, "DesiredFOV", 0, Value::Float(default_fov - 20.0)).unwrap();
    run(&mut s, 1);
    assert!(s.vm.canvas_draws.is_empty(), "zoomed: no weapon");
    s.vm.set_prop(p, "DesiredFOV", 0, Value::Float(default_fov)).unwrap();
    run(&mut s, 1);
    assert_eq!(s.vm.canvas_draws.len(), 1);
    // rendering off (menus over a snapshot): no scene frame, no weapon, no stamps
    let root = s.vm.ui_root(p);
    s.vm.call_by_name(root, "EnableRendering", vec![Value::Bool(false)]).unwrap();
    run(&mut s, 2);
    assert!(s.vm.canvas_draws.is_empty());
    let cam = s.camera();
    assert!(s.scene_view(&cam).is_none());
    let before = s.vm.fp(w, "LastRenderTime");
    run(&mut s, 3);
    assert_eq!(s.vm.fp(w, "LastRenderTime"), before, "nothing is stamped while the world is not drawn");
    s.vm.call_by_name(root, "EnableRendering", vec![Value::Bool(true)]).unwrap();
    // a render viewport (root units times the multiplier): a band with the screen's horizontal angle
    s.vm.call_by_name(root, "SetRenderViewport", vec![Value::Float(0.0), Value::Float(100.0), Value::Float(10000.0), Value::Float(200.0)]).unwrap();
    let (c, r) = s.scene_view(&cam).unwrap();
    // the multiplier (1 or more) scales the root units; the width is clipped to the screen
    let m = r[1] / 100;
    assert!(m >= 1 && r[1] == 100 * m, "{r:?}");
    assert_eq!(r, [0, 100 * m, 800, 200 * m]);
    let full = dx_vk::world::hor_plus_tan(cam.fov, 800.0 / 600.0);
    let band = dx_vk::world::hor_plus_tan(c.fov, r[2] as f32 / r[3] as f32);
    assert!((full - band).abs() < 1e-4, "horizontal angle {full} vs {band}");
    s.vm.call_by_name(root, "ResetRenderViewport", vec![]).unwrap();
}

/// Puts the player 140 units in front of the first UNATCO troop with a weapon, facing him; returns the troop.
fn face_a_troop(s: &mut Session) -> u32 {
    let c = s.vm.reg.class_by_name("UNATCOTroop").unwrap();
    let troop = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false) && s.vm.op(a, "Weapon") != 0).expect("an armed troop");
    let t = s.vm.location(troop);
    let yaw = (s.vm.rotation(troop)[1] & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
    let at = [t[0] + 140.0 * yaw.cos(), t[1] + 140.0 * yaw.sin(), t[2]];
    let face = (t[1] - at[1]).atan2(t[0] - at[0]).to_degrees();
    s.teleport(at, face);
    troop
}

/// `DrawActorSprite` draws a pawn's weapon in third person on its weapon triangle (the human meshes' special face):
/// the weapon's `ThirdPersonMesh` at its `ThirdPersonScale`, at the hand, lit as the pawn. NPCs were drawn unarmed.
#[test]
fn npcs_hold_their_weapons_in_third_person() {
    let Some(mut s) = new_game() else { return };
    let troop = face_a_troop(&mut s);
    run(&mut s, 3);
    let w = s.vm.op(troop, "Weapon");
    let tpm = s.vm.op(w, "ThirdPersonMesh");
    let tpm_path = s.vm.reg.obj_path(ue_registry::ObjId(tpm)).to_string();
    let cam = s.camera();
    let draws = s.actor_draws(&cam);
    let t = s.vm.location(troop);
    let held: Vec<_> = draws.iter().filter(|d| d.mesh == tpm_path).collect();
    assert_eq!(held.len(), 1, "the troop's weapon ({tpm_path}) is drawn once");
    let d = held[0];
    let dist = ((d.location[0] - t[0]).powi(2) + (d.location[1] - t[1]).powi(2) + (d.location[2] - t[2]).powi(2)).sqrt();
    assert!(dist < 60.0, "the weapon is at the hand, {dist} units from the pawn's centre");
    assert!((d.scale - s.vm.fp(w, "ThirdPersonScale")).abs() < 1e-6);
    assert!(d.lighting.is_some(), "lit as the pawn");
    assert!(s.vm.bp(w, "bHidden"), "the weapon actor itself stays hidden");
}

/// Picture of a UNATCO troop holding his weapon (`../out/screenshots/s19_npc_weapon.png`); needs a Vulkan device.
#[test]
#[ignore]
fn shot_of_an_npc_holding_a_weapon() {
    let Some(mut s) = new_game() else { return };
    face_a_troop(&mut s);
    run(&mut s, 3);
    let mut r = None;
    shot(&mut s, &mut r, "s19_npc_weapon.png");
    run(&mut s, 60);
    shot(&mut s, &mut r, "s19_npc_weapon_2s.png");
    r.take().unwrap().destroy();
}

/// The item in hand stays fixed in the drawn view while the mouse turns it between fixed steps (user report: a trailing
/// ghost). The original runs `RenderOverlays` with the view it draws; the port keeps the item's pose relative to the
/// step's view and puts it in the drawn one: its angle off the view axis and its yaw relative to the view are the same
/// on every drawn frame (WORKLOG 3.186).
#[test]
fn the_item_in_hand_stays_fixed_in_the_view_during_mouse_look() {
    let Some(mut s) = new_game() else { return };
    let p = s.player;
    let w = give(&mut s, "WeaponPistol");
    s.vm.call_by_name(p, "PutInHand", vec![Value::Obj(w)]).unwrap();
    run(&mut s, 45);
    let (mut angles, mut yaws) = (Vec::new(), Vec::new());
    for _ in 0..240 {
        s.input.axis(0xE4, 8.0);
        s.advance(1.0 / 144.0);
        let cam = s.render_camera();
        let Some((loc, rot)) = s.hand_overlay_poses().first().copied() else { continue };
        let fwd = ue_vm::rotate_vector([1.0, 0.0, 0.0], [cam.pitch, cam.yaw, cam.roll], false);
        let rel = [loc[0] - cam.pos[0], loc[1] - cam.pos[1], loc[2] - cam.pos[2]];
        let l = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
        let c = (rel[0] * fwd[0] + rel[1] * fwd[1] + rel[2] * fwd[2]) / l;
        angles.push(c.clamp(-1.0, 1.0).acos().to_degrees());
        yaws.push(((rot[1] - cam.yaw) as i16) as i32);
    }
    let spread = |v: &[f32]| v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min);
    let ya: Vec<f32> = yaws.iter().map(|&y| y as f32).collect();
    eprintln!("{} frames: angle off the view axis spread {:.3} deg, relative yaw spread {} units", angles.len(), spread(&angles), spread(&ya));
    assert!(angles.len() > 200);
    assert!(spread(&angles) < 0.2, "the item moves in the view: {:.3} degrees", spread(&angles));
    assert!(spread(&ya) <= 2.0, "the item turns against the view: {} units", spread(&ya));
}

/// User report: after a poison dart the weapon in hand vanished for good (it still fired). Poison runs the drug effect
/// (`DeusExPlayer.DrugEffects`: the view sways, `DesiredFOV` swings for about four seconds); the item in hand must be
/// drawn in front of the eye before, during and after it.
#[test]
fn the_item_in_hand_is_drawn_after_poison() {
    let Some(s) = new_game() else { return };
    poison_and_check(s);
}

/// The same with the field of view set to 95 in the menu (`MenuChoice_AerugoFOV`), in the session that set it and in
/// a new one that reads it back from the player's settings.
#[test]
fn the_item_in_hand_is_drawn_after_poison_with_another_field_of_view() {
    let Some(mut s) = new_game() else { return };
    // the menu's own choice: its `SaveSetting` (as the Display screen's OK runs it)
    assert!(s.open_menu_screen("MenuScreenDisplay"));
    run(&mut s, 2);
    let child = |s: &mut Session, w: u32, f: &str| match s.vm.get_prop(w, f, 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    let (mut stack, mut choice) = (vec![s.vm.ui_root(s.player)], 0);
    while let Some(w) = stack.pop() {
        if s.vm.class_of(w).is_some_and(|c| s.vm.reg.class(c).path.ends_with("MenuChoice_AerugoFOV")) {
            choice = w;
            break;
        }
        let mut c = child(&mut s, w, "firstChild");
        while c != 0 {
            stack.push(c);
            c = child(&mut s, c, "nextSibling");
        }
    }
    assert_ne!(choice, 0, "the field of view choice");
    s.vm.call_by_name(choice, "SetValue", vec![Value::Int(4)]).unwrap();
    s.vm.call_by_name(choice, "SaveSetting", vec![]).unwrap();
    s.exec("ShowMainMenu");
    let root = s.vm.ui_root(s.player);
    s.vm.call_by_name(root, "ClearWindowStack", vec![]).unwrap();
    run(&mut s, 2);
    let p = s.player;
    eprintln!("after the menu: instance {:?} {:?}, class defaults {:?} {:?}", s.vm.get_prop(p, "DefaultFOV", 0).ok(), s.vm.get_prop(p, "DesiredFOV", 0).ok(), s.vm.default_value_of("JCDentonMale", "DefaultFOV", 0), s.vm.default_value_of("JCDentonMale", "DesiredFOV", 0));
    eprintln!("-- the session that set it");
    poison_and_check(s);
    eprintln!("-- a new session");
    let s = new_game().unwrap();
    let fovs = |s: &mut Session| (s.vm.get_prop(s.player, "DefaultFOV", 0).ok(), s.vm.get_prop(s.player, "DesiredFOV", 0).ok(), s.vm.default_value_of("JCDentonMale", "DesiredFOV", 0));
    let mut s = s;
    eprintln!("instance DefaultFOV, DesiredFOV, class default DesiredFOV: {:?}", fovs(&mut s));
    poison_and_check(s);
}

fn poison_and_check(mut s: Session) {
    let p = s.player;
    let w = give(&mut s, "WeaponPistol");
    s.vm.call_by_name(p, "PutInHand", vec![Value::Obj(w)]).unwrap();
    run(&mut s, 45);
    let near = |s: &mut Session| -> Option<f32> {
        s.advance(DT);
        let cam = s.render_camera();
        let (loc, _) = s.hand_overlay_poses().first().copied()?;
        let d = [loc[0] - cam.pos[0], loc[1] - cam.pos[1], loc[2] - cam.pos[2]];
        Some((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt())
    };
    let before = near(&mut s).expect("the pistol is drawn before");
    let loc = s.vm.location(p);
    let poison = s.vm.reg.names.intern("Poison");
    s.vm.call_by_name(p, "TakeDamage", vec![Value::Int(5), Value::Obj(0), Value::Vector(loc), Value::Vector([0.0; 3]), Value::Name(poison)]).unwrap();
    let mut log = Vec::new();
    for i in 0..(12 * 30) {
        let d = near(&mut s);
        let fov = s.vm.get_prop(p, "FOVAngle", 0).ok().and_then(|v| v.as_f32());
        if i % 15 == 0 {
            log.push(format!("{:.1}s: hand {d:?} fov {fov:?} desired {:?}", i as f32 * DT, s.vm.get_prop(p, "DesiredFOV", 0).ok()));
        }
    }
    let after = near(&mut s);
    for l in &log {
        eprintln!("{l}");
    }
    eprintln!("before {before}, after {after:?}, in hand {:?}", s.vm.get_prop(p, "inHand", 0).ok());
    let after = after.expect("the pistol is drawn after the poison");
    assert!((after - before).abs() < 2.0, "drawn {after} units from the eye, {before} before");
}

/// The same with a real poison dart (`DartPoison`) fired at the player from a few metres in front.
#[test]
fn the_item_in_hand_is_drawn_after_a_poison_dart() {
    let Some(mut s) = new_game() else { return };
    let p = s.player;
    let w = give(&mut s, "WeaponPistol");
    s.vm.call_by_name(p, "PutInHand", vec![Value::Obj(w)]).unwrap();
    run(&mut s, 45);
    let rot = s.vm.rotation(p);
    let fwd = ue_vm::rotate_vector([1.0, 0.0, 0.0], [0, rot[1], 0], false);
    let loc = s.vm.location(p);
    let from = [loc[0] + fwd[0] * 200.0, loc[1] + fwd[1] * 200.0, loc[2] + 10.0];
    let c = s.vm.reg.class_by_name("DartPoison").unwrap();
    let cls = Value::Obj(s.vm.reg.obj_id(&s.vm.reg.class(c).path.clone()).0);
    // aimed at the eye (the projectile sets its velocity from its rotation)
    let eye = loc[2] + s.vm.get_prop(p, "BaseEyeHeight", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0) - 10.0;
    let from = [from[0], from[1], eye];
    // fired by an NPC (a projectile never hits its own instigator)
    let tc = s.vm.reg.class_by_name("ScriptedPawn").unwrap();
    let npc = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, tc)).unwrap_or(false)).unwrap();
    let dart = match s.vm.call_by_name(npc, "Spawn", vec![cls, Value::Obj(0), Value::Name(0), Value::Vector(from), Value::Rotator([0, (rot[1] + 32768) & 0xFFFF, 0])]) { Ok(Value::Obj(o)) if o != 0 => o, other => panic!("{other:?}") };
    assert!(s.vm.is_live(dart));
    let health = |s: &mut Session| s.vm.get_prop(s.player, "Health", 0).ok().and_then(|v| v.as_i32());
    let h0 = health(&mut s);
    let mut hidden = 0;
    for i in 0..(14 * 30) {
        s.advance(DT);
        let drawn = !s.hand_overlay_poses().is_empty();
        if !drawn {
            hidden += 1;
        }
        if i % 30 == 0 {
            eprintln!("{}s: drawn {drawn} health {:?} desired {:?} default {:?} drug {:?}", i / 30, health(&mut s), s.vm.get_prop(p, "DesiredFOV", 0).ok(), s.vm.get_prop(p, "DefaultFOV", 0).ok(), s.vm.get_prop(p, "drugEffectTimer", 0).ok());
        }
    }
    eprintln!("health {h0:?} -> {:?}, frames hidden {hidden}", health(&mut s));
    assert!(health(&mut s) < h0, "the dart hit");
    assert!(!s.hand_overlay_poses().is_empty(), "the pistol is drawn after the poison");
}

/// A game saved at one field of view and loaded after the menu changed it: the saved player brings its own `DefaultFOV`,
/// the drug effect's reset uses the class default (`Default.DesiredFOV`).
#[test]
fn the_item_in_hand_is_drawn_after_poison_in_a_game_saved_at_another_field_of_view() {
    let Some(mut s) = new_game() else { return };
    s.save_game(0, "fov 75").unwrap();
    let p = s.player;
    s.vm.call_by_name(p, "SetDesiredFOV", vec![Value::Float(95.0)]).unwrap();
    for c in ["DeusExPlayer", "JCDentonMale"] {
        for k in ["DefaultFOV", "DesiredFOV"] {
            s.vm.set_class_default(c, k, 0, Value::Float(95.0)).unwrap();
        }
    }
    let index = (1..100).find(|i| s.vm.save_slot_dir(*i).is_some_and(|d| d.exists())).unwrap();
    s.load_game(index).unwrap();
    run(&mut s, 5);
    let p = s.player;
    eprintln!("after loading: instance DefaultFOV {:?} DesiredFOV {:?}, class default DesiredFOV {:?}", s.vm.get_prop(p, "DefaultFOV", 0).ok(), s.vm.get_prop(p, "DesiredFOV", 0).ok(), s.vm.default_value_of("JCDentonMale", "DesiredFOV", 0));
    poison_and_check(s);
}

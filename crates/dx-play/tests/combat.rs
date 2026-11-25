//! The first mission's weapon: equip the pistol from the belt and fire it. Set `DEUSEX_DIR` to the game folder.

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

#[test]
fn the_pistol_can_be_equipped_and_fired() {
    let Some(mut s) = new_game() else { return };
    s.issues.clear();
    s.vm.log.clear();
    assert!(s.exec("ActivateBelt 1"));
    run(&mut s, 60);
    let hand = match s.vm.get_prop(s.player, "inHand", 0) { Ok(Value::Obj(h)) => h, _ => 0 };
    let class = s.vm.class_of(hand).map(|c| s.vm.reg.class(c).path.clone());
    eprintln!("in hand: {class:?}");
    for i in s.issues.iter().take(10) {
        eprintln!("issue {} {}: {}", i.class, i.event, i.error);
    }
    let ammo = |s: &mut Session| -> (i32, i32) {
        let w = match s.vm.get_prop(s.player, "inHand", 0) { Ok(Value::Obj(h)) => h, _ => 0 };
        let at = match s.vm.get_prop(w, "AmmoType", 0) { Ok(Value::Obj(a)) => a, _ => 0 };
        let n = |s: &mut Session, o: u32, f: &str| s.vm.get_prop(o, f, 0).ok().and_then(|v| v.as_i32()).unwrap_or(-1);
        (n(s, at, "AmmoAmount"), n(s, w, "ClipCount"))
    };
    let before = ammo(&mut s);
    eprintln!("ammo before {before:?}");
    s.input.key(key_code("LeftMouse").unwrap(), true);
    run(&mut s, 30);
    s.input.key(key_code("LeftMouse").unwrap(), false);
    run(&mut s, 30);
    for i in s.issues.iter().take(10) {
        eprintln!("issue {} {}: {}", i.class, i.event, i.error);
    }
    let after = ammo(&mut s);
    eprintln!("ammo after {after:?}");
    assert!(after.0 < before.0 || after.1 > before.1, "firing used no ammo: {before:?} -> {after:?}");
    for l in s.vm.log.iter().take(12) {
        eprintln!("log {l}");
    }
}

/// The first terrorist of the map and a spot 350 units from it with a clear line to it, as (spot, yaw in degrees).
fn terrorist_and_firing_spot(s: &mut Session) -> (u32, [f32; 3], f32) {
    terrorist_and_firing_spot_at(s, 350.0)
}

fn terrorist_and_firing_spot_at(s: &mut Session, dist: f32) -> (u32, [f32; 3], f32) {
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

/// Aims the view at a pawn's chest from `spot`: the eye is level with the top of its head, where an upward spread or
/// kick misses.
fn aim_at_chest(s: &mut Session, target: u32, spot: [f32; 3]) {
    let tl = s.vm.location(target);
    let eye = s.vm.get_prop(s.player, "BaseEyeHeight", 0).ok().and_then(|v| v.as_f32()).unwrap_or(38.0);
    let horiz = ((tl[0] - spot[0]).powi(2) + (tl[1] - spot[1]).powi(2)).sqrt();
    s.pitch = ((tl[2] - (spot[2] + eye)).atan2(horiz).to_degrees() / 360.0 * 65536.0) as i32;
}

fn fire_burst(s: &mut Session) {
    assert!(s.exec("ActivateBelt 1"));
    run(s, 60);
    s.input.key(key_code("LeftMouse").unwrap(), true);
    run(s, 20);
    s.input.key(key_code("LeftMouse").unwrap(), false);
}

fn count_carcasses(s: &mut Session) -> usize {
    s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.reg.class(c).path.contains("Carcass")).unwrap_or(false)).count()
}

#[test]
fn shooting_a_terrorist_hurts_it_and_it_reacts() {
    let Some(mut s) = new_game() else { return };
    let (target, spot, yaw) = terrorist_and_firing_spot_at(&mut s, 150.0);
    aim_at_chest(&mut s, target, spot);
    s.teleport(spot, yaw);
    let health = |s: &mut Session| s.vm.get_prop(target, "Health", 0).ok().and_then(|x| x.as_i32()).unwrap();
    let before = health(&mut s);
    let state_before = s.vm.state_name(target).map(String::from);
    // the pistol's spread is random (one generator for the whole game): up to three bursts
    for _ in 0..3 {
        fire_burst(&mut s);
        run(&mut s, 60);
        if health(&mut s) < before {
            break;
        }
    }
    eprintln!("health {before} -> {}, state {state_before:?} -> {:?}", health(&mut s), s.vm.state_name(target));
    assert!(health(&mut s) < before, "no damage");
    assert_ne!(state_before.as_deref(), s.vm.state_name(target), "the terrorist did not react");
}

#[test]
fn a_dying_terrorist_leaves_a_carcass() {
    let Some(mut s) = new_game() else { return };
    let (target, spot, yaw) = terrorist_and_firing_spot(&mut s);
    // a pawn's Health is recomputed from its body parts when it is hit
    for f in ["Health", "HealthHead", "HealthTorso", "HealthLegLeft", "HealthLegRight", "HealthArmLeft", "HealthArmRight"] {
        s.vm.set_prop(target, f, 0, Value::Int(1)).unwrap();
    }
    let carcasses = count_carcasses(&mut s);
    aim_at_chest(&mut s, target, spot);
    s.teleport(spot, yaw);
    // the untrained pistol's spread is random: up to three bursts
    for _ in 0..3 {
        fire_burst(&mut s);
        run(&mut s, 90);
        if s.vm.state_name(target) == Some("Dying") {
            break;
        }
    }
    assert_eq!(s.vm.state_name(target), Some("Dying"));
    run(&mut s, 300);
    // (others may die in the crossfire too: at least the terrorist's)
    assert!(count_carcasses(&mut s) > carcasses, "the dead terrorist left no carcass");
    let tc = s.vm.reg.class_by_name("TerroristCarcass").unwrap();
    assert!(s.vm.live_actors().into_iter().any(|a| s.vm.class_of(a) == Some(tc)), "no terrorist carcass");
    assert!(!s.vm.is_live(target), "the dead terrorist is still in the world");
}

#[test]
#[ignore]
fn survey_effect_actors() {
    let Some(mut s) = new_game() else { return };
    let (_target, spot, yaw) = terrorist_and_firing_spot(&mut s);
    s.teleport(spot, yaw);
    assert!(s.exec("ActivateBelt 1"));
    run(&mut s, 60);
    let before: std::collections::HashSet<u32> = s.vm.live_actors().into_iter().collect();
    s.input.key(key_code("LeftMouse").unwrap(), true);
    let mut seen = std::collections::BTreeMap::new();
    for _ in 0..12 {
        run(&mut s, 1);
        for a in s.vm.live_actors() {
            if before.contains(&a) { continue; }
            let c = s.vm.class_of(a).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
            let p = |s: &mut Session, f: &str| s.vm.get_prop(a, f, 0).ok();
            let r = |s: &mut Session, f: &str| match s.vm.get_prop(a, f, 0) { Ok(Value::Obj(o)) if o != 0 => s.vm.reg.obj_path(ue_registry::ObjId(o)).to_string(), _ => "-".into() };
            let line = format!("DrawType {:?} Style {:?} Mesh {} Texture {} Scale {:?} Glow {:?} Unlit {:?} LifeSpan {:?}", p(&mut s, "DrawType"), p(&mut s, "Style"), r(&mut s, "Mesh"), r(&mut s, "Texture"), p(&mut s, "DrawScale"), p(&mut s, "ScaleGlow"), p(&mut s, "bUnlit"), p(&mut s, "LifeSpan"));
            seen.entry(c).or_insert(line);
        }
    }
    s.input.key(key_code("LeftMouse").unwrap(), false);
    for (c, l) in seen { eprintln!("{c}: {l}"); }
    let w = match s.vm.get_prop(s.player, "inHand", 0) { Ok(Value::Obj(h)) => h, _ => 0 };
    for f in ["Mesh", "PlayerViewMesh", "PlayerViewOffset", "PlayerViewScale", "ThirdPersonMesh", "MuzzleFlashStyle", "MuzzleFlashMesh", "MuzzleFlashTexture", "FlashTime", "bMuzzleFlash", "DrawType", "bHidden"] {
        eprintln!("weapon {f}: {:?}", s.vm.get_prop(w, f, 0));
    }
}

#[test]
#[ignore]
fn survey_effect_classes() {
    let Some(mut s) = new_game() else { return };
    let loc = s.vm.location(s.player);
    for name in ["Shadow", "SmokeTrail", "Spark", "MuzzleFlash", "BulletHole", "BloodSplat", "RockChip", "Tracer", "ShellCasing", "FleshFragment", "WoodFragment", "Rockchip", "ParticleGenerator", "SpoofedExplosion", "ExplosionLight"] {
        let Some(c) = s.vm.reg.class_by_name(name) else { eprintln!("{name}: no class"); continue };
        let Ok(Some(a)) = s.vm.spawn(c, 0, None, Some(loc), None) else { eprintln!("{name}: spawn failed"); continue };
        let p = |s: &mut Session, f: &str| s.vm.get_prop(a, f, 0).ok();
        let r = |s: &mut Session, f: &str| match s.vm.get_prop(a, f, 0) { Ok(Value::Obj(o)) if o != 0 => s.vm.reg.obj_path(ue_registry::ObjId(o)).to_string(), _ => "-".into() };
        eprintln!("{name}: DrawType {:?} Style {:?} Mesh {} Texture {} Scale {:?} Glow {:?} Unlit {:?} Life {:?} LightType {:?} Radius {:?} Bright {:?} Hidden {:?} Physics {:?}", p(&mut s, "DrawType"), p(&mut s, "Style"), r(&mut s, "Mesh"), r(&mut s, "Texture"), p(&mut s, "DrawScale"), p(&mut s, "ScaleGlow"), p(&mut s, "bUnlit"), p(&mut s, "LifeSpan"), p(&mut s, "LightType"), p(&mut s, "LightRadius"), p(&mut s, "LightBrightness"), p(&mut s, "bHidden"), p(&mut s, "Physics"));
    }
}

fn shot(s: &mut Session, r: &mut Option<dx_vk::renderer::Renderer>, name: &str) {
    let img = s.render_vk(r, 800, 600).expect("vulkan render");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
}

/// Pictures of a firefight for a human to look at (`../out/screenshots/s9_fire_*.png`); needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_the_firefight() {
    let Some(mut s) = new_game() else { return };
    let (_target, spot, yaw) = terrorist_and_firing_spot_at(&mut s, 130.0);
    s.teleport(spot, yaw);
    let mut r = None;
    assert!(s.exec("ActivateBelt 1"));
    run(&mut s, 60);
    shot(&mut s, &mut r, "s9_fire_0_idle.png");
    s.input.key(key_code("LeftMouse").unwrap(), true);
    for i in 1..=8 {
        run(&mut s, 1);
        let cam = s.camera();
        for d in s.actor_draws(&cam).iter().filter(|d| !d.overlay && (d.mesh.contains("Blood") || d.mesh.contains("Tracer") || d.sprite.is_some() || d.mesh.contains("FlatFX"))) {
            eprintln!("f{i} {} sprite {:?} at {:?} scale {} style {} glow {} frames {} {} {}", d.mesh, d.sprite, d.location, d.scale, d.style, d.glow, d.frame_a, d.frame_b, d.blend);
        }
        shot(&mut s, &mut r, &format!("s9_fire_{i}.png"));
    }
    s.input.key(key_code("LeftMouse").unwrap(), false);
    run(&mut s, 30);
    shot(&mut s, &mut r, "s9_fire_5_after.png");
    r.take().unwrap().destroy();
}

/// Stands the player 100 units in front of the nearest wall (found from the terrorist's firing spot), facing it.
fn face_nearest_wall(s: &mut Session) {
    let (_t, spot, _yaw) = terrorist_and_firing_spot_at(s, 130.0);
    // the nearest wall in any direction (a clear segment of 80 units first), then stand 90 units in front of it
    let bsp = s.vm.bsp.as_ref().unwrap();
    let mut best: Option<(f32, f32)> = None;
    for deg in (0..360).step_by(5) {
        let a = (deg as f32).to_radians();
        let at = |d: f32| [spot[0] + d * a.cos(), spot[1] + d * a.sin(), spot[2]];
        if let Some(d) = (1..80).map(|k| k as f32 * 25.0).find(|&d| !bsp.fast_line_check(spot, at(d))) {
            if d > 200.0 && best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, deg as f32));
            }
        }
    }
    let (d, dir) = best.expect("no wall around");
    let a = dir.to_radians();
    let stand = [spot[0] + (d - 100.0) * a.cos(), spot[1] + (d - 100.0) * a.sin(), spot[2]];
    eprintln!("wall {d} units away at {dir} deg");
    s.teleport(stand, dir);
}

/// Shots at a wall: smoke, sparks, chips, bullet hole. Pictures in `../out/screenshots/s9_wall_*.png`.
#[test]
#[ignore]
fn shots_of_wall_hits() {
    let Some(mut s) = new_game() else { return };
    face_nearest_wall(&mut s);
    let mut r = None;
    assert!(s.exec("ActivateBelt 1"));
    run(&mut s, 60);
    s.input.key(key_code("LeftMouse").unwrap(), true);
    for i in 1..=12 {
        run(&mut s, 1);
        if i == 6 {
            s.input.key(key_code("LeftMouse").unwrap(), false);
        }
        let cam = s.camera();
        for d in s.actor_draws(&cam).iter().filter(|d| !d.overlay && (d.sprite.is_some() || d.mesh.contains("FlatFX") || d.mesh.contains("Tracer"))) {
            eprintln!("f{i} {} {:?} at {:?} scale {} style {} glow {}", d.mesh, d.sprite, d.location, d.scale, d.style, d.glow);
        }
        shot(&mut s, &mut r, &format!("s9_wall_{i}.png"));
    }
    r.take().unwrap().destroy();
}

#[test]
fn a_bullet_hole_stays_on_the_wall() {
    let Some(mut s) = new_game() else { return };
    // the shots draw the terrorists' fire: the view must stay where it is for the half minute
    let all = s.vm.name_value("All");
    let p = s.player;
    s.vm.set_prop(p, "ReducedDamageType", 0, all).unwrap();
    face_nearest_wall(&mut s);
    fire_burst(&mut s);
    run(&mut s, 90);
    let hole = s.vm.reg.class_by_name("BulletHole").unwrap();
    let holes: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, hole)).unwrap_or(false)).collect();
    assert!(!holes.is_empty(), "no bullet hole was made");
    assert!(holes.iter().any(|h| s.vm.decals.contains_key(h)), "the bullet hole is not attached to a surface");
    // and it is handed to the renderer as a decal lying on the wall
    let cam = s.camera();
    assert!(s.actor_draws(&cam).iter().any(|d| d.decal.is_some()), "no decal is drawn");
    // `AttachDecal`: a square centred on the hit point, corners on the surface's plane at
    // sqrt(DrawScale^2 * USize^2 / 2) from the centre, the touched nodes listed
    let h = *holes.iter().find(|h| s.vm.decals.contains_key(h)).unwrap();
    let place = s.vm.decals[&h].clone();
    let model = s.vm.world_model.clone().unwrap();
    let e = &place.surfs[0];
    let n = model.vectors[model.surfs[e.surf].v_normal as usize];
    let base = model.points[model.surfs[e.surf].p_base as usize];
    let tex = s.vm.op(h, "Texture");
    let (u, _) = s.vm.texture_size(tex);
    let scale = s.vm.fp(h, "DrawScale");
    let half = (scale * scale * u * u * 0.5).sqrt();
    for v in &e.verts {
        let off = (v[0] - base[0]) * n[0] + (v[1] - base[1]) * n[1] + (v[2] - base[2]) * n[2];
        assert!(off.abs() < 0.01, "corner off the plane by {off}");
        let r = ((v[0] - place.center[0]).powi(2) + (v[1] - place.center[1]).powi(2) + (v[2] - place.center[2]).powi(2)).sqrt();
        assert!((r - half).abs() < 0.01, "corner {r} from the centre, expected {half}");
    }
    assert!(!e.nodes.is_empty(), "no node touched");
    // `ClipDecal`: the pieces lie 0.75 off the surface, with texel coordinates inside the square (0..USize)
    let pieces = s.vm.decal_pieces(h);
    assert!(!pieces.is_empty(), "no piece to draw");
    for (p, t) in pieces.iter().flatten() {
        let off = (p[0] - base[0]) * n[0] + (p[1] - base[1]) * n[1] + (p[2] - base[2]) * n[2];
        assert!((off - 0.75).abs() < 0.01, "piece {off} off the surface");
        assert!(t.iter().all(|&x| x > -0.01 && x < u + 0.01), "texel {t:?} outside 0..{u}");
    }
    // seen all the time, it is stamped every frame (`LastRenderedTime`) and so outlives `DeusExDecal`'s 18-23 s check
    run(&mut s, 30 * 30);
    assert!(s.vm.is_live(h), "the bullet hole in view was destroyed");
    let now = s.vm.fp(s.vm.level, "TimeSeconds");
    assert!(now - s.vm.fp(h, "LastRenderedTime") < 0.1);
}

/// A UNATCO trooper provoked by the player turns on him and keeps fighting: its `Attacking` state code keeps running (a
/// `GotoState` made inside `BeginState` used to be overwritten by the outer one, and the missing `StrafeFacing` native
/// stopped the code), it moves and it fires. A trooper allows one provocation (`MaxProvocations` 1: the first hit only
/// raises its agitation to 0.5, and a fearful pawn flees), so it is hit twice, the agitation check timer (1.5 s) apart.
#[test]
fn a_hurt_trooper_fights_back() {
    let Some(mut s) = new_game() else { return };
    let tc = s.vm.reg.class_by_name("UNATCOTroop").unwrap();
    let target = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, tc)).unwrap_or(false)).unwrap();
    let p = s.player;
    let shot = s.vm.reg.names.intern("Shot");
    let face = |s: &mut Session| {
        let t = s.vm.location(target);
        let (spot, yaw) = (0..16)
            .find_map(|k| {
                let a = k as f32 * std::f32::consts::TAU / 16.0;
                let p = [t[0] + 400.0 * a.cos(), t[1] + 400.0 * a.sin(), t[2]];
                s.vm.bsp.as_ref().unwrap().fast_line_check(p, t).then_some((p, (a + std::f32::consts::PI).to_degrees()))
            })
            .expect("a spot with a line of sight");
        s.teleport(spot, yaw);
        t
    };
    for _ in 0..2 {
        let t = face(&mut s);
        s.vm.call_by_name(target, "TakeDamage", vec![Value::Int(15), Value::Obj(p), Value::Vector([t[0], t[1], t[2] + 10.0]), Value::Vector([0.0; 3]), Value::Name(shot)]).unwrap();
        run(&mut s, 50);
    }
    assert_eq!(s.vm.pawn_alliance_type(target, p), 2, "two provocations should make the trooper hostile");
    // god mode (`ReducedDamageType = 'All'`): whether the trooper's shots kill the player depends on luck, and a dead
    // player ends the attack
    let all = s.vm.reg.names.intern("All");
    s.vm.set_prop(p, "ReducedDamageType", 0, Value::Name(all)).unwrap();
    face(&mut s);
    let weapon = match s.vm.get_prop(target, "Weapon", 0) { Ok(Value::Obj(w)) => w, _ => 0 };
    let clip = |s: &mut Session| s.vm.get_prop(weapon, "ClipCount", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0);
    let (clip0, loc0) = (clip(&mut s), s.vm.location(target));
    // whether it strafes or stands and shoots is random: watch it for 12 s
    let mut attacking_with_code = 0;
    let mut fired = 0;
    for _ in 0..24 {
        for _ in 0..30 {
            let c0 = clip(&mut s);
            run(&mut s, 1);
            let c1 = clip(&mut s);
            if c1 > c0 {
                fired += c1 - c0;
            }
        }
        if std::env::var_os("DX_DEBUG_TROOP").is_some() {
            let (tl, pl) = (s.vm.location(target), s.player_location());
            let d = ((tl[0]-pl[0]).powi(2)+(tl[1]-pl[1]).powi(2)).sqrt();
            eprintln!("dist {d:.0} los {}", s.vm.bsp.as_ref().unwrap().fast_line_check(tl, pl));
        }
        let exec = s.vm.debug_state_exec(target);
        if std::env::var_os("DX_DEBUG_TROOP").is_some() {
            let en = s.vm.get_prop(target, "Enemy", 0).unwrap();
            eprintln!("state {:?} enemy {en:?} orders {:?} exec {exec}", s.vm.state_name(target), s.vm.get_prop(target, "Orders", 0));
        }
        if s.vm.state_name(target) == Some("Attacking") && !exec.contains("no state code running") {
            attacking_with_code += 1;
        }
    }
    let moved = { let l = s.vm.location(target); ((l[0] - loc0[0]).powi(2) + (l[1] - loc0[1]).powi(2)).sqrt() };
    let _ = clip0;
    let shots = fired;
    eprintln!("half-seconds attacking with code: {attacking_with_code}/24, moved {moved:.0}, shots {shots}, missing natives {:?}", s.vm.missing_natives);
    assert!(attacking_with_code >= 20, "the attack code stopped");
    // strafing or standing to shoot is the trooper's random choice: movement is reported, not required
    assert!(shots >= 1, "the trooper never fired");
}

/// A particle generator in front of the player: once the renderer has seen it (`actor_draws` makes its render interface
/// and stamps its proxy) it spews particles that are drawn as the generator's proxy, one per live particle.
#[test]
fn a_particle_generator_spews_particles_that_are_drawn() {
    let Some(mut s) = new_game() else { return };
    let cam = s.camera();
    let yaw = (cam.yaw & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
    let at = [cam.pos[0] + yaw.cos() * 300.0, cam.pos[1] + yaw.sin() * 300.0, cam.pos[2]];
    let class = s.vm.reg.class_by_name("ParticleGenerator").unwrap();
    let g = s.vm.spawn(class, 0, None, Some(at), None).unwrap().unwrap();
    for (n, v) in [("frequency", Value::Float(1.0)), ("checkTime", Value::Float(0.05)), ("particleLifeSpan", Value::Float(2.0)), ("ejectSpeed", Value::Float(60.0)), ("numPerSpawn", Value::Int(2)), ("riseRate", Value::Float(30.0)), ("particleDrawScale", Value::Float(1.0))] {
        s.vm.set_prop(g, n, 0, v).unwrap();
    }
    s.vm.set_prop(g, "bSpewing", 0, Value::Bool(true)).unwrap();
    for _ in 0..60 {
        let cam = s.camera();
        let _ = s.actor_draws(&cam);
        run(&mut s, 1);
    }
    let cam = s.camera();
    let draws = s.actor_draws(&cam);
    let sprites = draws.iter().filter(|d| d.sprite.is_some() && !d.overlay).count();
    assert!(sprites >= 3, "the generator's particles are not drawn ({sprites} sprites)");
    // the renderer draws (and stamps) the proxy, once per particle; the generator itself has no sprite
    let proxy = s.vm.op(g, "proxy");
    assert!(s.vm.fp(proxy, "LastRenderTime") > 0.0, "the proxy was never stamped as rendered");
}


/// A poison barrel broken by the player's first shot fills the place with gas that belongs to the player, so the
/// people in it are hurt and react: pain, anger or flight (`ScriptedPawn.ReactToInjury` needs an instigator). In the
/// original the gas belonged to nobody (`Barrel1.TakeDamage` hands its clouds the barrel's `Instigator` before
/// `Super.TakeDamage` sets it), so nobody reacted; Aerugo repairs that (`Vm::port_repair`).
#[test]
fn people_in_a_poison_barrels_gas_are_hurt_and_react() {
    let Some(mut s) = new_game() else { return };
    let tc = s.vm.reg.class_by_name("Terrorist").unwrap();
    let t = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, tc)).unwrap_or(false)).unwrap();
    let loc = s.vm.location(t);
    let bc = s.vm.reg.class_by_name("Barrel1").unwrap();
    let barrel = s.vm.spawn(bc, 0, None, Some([loc[0] + 60.0, loc[1], loc[2]]), None).unwrap().unwrap();
    // a poison barrel (`SC_Poison`), set up as its `BeginPlay` does for the skin
    s.vm.set_prop(barrel, "SkinColor", 0, Value::Byte(7)).unwrap();
    s.vm.send_event(barrel, "BeginPlay", vec![]).unwrap();
    let health = |s: &mut Session| s.vm.get_prop(t, "Health", 0).ok().and_then(|v| v.as_i32()).unwrap();
    let h0 = health(&mut s);
    let p = s.player;
    let (at, shot) = (s.vm.location(barrel), s.vm.reg.names.get("Shot").unwrap());
    s.vm.send_event(barrel, "TakeDamage", vec![Value::Int(100), Value::Obj(p), Value::Vector(at), Value::Vector([0.0; 3]), Value::Name(shot)]).unwrap();
    assert!(!s.vm.is_live(barrel) || s.vm.get_prop(barrel, "bDeleteMe", 0).ok() == Some(Value::Bool(true)), "the barrel broke");
    let gc = s.vm.reg.class_by_name("PoisonGas").unwrap();
    let clouds: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a) == Some(gc)).collect();
    assert!(!clouds.is_empty(), "the barrel let out gas");
    for &c in &clouds {
        assert_eq!(s.vm.get_prop(c, "Instigator", 0).unwrap(), Value::Obj(p), "the gas belongs to the player");
    }
    let mut states = Vec::new();
    for _ in 0..90 {
        s.step_bound(DT);
        states.push(s.vm.state_name(t).unwrap_or("-").to_string());
    }
    assert!(health(&mut s) < h0, "the gas hurt him ({h0} -> {})", health(&mut s));
    assert!(states.iter().any(|st| st != "Standing" && st != "Wandering" && st != "Patrolling"), "he reacted: {states:?}");
}

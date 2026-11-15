//! Player movement details: crouching (Duck is LControl in the shipped bindings). Set `DEUSEX_DIR` to the game folder.

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

fn f(s: &mut Session, name: &str) -> f32 {
    s.vm.get_prop(s.player, name, 0).ok().and_then(|v| v.as_f32()).unwrap_or(f32::NAN)
}

fn b(s: &mut Session, name: &str) -> bool {
    matches!(s.vm.get_prop(s.player, name, 0), Ok(Value::Bool(true)))
}

/// Distance walked forward in `frames` frames with `W` held.
fn walk(s: &mut Session, frames: usize) -> f32 {
    let a = s.player_location();
    s.input.key(key_code("W").unwrap(), true);
    run(s, frames);
    s.input.key(key_code("W").unwrap(), false);
    let b = s.player_location();
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

#[test]
fn the_player_crouches_while_the_duck_key_is_held() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 30);
    let (h0, eye0, z0) = (f(&mut s, "CollisionHeight"), f(&mut s, "EyeHeight"), s.player_location()[2]);
    let speed_standing = walk(&mut s, 15);
    run(&mut s, 15);
    s.input.key(key_code("LControl").unwrap(), true);
    run(&mut s, 30);
    let (h1, eye1, z1) = (f(&mut s, "CollisionHeight"), f(&mut s, "EyeHeight"), s.player_location()[2]);
    let crouching = b(&mut s, "bIsCrouching");
    let anim = s.vm.get_prop(s.player, "AnimSequence", 0);
    eprintln!("DBG bDuck {:?} phys {:?} anim {anim:?} state {:?}", s.vm.get_prop(s.player, "bDuck", 0), s.vm.get_prop(s.player, "Physics", 0), s.vm.state_name(s.player));
    let speed_crouched = walk(&mut s, 15);
    s.input.key(key_code("LControl").unwrap(), false);
    run(&mut s, 30);
    let (h2, eye2, z2) = (f(&mut s, "CollisionHeight"), f(&mut s, "EyeHeight"), s.player_location()[2]);
    eprintln!("standing: height {h0} eye {eye0:.1} z {z0:.1} walk {speed_standing:.0}");
    eprintln!("crouched ({crouching}): height {h1} eye {eye1:.1} z {z1:.1} walk {speed_crouched:.0}");
    eprintln!("stood up: height {h2} eye {eye2:.1} z {z2:.1}; log {:?}", s.vm.log);
    assert!(crouching, "bIsCrouching not set");
    assert!(h1 < h0 * 0.8, "collision height did not shrink: {h0} -> {h1}");
    assert!(z1 < z0 - 5.0, "the centre did not move down with the smaller cylinder: {z0} -> {z1}");
    assert!(eye1 < eye0 - 5.0, "the eye did not go down: {eye0} -> {eye1}");
    assert!(speed_crouched < speed_standing * 0.6, "crouched walk {speed_crouched} vs {speed_standing}");
    assert!((h2 - h0).abs() < 0.5 && (z2 - z0).abs() < 2.0, "did not stand up: height {h2}, z {z2}");
}

/// The camera comes from the game's `PlayerCalcView`: walking bobs it up and down (by `Bob`), standing still does not.
#[test]
fn the_view_bobs_while_walking() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 30);
    let bob = f(&mut s, "Bob");
    let heights = |s: &mut Session, walk: bool| {
        let mut zs = Vec::new();
        if walk {
            s.input.key(key_code("W").unwrap(), true);
        }
        for _ in 0..30 {
            run(s, 1);
            zs.push(s.camera().pos[2] - s.player_location()[2]);
        }
        s.input.key(key_code("W").unwrap(), false);
        let lo = zs.iter().cloned().fold(f32::MAX, f32::min);
        let hi = zs.iter().cloned().fold(f32::MIN, f32::max);
        hi - lo
    };
    let still = heights(&mut s, false);
    let walking = heights(&mut s, true);
    eprintln!("Bob {bob}: camera height range standing {still:.2}, walking {walking:.2}");
    assert!(still < 0.5, "the view moves while standing: {still}");
    if bob > 0.0 {
        assert!(walking > 1.0, "no head bob while walking: {walking}");
    }
}

fn health(s: &mut Session) -> [i32; 6] {
    let mut h = [0; 6];
    for (k, n) in ["HealthHead", "HealthTorso", "HealthArmLeft", "HealthArmRight", "HealthLegLeft", "HealthLegRight"].iter().enumerate() {
        h[k] = s.vm.get_prop(s.player, n, 0).ok().and_then(|v| v.as_i32()).unwrap_or(-1);
    }
    h
}

/// A long fall hurts the legs and torso by the script's rule (`-0.16 * (vz + 700)` per leg, `-0.06 * (vz + 700)`
/// torso, which `TakeDamage` doubles), a short one does not; landing plays a footstep sound.
#[test]
fn falling_far_hurts_and_landing_makes_a_sound() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 30);
    let start = s.player_location();
    for (height, hurt) in [(100.0, false), (900.0, true)] {
        let before = health(&mut s);
        s.teleport([start[0], start[1], start[2] + height], 0.0);
        s.vm.ai.sounds.clear();
        let mut impact = 0.0f32;
        for _ in 0..120 {
            let vz = match s.vm.get_prop(s.player, "Velocity", 0) {
                Ok(Value::Vector(v)) => v[2],
                _ => 0.0,
            };
            run(&mut s, 1);
            let phys = s.vm.get_prop(s.player, "Physics", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0);
            if phys == 1 {
                impact = vz;
                break;
            }
        }
        run(&mut s, 5);
        let after = health(&mut s);
        let sounds: Vec<String> = s.vm.ai.sounds.iter().map(|r| s.vm.reg.obj_path(ue_registry::ObjId(r.sound)).to_string()).collect();
        eprintln!("fall {height}: impact {impact:.0}, health {before:?} -> {after:?}, sounds {sounds:?}");
        assert!(sounds.iter().any(|n| n.to_ascii_lowercase().contains("step")), "no footstep on landing: {sounds:?}");
        let lost: i32 = before.iter().zip(after.iter()).map(|(a, b)| a - b).sum();
        if hurt {
            assert!(impact < -700.0, "the fall was not fast enough: {impact}");
            // per leg 0.16 * (|vz| - 700), torso 0.06 * (|vz| - 700) counted twice by `DeusExPlayer.TakeDamage`
            // (health stops at 0)
            let leg = (0.16 * (-impact - 700.0)) as i32;
            let torso = 2 * (0.06 * (-impact - 700.0)) as i32;
            assert!((before[5] - after[5] - leg.min(before[5])).abs() <= 2, "right leg lost {} for {leg}", before[5] - after[5]);
            assert!((before[1] - after[1] - torso.min(before[1])).abs() <= 3, "torso lost {} for {torso}", before[1] - after[1]);
        } else {
            assert_eq!(lost, 0, "a short fall hurt");
        }
        // heal for the next drop
        s.exec("RestoreAllHealth");
        run(&mut s, 2);
    }
}

/// `ShakeView` (explosions) shakes the camera up and down (`ShakeVert`) and rolls it, then it settles.
#[test]
fn shake_view_moves_the_camera() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 30);
    let p = s.player;
    let rest = s.camera().pos[2] - s.player_location()[2];
    s.vm.call_by_name(p, "ShakeView", vec![Value::Float(1.0), Value::Float(500.0), Value::Float(8.0)]).unwrap();
    let mut range = (f32::MAX, f32::MIN);
    let mut max_roll = 0;
    for _ in 0..30 {
        run(&mut s, 1);
        let cam = s.camera();
        let h = cam.pos[2] - s.player_location()[2];
        range = (range.0.min(h), range.1.max(h));
        let r = cam.roll & 0xFFFF;
        max_roll = max_roll.max(r.min(65536 - r));
    }
    run(&mut s, 60);
    let after = s.camera().pos[2] - s.player_location()[2];
    eprintln!("eye {rest:.1}; shaking {:.1}..{:.1}, roll up to {max_roll}; after {after:.1}, roll {}", range.0, range.1, s.camera().roll);
    assert!(range.1 - range.0 > 4.0, "the view did not shake");
    assert!(max_roll > 100, "the view did not roll");
    let r = s.camera().roll & 0xFFFF;
    assert!(r.min(65536 - r) < 10, "the roll did not settle: {r}");
    assert!((after - rest).abs() < 1.0, "the view did not settle");
}

/// Holding `E` leans right: the camera moves sideways and rolls; letting go comes back.
#[test]
fn leaning_moves_and_rolls_the_view() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 30);
    let c0 = s.camera();
    s.input.key(key_code("E").unwrap(), true);
    run(&mut s, 30);
    let c1 = s.camera();
    let shot = std::env::var_os("DX_SHOTS").is_some();
    if shot {
        let mut r = None;
        let img = s.render_vk(&mut r, 800, 600).expect("vulkan render");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
        std::fs::write(dir.join("s12_lean_right.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
        r.take().unwrap().destroy();
    }
    s.input.key(key_code("E").unwrap(), false);
    run(&mut s, 60);
    let c2 = s.camera();
    let side = |c: &dx_soft::Camera| {
        let (_, right, _) = c0.axes();
        (c.pos[0] - c0.pos[0]) * right[0] + (c.pos[1] - c0.pos[1]) * right[1]
    };
    let roll = |c: &dx_soft::Camera| {
        let r = c.roll & 0xFFFF;
        if r > 32768 { r - 65536 } else { r }
    };
    eprintln!("lean right: side {:.1}, roll {}; back: side {:.1}, roll {}", side(&c1), roll(&c1), side(&c2), roll(&c2));
    assert!(side(&c1) > 10.0, "the eye did not move right");
    assert!(roll(&c1).abs() > 500, "the view did not roll");
    assert!(side(&c2).abs() < 5.0 && roll(&c2).abs() < 20, "did not come back");
}

/// `FindStairRotation` (the `bLookUpStairs` option): on level ground the view pitch eases back to level, by
/// `min(1, 8 * dt)` of the gap for gaps of 1000 and more; a frame longer than 0.33 s leaves it alone.
#[test]
fn stair_look_eases_the_view_back_to_level_on_flat_ground() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 30);
    let set_pitch = |s: &mut Session, p: i32| {
        let r = match s.vm.get_prop(s.player, "ViewRotation", 0) { Ok(Value::Rotator(r)) => r, _ => [0; 3] };
        s.vm.set_prop(s.player, "ViewRotation", 0, Value::Rotator([p, r[1], r[2]])).unwrap();
    };
    set_pitch(&mut s, 3000);
    assert_eq!(s.vm.find_stair_rotation(s.player, 0.5).unwrap(), 3000);
    let p = s.vm.find_stair_rotation(s.player, 0.1).unwrap();
    // 0.1 as a float is a hair above 0.1: 3000 * (1 - 0.8000000119) = 599.99996, truncated
    assert_eq!(p, 599, "3000 eased by 0.8 towards 0");
    // a pitch above 32768 is a downward look: folded to negative first
    set_pitch(&mut s, 65536 - 2000);
    let p = s.vm.find_stair_rotation(s.player, 0.05).unwrap();
    assert_eq!(p, -1199, "-2000 eased by 0.4 towards 0 (truncated towards zero)");
}

/// `PickAnyTarget` (auto-aim fallback): a projectile target that is not a pawn, straight ahead in line of sight.
#[test]
fn pick_any_target_finds_a_decoration_ahead() {
    let Some(mut s) = new_game() else { return };
    let pawn = s.vm.reg.class_by_name("Pawn").unwrap();
    let deco = s.vm.reg.class_by_name("Decoration").unwrap();
    let eye = |s: &mut Session| {
        let l = s.player_location();
        let e = f(s, "BaseEyeHeight");
        [l[0], l[1], l[2] + e]
    };
    let targets: Vec<u32> = s
        .vm
        .live_actors()
        .into_iter()
        .filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, deco) && !s.vm.is_a(c, pawn)).unwrap_or(false))
        .filter(|&a| matches!(s.vm.get_prop(a, "bProjTarget", 0), Ok(Value::Bool(true))))
        .collect();
    assert!(!targets.is_empty(), "no decoration is a projectile target");
    for t in targets {
        let tl = s.vm.location(t);
        // a spot 150 units away with a clear view of it
        let Some((spot, yaw)) = (0..16).find_map(|k| {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            let p = [tl[0] + 150.0 * a.cos(), tl[1] + 150.0 * a.sin(), tl[2] + 20.0];
            s.vm.bsp.as_ref().unwrap().fast_line_check(p, tl).then_some((p, (a + std::f32::consts::PI).to_degrees()))
        }) else { continue };
        s.teleport(spot, yaw);
        run(&mut s, 3);
        let start = eye(&mut s);
        let d = [tl[0] - start[0], tl[1] - start[1], tl[2] - start[2]];
        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let dir = [d[0] / n, d[1] / n, d[2] / n];
        let (got, aim, dist) = s.vm.pick_any_target(s.player, 0.9, 0.0, dir, start);
        if got == 0 {
            continue; // the line of sight from the eye is blocked here; try the next one
        }
        eprintln!("picked {got} (wanted {t}) aim {aim:.3} dist {dist:.0}");
        assert!(aim > 0.9 && dist > 0.0 && dist < 2000.0);
        assert!(!s.vm.class_of(got).map(|c| s.vm.is_a(c, pawn)).unwrap_or(true), "a pawn was picked");
        return;
    }
    panic!("no decoration could be picked");
}

/// On real geometry the stair look finds both directions: standing at the map's path nodes and facing 8 ways, some
/// spots give the up-stairs pitch (5400) and some the down-stairs pitch (-5000) when eased fully (dt 0.3).
#[test]
fn stair_look_finds_stairs_up_and_down() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 30);
    let node = s.vm.reg.class_by_name("PathNode").unwrap();
    let nodes: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, node)).unwrap_or(false)).collect();
    let (mut up, mut down) = (0, 0);
    for &n in nodes.iter().take(400) {
        let l = s.vm.location(n);
        for k in 0..8 {
            let _ = s.vm.call_by_name(s.player, "SetLocation", vec![Value::Vector(l)]);
            s.vm.set_prop(s.player, "ViewRotation", 0, Value::Rotator([0, k * 8192, 0])).unwrap();
            match s.vm.find_stair_rotation(s.player, 0.3).unwrap() {
                5400 => up += 1,
                -5000 => down += 1,
                _ => {}
            }
        }
    }
    eprintln!("{} nodes: {up} looks up, {down} looks down", nodes.len());
    assert!(up > 0 && down > 0);
}

/// `stepUp` (Deus Ex): a player walking straight into a pushable decoration does not step over it but is slowed by
/// the mass ratio, and the decoration's `Bump` pushes it along.
#[test]
fn walking_into_a_pushable_decoration_pushes_it() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 30);
    let deco = s.vm.reg.class_by_name("DeusExDecoration").unwrap();
    let candidates: Vec<u32> = s
        .vm
        .live_actors()
        .into_iter()
        .filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, deco)).unwrap_or(false))
        .filter(|&a| matches!(s.vm.get_prop(a, "bPushable", 0), Ok(Value::Bool(true))))
        .collect();
    assert!(!candidates.is_empty(), "no pushable decoration");
    for d in candidates {
        let dl = s.vm.location(d);
        let dr = s.vm.get_prop(d, "CollisionRadius", 0).ok().and_then(|v| v.as_f32()).unwrap_or(20.0);
        // a spot on open floor 30 units outside the decoration, with a clear line to it
        let found = (0..8).find_map(|k| {
            let ang = k as f32 * std::f32::consts::FRAC_PI_4;
            let dist = dr + 17.0 + 30.0;
            let p = [dl[0] + dist * ang.cos(), dl[1] + dist * ang.sin(), dl[2]];
            s.vm.bsp.as_ref().unwrap().fast_line_check(p, dl).then_some((p, (ang + std::f32::consts::PI).to_degrees()))
        });
        let Some((spot, yaw)) = found else { continue };
        s.teleport(spot, yaw);
        run(&mut s, 10);
        if !s.on_ground() {
            continue;
        }
        let before = s.vm.location(d);
        let p0 = s.player_location();
        s.input.key(key_code("W").unwrap(), true);
        run(&mut s, 30);
        s.input.key(key_code("W").unwrap(), false);
        run(&mut s, 10);
        let after = s.vm.location(d);
        let moved = ((after[0] - before[0]).powi(2) + (after[1] - before[1]).powi(2)).sqrt();
        let p1 = s.player_location();
        let walked = ((p1[0] - p0[0]).powi(2) + (p1[1] - p0[1]).powi(2)).sqrt();
        eprintln!("{}: pushed {moved:.1}, player walked {walked:.1}", s.vm.name_of(d));
        if moved > 2.0 {
            assert!(walked < 300.0, "the player was not slowed");
            return;
        }
    }
    panic!("no decoration could be pushed");
}

/// `InStasis`: after more than 5 s, actors with `bStasis` that do not move by themselves and are neither seen nor in a
/// zone the camera sees (or are far away) are switched off; whatever is in stasis is out of sight and either far or in an
/// unseen zone.
#[test]
fn unseen_far_actors_go_into_stasis() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 200);
    let now = s.vm.fp(s.vm.level, "TimeSeconds");
    let mut stasis = 0;
    let mut eligible = 0;
    for a in s.vm.live_actors() {
        if !matches!(s.vm.get_prop(a, "bStasis", 0), Ok(Value::Bool(true))) {
            continue;
        }
        eligible += 1;
        if s.vm.in_stasis(a) {
            stasis += 1;
            assert!(now - s.vm.fp(a, "LastRenderTime") > 5.0, "a recently seen actor is in stasis");
            let far = s.vm.fp(a, "DistanceFromPlayer") > 1200.0;
            let zn = { let l = s.vm.location(a); s.vm.point_region(l).zone_number as usize };
            let zone_unseen = now - s.vm.zone_render_times[zn] > 5.0;
            assert!(far || zone_unseen, "an actor near the player in a seen zone is in stasis");
        }
    }
    eprintln!("{eligible} actors with bStasis, {stasis} in stasis");
    assert!(stasis > 0, "nothing went into stasis");
}

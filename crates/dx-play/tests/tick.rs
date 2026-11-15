//! The frame as `ULevel::Tick` runs it: players-only ticks, actors spawned during a frame, the player's slot. Set
//! `DEUSEX_DIR` to the game folder.

use dx_play::{Input, Session};
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

fn pawns(s: &mut Session) -> Vec<u32> {
    let c = s.vm.reg.class_by_name("ScriptedPawn").unwrap();
    s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).collect()
}

/// `Level.bPlayersOnly`: the player walks, the level clock runs, every other actor stands still (no state code, no
/// physics); switched off, the NPCs go on.
#[test]
fn players_only_freezes_everyone_but_the_player() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 60);
    let level = s.vm.level;
    s.vm.set_prop(level, "bPlayersOnly", 0, Value::Bool(true)).unwrap();
    let npcs = pawns(&mut s);
    let before: Vec<[f32; 3]> = npcs.iter().map(|&a| s.vm.location(a)).collect();
    let p0 = s.player_location();
    let t0 = s.vm.fp(level, "TimeSeconds");
    for _ in 0..90 {
        s.step(&Input { forward: 1.0, run: true, ..Default::default() }, DT);
    }
    let walked = ((s.player_location()[0] - p0[0]).powi(2) + (s.player_location()[1] - p0[1]).powi(2)).sqrt();
    let moved: Vec<f32> = npcs.iter().zip(&before).map(|(&a, b)| {
        let l = s.vm.location(a);
        ((l[0] - b[0]).powi(2) + (l[1] - b[1]).powi(2) + (l[2] - b[2]).powi(2)).sqrt()
    }).collect();
    let t1 = s.vm.fp(level, "TimeSeconds");
    eprintln!("player walked {walked:.0}; {} NPCs, largest move {:?}; clock {t0:.2} -> {t1:.2}", npcs.len(), moved.iter().cloned().fold(0.0f32, f32::max));
    assert!(walked > 100.0, "the player did not move in a players-only tick");
    assert!(moved.iter().all(|&m| m < 0.01), "an NPC moved in a players-only tick");
    assert!(t1 - t0 > 2.9, "the level clock stopped");
    s.vm.set_prop(level, "bPlayersOnly", 0, Value::Bool(false)).unwrap();
    run(&mut s, 150);
    let moved_after = npcs.iter().zip(&before).filter(|(&a, b)| {
        s.vm.is_live(a) && {
            let l = s.vm.location(a);
            ((l[0] - b[0]).powi(2) + (l[1] - b[1]).powi(2)).sqrt() > 1.0
        }
    }).count();
    assert!(moved_after > 0, "no NPC moved after the players-only tick ended");
}

/// The actor list is read live: actors spawned during a frame (the trooper's shell casings, tracers) tick in that
/// same frame, so their `bTicked` equals the player's when the frame ends. Actors owned by the level (bullet holes:
/// `TraceHitSpawner` gets what was hit as owner, the world is the `LevelInfo`) wait for an owner that never ticks, so
/// they tick only on the frames where the level's toggle matches the level's unchanging `bTicked`: every other frame.
#[test]
fn actors_spawned_during_a_frame_tick_in_it() {
    let Some(mut s) = new_game() else { return };
    // a hostile terrorist that attacks the player on sight: its misses leave bullet holes (level-owned)
    let tc = s.vm.reg.class_by_name("Terrorist").unwrap();
    let target = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, tc)).unwrap_or(false)).unwrap();
    let p = s.player;
    let level = s.vm.level;
    let all = s.vm.reg.names.intern("All");
    s.vm.set_prop(p, "ReducedDamageType", 0, Value::Name(all)).unwrap();
    let shot = s.vm.reg.names.intern("Shot");
    let t = s.vm.location(target);
    let spot = (0..16)
        .find_map(|k| {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            let q = [t[0] + 400.0 * a.cos(), t[1] + 400.0 * a.sin(), t[2]];
            s.vm.bsp.as_ref().unwrap().fast_line_check(q, t).then_some((q, (a + std::f32::consts::PI).to_degrees()))
        })
        .unwrap();
    s.teleport(spot.0, spot.1);
    let _ = shot;
    run(&mut s, 60);
    let owner_of = |s: &mut Session, a: u32| match s.vm.get_prop(a, "Owner", 0).unwrap() {
        Value::Obj(o) => o,
        _ => 0,
    };
    let (mut checked, mut late, mut level_owned) = (0, Vec::new(), Vec::new());
    for _ in 0..300 {
        let before: std::collections::HashSet<u32> = s.vm.live_actors().into_iter().collect();
        s.step_bound(DT);
        let ticked = s.vm.get_prop(p, "bTicked", 0).unwrap();
        for a in s.vm.live_actors() {
            if before.contains(&a) || s.vm.get_prop(a, "bStatic", 0).unwrap() == Value::Bool(true) {
                continue;
            }
            if owner_of(&mut s, a) == level {
                level_owned.push(a);
                continue;
            }
            checked += 1;
            if std::env::var_os("DX_TEST_VERBOSE").is_some() {
                let ow = owner_of(&mut s, a);
                eprintln!("spawned {} owner {ow}", s.vm.reg.class(s.vm.class_of(a).unwrap()).path);
            }
            if s.vm.get_prop(a, "bTicked", 0).unwrap() != ticked {
                late.push(s.vm.reg.class(s.vm.class_of(a).unwrap()).path.clone());
            }
        }
    }
    eprintln!("{checked} actors spawned during frames; not ticked in their first frame: {late:?}; {} owned by the level", level_owned.len());
    assert!(checked >= 5, "too few spawned actors to check");
    assert!(late.is_empty(), "spawned actors waited for the next frame: {late:?}");
    // a level-owned actor that lives on (a bullet hole) keeps the level's bTicked while the player's alternates
    let hole = level_owned.iter().copied().find(|&a| s.vm.is_live(a)).expect("a bullet hole");
    let level_ticked = s.vm.get_prop(level, "bTicked", 0).unwrap();
    let mut player_values = std::collections::HashSet::new();
    for _ in 0..6 {
        s.step_bound(DT);
        player_values.insert(format!("{:?}", s.vm.get_prop(p, "bTicked", 0).unwrap()));
        assert_eq!(s.vm.get_prop(hole, "bTicked", 0).unwrap(), level_ticked, "a level-owned actor ticked on an odd frame");
    }
    assert_eq!(player_values.len(), 2, "the tick toggle does not alternate");
}

/// `UGameEngine::LoadMap` brings a level that has not begun play up with both clocks at 0 (maps store the editor
/// session's time: Airfield 231 s) and every actor's `LastRenderTime` at -10 between `BeginPlay` and `PostBeginPlay`,
/// so `LastRendered()` reads 10 until the renderer draws it. `bBegunPlay` stays set, `bStartup` only lasts the
/// start-up, `bHighDetailMode` is the device's `HighDetailActors` (on for D3D). Gunther, who starts out of the world,
/// must keep waiting there (with "just seen" pawns he followed his patrol orders from 20000 units up and spun forever).
#[test]
fn a_fresh_level_starts_at_time_zero_with_everyone_unseen() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "03_NYC_Airfield").unwrap();
    let level = s.vm.level;
    assert_eq!(s.vm.fp(level, "TimeSeconds"), 0.0);
    assert_eq!(s.vm.time, 0.0);
    assert!(s.vm.bp(level, "bBegunPlay"));
    assert!(!s.vm.bp(level, "bStartup"));
    assert!(s.vm.bp(level, "bHighDetailMode") && s.vm.high_detail_actors);
    let c = s.vm.reg.class_by_name("GuntherHermann").unwrap();
    let gun = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).unwrap();
    assert_eq!(s.vm.fp(gun, "LastRenderTime"), -10.0);
    let lr = s.vm.call_by_name(gun, "LastRendered", vec![]).unwrap();
    assert_eq!(lr.as_f32(), Some(10.0));
    run(&mut s, 60);
    let after = s.vm.fp(level, "TimeSeconds");
    assert!((s.vm.time - after).abs() < 1e-3, "the clocks drifted apart");
    assert!(after > 1.9, "clock after 2 s: {after}");
    let state = s.vm.state_name(gun).map(|x| x.to_string());
    let errors: Vec<String> = s.issues.iter().map(|i| format!("{} {}: {}", i.class, i.event, i.error)).collect();
    assert_eq!(state.as_deref(), Some("Idle"), "Gunther should wait out of the world");
    assert!(errors.is_empty(), "script errors: {errors:?}");
}

/// A position that is not a number is possible in the engine's own maths (a `PawnGenerator`'s scout swims with
/// `Mass` and `Buoyancy` 0, so `1 - Buoyancy/Mass` is 0/0, exactly as in Engine.dll); the original then just never
/// draws it. Walking from the start of Liberty Island made the fish generator's scout and its water splash NaN and the
/// draw sort panicked (user report). Such actors are now left out of drawing, lighting and the mixer.
#[test]
fn actors_at_nan_positions_are_not_drawn_and_do_not_crash() {
    let Some(mut s) = new_game() else { return };
    let mixer = std::sync::Arc::new(std::sync::Mutex::new(dx_audio::Mixer::new(44100)));
    s.attach_audio(mixer.clone());
    // put a visible actor near the player at NaN, and give it a light and a sound
    let p = s.player;
    let deco = s.vm.reg.class_by_name("DeusExDecoration").unwrap();
    let lp = s.vm.location(p);
    let decos: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, deco)).unwrap_or(false)).collect();
    let mut by_dist: Vec<(f32, u32)> = decos.into_iter().map(|a| {
        let l = s.vm.location(a);
        ((l[0] - lp[0]).powi(2) + (l[1] - lp[1]).powi(2), a)
    }).collect();
    by_dist.sort_by(|x, y| x.0.total_cmp(&y.0));
    let a = by_dist[0].1;
    let nan = f32::NAN;
    s.vm.set_prop(a, "Location", 0, Value::Vector([nan; 3])).unwrap();
    s.vm.set_prop(a, "LightType", 0, Value::Byte(1)).unwrap();
    s.vm.set_prop(a, "LightBrightness", 0, Value::Byte(200)).unwrap();
    let sound = match s.vm.get_prop(p, "JumpSound", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
    if sound != 0 {
        s.vm.call_by_name(a, "PlaySound", vec![Value::Obj(sound)]).unwrap();
    }
    for _ in 0..5 {
        run(&mut s, 1);
        let mut buf = vec![0.0f32; 2 * 1470];
        mixer.lock().unwrap().mix(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite()), "the mix went NaN");
    }
    let cam = s.camera();
    let draws = s.actor_draws(&cam);
    assert!(draws.iter().all(|d| d.location.iter().all(|x| x.is_finite())), "an actor at NaN is drawn");
    // a light at NaN reaches no actor: every drawn actor's lighting stays a number
    assert!(draws.iter().filter_map(|d| d.lighting.as_ref()).all(|l| l.ambient.iter().chain(l.lights.iter().flat_map(|x| x.1.iter())).all(|c| c.is_finite())), "a light at NaN is used");
}

/// "Rendered" as the original decides it (Render.dll's visibility pass, `dx-vis`): an NPC the camera faces but that a
/// wall hides completely (every corner of its box blocked) is not drawn, so its `LastRenderTime` stays; from an open
/// spot it is drawn and stamped. Zones seen include the camera zone.
#[test]
fn actors_behind_walls_are_not_rendered() {
    let Some(mut s) = new_game() else { return };
    run(&mut s, 10);
    let eye_h = s.vm.get_prop(s.player, "BaseEyeHeight", 0).unwrap().as_f32().unwrap();
    let sp = s.vm.reg.class_by_name("ScriptedPawn").unwrap();
    let npcs: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, sp)).unwrap_or(false) && !s.vm.bp(a, "bHidden")).collect();
    let face = |from: [f32; 3], to: [f32; 3]| ((to[1] - from[1]).atan2(to[0] - from[0])).to_degrees();
    let mut checked_hidden = 0;
    let mut checked_open = 0;
    'npc: for &npc in &npcs {
        let t = s.vm.location(npc);
        let (r, h) = (s.vm.fp(npc, "CollisionRadius"), s.vm.fp(npc, "CollisionHeight"));
        for dist in [250.0f32, 400.0, 600.0] {
            for k in 0..16 {
                let a = k as f32 * std::f32::consts::TAU / 16.0;
                let spot = [t[0] + dist * a.cos(), t[1] + dist * a.sin(), t[2]];
                s.teleport(spot, face(spot, t));
                let at = s.player_location();
                if ((at[0] - spot[0]).powi(2) + (at[1] - spot[1]).powi(2)).sqrt() > 2.0 {
                    continue;
                }
                let eye = [at[0], at[1], at[2] + eye_h];
                let bsp = s.vm.bsp.as_ref().unwrap();
                let corners: Vec<[f32; 3]> = (0..8).map(|i| [t[0] + if i & 1 != 0 { r } else { -r }, t[1] + if i & 2 != 0 { r } else { -r }, t[2] + if i & 4 != 0 { h } else { -h }]).collect();
                let blocked = corners.iter().all(|&c| !bsp.fast_line_check(eye, c));
                let clear = corners.iter().all(|&c| bsp.fast_line_check(eye, c));
                if !blocked && !clear {
                    continue;
                }
                let mut cam = s.camera();
                cam.pos = eye;
                cam.yaw = (face(eye, t) / 360.0 * 65536.0) as i32;
                cam.pitch = 0;
                let (drawn, frames) = s.compute_visibility(&cam, 800, 600);
                let seen = drawn.contains(&npc);
                if blocked && checked_hidden < 3 {
                    assert!(!seen, "NPC {npc} drawn through a wall from {eye:?}");
                    checked_hidden += 1;
                } else if clear && checked_open < 3 {
                    assert!(seen, "NPC {npc} in the open not drawn from {eye:?}");
                    assert!(dx_vis::walk::zones_seen(&frames) != 0);
                    checked_open += 1;
                }
                if checked_hidden >= 3 && checked_open >= 3 {
                    break 'npc;
                }
            }
        }
    }
    eprintln!("checked {checked_hidden} hidden and {checked_open} open views");
    assert!(checked_hidden >= 1 && checked_open >= 1, "no view to check: hidden {checked_hidden}, open {checked_open}");
}

/// `LoadMap`'s base pass: an actor with an `AttachTag` is based on the first actor whose `Tag` matches. The training
/// range's glass panes ride their moving targets (they stayed behind before), and nothing ends up based on the level.
#[test]
fn attach_tags_base_actors_at_start_up() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "00_TrainingCombat").unwrap();
    let mut panes = 0;
    for a in s.vm.live_actors() {
        let Ok(Value::Name(tag)) = s.vm.get_prop(a, "AttachTag", 0) else { continue };
        if tag == 0 {
            continue;
        }
        let base = s.vm.op(a, "Base");
        assert_ne!(base, 0, "{:?} not attached", s.vm.reg.names.str(tag));
        assert_eq!(s.vm.get_prop(base, "Tag", 0).ok(), Some(Value::Name(tag)));
        panes += 1;
    }
    assert_eq!(panes, 4);
    let level = s.vm.level;
    assert!(s.vm.live_actors().into_iter().all(|a| s.vm.op(a, "Base") != level));
}

/// `LoadMap` spawns each level its own game rules object (`[Engine.Engine] DefaultGame`, `DeusExGameInfo`) and calls
/// `InitGame` with the URL options (`[DefaultPlayer]`): it spawns the base mutator, and the second `PreBeginPlay` (from
/// the start-up passes) the replication info. The previous level's rules object ends with that level.
#[test]
fn every_level_gets_its_own_game_rules_object() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    let count = |s: &mut Session, n: &str| {
        let c = s.vm.reg.class_by_name(n).unwrap();
        s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).count()
    };
    let first = s.vm.op(s.vm.level, "Game");
    assert_eq!(s.vm.class_of(first).map(|c| s.vm.reg.class(c).path.clone()).as_deref(), Some("DeusEx.DeusExGameInfo"));
    assert_eq!(count(&mut s, "GameInfo"), 1);
    assert_eq!(count(&mut s, "GameReplicationInfo"), 1);
    let bm = s.vm.op(first, "BaseMutator");
    assert!(bm != 0 && s.vm.is_live(bm));
    assert!(s.vm.log.iter().any(|l| l == "InitGame: ?Name=Player?Class=DeusEx.JCDentonMale"), "InitGame options");
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    run(&mut s, 5);
    let second = s.vm.op(s.vm.level, "Game");
    assert!(second != first && s.vm.is_live(second) && !s.vm.is_live(first));
    assert_eq!(count(&mut s, "GameInfo"), 1);
    assert_eq!(count(&mut s, "GameReplicationInfo"), 1);
}

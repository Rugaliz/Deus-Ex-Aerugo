//! Actor lighting as Render.dll's `FLightManager::SetupForActor` does it: lights from the actor's leaf, sorted by
//! influence, faded in and out by the frame's step, static lights traced. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::lighting::ActorFrame;

fn open(map: &str) -> Option<Session> {
    let game = std::env::var_os("DEUSEX_DIR").map(PathBuf::from)?;
    let mut s = Session::open(&game, map).unwrap();
    s.step_bound(1.0 / 30.0);
    Some(s)
}

/// The lights permeating the leaf of actor `a`.
fn permeating(s: &mut Session, a: u32) -> Vec<u32> {
    let leaf = s.vm.region_leaf(a);
    let model = &s.view.model;
    let mut out = Vec::new();
    if let Some(lf) = model.leaves.get(leaf.max(0) as usize).filter(|_| leaf >= 0) {
        let mut i = lf.i_permeating.max(0) as usize;
        while lf.i_permeating >= 0 && model.lights.get(i).copied().unwrap_or(0) != 0 {
            out.push(s.vm.model_light_actors[i]);
            i += 1;
        }
    }
    out
}

/// A lit actor of the map with a steady light of its leaf in range, and that light (the strongest by influence).
fn actor_and_light(s: &mut Session) -> Option<(u32, u32)> {
    for a in s.vm.live_actors() {
        // a dynamic mesh actor (DrawType mesh), lit
        if s.vm.u8p(a, "DrawType") != 2 || s.vm.bp(a, "bStatic") || s.vm.bp(a, "bUnlit") || s.vm.bp(a, "bHidden") {
            continue;
        }
        let loc = s.vm.location(a);
        let mut best: Option<(f32, u32)> = None;
        for l in permeating(s, a) {
            if s.vm.u8p(l, "LightType") != 1 || s.vm.u8p(l, "LightBrightness") == 0 || s.vm.u8p(l, "LightEffect") == 17 || s.vm.bp(l, "bSpecialLit") != s.vm.bp(a, "bSpecialLit") {
                continue;
            }
            let lp = s.vm.location(l);
            let d = ((lp[0] - loc[0]).powi(2) + (lp[1] - loc[1]).powi(2) + (lp[2] - loc[2]).powi(2)).sqrt();
            let r = (s.vm.u8p(l, "LightRadius") as f32 + 1.0) * 25.0;
            let infl = (1.0 - d / r) * s.vm.u8p(l, "LightBrightness") as f32;
            if d < r && best.map(|b| infl > b.0).unwrap_or(true) {
                best = Some((infl, l));
            }
        }
        if let Some((_, l)) = best {
            return Some((a, l));
        }
    }
    None
}

#[test]
fn a_leaf_light_fades_in_by_the_frame_step_and_drops_out_when_not_gathered() {
    let Some(mut s) = open("01_NYC_UNATCOIsland") else { return };
    let (a, l) = actor_and_light(&mut s).expect("an actor with a light of its leaf in range");
    // a movable, non-static light is not traced: it starts dark and gains the step each frame
    s.vm.set_bp(l, "bStatic", false);
    s.vm.set_bp(l, "bMovable", true);
    let perm = permeating(&mut s, a);
    let fr = ActorFrame { frame_count: 1, fade_step: 64, viewport_time: 0.0, has_leaves: true, lit_view: true, permeating: &perm, leaf_lights: &[], volumetrics: &[] };
    let mut tags = Vec::new();
    for _ in 0..5 {
        let lit = s.vm.setup_for_actor(a, &fr);
        tags.push(s.vm.ip(l, "SpecialTag"));
        let lp = s.vm.location(l);
        let m = lit.lights.iter().find(|m| m.pos == lp).expect("the light lights the actor");
        let full = s.vm.light_entry_colour(l);
        let fade = *tags.last().unwrap() as f32 / 255.0;
        for c in 0..3 {
            assert!((m.color[c] - full[c] * fade).abs() < 1e-5, "{:?} vs {:?} x {fade}", m.color, full);
        }
    }
    assert_eq!(tags, vec![65, 129, 193, 255, 255]);
    // out of reach of its radius it is not gathered any more and drops out of the actor's list at once; a light
    // that is gathered but refused fades out instead: switch the actor to special lighting, the light no longer counts
    let other = !s.vm.bp(l, "bSpecialLit");
    s.vm.set_bp(a, "bSpecialLit", other);
    let lit = s.vm.setup_for_actor(a, &fr);
    let lp = s.vm.location(l);
    assert!(!lit.lights.iter().any(|m| m.pos == lp));
}

#[test]
fn a_new_cache_shows_visible_static_lights_at_once() {
    let Some(mut s) = open("01_NYC_UNATCOIsland") else { return };
    let (a, l) = actor_and_light(&mut s).expect("an actor with a light of its leaf in range");
    let perm = permeating(&mut s, a);
    let fr = ActorFrame { frame_count: 3, fade_step: 10, viewport_time: 0.0, has_leaves: true, lit_view: true, permeating: &perm, leaf_lights: &[], volumetrics: &[] };
    s.vm.set_bp(l, "bStatic", true);
    let lit = s.vm.setup_for_actor(a, &fr);
    let st = s.vm.ip(l, "SpecialTag");
    let lp = s.vm.location(l);
    if let Some(m) = lit.lights.iter().find(|m| m.pos == lp) {
        // traced visible on the first call: full at once (0xff), not faded in from 0
        assert_eq!(st, 255);
        let full = s.vm.light_entry_colour(l);
        assert!((0..3).all(|c| (m.color[c] - full[c]).abs() < 1e-5));
    } else {
        // hidden from the actor: refused, nothing left to fade
        assert_eq!(st, 0);
    }
    // the next call (cache present, not this light's trace frame) keeps its visibility bit without tracing
    let before = s.vm.ip(l, "SpecialTag");
    let idx_frame = 3u32;
    let fr2 = ActorFrame { frame_count: idx_frame, ..fr };
    s.vm.setup_for_actor(a, &fr2);
    let after = s.vm.ip(l, "SpecialTag");
    if before == 255 {
        assert_eq!(after, 255);
    }
    // unlit actors get no lights, only their ambient
    s.vm.set_bp(a, "bUnlit", true);
    let lit = s.vm.setup_for_actor(a, &fr);
    assert!(lit.lights.is_empty() && !lit.render_fog);
}

#[test]
fn a_light_below_an_eighth_of_the_strongest_fades_out() {
    let Some(mut s) = open("01_NYC_UNATCOIsland") else { return };
    let (a, l) = actor_and_light(&mut s).expect("an actor with a light of its leaf in range");
    s.vm.set_bp(l, "bStatic", false);
    s.vm.set_bp(l, "bMovable", true);
    let perm = permeating(&mut s, a);
    let fr = ActorFrame { frame_count: 1, fade_step: 64, viewport_time: 0.0, has_leaves: true, lit_view: true, permeating: &perm, leaf_lights: &[], volumetrics: &[] };
    for _ in 0..4 {
        s.vm.setup_for_actor(a, &fr);
    }
    assert_eq!(s.vm.ip(l, "SpecialTag"), 255);
    // a very strong dynamic light linked to the actor's leaf: the weak light falls under an eighth of it
    let p = s.player;
    let loc = s.vm.location(a);
    s.teleport([loc[0] + 60.0, loc[1], loc[2] + 40.0], 0.0);
    for (k, v) in [("LightType", 1u8), ("LightBrightness", 255), ("LightRadius", 255)] {
        s.vm.set_prop(p, k, 0, ue_vm::Value::Byte(v)).unwrap();
    }
    let sp = s.vm.bp(a, "bSpecialLit");
    s.vm.set_bp(p, "bSpecialLit", sp);
    s.vm.set_prop(l, "LightBrightness", 0, ue_vm::Value::Byte(1)).unwrap();
    let leaf = [p];
    let fr = ActorFrame { leaf_lights: &leaf, ..fr };
    let mut tags = Vec::new();
    for _ in 0..5 {
        let lit = s.vm.setup_for_actor(a, &fr);
        let st = s.vm.ip(l, "SpecialTag");
        tags.push(st);
        // while it fades it still lights the actor, dimmed
        let lp = s.vm.location(l);
        assert_eq!(lit.lights.iter().any(|m| m.pos == lp), st > 0);
        assert!(lit.lights.iter().any(|m| m.pos == s.vm.location(p)), "the strong light");
    }
    assert_eq!(tags, vec![190, 126, 62, 0, 0]);
}

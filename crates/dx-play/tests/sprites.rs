//! `URender::DrawActorSprite`'s sprite branch: once-animated sprites, the `Matrix` texture override, editor icons.
//! Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

fn open() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    Some(s)
}

/// A visible `DT_Sprite` actor of a class with a default `LifeSpan` (spawned near the player).
fn sprite_actor(s: &mut Session) -> (u32, f32) {
    let actor = s.vm.reg.class_by_name("Actor").unwrap();
    let mut pick = None;
    for i in 0..s.vm.reg.classes.len() {
        let c = ue_registry::ClassId(i as u32);
        if !s.vm.is_a(c, actor) || s.vm.reg.class(c).flags & 1 != 0 || s.vm.reg.class(c).path != "DeusEx.SphereEffect" {
            continue;
        }
        pick = Some(c);
    }
    let c = pick.unwrap_or_else(|| s.vm.reg.class_by_name("Effects").unwrap());
    let p = s.vm.location(s.player);
    let a = s.vm.spawn(c, 0, None, Some([p[0] + 100.0, p[1], p[2]]), None).unwrap().unwrap();
    s.vm.set_prop(a, "DrawType", 0, Value::Byte(1)).unwrap();
    s.vm.set_prop(a, "bHidden", 0, Value::Bool(false)).unwrap();
    let life = s.vm.fp(a, "LifeSpan");
    (a, life)
}

fn tex(s: &mut Session, path: &str) -> u32 {
    s.vm.reg.ensure_package_for_path(path);
    s.vm.reg.obj_id(path).0
}

/// `DT_SpriteAnimOnce`: the frame of the texture's `AnimNext` chain follows the spent part of `LifeSpan`:
/// `round_even(float(n * clamp(1 - LifeSpan / default, 0, 1)) - 0.5)`, clamped to the chain.
#[test]
fn once_animated_sprites_follow_their_life_span() {
    let Some(mut s) = open() else { return };
    let (a, default_life) = sprite_actor(&mut s);
    assert!(default_life > 0.0, "the test class needs a default LifeSpan");
    let first = tex(&mut s, "Effects.water.WtrFall_A02");
    let mut chain = vec![first];
    loop {
        let n = s.vm.texture_anim_next(*chain.last().unwrap());
        if n == 0 || n == first {
            break;
        }
        chain.push(n);
    }
    let n = chain.len();
    assert!(n >= 4, "chain of {n}");
    s.vm.set_prop(a, "DrawType", 0, Value::Byte(7)).unwrap();
    s.vm.set_prop(a, "Texture", 0, Value::Obj(first)).unwrap();
    let frame_at = |s: &mut Session, life: f32| {
        s.vm.set_prop(a, "LifeSpan", 0, Value::Float(life)).unwrap();
        let d = s.vm.mesh_draw(a).unwrap();
        let path = d.sprite.unwrap();
        chain.iter().position(|&t| s.vm.reg.obj_path(ue_registry::ObjId(t)) == path).unwrap()
    };
    // fresh: t = 0 -> round_even(-0.5) = 0; spent: t = 1 -> n - 0.5 rounds to even, clamped to n - 1
    assert_eq!(frame_at(&mut s, default_life), 0);
    assert_eq!(frame_at(&mut s, 0.0), n - 1);
    // half spent: n / 2 - 0.5 rounds half to even
    let half = ((n as f64 / 2.0) - 0.5).round_ties_even() as usize;
    assert_eq!(frame_at(&mut s, default_life * 0.5), half.min(n - 1));
}

/// Every visible sprite is drawn, editor icons included (the port used to skip `Engine.S_*` textures), and the viewing
/// player's `Sprite` (the `Matrix` easter egg) stands in for every sprite's texture.
#[test]
fn sprites_draw_icons_and_the_matrix_override() {
    let Some(mut s) = open() else { return };
    let (a, _) = sprite_actor(&mut s);
    let icon = tex(&mut s, "Engine.S_Light");
    s.vm.set_prop(a, "Texture", 0, Value::Obj(icon)).unwrap();
    assert_eq!(s.vm.mesh_draw(a).and_then(|d| d.sprite).as_deref(), Some("Engine.S_Light"));
    let p = s.player;
    s.vm.call_by_name(p, "Matrix", vec![]).unwrap();
    let over = s.vm.op(p, "Sprite");
    assert_ne!(over, 0, "Matrix sets the player's Sprite");
    let over_path = s.vm.reg.obj_path(ue_registry::ObjId(over)).to_string();
    assert_eq!(s.vm.mesh_draw(a).and_then(|d| d.sprite), Some(over_path));
    s.vm.call_by_name(p, "Matrix", vec![]).unwrap();
    assert_eq!(s.vm.mesh_draw(a).and_then(|d| d.sprite).as_deref(), Some("Engine.S_Light"));
}

/// Animated surface textures (`AnimNext` chains, e.g. UNATCO's screens) step once per engine frame in which a surface
/// using them is on a draw list (`UTexture::Get` on the viewport clock); the renderer is told the frame. A camera put in
/// front of such a surface sees it advance from step to step.
#[test]
fn animated_surface_textures_step_while_seen() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOHQ").unwrap();
    s.step_bound(1.0 / 30.0);
    let animated = s.animated_surfaces();
    assert!(!animated.is_empty());
    // a camera 120 units in front of an animated face that the visibility pass draws from there
    let faces: Vec<_> = s.view.model.faces().into_iter().filter(|f| animated.contains(&f.surf)).collect();
    let mut cam = None;
    for face in &faces {
        let n = face.vertices.len() as f32;
        let c: [f32; 3] = std::array::from_fn(|k| face.vertices.iter().map(|v| v[k]).sum::<f32>() / n);
        let plane = s.view.model.nodes[face.node].plane;
        let eye = [c[0] + plane[0] * 120.0, c[1] + plane[1] * 120.0, c[2] + plane[2] * 120.0];
        let dir = [-plane[0], -plane[1], -plane[2]];
        let yaw = (dir[1].atan2(dir[0]) * 32768.0 / std::f32::consts::PI) as i32;
        let pitch = (dir[2].clamp(-1.0, 1.0).asin() * 32768.0 / std::f32::consts::PI) as i32;
        let c = dx_soft::Camera { pos: eye, pitch, yaw, fov: 90.0, roll: 0 };
        s.mark_rendered(&c);
        if !s.anim_frames().is_empty() {
            cam = Some(c);
            break;
        }
    }
    let cam = cam.unwrap_or_else(|| panic!("none of {} animated faces is drawn from in front of it", faces.len()));
    let mut seen = Vec::new();
    // the screens here run at 5 to 7 frames a second: 12 steps of 1/30 s cover at least one frame time
    for _ in 0..12 {
        s.mark_rendered(&cam);
        assert!(!s.anim_frames().is_empty(), "the animated surface is on a draw list");
        seen.push(s.anim_frames().clone());
        s.step_bound(1.0 / 30.0);
    }
    assert!(seen.windows(2).any(|w| w[0] != w[1]), "the frame changes while the surface is seen: {seen:?}");
}

/// Corona colours (`DrawFrame`): hue in three 85-wide segments, then pulled toward white by the saturation.
#[test]
fn corona_colours_follow_hue_and_saturation() {
    use dx_play::corona::corona_color;
    let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5);
    assert!(near(corona_color(0, 0), [1.0, 0.0, 0.0]));
    assert!(near(corona_color(85, 0), [0.0, 1.0, 0.0]));
    assert!(near(corona_color(170, 0), [0.0, 0.0, 1.0]));
    // the last segment steps by 1/84 for blue: hue 255 is red with no blue
    assert!(near(corona_color(255, 0), [85.0 / 85.0, 0.0, 0.0]));
    assert!(near(corona_color(42, 255), [1.0, 1.0, 1.0]));
}

/// The dock lamp at the start of Liberty Island is a `bCorona` light: from the start it is clear of the eye, so its
/// corona brightens to 1 (twice the fade per pass) and is drawn centred on the lamp, `width * DrawScale * 0.2` wide.
#[test]
fn a_visible_corona_light_glows() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    for _ in 0..30 {
        s.step_bound(1.0 / 30.0);
    }
    let coronas = s.coronas();
    assert!(!coronas.is_empty(), "no corona");
    let (l, bright) = coronas[0];
    assert_eq!(bright, 1.0);
    assert!(s.vm.bp(l, "bCorona"));
    // face the lamp
    let lp = s.vm.location(l);
    let pp = s.player_location();
    s.teleport(pp, (lp[1] - pp[1]).atan2(lp[0] - pp[0]).to_degrees());
    s.step_bound(1.0 / 30.0);
    let cam = s.camera();
    let quads = s.corona_quads(&cam, [0, 0, 800, 600]);
    let q = quads.iter().find(|q| q.texture.contains("Corona")).expect("a corona tile");
    let size = 800.0 * s.vm.fp(l, "DrawScale") * 0.2;
    assert!((q.rect[2] - size).abs() < 1e-3);
    let centre = [q.rect[0] + q.rect[2] / 2.0, q.rect[1] + q.rect[3] / 2.0];
    assert!((centre[0] - 400.0).abs() < 20.0, "the lamp is ahead: {centre:?}");
    let c = dx_play::corona::corona_color(s.vm.u8p(l, "LightHue"), s.vm.u8p(l, "LightSaturation"));
    assert_eq!(q.color, c);
    assert_eq!(q.poly_flags, 0x104);
}

/// Picture of the dock lamp's corona (`../out/screenshots/s19_corona.png`); needs a Vulkan device.
#[test]
#[ignore]
fn shot_of_a_corona() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    for _ in 0..30 {
        s.step_bound(1.0 / 30.0);
    }
    let l = s.coronas()[0].0;
    let lp = s.vm.location(l);
    let pp = s.player_location();
    s.teleport(pp, (lp[1] - pp[1]).atan2(lp[0] - pp[0]).to_degrees());
    for _ in 0..10 {
        s.step_bound(1.0 / 30.0);
    }
    let mut r = None;
    let img = s.render_vk(&mut r, 800, 600).unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s19_corona.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    r.take().unwrap().destroy();
}

/// A mesh drawn with an animated texture (a skin, or a texture of the mesh's own) shows the texture's current frame;
/// `DrawLodMesh` gets every material texture, so drawing the mesh steps it once per engine frame.
#[test]
fn mesh_skins_show_their_animation_frame() {
    let Some(mut s) = open() else { return };
    let c = s.vm.reg.class_by_name("ScriptedPawn").unwrap();
    let pawn = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).unwrap();
    let t = tex(&mut s, "Effects.water.WtrFall_A02");
    s.vm.set_prop(pawn, "Skin", 0, Value::Obj(t)).unwrap();
    s.vm.set_prop(pawn, "MultiSkins", 0, Value::Obj(0)).unwrap();
    let skin0 = |s: &mut Session| s.vm.mesh_draw(pawn).unwrap().skins[0].clone().unwrap();
    assert_eq!(skin0(&mut s), "Effects.water.WtrFall_A02");
    s.vm.texture_get(t, 10.0);
    assert_eq!(skin0(&mut s), "Effects.water.WtrFall_A03");
}

/// The `Matrix` easter egg's `ConsoleCommand("RMODE 6")` reaches `UViewport::Exec`: the player's `RendMap` becomes 6, so
/// Render.dll sets up no lights (surfaces plain, actors ambient only: `SetupForActor` leaves before choosing lights
/// unless `RendMap` is 5); `RMODE 5` brings them back; values outside 1..19 are ignored.
#[test]
fn the_matrix_cheat_switches_the_render_mode() {
    let Some(mut s) = open() else { return };
    let p = s.player;
    assert_eq!(s.vm.ip(p, "RendMap"), 5);
    assert!(s.lit_view());
    // an actor with lights reaching it
    let lit = s.vm.live_actors().into_iter().find(|&a| s.vm.mesh_draw(a).map(|d| d.sprite.is_none() && !d.unlit).unwrap_or(false) && !s.actor_lighting(a, false).lights.is_empty()).expect("a lit mesh");
    s.vm.call_by_name(p, "Matrix", vec![]).unwrap();
    s.step_bound(1.0 / 30.0);
    assert_eq!(s.vm.ip(p, "RendMap"), 6);
    assert!(!s.lit_view());
    assert!(s.actor_lighting(lit, false).lights.is_empty(), "lights in RMODE 6");
    assert!(s.exec("RMODE 0") && s.exec("RMODE 20"));
    assert_eq!(s.vm.ip(p, "RendMap"), 6);
    s.vm.call_by_name(p, "Matrix", vec![]).unwrap();
    s.step_bound(1.0 / 30.0);
    assert_eq!(s.vm.ip(p, "RendMap"), 5);
    assert!(!s.actor_lighting(lit, false).lights.is_empty());
}

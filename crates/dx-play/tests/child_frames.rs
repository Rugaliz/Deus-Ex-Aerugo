//! Child frames of the original's visibility pass (`URender::OccludeFrame`, `CreateChildFrame`): looking at the sky
//! opens the sky zone from its `SkyZone` actor (that zone counts as seen), looking into a mirror opens the mirrored
//! camera, in which the player is drawn (the main view leaves the player out). Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use dx_soft::Camera;
use std::path::PathBuf;
use ue_vm::Value;

fn open(map: &str) -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, map).unwrap();
    for _ in 0..3 {
        s.step_bound(1.0 / 30.0);
    }
    Some(s)
}

#[test]
fn looking_at_the_sky_opens_the_sky_zone() {
    let Some(mut s) = open("01_NYC_UNATCOIsland") else { return };
    let sky_class = s.vm.reg.class_by_name("SkyZoneInfo").unwrap();
    let sky = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, sky_class)).unwrap_or(false)).unwrap();
    let sky_zone = s.vm.zone_number(sky);
    assert_ne!(sky_zone, 0);
    let mut cam = s.camera();
    cam.pitch = 12000;
    let mut opened = 0;
    for k in 0..8 {
        cam.yaw = k * 8192;
        let (_, frames) = s.compute_visibility(&cam, 800, 600);
        assert_eq!(frames[0].frame.recursion, 0);
        let skies: Vec<_> = frames.iter().filter(|f| f.frame.recursion == 1 && f.frame.zone == sky_zone).collect();
        assert!(skies.len() <= 1, "one sky frame per parent (backdrop surfaces merge)");
        if let Some(f) = skies.first() {
            assert_eq!(f.parent, Some(0));
            assert!(f.frame.span.valid() > 0);
            assert!(dx_vis::walk::zones_seen(&frames) & (1u64 << sky_zone) != 0);
            assert!(frames[0].vis.zones & (1u64 << sky_zone) == 0, "the main view does not reach the sky zone itself");
            if k == 0 {
                eprintln!("sky frame: {} surfaces, {} of {} sprites drawn", f.vis.surfaces.len(), f.vis.sprites.iter().filter(|&&v| v).count(), f.sprites.len());
            }
            opened += 1;
        }
    }
    eprintln!("sky frame opened in {opened} of 8 directions");
    assert!(opened >= 6, "looking up outdoors shows the sky: {opened} of 8");
}

#[test]
fn the_player_is_drawn_in_a_mirror() {
    let mut tried = 0;
    let mut seen = 0;
    for map in ["02_NYC_Hotel", "10_Paris_Club", "06_HongKong_WanChai_Underworld"] {
        let Some(mut s) = open(map) else { return };
        let player = s.player;
        let eye_h = s.vm.fp(player, "BaseEyeHeight");
        let model = s.view.model.clone();
        for (i, nd) in model.nodes.iter().enumerate() {
            if nd.i_surf < 0 || model.surfs[nd.i_surf as usize].poly_flags & 0x800_0000 == 0 || nd.num_vertices < 3 {
                continue;
            }
            let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| model.points[model.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
            let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
            let n = [nd.plane[0], nd.plane[1], nd.plane[2]];
            if n[2].abs() > 0.3 {
                continue;
            }
            let eye = [c[0] + n[0] * 60.0, c[1] + n[1] * 60.0, c[2]];
            s.teleport([eye[0], eye[1], eye[2] - eye_h], 0.0);
            let at = s.vm.location(player);
            if (at[0] - eye[0]).abs() > 1.0 || (at[1] - eye[1]).abs() > 1.0 {
                continue;
            }
            let yaw = ((-n[1]).atan2(-n[0]).to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
            let cam = Camera { pos: [at[0], at[1], at[2] + eye_h], pitch: -2000, yaw, fov: 75.0, roll: 0 };
            let (drawn, frames) = s.compute_visibility(&cam, 800, 600);
            tried += 1;
            let mirrors: Vec<_> = frames.iter().filter(|f| f.frame.mirror == -1.0).collect();
            assert!(!frames[0].sprites.iter().any(|sp| sp.id == player), "the main view leaves the player out");
            if mirrors.is_empty() {
                eprintln!("{map} node {i}: no mirror frame");
                continue;
            }
            assert!(mirrors.iter().all(|f| f.frame.recursion >= 1 && f.frame.near_clip[3] != 0.0));
            if drawn.contains(&player) {
                seen += 1;
            } else {
                eprintln!("{map} node {i}: mirror frame, player not drawn");
            }
        }
    }
    eprintln!("player seen in {seen} of {tried} mirrors");
    assert!(tried > 0 && seen * 2 >= tried, "player seen in {seen} of {tried} mirrors");
}

/// Render iterators (`SetupDynamics`, `UParticleIterator::CurrentItem`): a spewing particle generator yields its proxy
/// once per live particle, moved to the particle; every such sprite is tested on its own, and the proxy is stamped as
/// rendered (`ParticleGenerator` reads `proxy.LastRendered()` to stay unfrozen). The generator itself draws nothing.
/// The island's faucets start closed (their spray generators get `PostBeginPlay` again from the level start-up, which
/// switches a triggered, not initially on generator off): the player opens them first.
#[test]
fn particle_generators_draw_their_proxy_per_particle() {
    let Some(mut s) = open("01_NYC_UNATCOIsland") else { return };
    let faucet = s.vm.reg.class_by_name("Faucet").unwrap();
    let faucets: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, faucet)).unwrap_or(false)).collect();
    assert!(!faucets.is_empty());
    for &f in &faucets {
        let wg = s.vm.op(f, "waterGen");
        assert!(!s.vm.bp(f, "bOpen") && !s.vm.bp(wg, "bSpewing"), "a closed faucet does not run");
        let p = s.player;
        s.vm.call_by_name(f, "Frob", vec![Value::Obj(p), Value::Obj(0)]).unwrap();
        assert!(s.vm.bp(wg, "bSpewing"), "an opened faucet runs");
    }
    let pg = s.vm.reg.class_by_name("ParticleGenerator").unwrap();
    let gens: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, pg)).unwrap_or(false)).collect();
    eprintln!("{} particle generators", gens.len());
    let mut checked = 0;
    for g in gens {
        let it = s.vm.op(g, "RenderInterface");
        let proxy = s.vm.op(g, "proxy");
        if it == 0 || proxy == 0 || s.vm.bp(g, "bHidden") {
            continue;
        }
        // look at the generator from 200 units away, from above
        let at = s.vm.location(g);
        let eye = [at[0] - 200.0, at[1], at[2] + 60.0];
        let cam = Camera { pos: eye, pitch: -3000, yaw: 0, fov: 75.0, roll: 0 };
        // render from there for a while: the drawn proxy keeps the generator unfrozen and spewing
        for _ in 0..45 {
            s.mark_rendered(&cam);
            s.step_bound(1.0 / 30.0);
        }
        let (drawn, frames) = s.compute_visibility(&cam, 800, 600);
        let main = &frames[0];
        assert_eq!(s.vm.op(it, "Outer"), g);
        assert!(!main.sprites.iter().any(|sp| sp.id == g), "the generator (DT_None) has no sprite");
        let n_proxy = main.sprites.iter().filter(|sp| sp.id == proxy).count();
        // live particles after the walk (a failed move deletes one)
        let live = s.vm.live_particles(it);
        let frozen = s.vm.bp(g, "bFrozen");
        eprintln!("generator {g}: frozen {frozen} live {live} proxy sprites {n_proxy} drawn {}", drawn.contains(&proxy));
        if frozen || live == 0 {
            continue;
        }
        let own = usize::from(!s.vm.bp(proxy, "bHidden") && main.sprites.iter().any(|sp| sp.id == proxy));
        // particles off the screen get no sprite (empty box), the others one each (the share off screen is random)
        assert!(n_proxy * 3 >= live * 2 && n_proxy <= live + own, "{n_proxy} proxy sprites for {live} live particles");
        assert!(drawn.contains(&proxy), "the proxy counts as rendered");
        checked += 1;
    }
    assert!(checked > 0, "no spewing generator checked");
}

/// `DrawFrame` draws each child frame from its own lists: a mirror frame's view carries the mirror's plane and its own
/// sprites, the player among them, while the main view's list leaves the player out; looking up outdoors gives one
/// sky view, which draws the level nodes its own walk drew.
#[test]
fn child_frames_draw_their_own_sprites() {
    let mut checked = 0;
    for map in ["02_NYC_Hotel", "10_Paris_Club"] {
        let Some(mut s) = open(map) else { return };
        let player = s.player;
        let eye_h = s.vm.fp(player, "BaseEyeHeight");
        let model = s.view.model.clone();
        for nd in &model.nodes {
            if nd.i_surf < 0 || model.surfs[nd.i_surf as usize].poly_flags & 0x800_0000 == 0 || nd.num_vertices < 3 || nd.plane[2].abs() > 0.3 {
                continue;
            }
            let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| model.points[model.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
            let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
            let n = [nd.plane[0], nd.plane[1], nd.plane[2]];
            let eye = [c[0] + n[0] * 60.0, c[1] + n[1] * 60.0, c[2]];
            s.teleport([eye[0], eye[1], eye[2] - eye_h], 0.0);
            let at = s.vm.location(player);
            if (at[0] - eye[0]).abs() > 1.0 || (at[1] - eye[1]).abs() > 1.0 {
                continue;
            }
            let yaw = ((-n[1]).atan2(-n[0]).to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
            let cam = Camera { pos: [at[0], at[1], at[2] + eye_h], pitch: -2000, yaw, fov: 75.0, roll: 0 };
            s.mark_rendered(&cam);
            let views = s.child_views(&cam).unwrap();
            let is_player = |d: &dx_soft::ActorDraw| !d.overlay && d.decal.is_none() && (0..3).all(|k| (d.location[k] - at[k]).abs() < 2.0);
            let Some(v) = views.iter().find(|v| v.mirror.map(|p| p[0] * n[0] + p[1] * n[1] + p[2] * n[2] > 0.999 && (p[3] - nd.plane[3]).abs() < 0.5).unwrap_or(false)) else { continue };
            if !v.actors.iter().any(is_player) {
                continue;
            }
            assert!(!s.actor_draws(&cam).iter().any(is_player), "the main view's list holds the player");
            checked += 1;
        }
    }
    eprintln!("{checked} mirrors show the player in their own list");
    assert!(checked > 0);

    let Some(mut s) = open("01_NYC_UNATCOIsland") else { return };
    let mut cam = s.camera();
    cam.pitch = 12000;
    s.mark_rendered(&cam);
    let views = s.child_views(&cam).unwrap();
    let skies = views.iter().filter(|v| v.mirror.is_none()).count();
    assert_eq!(skies, 1, "{} child views", views.len());
    // the sky frame draws its own level nodes: those its walk drew, all in the sky zone
    let sky = views.iter().find(|v| v.mirror.is_none()).unwrap();
    let fv = s.vis_frames().iter().find(|f| f.parent == Some(0) && f.frame.mirror == 1.0).unwrap();
    assert!(!sky.solid.is_empty());
    let model = s.static_world_model();
    for n in &sky.solid {
        let k = fv.vis.surfaces.iter().position(|x| x.0 == *n).expect("a node the sky frame drew");
        assert_eq!(fv.vis.surfaces[k].2, fv.frame.zone, "node {n} outside the sky zone");
        assert!((*n as usize) < model.nodes.len());
    }
    eprintln!("sky frame: {} level nodes", sky.solid.len());
}

/// A mover seen only in a mirror (Paris club washroom: a grated door behind the player) is listed through the mirror
/// frame: its fragments exist and the mirror's view names their nodes, while the main frame's list does not.
#[test]
fn movers_seen_in_a_mirror_are_drawn_from_its_frame() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "10_Paris_Club").unwrap();
    s.teleport([-1068.0, -1436.0, -112.0], 0.0);
    for _ in 0..10 {
        s.step_bound(1.0 / 30.0);
    }
    let cam = s.render_camera();
    s.mark_rendered(&cam);
    let views = s.child_views(&cam).unwrap();
    let listed = s.listed_movers().unwrap();
    let mirror = views.iter().find(|v| v.mirror.is_some() && !v.dynamic.is_empty()).expect("a mirror frame drawing a mover");
    for n in &mirror.dynamic {
        assert!(listed.nodes.contains_key(n), "node {n} without a fragment");
        assert!(!listed.main.contains(n), "node {n} also drawn by the main frame");
    }
    eprintln!("{} mover fragments only in the mirror", mirror.dynamic.len());
}

/// A mirror on a mover: the smugglers' bedroom mirror (`04_NYC_Smug`) is the front of the `DeusExMover` hiding the
/// stash, a translucent mirror 2 units in front of the wall. The walk opens its frame through the mover's dynamic
/// nodes; the renderer only knew the level's own mirror planes, so nothing showed but the clear colour (the tester's
/// "flat pale-blue rectangle"), and with the reflection drawn the wall behind hid all of it but the stash's opening.
/// The original (probe shot at the same place) shows the room reflected over the whole glass.
#[test]
fn a_mirror_on_a_mover_shows_the_room() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "04_NYC_Smug").unwrap();
    s.teleport([-720.0, 1400.0, 240.0], 90.0);
    for _ in 0..20 {
        s.step_bound(1.0 / 30.0);
    }
    let mut r = None;
    let img = s.render_vk(&mut r, 640, 480).expect("vulkan render");
    r.take().unwrap().destroy();
    let cam = s.render_camera();
    assert!(s.child_views(&cam).unwrap().iter().any(|v| v.mirror.is_some()), "the mirror opens its frame");
    // the glass, inside its frame (left of the bed), top and bottom strips included
    let stats = |x0: u32, y0: u32, x1: u32, y1: u32| {
        let lum: Vec<f64> = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (y, x))).map(|(y, x)| {
            let i = ((y * img.width + x) * 4) as usize;
            (img.rgba[i] as f64 * 0.3 + img.rgba[i + 1] as f64 * 0.59 + img.rgba[i + 2] as f64 * 0.11)
        }).collect();
        let mean = lum.iter().sum::<f64>() / lum.len() as f64;
        (lum.iter().map(|l| (l - mean) * (l - mean)).sum::<f64>() / lum.len() as f64).sqrt()
    };
    for (name, rect) in [("whole", (30, 165, 150, 295)), ("top strip", (30, 135, 150, 150)), ("bottom strip", (30, 310, 150, 330))] {
        // flat clear colour: 0.2; the glass over the wall behind it (the reflection hidden by its depth): 2 to 7
        let sd = stats(rect.0, rect.1, rect.2, rect.3);
        eprintln!("{name}: deviation {sd:.1}");
        assert!(sd > 10.0, "the {name} of the mirror is flat (deviation {sd:.1}): no reflection");
    }
}

/// A sprite reflected in a mirror faces the screen unflipped, as a mirror frame draws it (screen-aligned at its
/// projected centre): a crowbar icon on an effect in front of a hotel mirror, seen directly and in the mirror.
/// `cargo test --release -p dx-play --test child_frames shot_of_a_sprite_in_a_mirror -- --ignored`
#[test]
#[ignore]
fn shot_of_a_sprite_in_a_mirror() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "02_NYC_Hotel").unwrap();
    s.teleport([1819.0, -1158.0, 108.0], 180.0);
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    let path = "DeusExUI.Icons.BeltIconCrowbar";
    assert!(s.vm.reg.ensure_package_for_path(path));
    let tex = s.vm.reg.obj_id(path).0;
    let light = s.vm.reg.class_by_name("Effects").unwrap();
    let cam = s.render_camera();
    let a = s.vm.spawn(light, 0, None, Some([cam.pos[0] - 60.0, cam.pos[1] + 22.0, cam.pos[2] - 4.0]), None).unwrap().unwrap();
    s.vm.set_prop(a, "bHidden", 0, Value::Bool(false)).unwrap();
    s.vm.set_prop(a, "DrawType", 0, Value::Byte(1)).unwrap();
    s.vm.set_prop(a, "Texture", 0, Value::Obj(tex)).unwrap();
    s.vm.set_prop(a, "DrawScale", 0, Value::Float(0.3)).unwrap();
    s.vm.set_prop(a, "Style", 0, Value::Byte(2)).unwrap();
    s.step_bound(1.0 / 30.0);
    let mut r = None;
    let img = s.render_vk(&mut r, 800, 600).expect("vulkan render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::write(dir.join("sprite_in_mirror.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    r.take().unwrap().destroy();
}

/// Frames deeper than one level are drawn from their own lists too: in the Paris chateau the bathroom mirror's frame
/// opens a second mirror frame (its plane one of the world's mirror planes, its parent the first), and in VersaLife a
/// mirror shows the sky zone (a frame whose parent is a mirror, with a rotation between the two frames' axes).
#[test]
fn deeper_frames_name_their_parents() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "10_Paris_Chateau").unwrap();
    s.teleport([-564.0, 274.0, 416.0], 0.0);
    for _ in 0..10 {
        s.step_bound(1.0 / 30.0);
    }
    let cam = s.render_camera();
    s.mark_rendered(&cam);
    let views = s.child_views(&cam).unwrap();
    let deep = views.iter().find(|v| v.parent.is_some() && v.mirror.is_some()).expect("a mirror seen in a mirror");
    let parent = &views[deep.parent.unwrap()];
    assert!(parent.mirror.is_some() && !deep.solid.is_empty());
    let p = deep.mirror.unwrap();
    let model = s.static_world_model();
    let on_a_mirror = model.nodes.iter().any(|nd| {
        nd.i_surf >= 0 && model.surfs[nd.i_surf as usize].poly_flags & 0x800_0000 != 0 && (nd.plane[0] * p[0] + nd.plane[1] * p[1] + nd.plane[2] * p[2]).abs() > 0.999 && (nd.plane[3].abs() - p[3].abs()).abs() < 0.5
    });
    assert!(on_a_mirror, "{p:?} is no mirror plane");

    let mut s = Session::open(&g, "06_HongKong_VersaLife").unwrap();
    s.teleport([-944.0, 548.0, -6.0], 270.0);
    for _ in 0..10 {
        s.step_bound(1.0 / 30.0);
    }
    let cam = s.render_camera();
    s.mark_rendered(&cam);
    let views = s.child_views(&cam).unwrap();
    let sky = views.iter().find(|v| v.parent.is_some() && v.mirror.is_none()).expect("the sky seen in a mirror");
    assert!(views[sky.parent.unwrap()].mirror.is_some());
    let (rot, _) = sky.sky.unwrap();
    // a rotation: orthonormal columns
    for a in 0..3 {
        for b in 0..3 {
            let d: f32 = (0..3).map(|k| rot[a][k] * rot[b][k]).sum();
            assert!((d - if a == b { 1.0 } else { 0.0 }).abs() < 1e-3, "{rot:?}");
        }
    }
}

/// `DrawLodMesh` clips each mesh polygon at the frame's `NearClip` (Render 10b0bab0): in a mirror frame only what lies
/// beyond the mirror plane is reflected; sprites are not clipped. In the training level, through the first door
/// (unlocked and opened), the next room's floor is a mirror 16 units above the player's floor: the reflected camera sits
/// inside the player's own mesh, whose legs used to cover the reflection. Reference: the original at the same place
/// (`scripts/probe.sh shots 00_Training ...` with `DX_SHOT_OPEN`/`DX_SHOT_SPAWN`): a bright mesh below the plane leaves no
/// trace, one straddling it shows the part above it, a sprite below it is reflected.
#[test]
fn meshes_are_cut_at_the_mirror_plane() {
    let Some(mut s) = open("00_Training") else { return };
    let p = s.player;
    let mover = s.vm.reg.class_by_name("DeusExMover").unwrap();
    for a in s.vm.live_actors() {
        let l = s.vm.location(a);
        if s.vm.class_of(a).map(|c| s.vm.is_a(c, mover)).unwrap_or(false) && ((l[0] - 384.0).powi(2) + (l[1] - 576.0).powi(2)).sqrt() < 80.0 {
            s.vm.set_prop(a, "bLocked", 0, Value::Bool(false)).unwrap();
            s.vm.send_event(a, "Frob", vec![Value::Obj(p), Value::Obj(0)]).unwrap();
        }
    }
    for _ in 0..150 {
        s.step_bound(1.0 / 30.0);
    }
    let level = s.vm.op(p, "Level");
    let bright = s.vm.op(level, "DefaultTexture");
    let effects = s.vm.reg.class_by_name("Effects").unwrap();
    let mesh = s.vm.op(p, "Mesh");
    let mut r = None;
    // the view of the original's reference shot: the next room's floor fills the doorway around the crosshair
    let mut shot = |s: &mut Session, marker: Option<([f32; 3], f32, bool)>| {
        let made = marker.map(|(at, scale, sprite)| {
            let a = s.vm.spawn(effects, 0, None, Some(at), None).unwrap().unwrap();
            s.vm.set_prop(a, "bCollideWorld", 0, Value::Bool(false)).unwrap();
            s.vm.set_prop(a, "Physics", 0, Value::Byte(0)).unwrap();
            s.vm.set_prop(a, "LifeSpan", 0, Value::Float(0.0)).unwrap();
            if sprite {
                s.vm.set_prop(a, "DrawType", 0, Value::Byte(1)).unwrap();
                s.vm.set_prop(a, "Texture", 0, Value::Obj(bright)).unwrap();
            } else {
                s.vm.set_prop(a, "DrawType", 0, Value::Byte(2)).unwrap();
                s.vm.set_prop(a, "Mesh", 0, Value::Obj(mesh)).unwrap();
                s.vm.set_prop(a, "Skin", 0, Value::Obj(bright)).unwrap();
                for i in 0..8 {
                    s.vm.set_prop(a, "MultiSkins", i, Value::Obj(bright)).unwrap();
                }
            }
            s.vm.set_prop(a, "DrawScale", 0, Value::Float(scale)).unwrap();
            s.vm.set_prop(a, "bUnlit", 0, Value::Bool(true)).unwrap();
            a
        });
        s.teleport([384.0, 700.0, -144.2], 270.0);
        s.set_pitch((64626 - 65536) as f32 * 360.0 / 65536.0);
        s.step_bound(1.0 / 30.0);
        s.render_vk(&mut r, 800, 600).unwrap();
        let img = s.render_vk(&mut r, 800, 600).unwrap();
        if let Some(a) = made {
            s.vm.destroy(a).unwrap();
        }
        img.rgba
    };
    let plain = shot(&mut s, None);
    let (frames, mirror) = {
        let f = s.vis_frames();
        (f.len(), f.iter().any(|f| f.frame.mirror == -1.0 && (f.frame.origin[2] + 151.8).abs() < 1.0))
    };
    assert!(frames > 1 && mirror, "the next room's floor opens a mirror frame (reflected eye 23.8 below z -128)");
    // the floor's reflection between the door posts, below the far light strip's reflection
    let region = |img: &[u8]| -> f32 {
        let mut sum = 0.0;
        for y in 280..330 {
            for x in 300..500 {
                let i = (y * 800 + x) * 4;
                sum += img[i] as f32 + img[i + 1] as f32 + img[i + 2] as f32;
            }
        }
        sum / (50.0 * 200.0 * 3.0)
    };
    let changed = |a: &[u8], b: &[u8]| a.chunks(4).zip(b.chunks(4)).filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > 8)).count();
    // the reflection shows the next room (lit walls and the light strip's reflection), not the player's dark legs
    assert!(region(&plain) > 20.0, "floor reflection mean {}", region(&plain));
    // a mesh entirely below the plane, 20 units in front of the reflected eye: no trace
    let below = shot(&mut s, Some(([384.0, 680.0, -150.0], 0.25, false)));
    assert_eq!(changed(&plain, &below), 0, "a mesh below the mirror plane is not reflected");
    // straddling it in the next room: its upper part shows, directly and reflected
    let straddle = shot(&mut s, Some(([384.0, 520.0, -136.0], 0.4, false)));
    assert!(changed(&plain, &straddle) > 100);
    // a sprite below the plane is reflected (sprites are not clipped)
    let sprite = shot(&mut s, Some(([384.0, 600.0, -142.0], 0.08, true)));
    let in_floor = |img: &[u8]| (280..330).flat_map(|y| (330..470).map(move |x| (y * 800 + x) * 4)).filter(|&i| img[i] as i32 - plain[i] as i32 > 40).count();
    assert!(in_floor(&sprite) > 200, "the sprite's reflection shows in the floor: {}", in_floor(&sprite));
}

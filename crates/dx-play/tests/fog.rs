//! Volumetric lighting (fog): with the viewing player in a fog zone the visibility walk gathers the leaves' volumetric
//! lights (`LeafVolumetricLighting`) and gives each drawn surface the ones whose volume its view pyramid reaches.
//! Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use dx_soft::Camera;
use std::path::PathBuf;

#[test]
fn surfaces_behind_a_volumetric_light_get_it_as_fog_light() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&game, "02_NYC_Underground").unwrap();
    s.step_bound(1.0 / 30.0);
    let lights = s.vm.model_light_actors.clone();
    let mut tried = 0;
    let mut seen = std::collections::HashSet::new();
    for &a in &lights {
        if a == 0 || !seen.insert(a) || s.vm.u8p(a, "VolumeBrightness") == 0 || s.vm.u8p(a, "VolumeRadius") == 0 {
            continue;
        }
        let zone = s.vm.zone_of_actor(a);
        if zone == 0 || !s.vm.bp(zone, "bFogZone") {
            continue;
        }
        tried += 1;
        // the player a little away from the light, looking at it (in the same fog zone)
        let at = s.vm.location(a);
        let rv = (s.vm.u8p(a, "VolumeRadius") as f32 + 1.0) * 25.0;
        for (dx, dy) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let spot = [at[0] + dx * (rv + 96.0), at[1] + dy * (rv + 96.0), at[2]];
            s.teleport([spot[0], spot[1], spot[2] - 40.0], 0.0);
            let p = s.player_location();
            if (p[0] - spot[0]).abs() > 2.0 || (p[1] - spot[1]).abs() > 2.0 || s.vm.zone_of_actor(s.player) != zone {
                continue;
            }
            let yaw = ((-dy).atan2(-dx).to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
            let cam = Camera { pos: spot, pitch: 0, yaw, fov: 75.0, roll: 0 };
            s.mark_rendered(&cam);
            let fogged = s.vis_frames()[0].vis.surf_vol_lights.iter().filter(|(_, _, id)| *id == a).count();
            eprintln!("light {a}: {fogged} surfaces fogged by it");
            if fogged == 0 {
                continue;
            }
            // their fog maps hold fog (colour and amount)
            let surfs: Vec<usize> = s.vis_frames()[0].vis.surf_vol_lights.iter().filter(|(_, _, id)| *id == a).map(|(si, _, _)| *si as usize).collect();
            let mut foggy = 0;
            for si in &surfs {
                let fog = s.fog_texels(*si).expect("a fog map");
                let max_a = fog.iter().map(|t| t[3]).max().unwrap_or(0);
                eprintln!("  surface {si}: {} texels, max fog alpha {max_a}", fog.len());
                if max_a > 0 {
                    foggy += 1;
                }
            }
            assert!(foggy > 0, "no fog on the surfaces behind the light");
            // drawn sprites in front of the light's volume carry it as fog light (`Volumetrics`), and their actor
            // lighting asks for the fog pass with it: a decoration moved into the volume, in front of the camera
            let deco = s.vm.live_actors().into_iter().find(|&d| d != s.player && s.vm.u8p(d, "DrawType") == 2 && !s.vm.bp(d, "bStatic") && !s.vm.bp(d, "bHidden") && s.vm.bp(d, "bPushable"));
            if let Some(d) = deco {
                let to = [spot[0] - dx * (rv * 0.5 + 96.0), spot[1] - dy * (rv * 0.5 + 96.0), spot[2]];
                let ok = s.vm.call_by_name(d, "SetLocation", vec![ue_vm::Value::Vector(to)]);
                eprintln!("  moved {d} to {to:?}: {ok:?}, now {:?}", s.vm.location(d));
                s.mark_rendered(&cam);
            }
            let fv = &s.vis_frames()[0];
            let carriers: Vec<u32> = fv.sprites.iter().enumerate().filter(|(i, sp)| fv.vis.sprites[*i] && sp.item == u32::MAX && fv.vis.sprite_vols[*i].contains(&a)).map(|(_, sp)| sp.id).collect();
            eprintln!("  {} drawn sprites carry the fog light; drawn {}, with any fog light {}", carriers.len(), fv.vis.sprites.iter().filter(|&&d| d).count(), fv.vis.sprite_vols.iter().filter(|v| !v.is_empty()).count());
            assert!(deco.is_none() || !carriers.is_empty(), "the decoration in the volume carries no fog light");
            if let Some(&c) = carriers.first() {
                let lit = s.actor_lighting(c, true);
                assert!(lit.render_fog && lit.fog_lights.contains(&a), "{lit:?}");
                // the mesh draw gets those fog lights, and the fog at the actor (what its vertices add as D3D's
                // specular colour) is not nothing
                let ml = s.mesh_lighting(c, true);
                assert!(ml.fog_lights.iter().any(|f| f.0 == s.vm.location(a)), "{:?}", ml.fog_lights);
                let fl: Vec<dx_light::manager::FogLight> = ml
                    .fog_lights
                    .iter()
                    .map(|&(p, r, b, f, br, col)| dx_light::manager::FogLight { view: [p[0] - cam.pos[0], p[1] - cam.pos[1], p[2] - cam.pos[2]], volume_radius: r, volume_brightness: b, volume_fog: f, brightness: br, colour: col })
                    .collect();
                let d = s.vm.location(c);
                let mut skip = false;
                let fog = dx_light::manager::fog_at([d[0] - cam.pos[0], d[1] - cam.pos[1], d[2] - cam.pos[2]], &fl, &mut skip);
                eprintln!("  fog at the carrier: {fog:?}");
                assert!(fog[3] > 0.0 && fog[0] + fog[1] + fog[2] > 0.0, "{fog:?}");
            }
            // the fog draws of the frame go to the fog atlas, doubled as the device uploads them
            s.enable_world_light();
            s.mark_rendered(&cam);
            let draws = s.fog_draws();
            assert!(surfs.iter().all(|si| draws.iter().any(|(d, _)| d == si)), "fog draws {draws:?}");
            let rects = s.fog_updates();
            assert_eq!(rects.len(), draws.len());
            assert!(rects.iter().any(|r| r.4.chunks(4).any(|t| t[3] > 0)) && rects.iter().all(|r| r.4.iter().all(|b| b & 1 == 0)));
            // out of the fog zone (an ordinary zone of the map): no fog lights at all
            let level = s.vm.level;
            let pz = s.vm.zone_of_actor(s.player);
            s.vm.set_prop(pz, "bFogZone", 0, ue_vm::Value::Bool(false)).unwrap();
            s.mark_rendered(&cam);
            assert!(s.vis_frames()[0].vis.surf_vol_lights.is_empty(), "fog lights while the viewer is not in a fog zone");
            // the surfaces that had fog are cleared in the atlas
            let cleared = s.fog_updates();
            assert_eq!(cleared.len(), draws.len());
            assert!(cleared.iter().all(|r| r.4.iter().all(|&b| b == 0)));
            assert!(s.fog_draws().is_empty());
            let _ = level;
            return;
        }
    }
    panic!("no volumetric light seen ({tried} tried)");
}

/// Pictures of a volumetric light with and without its fog (needs a Vulkan device): `out/screenshots/fog_on.png`,
/// `fog_off.png`. The fog brightens the pixels around the light.
#[test]
#[ignore]
fn shot_of_fog() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&game, "02_NYC_Underground").unwrap();
    s.step_bound(1.0 / 30.0);
    let lights = s.vm.model_light_actors.clone();
    let mut r = None;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    for &a in &lights {
        if a == 0 || s.vm.u8p(a, "VolumeBrightness") == 0 || s.vm.u8p(a, "VolumeRadius") == 0 {
            continue;
        }
        let zone = s.vm.zone_of_actor(a);
        if zone == 0 || !s.vm.bp(zone, "bFogZone") {
            continue;
        }
        let at = s.vm.location(a);
        let rv = (s.vm.u8p(a, "VolumeRadius") as f32 + 1.0) * 25.0;
        for (dx, dy) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let spot = [at[0] + dx * (rv + 96.0), at[1] + dy * (rv + 96.0), at[2]];
            s.teleport([spot[0], spot[1], spot[2] - 40.0], (-dy).atan2(-dx).to_degrees());
            let p = s.player_location();
            if (p[0] - spot[0]).abs() > 2.0 || (p[1] - spot[1]).abs() > 2.0 || s.vm.zone_of_actor(s.player) != zone {
                continue;
            }
            // the first render makes the renderer (and turns the world light on); the next step makes the fog
            s.render_vk(&mut r, 800, 600).expect("vulkan render");
            // the zone's screen flash settles first
            for _ in 0..60 {
                s.step_bound(1.0 / 30.0);
            }
            if s.fog_draws().is_empty() {
                continue;
            }
            for (si, _) in s.fog_draws() {
                let t = s.fog_texels(si).unwrap();
                let mx = t.iter().fold([0u8; 4], |m, x| [m[0].max(x[0]), m[1].max(x[1]), m[2].max(x[2]), m[3].max(x[3])]);
                eprintln!("  fog surface {si}: max {mx:?}");
            }
            let on = s.render_vk(&mut r, 800, 600).expect("vulkan render");
            // the same frame without the fog layer (the fog maps stay in the atlas)
            r.as_mut().unwrap().world.fog_on = false;
            let off = s.render_vk(&mut r, 800, 600).expect("vulkan render");
            std::fs::write(dir.join("fog_on.png"), dx_soft::png(on.width, on.height, &on.rgba)).unwrap();
            std::fs::write(dir.join("fog_off.png"), dx_soft::png(off.width, off.height, &off.rgba)).unwrap();
            let (mut brighter, mut darker) = (0, 0);
            for (p, q) in on.rgba.chunks(4).zip(off.rgba.chunks(4)) {
                let (a, b) = (p[0] as i32 + p[1] as i32 + p[2] as i32, q[0] as i32 + q[1] as i32 + q[2] as i32);
                if a > b + 6 {
                    brighter += 1;
                } else if a + 6 < b {
                    darker += 1;
                }
            }
            eprintln!("light {a}: {brighter} pixels brighter with fog, {darker} darker");
            assert!(brighter > 500 && darker == 0, "fog not visible or darkening");
            return;
        }
    }
    panic!("no volumetric light in view");
}

/// Movers' surfaces get fog maps as level surfaces do (`SetupForSurf` on a dynamic surface: the brush's lightmap in the
/// surface's world frame): in NSF HQ, a camera behind a volumetric light looking at a mover through it makes fog
/// rectangles for the mover's own world; looking away clears them.
#[test]
fn movers_seen_through_a_volumetric_light_get_fog_maps() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&game, "04_NYC_NSFHQ").unwrap();
    s.enable_world_light();
    s.step_bound(1.0 / 30.0);
    let mut vols = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &a in &s.vm.model_light_actors.clone() {
        if a == 0 || !seen.insert(a) || s.vm.u8p(a, "VolumeBrightness") == 0 || s.vm.u8p(a, "VolumeRadius") == 0 {
            continue;
        }
        let zone = s.vm.zone_of_actor(a);
        if zone != 0 && s.vm.bp(zone, "bFogZone") {
            vols.push((a, zone));
        }
    }
    let mover = s.vm.reg.class_by_name("Mover").unwrap();
    let movers: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, mover)).unwrap_or(false)).collect();
    let level_surfs = s.view.model.surfs.len();
    for &m in &movers {
        let Some((model, nodes, _)) = s.tracked_model(m) else { continue };
        let pts: Vec<[f32; 3]> = nodes.iter().flat_map(|&n| (0..model.nodes[n].num_vertices as usize).map(|k| model.points[model.verts[model.nodes[n].i_vert_pool as usize + k].p_vertex as usize]).collect::<Vec<_>>()).collect();
        if pts.is_empty() {
            continue;
        }
        let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
        for &(a, zone) in &vols {
            let at = s.vm.location(a);
            let rv = (s.vm.u8p(a, "VolumeRadius") as f32 + 1.0) * 25.0;
            let d = [at[0] - c[0], at[1] - c[1]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
            if !(1.0..=1500.0).contains(&l) {
                continue;
            }
            let dir = [d[0] / l, d[1] / l];
            let spot = [at[0] + dir[0] * (rv + 60.0), at[1] + dir[1] * (rv + 60.0), at[2]];
            s.teleport([spot[0], spot[1], spot[2] - 40.0], 0.0);
            let p = s.player_location();
            if (p[0] - spot[0]).abs() > 2.0 || (p[1] - spot[1]).abs() > 2.0 || s.vm.zone_of_actor(s.player) != zone {
                continue;
            }
            let yaw = ((-dir[1]).atan2(-dir[0]).to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
            let to = [c[0] - spot[0], c[1] - spot[1], c[2] - spot[2]];
            let pitch = ((to[2] / (to[0] * to[0] + to[1] * to[1]).sqrt()).atan().to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
            s.fog_updates();
            s.mover_fog_updates();
            s.mark_rendered(&Camera { pos: spot, pitch, yaw, fov: 90.0, roll: 0 });
            let dynamic: Vec<usize> = s.fog_draws().into_iter().map(|(si, _)| si).filter(|&si| si >= level_surfs).collect();
            // the main view's (frame 0)
            let rects: Vec<(usize, (u32, u32, u32, u32, Vec<u8>))> = s.mover_fog_updates().into_iter().filter(|e| e.1 == 0).map(|(e, _, r)| (e, r)).collect();
            // every fogged mover surface sent a rectangle (bytes doubled, a `BORDER`-texel border)
            assert_eq!(rects.len(), dynamic.len());
            if !rects.iter().any(|(_, r)| r.4.chunks(4).any(|t| t[3] > 0)) {
                continue;
            }
            for &si in &dynamic {
                let t = s.fog_texels(si).unwrap();
                assert!(rects.iter().any(|(_, r)| (r.2 - 2 * dx_light::BORDER) as usize * (r.3 - 2 * dx_light::BORDER) as usize == t.len()), "surface {si}");
            }
            // looking the other way: no fog draw on the mover, its rectangles cleared
            s.mark_rendered(&Camera { pos: spot, pitch: 0, yaw: (yaw + 32768) & 0xffff, fov: 90.0, roll: 0 });
            let still: Vec<usize> = s.fog_draws().into_iter().map(|(si, _)| si).filter(|&si| si >= level_surfs).collect();
            let cleared: Vec<(usize, (u32, u32, u32, u32, Vec<u8>))> = s.mover_fog_updates().into_iter().filter(|e| e.1 == 0).map(|(e, _, r)| (e, r)).collect();
            assert!(still.is_empty());
            assert_eq!(cleared.len(), rects.len());
            assert!(cleared.iter().all(|(e, r)| rects.iter().any(|(e2, _)| e2 == e) && r.4.iter().all(|&b| b == 0)));
            println!("mover {m} seen through light {a}: {} fog rectangles, cleared after", rects.len());
            return;
        }
    }
    panic!("no mover seen through a volumetric light");
}

/// Every frame of the walk makes its own fog maps with its own camera (`SetupForSurf` reads the frame's coordinates;
/// `OccludeBsp` bumps the stamp, so each frame gathers its fog lights afresh): in the Wan Chai underworld, looking down
/// at a shiny floor (a translucent mirror) in the fog, the mirror frame has fog draws of its own, their rectangles go
/// to that frame's atlas (`child_fog_updates`), its fogged surfaces get no detail layers, and looking away clears them.
#[test]
fn mirror_frames_make_their_own_fog_maps() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&game, "06_HongKong_WanChai_Underworld").unwrap();
    s.enable_world_light();
    s.step_bound(1.0 / 30.0);
    let model = s.view.model.clone();
    for nd in &model.nodes {
        if nd.i_surf < 0 || nd.num_vertices < 3 || model.surfs[nd.i_surf as usize].poly_flags & 0x800_0000 == 0 || nd.plane[2] < 0.9 {
            continue;
        }
        let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| model.points[model.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
        let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
        let eye = [c[0], c[1], c[2] + 120.0];
        let z = s.vm.point_region(eye).zone_number as usize;
        let za = s.vm.zone_actors.get(z).copied().unwrap_or(0);
        if za == 0 || !s.vm.bp(za, "bFogZone") {
            continue;
        }
        // the player stands there (the walk gathers fog lights only when the viewer is in a fog zone)
        s.teleport([eye[0], eye[1], eye[2] - 40.0], 0.0);
        s.fog_updates();
        s.child_fog_updates();
        s.mark_rendered(&Camera { pos: eye, pitch: 49152, yaw: 0, fov: 90.0, roll: 0 });
        let level_surfs = model.surfs.len();
        let Some(fi) = (1..s.vis_frames().len()).find(|&fi| s.frame_fog_draws(fi).iter().any(|(si, _)| *si < level_surfs)) else { continue };
        let draws: Vec<usize> = s.frame_fog_draws(fi).into_iter().map(|(si, _)| si).filter(|&si| si < level_surfs).collect();
        let rects: Vec<(usize, (u32, u32, u32, u32, Vec<u8>))> = s.child_fog_updates();
        let mine: Vec<&(u32, u32, u32, u32, Vec<u8>)> = rects.iter().filter(|(f, _)| *f == fi).map(|(_, r)| r).collect();
        // one rectangle per fogged level surface of the frame (the lightmap's size plus a border), fog in some
        assert_eq!(mine.len(), draws.len());
        for &si in &draws {
            let t = s.frame_fog_texels(fi, si).unwrap();
            assert!(mine.iter().any(|r| (r.2 - 2 * dx_light::BORDER) as usize * (r.3 - 2 * dx_light::BORDER) as usize == t.len()), "surface {si}");
        }
        assert!(mine.iter().any(|r| r.4.chunks(4).any(|t| t[3] > 0)), "no fog in the mirror frame's maps");
        // the frame's own fog maps take its detail layers away
        let (solid, by_node) = s.frame_details(fi);
        let surf_of = |n: u32| s.vm.world_model.as_ref().unwrap().nodes.get(n as usize).map(|nd| nd.i_surf as usize);
        assert!(solid.iter().chain(by_node.values().flatten()).all(|p| !draws.contains(&surf_of(p.node).unwrap_or(usize::MAX))));
        // looking up: the frame's rectangles are cleared (or redone if a new frame of that index has fog there)
        s.mark_rendered(&Camera { pos: eye, pitch: 16384, yaw: 0, fov: 90.0, roll: 0 });
        let after = s.child_fog_updates();
        for r in &mine {
            assert!(after.iter().any(|(f, a)| *f == fi && (a.0, a.1) == (r.0, r.1)), "rectangle at {},{} not cleared", r.0, r.1);
        }
        println!("frame {fi}: {} fogged surfaces, cleared after", draws.len());
        return;
    }
    panic!("no shiny floor in fog found");
}

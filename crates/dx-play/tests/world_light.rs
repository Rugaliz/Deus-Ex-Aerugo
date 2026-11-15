//! The per-frame part of the world lightmaps (`FLightManager::SetupForSurf`): lights of the stored lists that are not
//! baked (animated `LightType`s, waver effects, lights that are not static) are merged every frame with their
//! brightness of the moment. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use dx_soft::Camera;
use std::collections::HashMap;
use std::path::PathBuf;

/// A camera 120 units in front of surface `si` (at the centre of a polygon drawing it), looking at it.
fn camera_at_surface(s: &Session, si: usize) -> Option<Camera> {
    let m = &s.view.model;
    let nd = m.nodes.iter().find(|n| n.i_surf == si as i32 && n.num_vertices >= 3)?;
    let pts: Vec<[f32; 3]> = (0..nd.num_vertices as usize).map(|k| m.points[m.verts[nd.i_vert_pool as usize + k].p_vertex as usize]).collect();
    let c: [f32; 3] = std::array::from_fn(|k| pts.iter().map(|p| p[k]).sum::<f32>() / pts.len() as f32);
    let n = [nd.plane[0], nd.plane[1], nd.plane[2]];
    let pos = [c[0] + n[0] * 120.0, c[1] + n[1] * 120.0, c[2] + n[2] * 120.0];
    let yaw = ((-n[1]).atan2(-n[0]).to_degrees() / 360.0 * 65536.0) as i32 & 0xffff;
    let pitch = ((-n[2]).asin().to_degrees() / 360.0 * 65536.0) as i32;
    Some(Camera { pos, pitch, yaw, fov: 75.0, roll: 0 })
}

/// A light of the stored lists that matches `pick` makes the surfaces it lights change from frame to frame.
fn surfaces_of_a_light_change(map: &str, pick: &dyn Fn(&mut Session, u32) -> bool) {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&game, map).unwrap();
    let all = s.vm.model_light_actors.clone();
    let light = all.iter().copied().find(|&a| a != 0 && s.vm.u8p(a, "LightBrightness") > 0 && s.vm.u8p(a, "LightType") != 0 && pick(&mut s, a)).expect("a matching light");
    let slots: Vec<usize> = all.iter().enumerate().filter(|(_, &a)| a == light).map(|(i, _)| i).collect();
    let surfs: Vec<usize> = (0..s.view.model.surfs.len()).filter(|&si| dx_light::surface_lights(&s.view.model, si).iter().any(|(at, _)| slots.contains(at))).collect();
    s.enable_world_light();
    let mut checked = 0;
    for &si in surfs.iter().take(6) {
        let Some(cam) = camera_at_surface(&s, si) else { continue };
        let mut seen: HashMap<(u32, u32), Vec<Vec<u8>>> = HashMap::new();
        for _ in 0..20 {
            s.step_bound(1.0 / 30.0);
            let _ = s.lightmap_updates();
            s.mark_rendered(&cam);
            for (x, y, w, h, rgba) in s.lightmap_updates() {
                assert_eq!(rgba.len(), (w * h * 4) as usize);
                seen.entry((x, y)).or_default().push(rgba);
            }
        }
        let varying = seen.values().filter(|v| v.windows(2).any(|p| p[0] != p[1])).count();
        eprintln!("{map} surface {si}: {} rectangles updated, {varying} change over time", seen.len());
        if varying > 0 {
            checked += 1;
        }
    }
    // (a surface at the edge of the light's reach may not change)
    assert!(checked > 0, "no surface of the light changed over time ({} surfaces)", surfs.len());
}

#[test]
fn a_pulsing_light_changes_the_lightmap_every_frame() {
    surfaces_of_a_light_change("06_HongKong_WanChai_Market", &|s, a| s.vm.u8p(a, "LightType") == 2);
}

/// Time-dependent spatial effects (rotor, disco, slow wave) are recomputed every frame.
#[test]
fn moving_light_patterns_change_the_lightmap_every_frame() {
    surfaces_of_a_light_change("09_NYC_ShipFan", &|s, a| s.vm.u8p(a, "LightEffect") == 18);
    surfaces_of_a_light_change("06_HongKong_WanChai_Underworld", &|s, a| s.vm.u8p(a, "LightEffect") == 10);
    surfaces_of_a_light_change("09_NYC_ShipBelow", &|s, a| s.vm.u8p(a, "LightEffect") == 5);
}

/// A light on an actor that is not static (here a decoration given a light) reaches the surfaces in front of it every
/// frame without shadows (`SetupDynamics` light items, `SurfLights`): their lightmap gets brighter, and back to the
/// static texels when the light is switched off.
#[test]
fn a_dynamic_light_brightens_the_wall_in_front_of_it() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&game, "01_NYC_UNATCOIsland").unwrap();
    s.enable_world_light();
    s.step_bound(1.0 / 30.0);
    let class = s.vm.reg.class_by_name("Barrel1").unwrap();
    let mut tried = 0;
    // walls (vertical lightmapped surfaces) near the player start
    let start = s.player_location();
    let m = s.view.model.clone();
    let mut walls: Vec<(f32, usize)> = (0..m.surfs.len())
        .filter(|&si| m.surfs[si].i_light_map >= 0 && m.surfs[si].poly_flags & 0x10_0000 == 0)
        .filter_map(|si| {
            let nd = m.nodes.iter().find(|n| n.i_surf == si as i32 && n.num_vertices >= 3)?;
            if nd.plane[2].abs() > 0.2 {
                return None;
            }
            let p = m.points[m.verts[nd.i_vert_pool as usize].p_vertex as usize];
            Some((((p[0] - start[0]).powi(2) + (p[1] - start[1]).powi(2)).sqrt(), si))
        })
        .collect();
    walls.sort_by(|a, b| a.0.total_cmp(&b.0));
    for &(_, si) in walls.iter().take(30) {
        let Some(cam) = camera_at_surface(&s, si) else { continue };
        let n = m.nodes.iter().find(|n| n.i_surf == si as i32).unwrap().plane;
        let spot = [cam.pos[0] - n[0] * 60.0, cam.pos[1] - n[1] * 60.0, cam.pos[2] - n[2] * 60.0];
        let Ok(Some(a)) = s.vm.spawn(class, 0, None, Some(spot), None) else { continue };
        tried += 1;
        for (name, v) in [("LightType", 1u8), ("LightBrightness", 255), ("LightRadius", 12), ("LightSaturation", 255)] {
            s.vm.set_prop(a, name, 0, ue_vm::Value::Byte(v)).unwrap();
        }
        let _ = s.lightmap_updates();
        s.mark_rendered(&cam);
        let filed = s.vis_frames().iter().any(|f| f.vis.surf_lights.contains(&(si as u32, a)));
        let lit: Vec<_> = s.lightmap_updates();
        // switched off: the surface goes back to its static texels
        s.vm.set_prop(a, "LightType", 0, ue_vm::Value::Byte(0)).unwrap();
        s.mark_rendered(&cam);
        let back: Vec<_> = s.lightmap_updates();
        let _ = s.vm.destroy(a);
        if !filed {
            continue;
        }
        let rect = back.iter().find(|r| lit.iter().any(|l| (l.0, l.1) == (r.0, r.1))).expect("the surface restored once the light is off");
        let on = lit.iter().find(|l| (l.0, l.1) == (rect.0, rect.1)).unwrap();
        let sum = |v: &Vec<u8>| v.chunks(4).map(|c| c[0] as u32 + c[1] as u32 + c[2] as u32).sum::<u32>();
        eprintln!("surface {si}: lit sum {} vs static {}", sum(&on.4), sum(&rect.4));
        assert!(sum(&on.4) > sum(&rect.4), "the dynamic light does not brighten the wall");
        return;
    }
    panic!("no wall got the dynamic light ({tried} tried)");
}

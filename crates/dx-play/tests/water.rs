//! Water textures in the game world. Set `DEUSEX_DIR` to the game folder.
use dx_play::Session;
use std::path::PathBuf;

const DT: f32 = 1.0 / 60.0;

/// Water of the Hong Kong canal (`Effects.water.drtywater_a`) seen from above at two moments (`../out/screenshots/s13_water_*.png`), from a free camera;
/// needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_water() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    // DX_WATER_MAP picks another map
    let map = std::env::var("DX_WATER_MAP").unwrap_or_else(|_| "06_HongKong_WanChai_Canal".into());
    let mut s = Session::open(&g, &map).unwrap();
    for _ in 0..5 {
        s.step_bound(DT);
    }
    // a point of a water face within 3000 units of the player with open sky above it (a pier hides much of the water)
    let start = s.player_location();
    let bsp = s.vm.bsp.as_ref().unwrap();
    let mut spot = None;
    'faces: for face in s.view.model.faces() {
        let surf = &s.view.model.surfs[face.surf];
        let Some(key) = s.view.texture_key(surf.texture) else { continue };
        if !s.view.is_procedural(&key) {
            continue;
        }
        let v = &face.vertices;
        let n = v.len() as f32;
        let c = v.iter().fold([0.0; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n]);
        if std::env::var_os("DX_WATER_PROBE").is_some() {
            let nrm = s.view.model.vectors.get(surf.v_normal as usize).copied().unwrap_or_default();
            eprintln!("water face {key:?} flags {:#x} normal {nrm:?} light_map {} centre {c:?}", surf.poly_flags, surf.i_light_map);
        }
        for p in v {
            for t in [0.0, 0.3, 0.6] {
                let q = [c[0] + (p[0] - c[0]) * t, c[1] + (p[1] - c[1]) * t, c[2] + 2.0];
                if ((q[0] - start[0]).powi(2) + (q[1] - start[1]).powi(2)).sqrt() > 3000.0 {
                    continue;
                }
                if bsp.line_check([q[0], q[1], q[2] + 400.0], q, [0.0; 3], 0).is_none() {
                    spot = Some(q);
                    break 'faces;
                }
            }
        }
    }
    let c = spot.expect("open water");
    eprintln!("open water at {c:?}, player at {start:?}");
    // a free camera 250 units above it, looking down at 45 degrees away from the start
    let dir = [start[0] - c[0], start[1] - c[1]];
    let pos = [c[0], c[1], c[2] + 250.0];
    let yaw = ((-dir[1]).atan2(-dir[0]) / std::f32::consts::TAU * 65536.0) as i32 & 0xFFFF;
    let pitch = -8192 & 0xFFFF;
    let cam = dx_soft::Camera { pos, pitch, yaw, fov: 90.0, roll: 0 };
    let mut r = None;
    s.render_vk(&mut r, 800, 600).expect("vulkan render");
    let mut rr = r.take().unwrap();
    let dir_out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir_out).unwrap();
    let mut frames = Vec::new();
    let t0 = rr.time;
    for (i, t) in [(0, 2.0), (1, 2.5)] {
        rr.time = t0 + t;
        let px = rr.render(&mut s.view, &cam, &[], 800, 600).expect("render");
        std::fs::write(dir_out.join(format!("s13_water_{i}.png")), dx_soft::png(800, 600, &px)).unwrap();
        frames.push(px);
    }
    let changed = frames[0].chunks(4).zip(frames[1].chunks(4)).filter(|(x, y)| x != y).count();
    eprintln!("{changed} pixels changed in half a second");
    assert!(changed > 1000);
    rr.destroy();
}

/// Surfaces of a map grouped by flags, with their textures (`DX_WATER_MAP`, default Liberty Island): finding which
/// portal or mirror surfaces exist.
#[test]
#[ignore]
fn survey_portal_and_mirror_surfaces() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let map = std::env::var("DX_WATER_MAP").unwrap_or_else(|_| "01_NYC_UNATCOIsland".into());
    let s = Session::open(&g, &map).unwrap();
    let mut seen = std::collections::BTreeMap::new();
    for face in s.view.model.faces() {
        let surf = &s.view.model.surfs[face.surf];
        if surf.poly_flags & (0x0400_0000 | 0x0800_0000) == 0 {
            continue;
        }
        let tex = s.view.texture_key(surf.texture).map(|(p, e)| format!("{p}:{e}")).unwrap_or_default();
        *seen.entry((surf.poly_flags, tex)).or_insert(0) += 1;
    }
    for ((flags, tex), n) in seen {
        eprintln!("flags {flags:#010x} texture {tex} faces {n}");
    }
}

/// A burning barrel (`BarrelFire`, its flame skin is the fire texture `Effects.Fire.Flame_J`) spawned in front of the
/// player on Liberty Island, at two moments (`../out/screenshots/s13_fire_*.png`); needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_a_burning_barrel() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(DT);
    }
    let p = s.player_location();
    let yaw = s.yaw as f32 / 65536.0 * std::f32::consts::TAU;
    let at = [p[0] + yaw.cos() * 110.0, p[1] + yaw.sin() * 110.0, p[2] - 20.0];
    let class = s.vm.reg.class_by_name("BarrelFire").expect("BarrelFire");
    s.vm.spawn(class, 0, None, Some(at), None).expect("spawn").expect("barrel");
    let mut r = None;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    let mut frames = Vec::new();
    for i in 0..2 {
        for _ in 0..15 {
            s.step_bound(DT);
        }
        let img = s.render_vk(&mut r, 800, 600).expect("render");
        std::fs::write(dir.join(format!("s13_fire_{i}.png")), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
        frames.push(img.rgba);
    }
    let changed = frames[0].chunks(4).zip(frames[1].chunks(4)).filter(|(a, b)| a != b).count();
    eprintln!("{changed} pixels changed in a quarter second");
    assert!(changed > 200);
    r.take().unwrap().destroy();
}

/// The screen flash (`FlashScale`/`FlashFog`, drawn like the Direct3D driver's `EndFlash`): the healing augmentation's
/// blue flash (`ClientFlash(0.5, vect(0, 0, 500))`) tints the whole picture, HUD included. Needs a Vulkan device.
#[test]
#[ignore]
fn the_healing_flash_tints_the_screen() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(DT);
    }
    let mut r = None;
    let mean = |px: &[u8]| {
        let n = (px.len() / 4) as f64;
        let sum = |k: usize| px.chunks(4).map(|p| p[k] as f64).sum::<f64>() / n;
        (sum(0), sum(1), sum(2))
    };
    let before = mean(&s.render_vk(&mut r, 400, 300).expect("render").rgba);
    assert_eq!(s.screen_flash().map(|f| f.1), Some([0.5; 3]), "no flash at rest");
    let p = s.player;
    s.vm.call_by_name(p, "ClientFlash", vec![ue_vm::Value::Float(0.5), ue_vm::Value::Vector([0.0, 0.0, 500.0])]).expect("ClientFlash");
    for _ in 0..6 {
        s.step_bound(DT);
    }
    let flash = s.screen_flash().unwrap();
    let img = s.render_vk(&mut r, 400, 300).expect("render");
    let after = mean(&img.rgba);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::write(dir.join("s13_flash_heal.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    eprintln!("flash {flash:?}; mean colour before {before:.1?} after {after:.1?}");
    assert!(flash.0[2] > 0.1, "blue fog builds up");
    assert!(after.2 > before.2 + 20.0, "the picture turns blue");
    r.take().unwrap().destroy();
}

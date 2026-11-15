//! Water textures generated from the game's own parameters. Set `DEUSEX_DIR` to the game folder.
use dx_soft::MapView;
use std::path::PathBuf;

const WATER: [&str; 9] = [
    "Effects.water.bluewater_a",
    "Effects.water.drtywater_a",
    "Effects.water.waterfntn_a",
    "Effects.Smoke.Gas_Tear_A",
    "Effects.Smoke.Gas_Poison_A",
    "Effects.Electricity.WavyBlade",
    "Effects.Electricity.Xplsn_EMPG",
    "Effects.UserInterface.DrunkFX",
    "Effects.UserInterface.DrunkBoy",
];

#[test]
fn every_water_texture_ripples() {
    let Some(dir) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut view = MapView::open(&dir, "DX").expect("map");
    for path in WATER {
        let key = view.procedural_key_by_path(path).unwrap_or_else(|| panic!("{path} is not procedural"));
        let (w, h, first) = view.procedural_frame(&key, 1.0).map(|(w, h, p)| (w, h, p.to_vec())).unwrap_or_else(|| panic!("{path} not generated"));
        assert_eq!(first.len(), (w * h * 4) as usize);
        // half a second later at 60 frames per second the water has moved
        let mut last = first.clone();
        for i in 1..=30 {
            last = view.procedural_frame(&key, 1.0 + i as f64 / 60.0).unwrap().2.to_vec();
        }
        let changed = first.chunks(4).zip(last.chunks(4)).filter(|(a, b)| a != b).count();
        eprintln!("{path}: {w}x{h}, {changed} pixels changed");
        assert!(changed > 0, "{path} did not change");
    }
}

#[test]
fn ordinary_textures_are_not_procedural() {
    let Some(dir) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let view = MapView::open(&dir, "DX").expect("map");
    assert!(view.procedural_key_by_path("DeusExDeco.Skins.BarrelFireTex1").is_none());
    assert!(view.procedural_key_by_path("Effects.Smoke.SmokePuff1").is_some(), "a fire texture");
}

/// Pictures of some water textures after 2 s (into `../out/screenshots/`, to look at).
#[test]
#[ignore]
fn shots_of_water() {
    let Some(dir) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut view = MapView::open(&dir, "DX").expect("map");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&out).unwrap();
    for path in WATER {
        let key = view.procedural_key_by_path(path).unwrap();
        for i in 0..=120 {
            view.procedural_frame(&key, 1.0 + i as f64 / 60.0);
        }
        let (w, h, px) = view.procedural_frame(&key, 3.5).unwrap();
        let name = path.rsplit('.').next().unwrap();
        std::fs::write(out.join(format!("water_{name}.png")), dx_soft::png(w, h, px)).unwrap();
    }
}

/// Every `FireTexture` of `Effects.utx` generates; shots of some after 2 s (`../out/screenshots/fire_*.png`, ignored test
/// below). Returns (path, changed pixels over half a second).
fn fire_textures(view: &mut MapView) -> Vec<(String, usize)> {
    let pkg = view.procedural_key_by_path("Effects.Fire.Flame_J").expect("Flame_J").0;
    let mut out = Vec::new();
    for path in FIRE {
        let key = view.procedural_key_by_path(path).unwrap_or_else(|| panic!("{path} is not procedural"));
        assert_eq!(key.0, pkg);
        let first = view.procedural_frame(&key, 1.0).unwrap_or_else(|| panic!("{path} not generated")).2.to_vec();
        let mut last = first.clone();
        for i in 1..=30 {
            last = view.procedural_frame(&key, 1.0 + i as f64 / 60.0).unwrap().2.to_vec();
        }
        out.push((path.to_string(), first.chunks(4).zip(last.chunks(4)).filter(|(a, b)| a != b).count()));
    }
    out
}

const FIRE: [&str; 12] = [
    "Effects.Fire.Flame_J",
    "Effects.Fire.flame_b",
    "Effects.Fire.OnFire_J",
    "Effects.Fire.flmethrwr_fire",
    "Effects.Fire.SparkFX1",
    "Effects.Electricity.Nano_SFX",
    "Effects.Electricity.RadAttack",
    "Effects.Laser.LaserBeam1",
    "Effects.UserInterface.WhiteStatic",
    "Effects.water.Fountain_FX",
    "Effects.Smoke.SmokePuff1",
    "Effects.liquid.ambrosia_SFX",
];

#[test]
fn fire_textures_burn() {
    let Some(dir) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut view = MapView::open(&dir, "DX").expect("map");
    for (path, changed) in fire_textures(&mut view) {
        eprintln!("{path}: {changed} pixels changed");
        assert!(changed > 0, "{path} did not change");
    }
}

#[test]
#[ignore]
fn shots_of_fire() {
    let Some(dir) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut view = MapView::open(&dir, "DX").expect("map");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&out).unwrap();
    for path in FIRE {
        let key = view.procedural_key_by_path(path).unwrap();
        for i in 0..=120 {
            view.procedural_frame(&key, 1.0 + i as f64 / 60.0);
        }
        let (w, h, px) = view.procedural_frame(&key, 3.5).unwrap();
        let name = path.rsplit('.').next().unwrap();
        std::fs::write(out.join(format!("fire_{name}.png")), dx_soft::png(w, h, px)).unwrap();
    }
}

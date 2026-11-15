//! Checks the texel layout and the bit order of the shadow bits against the BSP: a texel the light can see (a line from just
//! in front of the texel to the light is free) should have its bit set, a texel it cannot see should not.
//! Needs `DEUSEX_DIR`.
use dx_light::{collect, shadow_bit, texel_positions};
use std::path::PathBuf;
use ue_assets::files::GameFiles;
use ue_assets::level::Level;
use ue_assets::model::Model;
use ue_package::ObjectRef;
use ue_registry::Registry;

/// Returns (P(bit set | light sees the texel), P(bit set | line blocked), number of texel/light pairs) over texels in open space
/// and within the light's radius. The bits are a superset of the BSP's line of sight (lamps sit inside their fixtures, which
/// blocks our line test but not the original's), so the first number must be near 1 and the second clearly lower.
fn agreement(game: &std::path::Path, map: &str) -> (f64, f64, usize) {
    let mut reg = Registry::load_game(game).unwrap();
    let pi = reg.add_package(&game.join("Maps").join(format!("{map}.dx")), map).unwrap();
    let files = GameFiles::new(game);
    let loaded = files.package(map).unwrap();
    let level_export = (0..loaded.pkg.exports.len()).find(|&i| loaded.pkg.export_class_name(i) == "Level").unwrap();
    let level = Level::load(&loaded.pkg, level_export).unwrap();
    let ObjectRef::Export(me) = ObjectRef::from_index(level.model) else { panic!() };
    let model = Model::load(&loaded.pkg, me).unwrap();
    let bsp = ue_bsp::Bsp::from_model(&model);
    let lighting = collect(&mut reg, pi, &model);
    // [seen][bit]
    let mut c = [[0usize; 2]; 2];
    for si in (0..model.surfs.len()).step_by(5) {
        let surf = &model.surfs[si];
        let Some(lm) = usize::try_from(surf.i_light_map).ok().and_then(|i| model.light_map.get(i)) else { continue };
        if lm.i_light_actors < 0 {
            continue;
        }
        let Some((w, h, pos, n)) = texel_positions(&model, si) else { continue };
        let mut k = 0;
        while model.lights.get(lm.i_light_actors as usize + k).copied().unwrap_or(0) != 0 {
            let Some(Some(light)) = lighting.lights.get(lm.i_light_actors as usize + k) else { k += 1; continue };
            let side = if (0..3).map(|a| (light.pos[a] - pos[0][a]) * n[a]).sum::<f32>() >= 0.0 { 1.0 } else { -1.0 };
            for j in 0..h {
                for i in 0..w {
                    let p = pos[j * w + i];
                    let d = [p[0] - light.pos[0], p[1] - light.pos[1], p[2] - light.pos[2]];
                    if (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() > light.radius() {
                        continue;
                    }
                    let q = [p[0] + n[0] * side * 2.0, p[1] + n[1] * side * 2.0, p[2] + n[2] * side * 2.0];
                    if !bsp.point_region(q).outside {
                        continue;
                    }
                    let seen = bsp.fast_line_check(q, light.pos);
                    c[seen as usize][shadow_bit(&model, si, k, i, j).unwrap() as usize] += 1;
                }
            }
            k += 1;
        }
    }
    let ratio = |a: usize, b: usize| a as f64 / (a + b).max(1) as f64;
    (ratio(c[1][1], c[1][0]), ratio(c[0][1], c[0][0]), c.iter().flatten().sum())
}

#[test]
fn shadow_bits_follow_line_of_sight() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    for map in ["00_Training", "02_NYC_Street"] {
        let (seen, blocked, n) = agreement(&game, map);
        eprintln!("{map}: {n} texel/light pairs; bit set for {:.1}% of seen texels, {:.1}% of blocked ones", seen * 100.0, blocked * 100.0);
        assert!(n > 100_000);
        assert!(seen > 0.97, "{map}: {:.1}% of the texels a light sees have their bit set", seen * 100.0);
        assert!(blocked < seen - 0.3, "{map}: bits do not depend on visibility ({:.1}% vs {:.1}%)", blocked * 100.0, seen * 100.0);
    }
}

/// The engine reads these fields at fixed offsets; the class layouts we computed must agree
/// (Actor: LightType 0x1e0, LightEffect 0x1e1, LightBrightness 0x1e2, LightHue 0x1e3, LightSaturation 0x1e4, LightRadius 0x1e5;
/// ZoneInfo: AmbientBrightness 0x38c, hue 0x38d, saturation 0x38e; LevelInfo: Brightness 0x500).
#[test]
fn light_property_offsets_match_the_original_code() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let reg = Registry::load_game(&game).unwrap();
    let off = |class: &str, prop: &str| {
        let c = reg.class_by_name(class).unwrap();
        reg.find_prop(c, prop).unwrap().offset
    };
    for (p, o) in [("LightType", 0x1e0), ("LightEffect", 0x1e1), ("LightBrightness", 0x1e2), ("LightHue", 0x1e3), ("LightSaturation", 0x1e4), ("LightRadius", 0x1e5), ("LightPeriod", 0x1e6), ("LightPhase", 0x1e7), ("LightCone", 0x1e8)] {
        assert_eq!(off("Light", p), o, "{p}");
    }
    for (p, o) in [("AmbientBrightness", 0x38c), ("AmbientHue", 0x38d), ("AmbientSaturation", 0x38e)] {
        assert_eq!(off("ZoneInfo", p), o, "{p}");
    }
    assert_eq!(off("LevelInfo", "Brightness"), 0x500);
}

/// Whole-map generation: every lightmapped surface gets a rectangle inside the atlas, no channel exceeds 127, and the result
/// is neither black nor saturated.
#[test]
fn generated_atlas_is_sane() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let map = "00_Training";
    let mut reg = Registry::load_game(&game).unwrap();
    let pi = reg.add_package(&game.join("Maps").join(format!("{map}.dx")), map).unwrap();
    let files = GameFiles::new(&game);
    let loaded = files.package(map).unwrap();
    let le = (0..loaded.pkg.exports.len()).find(|&i| loaded.pkg.export_class_name(i) == "Level").unwrap();
    let level = Level::load(&loaded.pkg, le).unwrap();
    let ObjectRef::Export(me) = ObjectRef::from_index(level.model) else { panic!() };
    let model = Model::load(&loaded.pkg, me).unwrap();
    let lighting = collect(&mut reg, pi, &model);
    assert!(lighting.lights.iter().flatten().count() > 10);
    assert!(lighting.level_brightness > 0.0);
    let maps = dx_light::generate(&model, &lighting);
    let wanted = model.surfs.iter().filter(|s| s.i_light_map >= 0).count();
    assert_eq!(maps.surfaces.iter().flatten().count(), wanted);
    for m in maps.surfaces.iter().flatten() {
        assert!(m.x >= 1 && m.y >= 1 && m.x + m.w < maps.width && m.y + m.h < maps.height);
    }
    let (mut max, mut sum, mut n) = (0u8, 0u64, 0u64);
    for px in maps.rgba.chunks(4) {
        max = max.max(px[0]).max(px[1]).max(px[2]);
        sum += px[0] as u64 + px[1] as u64 + px[2] as u64;
        n += 3;
        assert_eq!(px[3] == 255 || px[3] == 0, true);
    }
    assert!(max <= 127, "channel value {max} above 127");
    let mean = sum as f64 / n as f64;
    eprintln!("atlas {}x{}, mean texel {mean:.1}, max {max}", maps.width, maps.height);
    assert!(mean > 3.0 && mean < 100.0);
}

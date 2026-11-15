//! Compares the Vulkan world render with the CPU reference renderer on real maps. Needs `DEUSEX_DIR` and a Vulkan device.
use dx_soft::{Camera, MapView};
use dx_vk::world::{render_world, World};
use dx_vk::Context;
use std::path::Path;

const CLEAR: [u8; 3] = [24, 28, 40];

fn parity(cx: &Context, game: &Path, map: &str, w: u32, h: u32) -> (f64, f64) {
    let mut view = MapView::open(game, map).unwrap();
    let (loc, rot) = view.find_actor("PlayerStart").unwrap();
    let cam = Camera { pos: [loc[0], loc[1], loc[2] + 40.0], pitch: 0, yaw: rot[1] as i32, fov: 90.0, roll: 0 };
    let soft = view.render(&cam, w, h).rgba;
    let world = World::new(cx, &mut view, None).unwrap();
    let vk = render_world(cx, &world, &cam, w, h).unwrap();
    world.destroy(cx);
    let (mut cover_diff, mut color_sum, mut both) = (0usize, 0u64, 0usize);
    for (a, b) in soft.chunks(4).zip(vk.chunks(4)) {
        let (ea, eb) = (a[..3] == CLEAR, b[..3] == CLEAR);
        if ea != eb {
            cover_diff += 1;
        } else if !ea {
            both += 1;
            color_sum += (0..3).map(|i| (a[i] as i32 - b[i] as i32).unsigned_abs() as u64).sum::<u64>();
        }
    }
    (cover_diff as f64 / (w * h) as f64, color_sum as f64 / (both.max(1) * 3) as f64)
}

#[test]
fn vulkan_world_matches_cpu_renderer() {
    let Some(game) = std::env::var_os("DEUSEX_DIR") else { return };
    let cx = match Context::new(true, &[]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    for map in ["00_Training", "01_NYC_UNATCOIsland", "02_NYC_Street", "02_NYC_Bar"] {
        let (cover, color) = parity(&cx, Path::new(&game), map, 480, 270);
        eprintln!("{map}: coverage mismatch {:.3}%, mean colour diff {:.1}/255", cover * 100.0, color);
        assert!(cover < 0.02, "{map}: coverage differs on {:.2}% of pixels", cover * 100.0);
        assert!(color < 12.0, "{map}: mean colour difference {color:.1}");
    }
}

//! The Vulkan view of a running session against the CPU renderer. Needs `DEUSEX_DIR` and a Vulkan device.
use dx_play::{Input, Session};
use std::path::PathBuf;

fn mask(a: &[u8], b: &[u8]) -> Vec<bool> {
    a.chunks(4).zip(b.chunks(4)).map(|(x, y)| x != y).collect()
}

/// Pixels that change when actor meshes are switched on must be nearly the same set in both renderers.
#[test]
fn actor_meshes_cover_the_same_pixels_as_in_the_cpu_renderer() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&game, "02_NYC_Street").unwrap();
    s.draw_sprites = false;
    for _ in 0..30 {
        s.step(&Input::default(), 1.0 / 30.0);
    }
    let (w, h) = (640, 360);
    let mut vk = None;
    s.draw_meshes = false;
    let soft_off = s.render(w, h).rgba;
    let vk_off = match s.render_vk(&mut vk, w, h) {
        Ok(i) => i.rgba,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    s.draw_meshes = true;
    let soft_on = s.render(w, h).rgba;
    let vk_on = s.render_vk(&mut vk, w, h).unwrap().rgba;
    vk.take().unwrap().destroy();
    let (ms, mv) = (mask(&soft_off, &soft_on), mask(&vk_off, &vk_on));
    let inter = ms.iter().zip(&mv).filter(|(a, b)| **a && **b).count();
    let union = ms.iter().zip(&mv).filter(|(a, b)| **a || **b).count();
    let soft_n = ms.iter().filter(|&&m| m).count();
    eprintln!("actor pixels: soft {soft_n}, vk {}, overlap {:.1}%", mv.iter().filter(|&&m| m).count(), 100.0 * inter as f64 / union as f64);
    assert!(soft_n > 2000, "the scene should show actors");
    assert!(inter as f64 / union as f64 > 0.9, "actor coverage differs: IoU {:.3}", inter as f64 / union as f64);
}

/// The window system's HUD reaches the screen: with the HUD on, the belt (bottom centre) and the health display (top left)
/// differ from the bare view, and the middle of the screen is untouched except for the crosshair.
#[test]
fn the_hud_is_drawn_over_the_view() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&game, "00_Training").unwrap();
    for _ in 0..30 {
        s.step(&Input::default(), 1.0 / 30.0);
    }
    let (w, h) = (800u32, 600u32);
    let mut vk = None;
    s.show_ui = false;
    let bare = match s.render_vk(&mut vk, w, h) {
        Ok(i) => i.rgba,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    s.show_ui = true;
    let with = s.render_vk(&mut vk, w, h).unwrap().rgba;
    vk.take().unwrap().destroy();
    let changed = |x0: u32, y0: u32, x1: u32, y1: u32| {
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * w + x) * 4) as usize;
                if bare[i..i + 3] != with[i..i + 3] {
                    n += 1;
                }
            }
        }
        n as f32 / ((x1 - x0) * (y1 - y0)) as f32
    };
    assert!(changed(260, 535, 740, 595) > 0.2, "no belt at the bottom");
    assert!(changed(5, 5, 80, 90) > 0.2, "no health display at the top left");
    assert!(changed(200, 150, 600, 450) < 0.02, "the HUD covers the middle of the screen");
}

//! Renders a few maps from their PlayerStart and checks that the picture is mostly covered by textured geometry.
//! Set `DEUSEX_DIR` to the game folder.

use dx_soft::{Camera, MapView};
use std::path::PathBuf;

#[test]
fn maps_render_from_the_player_start() {
    let Some(dir) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    for map in ["00_Training", "02_NYC_Street", "03_NYC_Airfield"] {
        let mut view = MapView::open(&dir, map).unwrap();
        let (loc, rot) = view.find_actor("PlayerStart").expect("map has a PlayerStart");
        let cam = Camera { pos: [loc[0], loc[1], loc[2] + 40.0], pitch: 0, yaw: rot[1] as i32, fov: 90.0, roll: 0 };
        let img = view.render(&cam, 320, 180);
        let background = [24u8, 28, 40, 255];
        let covered = img.rgba.chunks(4).filter(|p| *p != background).count();
        let frac = covered as f32 / (320.0 * 180.0);
        assert!(frac > 0.6, "{map}: only {:.0}% of the picture is geometry", frac * 100.0);
        // the picture is not a single flat colour
        let first = &img.rgba[..4];
        assert!(img.rgba.chunks(4).any(|p| p != first), "{map}");
    }
}

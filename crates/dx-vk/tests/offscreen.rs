//! Needs a Vulkan device; skips (prints a note) when none can be created.
use dx_vk::{render_triangles, ColorVertex, Context};

const IDENTITY: [f32; 16] = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

fn v(x: f32, y: f32, z: f32, c: [f32; 3]) -> ColorVertex {
    ColorVertex { pos: [x, y, z], color: c }
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

#[test]
fn depth_ordered_triangles_read_back() {
    let cx = match Context::new(true, &[]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let (w, h) = (64, 64);
    let red = [1.0, 0.0, 0.0];
    let green = [0.0, 1.0, 0.0];
    // Far red triangle covers the lower-left half of clip space; the green one is nearer and drawn FIRST,
    // so the result only comes out right if the depth test works.
    let verts = [
        v(-0.2, 0.2, 0.2, green), v(0.6, 0.2, 0.2, green), v(-0.2, -0.6, 0.2, green),
        v(-1.0, 1.0, 0.8, red), v(1.0, 1.0, 0.8, red), v(-1.0, -1.0, 0.8, red),
    ];
    let img = render_triangles(&cx, w, h, &verts, &IDENTITY).unwrap();
    // Clip +Y is up, so row 0 is the top. Pixel (x,y) maps to clip ((x+.5)/32-1, 1-(y+.5)/32).
    assert_eq!(px(&img, w, 28, 28), [0, 255, 0], "near triangle wins over the far one");
    assert_eq!(px(&img, w, 5, 20), [255, 0, 0], "far triangle visible where uncovered");
    // Clear colour (0.05, 0.05, 0.1) in UNORM8: 12.75 -> 13, 25.5 -> 25 or 26 depending on the GPU's rounding.
    let c = px(&img, w, 60, 60);
    assert!(c[0] == 13 && c[1] == 13 && (25..=26).contains(&c[2]), "clear colour outside both: {c:?}");
}

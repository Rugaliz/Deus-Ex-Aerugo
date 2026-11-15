//! Surface clipping and rasterisation (`URender::ClipBspSurf` 10b14cb0, the span rasteriser 10b19cc0).

use crate::frame::{Frame, Vec3};
use crate::span::{Span, SpanBuffer};

/// A clipped, projected point (`FTransform`): view position, outcode, screen position, integer row, `Proj.Z / z`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TPoint {
    pub view: Vec3,
    pub flags: u8,
    pub sx: f32,
    pub sy: f32,
    pub int_y: i32,
    pub rz: f32,
}

const OUT_LEFT: u8 = 0x04;
const OUT_RIGHT: u8 = 0x08;
const OUT_TOP: u8 = 0x10;
const OUT_BOTTOM: u8 = 0x20;

/// `round(v - 0.5)` with the FPU's round-to-nearest-even, as the engine converts screen rows.
fn row(v: f64) -> i32 {
    (v - 0.5).round_ties_even() as i32
}

/// `row` of an f32 value, subtracted in f32 (the rasteriser's pixel box).
fn row32(v: f32) -> i32 {
    (v - 0.5).round_ties_even() as i32
}

// Precision as the x87 code (asm read for `ClipBspSurf` 10b14cb0, its near clipper 10b15540 and projection helper
// 10b12400): what stays on the FPU stack is computed in f64 here, what the code stores is rounded to f32.

impl TPoint {
    /// Screen position from `RZ` kept unrounded (`rz`, the division of `Proj.Z` by `z`): `RZ` stored, `X`/`Y` stored,
    /// the row from the stored `Y` (clipped points).
    fn project_from(&mut self, f: &Frame, rz: f64) {
        self.rz = rz as f32;
        self.sx = (rz * self.view[0] as f64 + f.fx15 as f64) as f32;
        self.sy = (rz * self.view[1] as f64 + f.fy15 as f64) as f32;
        self.int_y = row(self.sy as f64);
    }

    /// Transforms a world point (`ClipBspSurf`'s point cache): view space (`(dx X.x + dy X.y) + dz X.z` per axis, the
    /// differences unrounded), outcode from the stored side distances' sign bits, and the screen position when the
    /// point is inside all four side planes (there the row comes from the unrounded `Y`).
    pub fn from_world(f: &Frame, p: Vec3) -> TPoint {
        let d = [p[0] as f64 - f.origin[0] as f64, p[1] as f64 - f.origin[1] as f64, p[2] as f64 - f.origin[2] as f64];
        let ax = |a: Vec3| ((d[0] * a[0] as f64 + d[1] * a[1] as f64) + d[2] * a[2] as f64) as f32;
        let v = [ax(f.x_axis), ax(f.y_axis), ax(f.z_axis)];
        let mut t = TPoint { view: v, ..Default::default() };
        let side = |prj: f32, c: f32, sign: f64| (v[2] as f64 * prj as f64 + sign * c as f64) as f32;
        let (left, top, right, bottom) = (side(f.prj_xm, v[0], 1.0), side(f.prj_ym, v[1], 1.0), side(f.prj_xp, v[0], -1.0), side(f.prj_yp, v[1], -1.0));
        // the engine looks at the sign bit, so -0.0 counts as outside
        let neg = |x: f32| x.is_sign_negative();
        t.flags = (if neg(bottom) { OUT_BOTTOM } else { 0 }) | (if neg(top) { OUT_TOP } else { 0 }) | (if neg(left) { OUT_LEFT } else { 0 }) | (if neg(right) { OUT_RIGHT } else { 0 });
        t.rz = f.proj_z / v[2];
        if t.flags == 0 {
            t.sx = (v[0] as f64 * t.rz as f64 + f.fx15 as f64) as f32;
            let sy = v[1] as f64 * t.rz as f64 + f.fy15 as f64;
            t.sy = sy as f32;
            t.int_y = row(sy);
        }
        t
    }
}

/// How a clipped point gets its `RZ`: from the unrounded new `z` (side planes but the bottom one, the near plane), or
/// from the stored `z` (the bottom plane projects through 10b12400).
#[derive(Clone, Copy, PartialEq)]
enum NewZ {
    Unrounded,
    Stored,
}

/// Clips a polygon against one plane: `dist` per point (stored as f32; the engine keeps points whose distance's sign
/// bit is clear and adds the crossing point of every edge whose ends differ in sign, from the previous point).
fn clip_side(f: &Frame, pts: &[TPoint], dist: impl Fn(&TPoint) -> f32, z: NewZ) -> Vec<TPoint> {
    let n = pts.len();
    let d: Vec<f32> = pts.iter().map(&dist).collect();
    let mut out = Vec::with_capacity(n + 2);
    let mut prev = n - 1;
    for cur in 0..n {
        if !d[prev].is_sign_negative() {
            out.push(pts[prev]);
        }
        if d[prev].is_sign_negative() != d[cur].is_sign_negative() {
            let t = d[prev] as f64 / (d[prev] as f64 - d[cur] as f64);
            let (a, b) = (pts[prev].view, pts[cur].view);
            let lerp = |k: usize| (b[k] as f64 - a[k] as f64) * t + a[k] as f64;
            let zz = lerp(2);
            let mut p = TPoint { view: [lerp(0) as f32, lerp(1) as f32, zz as f32], ..Default::default() };
            let zr = if z == NewZ::Unrounded { zz } else { p.view[2] as f64 };
            p.project_from(f, f.proj_z as f64 / zr);
            out.push(p);
        }
        prev = cur;
    }
    out
}

/// `ClipBspSurf`: the points of a surface (world positions, in order) clipped to the view; empty when nothing is left.
pub fn clip_polygon(f: &Frame, world: &[Vec3]) -> Vec<TPoint> {
    let mut pts: Vec<TPoint> = world.iter().map(|&p| TPoint::from_world(f, p)).collect();
    if pts.is_empty() {
        return pts;
    }
    let all = pts.iter().fold(0x3c, |a, p| a & p.flags);
    if all != 0 {
        return Vec::new();
    }
    let any = pts.iter().fold(0, |a, p| a | p.flags);
    // a side distance as stored: `z * Prj + c` (or `- c`) rounded to f32
    let side = |prj: f32, k: usize, sign: f64| move |p: &TPoint| (p.view[2] as f64 * prj as f64 + sign * p.view[k] as f64) as f32;
    for (bit, dist, z) in [(OUT_LEFT, side(f.prj_xm, 0, 1.0), NewZ::Unrounded), (OUT_RIGHT, side(f.prj_xp, 0, -1.0), NewZ::Unrounded), (OUT_TOP, side(f.prj_ym, 1, 1.0), NewZ::Unrounded), (OUT_BOTTOM, side(f.prj_yp, 1, -1.0), NewZ::Stored)] {
        if any & bit != 0 {
            pts = clip_side(f, &pts, dist, z);
            if pts.is_empty() {
                return pts;
            }
        }
    }
    if f.near_clip[3] != 0.0 {
        // `((n.z z + n.y y) + n.x x) - W`: the test on the unrounded value, the clip on the stored one
        let pl = f.near_clip;
        let d = |p: &TPoint| ((pl[2] as f64 * p.view[2] as f64 + pl[1] as f64 * p.view[1] as f64) + pl[0] as f64 * p.view[0] as f64) - pl[3] as f64;
        if pts.iter().any(|p| d(p) < 0.0) {
            pts = clip_side(f, &pts, |p| d(p) as f32, NewZ::Unrounded);
        }
    }
    pts
}

/// The raster of a clipped polygon: rows `y1..y2` with one span per row, and the polygon's pixel box.
#[derive(Debug, Clone, Default)]
pub struct Raster {
    pub y1: i32,
    pub y2: i32,
    pub min_x: i32,
    pub max_x: i32,
    pub spans: Vec<Span>,
}

/// The span rasteriser (10b19cc0): rows from the points' integer rows (clamped to the frame), an early test of the
/// pixel box against `test` (when given), then every edge walked in 16.16 fixed point: edges going down the screen set
/// the span ends, edges going up the starts. `None` when the box test fails.
pub fn rasterize(pts: &mut [TPoint], frame_y: i32, test: Option<&SpanBuffer>) -> Option<Raster> {
    let n = pts.len();
    if n == 0 {
        return None;
    }
    let mut y1 = pts[0].int_y;
    let mut y2 = y1;
    let mut min_x = row32(pts[0].sx);
    let mut max_x = min_x;
    for p in &pts[1..] {
        if p.int_y < y1 {
            y1 = p.int_y;
        } else if p.int_y > y2 {
            y2 = p.int_y;
        }
        let x = row32(p.sx);
        if x < min_x {
            min_x = x;
        } else if x > max_x {
            max_x = x;
        }
    }
    // rows outside the frame: clamp the range and every point's row (and screen row) into it
    if y1 < 0 || y2 > frame_y {
        y1 = y1.clamp(0, frame_y);
        y2 = y2.clamp(0, frame_y);
        for p in pts.iter_mut() {
            p.int_y = p.int_y.clamp(0, frame_y);
            p.sy = p.int_y as f32;
        }
    }
    if let Some(t) = test {
        if !t.box_is_visible(min_x, y1, max_x, y2) {
            return None;
        }
    }
    let mut spans = vec![(0, 0); (y2 - y1).max(0) as usize];
    let mut prev = n - 1;
    for cur in 0..n {
        let (a, b) = (pts[prev], pts[cur]);
        if a.int_y != b.int_y {
            let down = a.int_y < b.int_y;
            let (top, bot) = if down { (a, b) } else { (b, a) };
            let slope = ((bot.sx - top.sx) * 65536.0) / (bot.sy - top.sy);
            let mut x = (top.sx * 65536.0 + (top.int_y as f32 - top.sy) * slope - 0.5).round_ties_even() as i32;
            let step = (slope - 0.5).round_ties_even() as i32;
            for yy in top.int_y..bot.int_y {
                x = x.wrapping_add(step);
                if let Some(s) = spans.get_mut((yy - y1) as usize) {
                    if down {
                        s.1 = x >> 16;
                    } else {
                        s.0 = x >> 16;
                    }
                }
            }
        }
        prev = cur;
    }
    Some(Raster { y1, y2, min_x, max_x, spans })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        // looking down +X, 90 degrees, 100x100
        Frame::new([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 90.0, 100, 100)
    }

    #[test]
    fn a_square_straight_ahead_fills_the_middle_of_the_screen() {
        let f = frame();
        // a square 100 units ahead, 50 units wide: at 90 degrees the screen spans +-100 units at that distance, so
        // the square covers 50 +- 12.5 px (rows and columns 37.5..62.5)
        let quad = [[100.0, -25.0, 25.0], [100.0, 25.0, 25.0], [100.0, 25.0, -25.0], [100.0, -25.0, -25.0]];
        let mut pts = clip_polygon(&f, &quad);
        assert_eq!(pts.len(), 4);
        let r = rasterize(&mut pts, f.y, None).unwrap();
        eprintln!("rows {}..{}, x {}..{}, first span {:?}", r.y1, r.y2, r.min_x, r.max_x, r.spans.first());
        assert!((r.y1 - 38).abs() <= 1 && (r.y2 - 63).abs() <= 1);
        for s in &r.spans {
            assert!((s.0 - 38).abs() <= 1 && (s.1 - 63).abs() <= 1, "span {s:?}");
        }
    }

    #[test]
    fn behind_the_camera_is_rejected_and_straddling_is_clipped() {
        let f = frame();
        let behind = [[-100.0, -25.0, 25.0], [-100.0, 25.0, 25.0], [-100.0, 25.0, -25.0], [-100.0, -25.0, -25.0]];
        assert!(clip_polygon(&f, &behind).is_empty());
        // a huge floor-like quad through the camera plane: clipped to the view, rows stay on the screen
        let floor = [[-1000.0, -1000.0, -50.0], [1000.0, -1000.0, -50.0], [1000.0, 1000.0, -50.0], [-1000.0, 1000.0, -50.0]];
        let mut pts = clip_polygon(&f, &floor);
        assert!(pts.len() >= 3);
        let r = rasterize(&mut pts, f.y, None).unwrap();
        assert!(r.y1 >= 0 && r.y2 <= 100 && r.y1 >= 50, "the floor is in the lower half: {}..{}", r.y1, r.y2);
    }

    /// The engine's precision (asm of 10b14cb0): a point inside the view takes its row from the unrounded
    /// `y * RZ + FY15` (the stored `Y` is rounded to f32 first only for clipped points). Somewhere along a fine sweep
    /// the two roundings differ; there the row must be the unrounded one.
    #[test]
    fn rows_of_view_points_come_from_the_unrounded_y() {
        let f = Frame::new([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 75.0, 800, 600);
        let mut differing = 0;
        for k in 0..200_000 {
            let p = [313.7 + k as f32 * 0.000_7, 0.0, -97.0 + k as f32 * 0.000_31];
            let t = TPoint::from_world(&f, p);
            assert_eq!(t.flags, 0);
            let v = f.to_view(p);
            let rz = f.proj_z / v[2];
            let y64 = v[1] as f64 * rz as f64 + f.fy15 as f64;
            let want = (y64 - 0.5).round_ties_even() as i32;
            let from_stored = ((y64 as f32) as f64 - 0.5).round_ties_even() as i32;
            if want != from_stored {
                differing += 1;
            }
            assert_eq!(t.int_y, want, "point {p:?}");
        }
        eprintln!("{differing} points where the stored Y would round to another row");
    }

    /// Clipping keeps the points in the engine's order: each plane starts from the last point (the previous of the
    /// first) and adds a crossing point after the kept one.
    #[test]
    fn clipped_polygons_start_from_the_last_point() {
        let f = frame();
        // a square 100 ahead reaching past the left edge (y = -100 at that distance): its last point is inside
        let quad = [[100.0, -150.0, 25.0], [100.0, 25.0, 25.0], [100.0, 25.0, -25.0], [100.0, -50.0, -25.0]];
        let pts = clip_polygon(&f, &quad);
        assert_eq!(pts.len(), 5);
        let v = |p: &TPoint| [p.view[0].round(), p.view[1].round(), p.view[2].round()];
        // view = (y, -z, x): the last input point, the crossing towards the (outside) first point, the crossing
        // from it to the second point, then the second and third points
        assert_eq!(v(&pts[0]), [-50.0, 25.0, 100.0]);
        assert_eq!(v(&pts[1]), [-100.0, 0.0, 100.0]);
        assert_eq!(v(&pts[2]), [-100.0, -25.0, 100.0]);
        assert_eq!(v(&pts[3]), [25.0, -25.0, 100.0]);
        assert_eq!(v(&pts[4]), [25.0, 25.0, 100.0]);
    }

    #[test]
    fn the_box_test_rejects_a_hidden_polygon() {
        let f = frame();
        let quad = [[100.0, -25.0, 25.0], [100.0, 25.0, 25.0], [100.0, 25.0, -25.0], [100.0, -25.0, -25.0]];
        let closed = SpanBuffer::alloc(0, 100);
        let mut pts = clip_polygon(&f, &quad);
        assert!(rasterize(&mut pts, f.y, Some(&closed)).is_none());
        let open = SpanBuffer::for_screen(100, 100);
        let mut pts = clip_polygon(&f, &quad);
        assert!(rasterize(&mut pts, f.y, Some(&open)).is_some());
    }
}

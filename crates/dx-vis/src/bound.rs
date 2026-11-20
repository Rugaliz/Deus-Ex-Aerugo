//! `URender::BoundVisible` (Render.dll 10b162e0): is a world-aligned box on screen (and, given a span buffer, in an
//! open part of it)? Also gives its screen rectangle and nearest depth.

use crate::frame::{Frame, Vec3};
use crate::span::SpanBuffer;

/// Minimum depth and outcode tolerance (2^-15).
const EPS: f32 = 3.051_757_8e-5;

/// `FScreenBounds`: pixel rectangle (min inclusive, max as the engine stores it) and the nearest depth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenBounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
    pub min_z: f32,
}

fn row(v: f32) -> i32 {
    (v - 0.5).round_ties_even() as i32
}

/// The box `min..max` seen from `f`; `None` when it cannot be seen (behind, outside the view, or hidden in `span`).
pub fn bound_visible(f: &Frame, min: Vec3, max: Vec3, span: Option<&SpanBuffer>) -> Option<ScreenBounds> {
    let lo = [min[0] - f.origin[0], min[1] - f.origin[1], min[2] - f.origin[2]];
    let hi = [max[0] - f.origin[0], max[1] - f.origin[1], max[2] - f.origin[2]];
    let inside = (0..3).all(|k| !(lo[k] > 0.0) && !(hi[k] < 0.0));
    if inside {
        // the camera is in the box: the whole screen, no span test
        return Some(ScreenBounds { min_x: 0.0, min_y: 0.0, max_x: f.fx, max_y: f.fy, min_z: 0.0 });
    }
    // each axis' share of the view coordinates, for the low and the high side of the box
    let share = |axis: Vec3, k: usize| (lo[k] * axis[k], hi[k] * axis[k]);
    let (zx, zy, zz) = (share(f.z_axis, 0), share(f.z_axis, 1), share(f.z_axis, 2));
    if [zx.0, zy.0, zz.0, zx.1, zy.1, zz.1].iter().all(|&v| v < 0.0) {
        return None;
    }
    let (xx, xy, xz) = (share(f.x_axis, 0), share(f.x_axis, 1), share(f.x_axis, 2));
    let (yx, yy, yz) = (share(f.y_axis, 0), share(f.y_axis, 1), share(f.y_axis, 2));
    let pick = |p: (f32, f32), hi: bool| if hi { p.1 } else { p.0 };
    let mut corners = [[0.0f32; 3]; 8];
    for (i, c) in corners.iter_mut().enumerate() {
        let (bx, by, bz) = (i & 1 != 0, i & 2 != 0, i & 4 != 0);
        *c = [pick(xx, bx) + pick(xy, by) + pick(xz, bz), pick(yx, bx) + pick(yy, by) + pick(yz, bz), pick(zx, bx) + pick(zy, by) + pick(zz, bz)];
    }
    let code = |c: &[f32; 3]| -> u8 {
        let mut b = 0;
        if c[2] * f.prj_xm + c[0] < EPS {
            b |= 1;
        }
        if c[2] * f.prj_xp - c[0] < EPS {
            b |= 2;
        }
        if c[2] * f.prj_ym + c[1] < EPS {
            b |= 4;
        }
        if c[2] * f.prj_yp - c[1] < EPS {
            b |= 8;
        }
        b
    };
    let codes: Vec<u8> = corners.iter().map(code).collect();
    if codes.iter().fold(0xff, |a, &c| a & c) != 0 {
        return None;
    }
    let any = codes.iter().fold(0, |a, &c| a | c);
    let min_z = corners.iter().map(|c| c[2]).fold(f32::INFINITY, f32::min);
    let screen = |c: &[f32; 3]| {
        let r = f.proj_z / c[2];
        (row(r * c[0] + f.fx2), row(r * c[1] + f.fy2))
    };
    let (x0, y0) = screen(&corners[0]);
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (x0, x0, y0, y0);
    if any & 1 != 0 {
        min_x = 0;
    }
    if any & 2 != 0 {
        max_x = f.x;
    }
    if any & 4 != 0 {
        min_y = 0;
    }
    if any & 8 != 0 {
        max_y = f.y;
    }
    for c in &corners[1..] {
        let (sx, sy) = screen(c);
        if sx < min_x {
            min_x = sx;
        } else if sx > max_x {
            max_x = sx;
        }
        if sy < min_y {
            min_y = sy;
        } else if sy > max_y {
            max_y = sy;
        }
    }
    let b = ScreenBounds { min_x: min_x.max(0) as f32, min_y: min_y.max(0) as f32, max_x: max_x.min(f.x) as f32, max_y: max_y.min(f.y) as f32, min_z: min_z.max(EPS) };
    if let Some(s) = span {
        if !s.box_is_visible(min_x, min_y, max_x, max_y) {
            return None;
        }
    }
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame::new([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 90.0, 100, 100)
    }

    #[test]
    fn boxes_ahead_behind_and_around_the_camera() {
        let f = frame();
        let b = bound_visible(&f, [90.0, -10.0, -10.0], [110.0, 10.0, 10.0], None).expect("ahead");
        // nearest face at x 90: +-10 units -> 50 +- 5.6 px; farther face smaller
        assert!((b.min_x - 44.0).abs() <= 1.0 && (b.max_x - 55.0).abs() <= 1.0, "{b:?}");
        assert!((b.min_z - 90.0).abs() < 1e-3);
        assert!(bound_visible(&f, [-110.0, -10.0, -10.0], [-90.0, 10.0, 10.0], None).is_none(), "behind");
        assert!(bound_visible(&f, [90.0, 200.0, -10.0], [110.0, 220.0, 10.0], None).is_none(), "off to the side");
        let around = bound_visible(&f, [-5.0, -5.0, -5.0], [5.0, 5.0, 5.0], Some(&SpanBuffer::alloc(0, 100))).expect("inside");
        assert_eq!((around.max_x, around.max_y), (100.0, 100.0));
    }

    #[test]
    fn a_box_straddling_the_side_reaches_the_screen_edge_and_is_tested_in_the_span_buffer() {
        let f = frame();
        let b = bound_visible(&f, [50.0, 40.0, -10.0], [100.0, 200.0, 10.0], None).expect("partly in view");
        assert_eq!(b.max_x, 100.0);
        let mut zone = SpanBuffer::for_screen(100, 100);
        zone.close_rect(0, 0, 100, 100);
        assert!(bound_visible(&f, [50.0, 40.0, -10.0], [100.0, 200.0, 10.0], Some(&zone)).is_none(), "hidden");
    }
}

/// Render bounds that hold what each subtree holds (an Aerugo repair, not the original's behaviour). The editor's
/// `bspBuildBounds` leaves some nodes of the shipped maps with a box that misses part of their subtree (the Helibase
/// stairs: node 3159's box ends at y 200 while its subtree's walls reach y 480; others are empty and flagged invalid),
/// and `BoundVisible` then skips whole rooms and the doors filtered into them from most views, in the original too.
///
/// `static_subtree_bounds` is made once per map; `repaired_render_bounds` per change of the movers' fragments.

fn grow(b: &mut Option<(Vec3, Vec3)>, lo: Vec3, hi: Vec3) {
    match b {
        None => *b = Some((lo, hi)),
        Some((a, z)) => {
            for k in 0..3 {
                a[k] = a[k].min(lo[k]);
                z[k] = z[k].max(hi[k]);
            }
        }
    }
}

fn polygon_box(model: &ue_assets::model::Model, i: usize) -> Option<(Vec3, Vec3)> {
    let nd = &model.nodes[i];
    let mut b = None;
    for k in 0..nd.num_vertices.max(0) as i32 {
        if let Some(p) = model.verts.get((nd.i_vert_pool + k) as usize).and_then(|v| model.points.get(v.p_vertex as usize)) {
            grow(&mut b, *p, *p);
        }
    }
    b
}

/// The box of every polygon in each node's subtree (its own, its coplanar nodes', its front and back) in the level's
/// model as stored (no movers).
pub fn static_subtree_bounds(model: &ue_assets::model::Model) -> Vec<Option<(Vec3, Vec3)>> {
    let n = model.nodes.len();
    let mut sub: Vec<Option<(Vec3, Vec3)>> = vec![None; n];
    let mut done = vec![false; n];
    let children = |i: usize| {
        let nd = &model.nodes[i];
        [nd.i_front, nd.i_back, nd.i_plane].into_iter().filter(|&c| c >= 0 && (c as usize) < n).map(|c| c as usize)
    };
    // children before parents, with an explicit stack (the trees are deep)
    let mut stack = Vec::new();
    for root in 0..n {
        if done[root] {
            continue;
        }
        stack.push((root, false));
        while let Some((i, expanded)) = stack.pop() {
            if done[i] {
                continue;
            }
            if !expanded {
                stack.push((i, true));
                stack.extend(children(i).filter(|&c| !done[c]).map(|c| (c, false)));
                continue;
            }
            let mut b = polygon_box(model, i);
            for c in children(i) {
                if let Some((lo, hi)) = sub[c] {
                    grow(&mut b, lo, hi);
                }
            }
            sub[i] = b;
            done[i] = true;
        }
    }
    sub
}

/// The box the walk tests for each node of `model` (the level's model, with the movers' fragments filtered in when
/// `first_dynamic_surf` is not `usize::MAX`): the stored box (when valid) grown to its subtree's polygons
/// (`static_sub`, from `static_subtree_bounds` of the stored model) and to the movers' fragments under it. `None`:
/// no box to test (the node has none, a mover's node uses the field as a list link, or an invalid box with nothing
/// under it).
pub fn repaired_render_bounds(model: &ue_assets::model::Model, first_dynamic_surf: usize, static_sub: &[Option<(Vec3, Vec3)>]) -> Vec<Option<(Vec3, Vec3)>> {
    let n = model.nodes.len();
    let dynamic = |i: usize| model.nodes[i].i_surf >= 0 && model.nodes[i].i_surf as usize >= first_dynamic_surf;
    let mut out: Vec<Option<(Vec3, Vec3)>> = (0..n)
        .map(|i| {
            let nd = &model.nodes[i];
            if nd.i_render_bound < 0 || dynamic(i) {
                return None;
            }
            let stored = model.bounds.get(nd.i_render_bound as usize)?;
            let mut b = stored.is_valid.then_some((stored.min, stored.max));
            if let Some((lo, hi)) = static_sub.get(i).copied().flatten() {
                grow(&mut b, lo, hi);
            }
            b
        })
        .collect();
    if first_dynamic_surf == usize::MAX {
        return out;
    }
    // the movers' fragments: each grows the boxes of the nodes above it
    let mut parent = vec![usize::MAX; n];
    for (i, nd) in model.nodes.iter().enumerate() {
        for c in [nd.i_front, nd.i_back, nd.i_plane] {
            if c >= 0 && (c as usize) < n {
                parent[c as usize] = i;
            }
        }
    }
    for i in (0..n).filter(|&i| dynamic(i)) {
        let Some((lo, hi)) = polygon_box(model, i) else { continue };
        let mut a = parent[i];
        let mut steps = 0;
        while a != usize::MAX && steps < n {
            if out[a].is_some() || (model.nodes[a].i_render_bound >= 0 && !dynamic(a)) {
                grow(&mut out[a], lo, hi);
            }
            a = parent[a];
            steps += 1;
        }
    }
    out
}

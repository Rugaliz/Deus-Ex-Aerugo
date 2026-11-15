//! Box-versus-hull overlap tests and swept traces: `UModel::PointCheck` (103f1570, walk 103f19b0, bevel test
//! 103f3050) and `UModel::LineCheck` (103f3c20; zero extent 103f3560, box walk 103f42f0, bevel test 103f5b80).

use crate::*;

/// Extra slack the engine adds to the box extent while walking the BSP (never when testing hulls).
const NODE_EXTENT_PAD: f32 = 1.1;
/// Push-out distance factor of a point check.
const PUSHOUT: f32 = 1.02;
/// Hull bounding-box slack.
const BOUND_SLACK: f32 = 0.1;
/// Tolerance below which a trace is considered parallel to a plane.
const PARALLEL_EPS: f32 = 1e-5;
/// Minimum squared length of the cross product of two hull plane normals for an edge to exist.
const EDGE_EPS_SQ: f32 = 1e-6;
/// Plane-pair overlap threshold for edge bevels.
const BEVEL_EPS: f32 = 0.001;
/// Initial best push-out distance of a point check (`0x47c35000`).
const POINT_BEST: f32 = 100000.0;

/// The engine's clamp of a hit time to 0..1: `0 <= t ? (1 <= t ? 1 : t) : 0` (a NaN time gives 0).
fn clamp01(t: f32) -> f32 {
    if 0.0 <= t {
        if 1.0 <= t { 1.0 } else { t }
    } else {
        0.0
    }
}

/// The six axis planes that bound a leaf hull: (normal, offset) from the hull's integer bounds `b`.
fn bound_planes(b: [f32; 6]) -> [([f32; 3], f32); 6] {
    let k = BOUND_SLACK;
    [
        ([0.0, 0.0, -1.0], k - b[2]),
        ([0.0, 0.0, 1.0], b[5] + k),
        ([-1.0, 0.0, 0.0], k - b[0]),
        ([1.0, 0.0, 0.0], b[3] - k),
        ([0.0, -1.0, 0.0], k - b[1]),
        ([0.0, 1.0, 0.0], b[4] - k),
    ]
}

/// Edge bevel planes of a hull: for every pair of planes whose normals have opposite signs along a box axis and point
/// the same way in the plane perpendicular to it, a plane through their common edge perpendicular to the axis.
fn bevel_planes(h: &Hull) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    let axes: [(u8, Vec3); 3] = [(0x03, [1.0, 0.0, 0.0]), (0x0C, [0.0, 1.0, 0.0]), (0x30, [0.0, 0.0, 1.0])];
    for i in 0..h.planes.len() {
        let ni = [h.planes[i][0], h.planes[i][1], h.planes[i][2]];
        for j in 0..i {
            let nj = [h.planes[j][0], h.planes[j][1], h.planes[j][2]];
            let fl = h.flags[i] | h.flags[j];
            for (mask, axis) in axes {
                if fl & mask != mask {
                    continue;
                }
                if dot(cross(axis, ni), cross(axis, nj)) <= BEVEL_EPS {
                    continue;
                }
                // the line where both planes meet
                let d = cross(ni, nj);
                let len2 = dot(d, d);
                if len2 < EDGE_EPS_SQ {
                    continue;
                }
                let (wi, wj) = (h.planes[i][3], h.planes[j][3]);
                let inv = 1.0 / len2;
                let p = [
                    ((d[2] * nj[1] - d[1] * nj[2]) * wi + (d[1] * ni[2] - d[2] * ni[1]) * wj) * inv,
                    ((d[0] * nj[2] - d[2] * nj[0]) * wi + (d[2] * ni[0] - d[0] * ni[2]) * wj) * inv,
                    ((d[1] * nj[0] - d[0] * nj[1]) * wi + (ni[1] * d[0] - d[1] * ni[0]) * wj) * inv,
                ];
                let dl = len2.sqrt();
                let dn = scale(d, 1.0 / dl);
                // normal perpendicular to both the edge and the box axis (`UnsafeNormal`: no zero check, a zero
                // vector gives a NaN plane that never separates)
                let mut nrm = cross(axis, dn);
                nrm = scale(nrm, 1.0 / dot(nrm, nrm).sqrt());
                if dot(ni, nrm) < 0.0 {
                    nrm = scale(nrm, -1.0);
                }
                out.push([nrm[0], nrm[1], nrm[2], dot(nrm, p)]);
            }
        }
    }
    out
}

/// Every plane a hull test uses, in the engine's order: hull planes, the six bound planes (static models only), the
/// edge bevels. Each carries the node index it came from (for hull planes) or -1.
fn all_planes(h: &Hull, with_bounds: bool, nodes: &[i32]) -> Vec<([f32; 4], i32)> {
    let mut v: Vec<([f32; 4], i32)> = h.planes.iter().zip(nodes).map(|(p, n)| (*p, *n)).collect();
    if with_bounds {
        for (n, w) in bound_planes(h.bounds) {
            v.push(([n[0], n[1], n[2], w], -1));
        }
    }
    for p in bevel_planes(h) {
        v.push((p, -1));
    }
    v
}

struct PointState {
    owner: Option<Coords>,
    loc: Vec3,
    extent: Vec3,
    extra: u32,
    best: f32,
    hit: Option<PointHit>,
}

struct LineState {
    owner: Option<Coords>,
    start: Vec3,
    end: Vec3,
    extent: Vec3,
    /// Best entry time found so far over all leaves (the engine's `Hit.Time`, initially 2).
    best_time: f32,
    best_normal: Vec3,
    found: bool,
}

impl Bsp {
    fn hull_node_ids(&self, node: usize) -> Vec<i32> {
        let ib = self.nodes[node].i_collision_bound;
        let list = &self.leaf_hulls[ib as usize..];
        list.iter().take_while(|&&v| v != -1).take(MAX_HULL_PLANES).map(|&v| (v as u32 & !HULL_FLIP) as i32).collect()
    }

    /// Tests a box of half-extent `extent` centred at `loc`. `None` = the box is in free space; `Some` = it overlaps
    /// solid, with a push-out suggestion (the engine's `PointCheck`).
    pub fn point_check(&self, loc: Vec3, extent: Vec3, extra_flags: u32) -> Option<PointHit> {
        self.point_check_owned(loc, extent, extra_flags, None)
    }

    /// `point_check` against a brush placed in the world by `owner` (the engine's `Owner->ToWorld()`); the hull's
    /// six bound planes are skipped then (they are in brush space).
    pub fn point_check_owned(&self, loc: Vec3, extent: Vec3, extra_flags: u32, owner: Option<&Coords>) -> Option<PointHit> {
        if self.nodes.is_empty() {
            return if self.root_outside { None } else { Some(PointHit { location: loc, normal: [0.0; 3], node: -1 }) };
        }
        if extent == [0.0; 3] {
            return self.point_check_zero(loc, owner);
        }
        let mut st = PointState { owner: owner.copied(), loc, extent, extra: extra_flags, best: POINT_BEST, hit: None };
        let free = self.pc_node(&mut st, 0, 0, self.root_outside);
        if free {
            None
        } else {
            Some(st.hit.unwrap_or(PointHit { location: loc, normal: [0.0; 3], node: -1 }))
        }
    }

    /// The zero-extent point check: the walk of `PointRegion`, except that a point on a plane goes behind it
    /// (`dist <= 0`); `Hit.Item` is `2 * node + side` of the last node.
    fn point_check_zero(&self, loc: Vec3, owner: Option<&Coords>) -> Option<PointHit> {
        let (mut i, mut outside) = (0usize, self.root_outside);
        loop {
            let n = &self.nodes[i];
            let p = self.plane_of(i, owner);
            let front = !(dot([p[0], p[1], p[2]], loc) - p[3] <= 0.0);
            outside = self.child_outside(n, front, outside, 0);
            let next = if front { n.i_front } else { n.i_back };
            if next < 0 {
                let item = 2 * i as i32 + front as i32;
                return (!outside).then_some(PointHit { location: loc, normal: [0.0; 3], node: item });
            }
            i = next as usize;
        }
    }

    /// Returns true when no collision was found in this subtree.
    fn pc_node(&self, st: &mut PointState, mut parent: usize, mut node: i32, mut outside: bool) -> bool {
        let mut free = true;
        loop {
            if node < 0 {
                if outside || self.nodes[parent].i_collision_bound == -1 {
                    return free;
                }
                let Some(h) = self.hull_of(parent, st.owner.as_ref()) else { return free };
                let ids = self.hull_node_ids(parent);
                return if self.pc_hull(st, &h, &ids) { free } else { false };
            }
            let n = &self.nodes[node as usize];
            let plane = self.plane_of(node as usize, st.owner.as_ref());
            let nn = [plane[0], plane[1], plane[2]];
            let push = push_dist(nn, scale(st.extent, NODE_EXTENT_PAD));
            let dist = dot(nn, st.loc) - plane[3];
            if -push < dist {
                let o = self.child_outside(n, true, outside, st.extra);
                if !self.pc_node(st, node as usize, n.i_front, o) {
                    free = false;
                }
            }
            outside = self.child_outside(n, false, outside, st.extra);
            parent = node as usize;
            node = n.i_back;
            if push < dist {
                return free;
            }
        }
    }

    /// Box-versus-hull test; true when separated (no overlap), false when overlapping.
    fn pc_hull(&self, st: &mut PointState, h: &Hull, ids: &[i32]) -> bool {
        for (plane, id) in all_planes(h, st.owner.is_none(), ids) {
            let nn = [plane[0], plane[1], plane[2]];
            let push = push_dist(nn, st.extent);
            let d = dot(nn, st.loc) - plane[3];
            if 0.0 < d && d < st.best {
                if push <= d {
                    return true;
                }
                st.best = d;
                // `(Plane * 1.02) * (push - d)`
                st.hit = Some(PointHit { location: add(st.loc, scale(scale(nn, PUSHOUT), push - d)), normal: nn, node: id });
            }
            if push <= d {
                return true;
            }
        }
        false
    }

    /// Sweeps a box of half-extent `extent` from `start` to `end` against the model. `None` = the path is clear.
    /// With zero extent this is a line trace. The returned time already includes the engine's back-off from the
    /// surface (`UModel::LineCheck`).
    pub fn line_check(&self, start: Vec3, end: Vec3, extent: Vec3, extra_flags: u32) -> Option<LineHit> {
        self.line_check_owned(start, end, extent, extra_flags, None)
    }

    /// `line_check` against a brush placed in the world by `owner`: planes moved to the world, no bound planes; a
    /// zero-extent hit's normal is the hit node's normal through the owner's axes (not turned to face the trace).
    pub fn line_check_owned(&self, start: Vec3, end: Vec3, extent: Vec3, extra_flags: u32, owner: Option<&Coords>) -> Option<LineHit> {
        if self.nodes.is_empty() {
            return if self.root_outside { None } else { Some(LineHit { time: 0.0, location: start, normal: [0.0; 3], node: -1 }) };
        }
        let delta = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
        if extent == [0.0; 3] {
            return self.zero_extent_line(start, end, delta, extra_flags, owner);
        }
        let _ = extra_flags;
        let mut st = LineState { owner: owner.copied(), start, end, extent, best_time: 2.0, best_normal: [0.0; 3], found: false };
        self.lc_node(&mut st, 0, 0, self.root_outside);
        if !st.found {
            return None;
        }
        let len = dot(delta, delta).sqrt();
        let back = (0.1 / len).max(0.1);
        let time = clamp01(st.best_time - back);
        let location = add(start, scale(delta, time));
        if time != 1.0 {
            // the box trace never sets `Hit.Item`
            Some(LineHit { time, location, normal: st.best_normal, node: -1 })
        } else {
            None
        }
    }

    /// Solid-making test the line walks use (they ignore the caller's extra flags).
    fn is_csg_flags(&self, n: &Node) -> bool {
        n.num_vertices != 0 && (n.flags & NODE_NON_SOLID_FLAGS) == 0
    }

    fn lc_node(&self, st: &mut LineState, mut parent: usize, mut node: i32, mut outside: bool) {
        loop {
            if node < 0 {
                if outside || self.nodes[parent].i_collision_bound == -1 {
                    return;
                }
                let Some(h) = self.hull_of(parent, st.owner.as_ref()) else { return };
                let ids = self.hull_node_ids(parent);
                self.lc_hull(st, &h, &ids);
                return;
            }
            let cur = node as usize;
            let n = &self.nodes[cur];
            let plane = self.plane_of(cur, st.owner.as_ref());
            let nn = [plane[0], plane[1], plane[2]];
            let d_start = dot(nn, st.start) - plane[3];
            let d_end = dot(nn, st.end) - plane[3];
            let push = push_dist(nn, scale(st.extent, NODE_EXTENT_PAD));
            // explore the back child if either end lies within `push` of the plane or behind it, the front child if
            // either end lies within `push` behind the plane or in front of it
            let ok = [d_start <= push || d_end <= push, -push <= d_start || -push <= d_end]; // [back, front]
            // the child on the start side first (1 = front)
            let first = (d_end <= d_start) as usize;
            let csg = self.is_csg_flags(n);
            if ok[first] {
                let o = if first == 0 { outside && !csg } else { outside || csg };
                let child = if first == 0 { n.i_back } else { n.i_front };
                self.lc_node(st, cur, child, o);
            }
            if !ok[1 - first] {
                return;
            }
            let second = 1 - first;
            node = if second == 1 { n.i_front } else { n.i_back };
            outside = if second == 1 { outside || csg } else { outside && !csg };
            parent = cur;
        }
    }

    /// Clips the swept box against a hull (planes, bounds, bevels), updating the best hit.
    fn lc_hull(&self, st: &mut LineState, h: &Hull, ids: &[i32]) {
        let mut tmin = -1.0f32;
        let mut tmax = st.best_time;
        let mut normal = [0.0f32; 3];
        for (plane, _) in all_planes(h, st.owner.is_none(), ids) {
            let nn = [plane[0], plane[1], plane[2]];
            let push = push_dist(nn, st.extent);
            let d_start = dot(nn, st.start) - plane[3];
            let d_end = dot(nn, st.end) - plane[3];
            let mut num = d_start - push;
            if d_end < d_start && -push <= num && num < 0.0 {
                num = 0.0;
            }
            let den = d_start - d_end;
            let t = num / den;
            if -PARALLEL_EPS <= den {
                if den <= PARALLEL_EPS {
                    if push < d_start && push < d_end {
                        return;
                    }
                } else if tmin < t {
                    tmin = t;
                    normal = nn;
                }
            } else if t < tmax {
                tmax = t;
            }
            if tmax <= tmin {
                return;
            }
        }
        if -1.0 < tmin && tmin < tmax && 0.0 < tmax {
            st.best_time = tmin;
            st.best_normal = normal;
            st.found = true;
        }
    }

    /// Zero-extent line trace (`UModel::LineCheck` with a zero box).
    fn zero_extent_line(&self, start: Vec3, end: Vec3, delta: Vec3, extra: u32, owner: Option<&Coords>) -> Option<LineHit> {
        let mut z = ZState { owner: owner.copied(), started_outside: false, extra, hit: None };
        let clear = self.lz(&mut z, 0, 0, end, start, self.root_outside);
        if clear {
            return None;
        }
        let (loc, parent) = z.hit?;
        // sums from z to x as the engine; the square root in double precision
        let len2 = delta[2] * delta[2] + delta[1] * delta[1] + delta[0] * delta[0];
        let time = ((loc[2] - start[2]) * delta[2] + (loc[1] - start[1]) * delta[1] + (loc[0] - start[0]) * delta[0]) / len2;
        let time = clamp01(time - 0.5 / (len2 as f64).sqrt() as f32);
        let location = add(start, scale(delta, time));
        let pn = &self.nodes[parent].plane;
        let mut normal = [pn[0], pn[1], pn[2]];
        if let Some(c) = owner {
            normal = c.transform_vector(normal);
        } else if dot(scale(delta, -1.0), normal) < 0.0 {
            // the normal faces against the direction of travel
            normal = scale(normal, -1.0);
        }
        Some(LineHit { time, location, normal, node: parent as i32 })
    }

    /// Segment walk of the zero-extent trace: `a` is the far end, `b` the end the walk starts from. Returns true when
    /// the segment is clear. `parent` is the last node the segment was split at (the engine only updates it there).
    fn lz(&self, z: &mut ZState, mut parent: usize, mut node: i32, a: Vec3, mut b: Vec3, mut outside: bool) -> bool {
        let extra = z.extra as u8;
        // the straight steps mask out 0x10 of the caller's flags, the split step (`FUN_103f68b0`) does not
        let solid = |n: &Node| n.num_vertices != 0 && ((extra & 0xEF | 0x21) & n.flags) == 0;
        let solid_split = |n: &Node| n.num_vertices != 0 && ((extra | 0x21) & n.flags) == 0;
        loop {
            if node < 0 {
                if outside {
                    z.started_outside = true;
                    return true;
                }
                if !z.started_outside && (z.extra & 0x10) != 0 {
                    return true;
                }
                z.hit = Some((b, parent));
                return false;
            }
            let cur = node as usize;
            let n = &self.nodes[cur];
            let plane = self.plane_of(cur, z.owner.as_ref());
            let nn = [plane[0], plane[1], plane[2]];
            let d_b = dot(nn, b) - plane[3];
            let d_a = dot(nn, a) - plane[3];
            if d_b > -0.001 && d_a > -0.001 {
                outside = outside || solid(n);
                node = n.i_front;
                continue;
            }
            if d_b < 0.001 && d_a < 0.001 {
                outside = outside && !solid(n);
                node = n.i_back;
                continue;
            }
            // the segment crosses the plane: split it
            let t = d_b / (d_a - d_b);
            let s = add(b, scale(sub(b, a), t));
            let (first, o) = if d_b <= 0.0 { (0usize, outside && !solid_split(n)) } else { (1usize, outside || solid_split(n)) };
            let child = if first == 0 { n.i_back } else { n.i_front };
            if !self.lz(z, parent, child, s, b, o) {
                return false;
            }
            // continue with the part from `a` to the split point, on the other side
            outside = if first == 1 { outside && !solid_split(n) } else { outside || solid_split(n) };
            node = if first == 0 { n.i_front } else { n.i_back };
            b = s;
            parent = cur;
        }
    }
}

struct ZState {
    owner: Option<Coords>,
    started_outside: bool,
    extra: u32,
    /// Where the walk first entered solid, and the node whose plane was crossed.
    hit: Option<(Vec3, usize)>,
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One node: the plane z = 0 with solid below (`root_outside` false: in front of a CSG plane is empty space).
    /// The solid leaf carries a hull made of that plane and the box -1000..1000 x/y, -1000..0 z.
    fn floor(node_flags: u8) -> Bsp {
        let node = Node {
            plane: [0.0, 0.0, 1.0, 0.0],
            flags: node_flags,
            num_vertices: 4,
            i_back: -1,
            i_front: -1,
            i_collision_bound: 0,
            i_surf: 0,
            i_zone: [0, 0],
            i_leaf: [0, 1],
        };
        let mut hull = vec![0, -1];
        hull.extend([-1000.0f32, -1000.0, -1000.0, 1000.0, 1000.0, 0.0].map(|f| f.to_bits() as i32));
        Bsp { nodes: vec![node], leaf_hulls: hull, root_outside: false, num_zones: 0 }
    }

    #[test]
    fn fast_line_side_is_the_sign_bit() {
        let w = floor(0);
        // n . p - W = -0.0 - 0.0 = -0.0: behind the plane (solid) for FastLineCheck, in front for PointRegion
        let p = [-0.0f32, -0.0, -0.0];
        assert!(!w.fast_line_check(p, p));
        assert!(w.point_region(p).outside);
        assert!(w.fast_line_check([0.0, 0.0, 0.0], [0.0, 0.0, 5.0]));
    }

    #[test]
    fn zero_extent_point_check_puts_a_point_on_a_plane_behind_it() {
        let w = floor(0);
        let h = w.point_check([3.0, 4.0, 0.0], [0.0; 3], 0).expect("on the plane counts as inside");
        assert_eq!((h.location, h.normal, h.node), ([3.0, 4.0, 0.0], [0.0; 3], 0));
        assert_eq!(w.point_check([3.0, 4.0, 0.5], [0.0; 3], 0), None);
        // Item = 2 * node + side
        assert_eq!(w.point_check([0.0, 0.0, -1.0], [0.0; 3], 0).unwrap().node, 0);
    }

    #[test]
    fn point_check_push_out_starts_from_100000() {
        // box 1 x 1 x 50 centred 10 above the floor: the floor plane (d = 10) and the hull's top bound (z = 0.1,
        // d = 9.9) are both nearer than 100000 but farther than the old seed (extent.x = 1)
        let w = floor(0);
        let h = w.point_check([0.0, 0.0, 10.0], [1.0, 1.0, 50.0], 0).unwrap();
        assert_eq!(h.normal, [0.0, 0.0, 1.0]);
        assert_eq!(h.node, -1, "the bound plane (d = 9.9) beats the floor plane (d = 10)");
        let d = 10.0f32 - 0.1;
        assert_eq!(h.location[2], 10.0 + 1.02f32 * (50.0 - d));
        // a box that does not reach the plane is free
        assert_eq!(w.point_check([0.0, 0.0, 60.0], [1.0, 1.0, 50.0], 0), None);
    }

    #[test]
    fn zero_extent_split_tests_solidity_with_the_raw_extra_flags() {
        // node flag 0x10 with extra flag 0x10: solid on straight steps (0x10 masked out), not solid where the trace
        // is split; 0x10 also means "a solid leaf only counts once the trace has been outside"
        let w = floor(0x10);
        assert_eq!(w.line_check([0.0, 0.0, 10.0], [0.0, 0.0, -10.0], [0.0; 3], 0x10), None);
        // without the extra flag the floor is hit at z = 0, backed off by 0.5 units
        let h = w.line_check([0.0, 0.0, 10.0], [0.0, 0.0, -10.0], [0.0; 3], 0).unwrap();
        assert_eq!(h.time, 0.5 - 0.5 / 20.0);
        assert_eq!((h.normal, h.node), ([0.0, 0.0, 1.0], 0));
    }

    #[test]
    fn hit_times_clamp_nan_to_zero() {
        // a zero-length trace inside solid: time 0 / 0 is clamped to 0
        let h = floor(0).line_check([0.0, 0.0, -10.0], [0.0, 0.0, -10.0], [0.0; 3], 0).unwrap();
        assert_eq!(h.time, 0.0);
        assert_eq!(clamp01(f32::NAN), 0.0);
        assert_eq!(clamp01(2.0), 1.0);
    }

    #[test]
    fn box_traces_leave_the_item_unset() {
        let h = floor(0).line_check([0.0, 0.0, 100.0], [0.0, 0.0, -100.0], [10.0, 10.0, 10.0], 0).unwrap();
        assert_eq!(h.node, -1);
        assert_eq!(h.normal, [0.0, 0.0, 1.0]);
        // contact at z = 10 (time 0.45); the box back-off is max(0.1 / length, 0.1) of the trace (103f403b)
        assert!((h.time - 0.35).abs() < 1e-5, "{}", h.time);
        assert!((h.location[2] - 30.0).abs() < 1e-3, "{:?}", h.location);
    }

    /// A brush frame (`ABrush::ToWorld` without scale): `p_world = R(p_local - pre) + loc` for a yaw of 0 or 45 deg.
    fn frame(loc: Vec3, yaw45: bool) -> Coords {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let sin = move |a: i32| match ((a >> 2) & 0x3fff, yaw45) {
            (0, _) | (0x2000, _) => 0.0,
            (0x1000, _) => 1.0,
            (0x0800, true) | (0x1800, true) => h,
            _ => unreachable!(),
        };
        let mut c = Coords::UNIT;
        c.translate(loc);
        c.mul_rotator([0, if yaw45 { 0x2000 } else { 0 }, 0], sin);
        c
    }

    #[test]
    fn owned_checks_use_world_planes() {
        let w = floor(0);
        let c = frame([0.0, 0.0, 100.0], false);
        // the brush's floor plane now lies at z = 100 (in brush space it is z = 0)
        let h = w.line_check_owned([0.0, 0.0, 200.0], [0.0, 0.0, 0.0], [0.0; 3], 0, Some(&c)).unwrap();
        // the zero-extent back-off is 0.5 units
        assert!((h.location[2] - 100.5).abs() < 1e-3, "{:?}", h.location);
        assert_eq!(h.normal, [0.0, 0.0, 1.0]);
        assert!(w.point_check_owned([0.0, 0.0, 99.0], [0.0; 3], 0, Some(&c)).is_some());
        assert!(w.point_check_owned([0.0, 0.0, 101.0], [0.0; 3], 0, Some(&c)).is_none());
        assert!(w.point_check_owned([0.0, 0.0, 105.0], [10.0, 10.0, 10.0], 0, Some(&c)).is_some());
        assert!(w.point_check_owned([0.0, 0.0, 111.0], [10.0, 10.0, 10.0], 0, Some(&c)).is_none());
    }

    #[test]
    fn owned_box_stays_world_aligned() {
        // a wall: the plane x = 0 with solid behind, then the brush turned 45 degrees about z
        let mut w = floor(0);
        w.nodes[0].plane = [1.0, 0.0, 0.0, 0.0];
        let c = frame([0.0; 3], true);
        let p = c.transform_plane([1.0, 0.0, 0.0, 0.0]);
        let n = [p[0], p[1], p[2]];
        // a world-aligned cube of half size 10 reaches 10 * (|nx| + |ny|) = 14.1 along the turned normal; in brush
        // space the same cube would reach only 10
        let at = scale(n, 12.0);
        assert!(w.point_check_owned(at, [10.0; 3], 0, Some(&c)).is_some());
        assert!(w.point_check_owned(scale(n, 15.0), [10.0; 3], 0, Some(&c)).is_none());
    }
}

//! BSP queries on a loaded world `Model` (spec: `docs/collision.md`).
//!
//! Distances use the engine's plane convention: `dist(p) = n . p - W`, positive in front of the plane. Everything is
//! done in `f32` like the original. A mover's brush is checked with its owner frame (`ABrush::ToWorld`): the engine
//! moves the brush planes into the world and keeps the query box world-aligned.

mod coords;
pub mod fill;
pub mod scene;
mod trace;

pub use coords::{safe_normal, sheer_snap, Coords};

use ue_assets::model::Model;

pub type Vec3 = [f32; 3];

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `NF_NotCsg | NF_IsNew`-style flags that make a node plane non-solid for collision purposes.
const NODE_NON_SOLID_FLAGS: u8 = 0x21;
/// Bit set in a leaf-hull entry when the node plane must be flipped.
const HULL_FLIP: u32 = 0x4000_0000;
/// Maximum number of planes in a leaf hull.
const MAX_HULL_PLANES: usize = 64;

#[derive(Debug, Clone)]
pub struct Node {
    pub plane: [f32; 4],
    pub flags: u8,
    pub num_vertices: u8,
    pub i_back: i32,
    pub i_front: i32,
    pub i_collision_bound: i32,
    pub i_surf: i32,
    pub i_zone: [u8; 2],
    pub i_leaf: [i32; 2],
}

/// Where a point lies in the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointRegion {
    /// Zone number (index into the model's zone table), 0 when the model has no zones.
    pub zone: u8,
    /// Leaf index, -1 when the model has no nodes.
    pub leaf: i32,
    /// True when the point is in empty space (not inside solid).
    pub outside: bool,
}

/// Result of a blocked trace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineHit {
    /// Fraction of the way from start to end where the moving box first touches (after the engine's back-off).
    pub time: f32,
    pub location: Vec3,
    pub normal: Vec3,
    /// The engine's `Hit.Item`: for a zero extent the node whose plane was hit (its surface gives the texture); a box
    /// trace leaves it unset (-1).
    pub node: i32,
}

/// Result of a box-overlap test that found the box inside solid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointHit {
    /// A nearby location pushed out of the solid along `normal`.
    pub location: Vec3,
    pub normal: Vec3,
    /// The engine's `Hit.Item`: the node whose hull plane produced the push-out (without the flip bit), -1 for a bound
    /// or bevel plane; for a zero extent `2 * node + side` of the last node walked.
    pub node: i32,
}

#[derive(Debug, Clone)]
pub struct Bsp {
    pub nodes: Vec<Node>,
    pub leaf_hulls: Vec<i32>,
    pub root_outside: bool,
    pub num_zones: usize,
}

/// A leaf's convex collision cell, unpacked.
struct Hull {
    planes: Vec<[f32; 4]>,
    /// Sign flags per plane: x<0 = 1, x>0 = 2, y<0 = 4, y>0 = 8, z<0 = 0x10, z>0 = 0x20.
    flags: Vec<u8>,
    /// Bounding box stored as float bits: min x, min y, min z, max x, max y, max z.
    bounds: [f32; 6],
}

fn sign_flags(n: [f32; 4]) -> u8 {
    let f = |v: f32, neg: u8, pos: u8| if v < 0.0 { neg } else if v > 0.0 { pos } else { 0 };
    f(n[0], 1, 2) | f(n[1], 4, 8) | f(n[2], 0x10, 0x20)
}

/// `sum |n_i * e_i|`: how far a box with half-extent `e` reaches along the plane normal.
fn push_dist(n: Vec3, e: Vec3) -> f32 {
    (e[0] * n[0]).abs() + (e[1] * n[1]).abs() + (e[2] * n[2]).abs()
}

impl Bsp {
    pub fn from_model(m: &Model) -> Bsp {
        Bsp {
            nodes: Self::nodes_of(m),
            leaf_hulls: m.leaf_hulls.clone(),
            root_outside: m.root_outside,
            num_zones: m.zones.len(),
        }
    }

    /// The collision view of a model's nodes.
    pub fn nodes_of(m: &Model) -> Vec<Node> {
        m.nodes
            .iter()
            .map(|n| Node {
                plane: n.plane,
                flags: n.node_flags,
                num_vertices: n.num_vertices,
                i_back: n.i_back,
                i_front: n.i_front,
                i_collision_bound: n.i_collision_bound,
                i_surf: n.i_surf,
                i_zone: n.i_zone,
                i_leaf: n.i_leaf,
            })
            .collect()
    }

    /// Does crossing this node's plane change solidity? (a node with geometry that is not flagged non-solid)
    fn is_csg(&self, n: &Node, extra: u32) -> bool {
        n.num_vertices != 0 && ((extra as u8 | NODE_NON_SOLID_FLAGS) & n.flags) == 0
    }

    fn child_outside(&self, n: &Node, front: bool, outside: bool, extra: u32) -> bool {
        let csg = self.is_csg(n, extra);
        if front {
            outside || csg
        } else {
            outside && !csg
        }
    }

    /// `UModel::PointRegion` (103f5db0): zone and leaf containing `p` (a point on a plane is in front), and whether it
    /// is empty space.
    pub fn point_region(&self, p: Vec3) -> PointRegion {
        if self.nodes.is_empty() {
            return PointRegion { zone: 0, leaf: -1, outside: self.root_outside };
        }
        let (mut i, mut outside) = (0usize, self.root_outside);
        loop {
            let n = &self.nodes[i];
            let front = dot([n.plane[0], n.plane[1], n.plane[2]], p) - n.plane[3] >= 0.0;
            outside = self.child_outside(n, front, outside, 0);
            let next = if front { n.i_front } else { n.i_back };
            if next < 0 {
                let side = front as usize;
                let zone = if self.num_zones == 0 { 0 } else { n.i_zone[side] };
                return PointRegion { zone, leaf: n.i_leaf[side], outside };
            }
            i = next as usize;
        }
    }

    /// `UModel::FastLineCheck(End, Start)` (103f3280): true when the whole segment from `b` to `a` stays in empty space.
    pub fn fast_line_check(&self, a: Vec3, b: Vec3) -> bool {
        if self.nodes.is_empty() {
            return self.root_outside;
        }
        self.fast_line(0, a, b, self.root_outside)
    }

    /// `FUN_103f33a0`: `a` is the end, `b` the start. The side of a point is the sign bit of its stored distance, so a
    /// distance of -0.0 lies behind the plane.
    fn fast_line(&self, mut node: i32, mut a: Vec3, b: Vec3, mut outside: bool) -> bool {
        while node >= 0 {
            let n = &self.nodes[node as usize];
            let nn = [n.plane[0], n.plane[1], n.plane[2]];
            let not_csg = n.flags & 1 != 0;
            let d_b = dot(nn, b) - n.plane[3];
            let d_a = dot(nn, a) - n.plane[3];
            let (front_b, front_a) = (!d_b.is_sign_negative(), !d_a.is_sign_negative());
            let next_flag = |front: bool, o: bool| if not_csg { o } else { front };
            if front_b != front_a {
                let t = d_b / (d_b - d_a);
                let mid = [(a[0] - b[0]) * t + b[0], (a[1] - b[1]) * t + b[1], (a[2] - b[2]) * t + b[2]];
                let child = if front_a { n.i_front } else { n.i_back };
                if !self.fast_line(child, mid, a, next_flag(front_a, outside)) {
                    return false;
                }
                a = mid;
            }
            let child = if front_b { n.i_front } else { n.i_back };
            outside = next_flag(front_b, outside);
            node = child;
        }
        outside
    }

    /// A node's plane, moved to the world through the owner's frame (`TransformPlaneByOrtho`) for a brush.
    fn plane_of(&self, node: usize, owner: Option<&Coords>) -> [f32; 4] {
        let p = self.nodes[node].plane;
        owner.map_or(p, |c| c.transform_plane(p))
    }

    /// Unpacks the collision hull stored for `node` (the node whose back child is solid), if it has one. With an
    /// owner every plane is moved to the world first, then flipped.
    fn hull_of(&self, node: usize, owner: Option<&Coords>) -> Option<Hull> {
        let ib = self.nodes[node].i_collision_bound;
        if ib < 0 {
            return None;
        }
        let list = self.leaf_hulls.get(ib as usize..)?;
        let mut planes = Vec::new();
        let mut flags = Vec::new();
        let mut k = 0;
        while k < list.len() && list[k] != -1 && planes.len() < MAX_HULL_PLANES {
            let raw = list[k] as u32;
            let i = (raw & !HULL_FLIP) as usize;
            self.nodes.get(i)?;
            let mut p = self.plane_of(i, owner);
            if raw & HULL_FLIP != 0 {
                p = [-p[0], -p[1], -p[2], -p[3]];
            }
            flags.push(sign_flags(p));
            planes.push(p);
            k += 1;
        }
        let b = list.get(k + 1..k + 7)?;
        Some(Hull { planes, flags, bounds: std::array::from_fn(|i| f32::from_bits(b[i] as u32)) })
    }
}

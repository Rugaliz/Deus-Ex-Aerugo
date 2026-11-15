//! The moving-brush tracker (`FMovingBrushTracker`, Engine.dll UnDynBsp, created by `GNewBrushTracker` 103021ee):
//! the polygons of every mover's brush are filtered into the level's BSP as extra nodes, so the visibility walk sees
//! movers like world surfaces (they hide what is behind them and join the draw lists).
//!
//! - Construction (1037ac40): each mover (a non-static brush actor with a `Brush`) gets one new level surface per brush
//!   polygon (1037b5c0); `SurfIsDynamic(i)` (1037ac10) is `i >=` the surface count before that. Then `Update(None)`.
//! - `Update` (1037a400): the brushes to redo are listed at the head of a list (`Update(None)`: every mover; a mover:
//!   only when its location or rotation changed since it was last added). Their nodes and everything filtered below
//!   them are marked (1037c4f0; the owners of nodes below join the list), the listed brushes' nodes are unlinked
//!   (1037c6d0), then each listed brush (head first) is put back: `ABrush::BuildCoords`, every polygon through
//!   `FPoly::Transform` (written into its level surface too), the bounding sphere precomputed against the BSP
//!   (`UModel::PrecomputeSphereFilter`), every polygon filtered from the root (1037c0c0) and added where it ends up
//!   outside (1037bb00). New nodes carry `NF_IsNew` (0x20) while their brush is added, so a brush never clips itself.
//!
//! Node, point and vertex slot numbers are not behaviour (nothing reads them but the links); this port appends and
//! reuses freed slots.

use ue_assets::model::{BspNode, BspSurf, Model, Vert};
use ue_bsp::{safe_normal, Coords};

type Vec3 = [f32; 3];

/// `FPoly::VERTEX_THRESHOLD`: the filter halves polygons with this many vertices or more.
const VERTEX_THRESHOLD: usize = 14;
/// `THRESH_SPLIT_POLY_WITH_PLANE` (doubles 1042d518/1042d528).
const SPLIT_THRESH: f64 = 0.25;
const NF_NOT_CSG: u8 = 0x01;
const NF_IS_NEW: u8 = 0x20;
const NF_IS_FRONT: u8 = 0x40;
const NF_IS_BACK: u8 = 0x80;
const PF_EDCUT: u32 = 0x8000_0000;

/// A brush polygon (`FPoly`): base, normal, texture vectors, vertices, flags.
#[derive(Debug, Clone)]
pub struct Poly {
    pub base: Vec3,
    pub normal: Vec3,
    pub texture_u: Vec3,
    pub texture_v: Vec3,
    pub vertices: Vec<Vec3>,
    pub poly_flags: u32,
    pub texture: i32,
    pub pan_u: i16,
    pub pan_v: i16,
    pub i_brush_poly: i32,
}

/// A mover as the tracker sees it: its brush's polygons (model space) and where they stand.
#[derive(Debug, Clone)]
pub struct Mover {
    /// The caller's id (the actor).
    pub id: u32,
    pub location: Vec3,
    pub rotation: [i32; 3],
    pub pre_pivot: Vec3,
    /// `ABrush::BuildCoords`' point frame (`GMath.UnitCoords * PostScale * Rotation * MainScale`).
    pub point: Coords,
    /// `BuildCoords`' vector frame (`UnitCoords / MainScale / Rotation / PostScale`, transposed).
    pub vector: Coords,
    /// `BuildCoords`' orientation: the sign of `MainScale`'s product times `PostScale.Orientation()`.
    pub orientation: f32,
    /// `bSpecialLit`: the brush's surfaces get `PF_SpecialLit`.
    pub special_lit: bool,
}

#[derive(Debug, Clone)]
struct Brush {
    id: u32,
    /// The level surface of each brush polygon (`FPoly::iLink` after 1037b5c0).
    surfs: Vec<usize>,
    /// Its nodes in the order they were added (the engine links them through `iRenderBound`).
    nodes: Vec<usize>,
    /// Location and rotation it was last added at (`(-1,-1,-1)` / `(123,456,789)` before the first add).
    old: (Vec3, [i32; 3]),
}

/// The tracker's state for one level.
#[derive(Debug, Clone)]
pub struct Tracker {
    /// `SurfIsDynamic`: surfaces from here on belong to movers.
    pub first_dynamic_surf: usize,
    first_dynamic_node: usize,
    brushes: Vec<Brush>,
    /// Per dynamic node (index - `first_dynamic_node`): the brush that owns it and the node it hangs from
    /// (`NodeMaps`, tracker+0x50); `None` for a free slot.
    owners: Vec<Option<(usize, usize)>>,
    free_points: Vec<usize>,
    free_verts: Vec<usize>,
    /// Bumped every time `Update` redid brushes (callers mirror the nodes elsewhere when it changes).
    pub generation: u64,
}

impl Tracker {
    /// `GNewBrushTracker`: registers `movers` (in the level's actor order) and adds them all.
    pub fn new(model: &mut Model, movers: &[(Mover, Vec<Poly>)]) -> Tracker {
        let mut t = Tracker { first_dynamic_surf: model.surfs.len(), first_dynamic_node: model.nodes.len(), brushes: Vec::new(), owners: Vec::new(), free_points: Vec::new(), free_verts: Vec::new(), generation: 0 };
        for (m, polys) in movers {
            // 1037b5c0: a fresh surface per polygon with new base, normal and texture vectors
            let mut surfs = Vec::new();
            for (k, p) in polys.iter().enumerate() {
                let new_vec = |v: &mut Vec<Vec3>| {
                    v.push([0.0; 3]);
                    (v.len() - 1) as i32
                };
                let v_normal = new_vec(&mut model.vectors);
                let v_texture_u = new_vec(&mut model.vectors);
                let v_texture_v = new_vec(&mut model.vectors);
                let p_base = new_vec(&mut model.points);
                let mut poly_flags = p.poly_flags & 0x3cff_ffff;
                if m.special_lit {
                    poly_flags |= 0x10_0000;
                }
                model.surfs.push(BspSurf { texture: p.texture, poly_flags, p_base, v_normal, v_texture_u, v_texture_v, i_light_map: p.i_brush_poly, i_brush_poly: k as i32, pan_u: p.pan_u, pan_v: p.pan_v, actor: 0 });
                surfs.push(model.surfs.len() - 1);
            }
            t.brushes.push(Brush { id: m.id, surfs, nodes: Vec::new(), old: ([-1.0; 3], [123, 456, 789]) });
        }
        // `Update(None)`: every brush, each listed at the head (so the last registered goes first)
        let list: Vec<usize> = (0..t.brushes.len()).rev().collect();
        let all: Vec<&(Mover, Vec<Poly>)> = movers.iter().collect();
        t.process(model, list, &|b| all.get(b).copied());
        t
    }

    /// Whether mover `id` stands somewhere else than where it was last added (what `Update(Actor)` tests first).
    pub fn moved(&self, id: u32, location: Vec3, rotation: [i32; 3]) -> bool {
        self.brushes.iter().find(|b| b.id == id).map(|b| b.old != (location, rotation)).unwrap_or(false)
    }

    /// `SurfIsDynamic` (1037ac10).
    pub fn surf_is_dynamic(&self, i_surf: usize) -> bool {
        i_surf >= self.first_dynamic_surf
    }

    /// `Update(Actor)`: redoes `mover` (and whatever hangs below its nodes) when it moved since it was last added.
    /// `others` gives the current state of any other mover by id (needed when it hangs below).
    pub fn update(&mut self, model: &mut Model, mover: &(Mover, Vec<Poly>), others: &dyn Fn(u32) -> Option<(Mover, Vec<Poly>)>) {
        let Some(b) = self.brushes.iter().position(|x| x.id == mover.0.id) else { return };
        if self.brushes[b].old == (mover.0.location, mover.0.rotation) {
            return;
        }
        let cache: std::cell::RefCell<std::collections::HashMap<usize, Option<(Mover, Vec<Poly>)>>> = Default::default();
        let ids: Vec<u32> = self.brushes.iter().map(|x| x.id).collect();
        // the listed brush's state comes from `mover`, the others from `others`
        let lookup = |i: usize| -> Option<(Mover, Vec<Poly>)> {
            if i == b {
                return Some(mover.clone());
            }
            cache.borrow_mut().entry(i).or_insert_with(|| others(ids[i])).clone()
        };
        self.process_owned(model, vec![b], &lookup);
    }

    fn process<'a>(&mut self, model: &mut Model, list: Vec<usize>, get: &dyn Fn(usize) -> Option<&'a (Mover, Vec<Poly>)>) {
        self.process_owned(model, list, &|i| get(i).cloned());
    }

    /// The body of `Update` after the list is made.
    fn process_owned(&mut self, model: &mut Model, mut list: Vec<usize>, get: &dyn Fn(usize) -> Option<(Mover, Vec<Poly>)>) {
        self.generation += 1;
        let mut queued: Vec<bool> = vec![false; self.brushes.len()];
        for &b in &list {
            queued[b] = true;
        }
        // mark the listed brushes' nodes and all below them (1037c4f0); owners of marked nodes join at the head
        let snapshot = list.clone();
        for b in snapshot {
            for n in self.brushes[b].nodes.clone() {
                self.mark(model, n, &mut list, &mut queued);
            }
        }
        // unlink (1037c6d0), head first
        for &b in &list {
            self.remove(model, b);
        }
        // put each back, head first
        for &b in &list {
            if let Some((m, polys)) = get(b) {
                self.add_brush(model, b, &m, &polys);
            }
        }
    }

    /// 1037c4f0: marks `node` and its subtree new (once) and lists the brushes that own them.
    fn mark(&mut self, model: &mut Model, node: usize, list: &mut Vec<usize>, queued: &mut Vec<bool>) {
        if model.nodes[node].node_flags & NF_IS_NEW != 0 {
            return;
        }
        model.nodes[node].node_flags |= NF_IS_NEW;
        let (f, b, p) = (model.nodes[node].i_front, model.nodes[node].i_back, model.nodes[node].i_plane);
        for c in [f, b, p] {
            if c >= 0 {
                self.mark(model, c as usize, list, queued);
            }
        }
        if let Some(Some((owner, _))) = node.checked_sub(self.first_dynamic_node).and_then(|i| self.owners.get(i)) {
            let owner = *owner;
            if !queued[owner] {
                queued[owner] = true;
                list.insert(0, owner);
            }
        }
    }

    /// 1037c6d0: every node of brush `b` is unlinked from the node it hangs from and its slots freed.
    fn remove(&mut self, model: &mut Model, b: usize) {
        for n in std::mem::take(&mut self.brushes[b].nodes) {
            let i = n - self.first_dynamic_node;
            if let Some((_, parent)) = self.owners[i] {
                let p = &mut model.nodes[parent];
                if p.i_front == n as i32 {
                    p.i_front = -1;
                } else if p.i_back == n as i32 {
                    p.i_back = -1;
                } else if p.i_plane == n as i32 {
                    p.i_plane = -1;
                }
            }
            let nd = &model.nodes[n];
            for k in 0..nd.num_vertices as usize {
                let vi = nd.i_vert_pool as usize + k;
                self.free_points.push(model.verts[vi].p_vertex as usize);
                self.free_verts.push(vi);
            }
            self.owners[i] = None;
        }
    }

    /// The re-add part of `Update` for brush `b`: transform, write the surfaces, sphere filter, filter every polygon.
    fn add_brush(&mut self, model: &mut Model, b: usize, m: &Mover, polys: &[Poly]) {
        self.brushes[b].old = (m.location, m.rotation);
        let world: Vec<Poly> = polys.iter().map(|p| transform(p, m)).collect();
        let pts: Vec<Vec3> = world.iter().flat_map(|p| p.vertices.iter().copied()).collect();
        let sphere = sphere_of(&pts);
        precompute_sphere_filter(model, sphere);
        for (k, p) in world.iter().enumerate() {
            let s = self.brushes[b].surfs[k];
            let surf = model.surfs[s].clone();
            model.points[surf.p_base as usize] = p.base;
            model.vectors[surf.v_normal as usize] = p.normal;
            model.vectors[surf.v_texture_u as usize] = p.texture_u;
            model.vectors[surf.v_texture_v as usize] = p.texture_v;
            if !model.nodes.is_empty() {
                self.filter(model, b, s as i32, 0, -1, p.clone(), true, p.normal);
            }
        }
        for &n in &self.brushes[b].nodes {
            model.nodes[n].node_flags &= !NF_IS_NEW;
        }
    }

    /// 1037c0c0: filters `poly` down from `node`, adding the pieces that end outside at empty child slots.
    #[allow(clippy::too_many_arguments)]
    fn filter(&mut self, model: &mut Model, b: usize, i_surf: i32, mut node: usize, mut i_coplanar: i32, mut poly: Poly, mut outside: bool, normal: Vec3) {
        loop {
            if poly.vertices.len() >= VERTEX_THRESHOLD {
                let half = split_in_half(&mut poly);
                self.filter(model, b, i_surf, node, i_coplanar, half, outside, normal);
            }
            let nd = model.nodes[node].clone();
            let is_csg = nd.num_vertices > 0 && nd.node_flags & (NF_IS_NEW | NF_NOT_CSG) == 0;
            let side = if nd.node_flags & NF_IS_FRONT != 0 {
                Side::Front
            } else if nd.node_flags & NF_IS_BACK != 0 {
                Side::Back
            } else {
                match split_with_plane_fast(&poly, nd.plane) {
                    Split::Front => Side::Front,
                    Split::Back => Side::Back,
                    Split::Coplanar => {
                        let d = (normal[2] as f64 * nd.plane[2] as f64 + normal[1] as f64 * nd.plane[1] as f64) + nd.plane[0] as f64 * normal[0] as f64;
                        if d >= 0.0 {
                            i_coplanar = node as i32;
                        }
                        Side::Front
                    }
                    Split::Split(front, back) => {
                        let o = outside || is_csg;
                        if nd.i_front >= 0 {
                            self.filter(model, b, i_surf, nd.i_front as usize, i_coplanar, front, o, normal);
                        } else if o {
                            self.add_fragment(model, b, i_surf, node, i_coplanar, true, &front);
                        }
                        poly = back;
                        Side::Back
                    }
                }
            };
            match side {
                Side::Front => {
                    outside = outside || is_csg;
                    if nd.i_front < 0 {
                        if outside {
                            self.add_fragment(model, b, i_surf, node, i_coplanar, true, &poly);
                        }
                        return;
                    }
                    node = nd.i_front as usize;
                }
                Side::Back => {
                    outside = outside && !is_csg;
                    if nd.i_back < 0 {
                        if outside {
                            self.add_fragment(model, b, i_surf, node, i_coplanar, false, &poly);
                        }
                        return;
                    }
                    node = nd.i_back as usize;
                }
            }
        }
    }

    /// 1037bb00: a new node for `poly`, hung below `node` (front or back) or at the end of `i_coplanar`'s chain.
    #[allow(clippy::too_many_arguments)]
    fn add_fragment(&mut self, model: &mut Model, b: usize, i_surf: i32, node: usize, i_coplanar: i32, front: bool, poly: &Poly) {
        let (mut parent, mut slot, mut other_chain) = (node, front as usize, false);
        if i_coplanar >= 0 {
            other_chain = node as i32 != i_coplanar;
            slot = 2;
            parent = i_coplanar as usize;
            while model.nodes[parent].i_plane >= 0 {
                parent = model.nodes[parent].i_plane as usize;
            }
        }
        let n = poly.normal;
        let w = ((poly.base[1] as f64 * n[1] as f64 + poly.base[2] as f64 * n[2] as f64) + poly.base[0] as f64 * n[0] as f64) as f32;
        let mut flags = NF_IS_NEW;
        if poly.poly_flags & 0x8 != 0 {
            flags = NF_IS_NEW | NF_NOT_CSG;
        }
        if poly.poly_flags & 0x400_0001 != 0 {
            flags |= 0x4;
        }
        if poly.poly_flags & 0x2 != 0 {
            flags |= 0x2;
        }
        let p = model.nodes[parent].clone();
        let (i_zone, i_leaf) = if i_coplanar < 0 || other_chain {
            let at = &model.nodes[node];
            let s = front as usize;
            ([at.i_zone[s]; 2], [at.i_leaf[s]; 2])
        } else {
            let d = (p.plane[2] as f64 * n[2] as f64 + p.plane[1] as f64 * n[1] as f64) + p.plane[0] as f64 * n[0] as f64;
            let u = (d < 0.0) as usize;
            ([p.i_zone[u], p.i_zone[1 - u]], [p.i_leaf[u], p.i_leaf[1 - u]])
        };
        // vertices: a new point each, `iSide` none
        let first_vert = self.alloc_verts(model, poly.vertices.len());
        for (k, v) in poly.vertices.iter().enumerate() {
            let pt = match self.free_points.pop() {
                Some(i) => {
                    model.points[i] = *v;
                    i
                }
                None => {
                    model.points.push(*v);
                    model.points.len() - 1
                }
            };
            model.verts[first_vert + k] = Vert { p_vertex: pt as i32, i_side: -1 };
        }
        let new = BspNode {
            plane: [n[0], n[1], n[2], w],
            zone_mask: p.zone_mask,
            node_flags: flags,
            i_vert_pool: first_vert as i32,
            i_surf,
            i_back: -1,
            i_front: -1,
            i_plane: -1,
            i_collision_bound: -1,
            i_render_bound: -1,
            i_zone,
            num_vertices: poly.vertices.len() as u8,
            i_leaf,
        };
        let idx = match self.owners.iter().position(|o| o.is_none()) {
            Some(i) => {
                model.nodes[self.first_dynamic_node + i] = new;
                self.first_dynamic_node + i
            }
            None => {
                model.nodes.push(new);
                self.owners.push(None);
                model.nodes.len() - 1
            }
        };
        self.owners[idx - self.first_dynamic_node] = Some((b, parent));
        model.nodes[node].zone_mask |= (1u64 << (i_zone[1] & 63)) | (1u64 << (i_zone[0] & 63));
        let pn = &mut model.nodes[parent];
        match slot {
            0 => pn.i_back = idx as i32,
            1 => pn.i_front = idx as i32,
            _ => pn.i_plane = idx as i32,
        }
        self.brushes[b].nodes.push(idx);
    }

    /// `count` consecutive vertex slots (freed runs are reused when long enough).
    fn alloc_verts(&mut self, model: &mut Model, count: usize) -> usize {
        self.free_verts.sort_unstable();
        let f = &self.free_verts;
        let mut run = 0;
        for k in 0..f.len() {
            run = if k > 0 && f[k] == f[k - 1] + 1 { run + 1 } else { 1 };
            if run == count {
                let start = f[k + 1 - count];
                self.free_verts.drain(k + 1 - count..=k);
                return start;
            }
        }
        let start = model.verts.len();
        model.verts.extend((0..count).map(|_| Vert { p_vertex: 0, i_side: -1 }));
        start
    }

    /// The mover owning dynamic surface `i_surf` (`Surf.Actor`, set by 1037b5c0) and the brush polygon it was made from.
    pub fn surf_owner(&self, i_surf: usize) -> Option<(u32, usize)> {
        self.brushes.iter().find_map(|b| b.surfs.iter().position(|&s| s == i_surf).map(|k| (b.id, k)))
    }

    /// The dynamic nodes of the mover with id `id` (tests and tools).
    pub fn nodes_of(&self, id: u32) -> Vec<usize> {
        self.brushes.iter().find(|b| b.id == id).map(|b| b.nodes.clone()).unwrap_or_default()
    }
}

enum Side {
    Front,
    Back,
}

/// `ESplitType`.
pub enum Split {
    Coplanar,
    Front,
    Back,
    Split(Poly, Poly),
}

/// `FPoly::SplitWithPlaneFast` (Engine 10386ad0): vertices at distance `>= 0` are front, the others back; a side
/// counts once a vertex is more than 0.25 beyond the plane. A split edge gets the intersection
/// (`t = (W - P.prev) / (P . (cur - prev))`, the differences stored as floats) in both halves.
pub fn split_with_plane_fast(p: &Poly, plane: [f32; 4]) -> Split {
    let dist = |v: Vec3| ((plane[2] as f64 * v[2] as f64 + plane[0] as f64 * v[0] as f64) + plane[1] as f64 * v[1] as f64) - plane[3] as f64;
    let status: Vec<bool> = p.vertices.iter().map(|&v| dist(v) < 0.0).collect();
    let front = p.vertices.iter().any(|&v| dist(v) > SPLIT_THRESH);
    let back = p.vertices.iter().any(|&v| dist(v) < -SPLIT_THRESH);
    if !front {
        return if back { Split::Back } else { Split::Coplanar };
    }
    if !back {
        return Split::Front;
    }
    let n = p.vertices.len();
    let (mut fv, mut bv) = (Vec::new(), Vec::new());
    let (mut prev, mut prev_back) = (p.vertices[n - 1], status[n - 1]);
    for (k, &cur) in p.vertices.iter().enumerate() {
        let cur_back = status[k];
        if cur_back == prev_back {
            if cur_back { bv.push(cur) } else { fv.push(cur) }
        } else {
            let d = [cur[0] - prev[0], cur[1] - prev[1], cur[2] - prev[2]];
            let num = plane[3] as f64 - ((plane[0] as f64 * prev[0] as f64 + plane[2] as f64 * prev[2] as f64) + plane[1] as f64 * prev[1] as f64);
            let den = (plane[2] as f64 * d[2] as f64 + plane[1] as f64 * d[1] as f64) + d[0] as f64 * plane[0] as f64;
            let t = (num / den) as f32;
            let i = [((d[0] as f64 * t as f64) + prev[0] as f64) as f32, (d[1] * t) + prev[1], (d[2] * t) + prev[2]];
            bv.push(i);
            fv.push(i);
            if !prev_back {
                bv.push(cur);
            } else {
                fv.push(cur);
            }
        }
        prev = cur;
        prev_back = cur_back;
    }
    let mk = |vertices: Vec<Vec3>| Poly { vertices, ..p.clone() };
    Split::Split(mk(fv), mk(bv))
}

/// `FPoly::SplitInHalf` (Engine 10386f70): the second half (from vertex `n/2`, closed with vertex 0) is returned, the
/// polygon keeps vertices `0..=n/2`; both get `PF_EdCut`.
pub fn split_in_half(p: &mut Poly) -> Poly {
    let n = p.vertices.len();
    let m = n / 2;
    let mut half = p.clone();
    half.vertices = p.vertices[m..].iter().copied().chain(std::iter::once(p.vertices[0])).collect();
    p.vertices.truncate(m + 1);
    p.poly_flags |= PF_EDCUT;
    half.poly_flags |= PF_EDCUT;
    half
}

/// `FPoly::Transform` (Engine 10387360) with `ABrush::BuildCoords`' frames: texture vectors through the vector frame;
/// base and vertices `(v - PrePivot)` through the point frame plus `Location` (the difference, the product and the
/// sum each stored as floats); vertex order reversed for a negative orientation; last the normal through the vector
/// frame, stored, then `SafeNormal`.
pub fn transform(p: &Poly, m: &Mover) -> Poly {
    let point = |v: Vec3| -> Vec3 {
        let r = [v[0] - m.pre_pivot[0], v[1] - m.pre_pivot[1], v[2] - m.pre_pivot[2]];
        let t = m.point.transform_vector(r);
        [t[0] + m.location[0], t[1] + m.location[1], t[2] + m.location[2]]
    };
    let mut vertices: Vec<Vec3> = p.vertices.iter().map(|&v| point(v)).collect();
    if m.orientation < 0.0 {
        vertices.reverse();
    }
    Poly {
        base: point(p.base),
        normal: safe_normal(m.vector.transform_vector(p.normal)),
        texture_u: m.vector.transform_vector(p.texture_u),
        texture_v: m.vector.transform_vector(p.texture_v),
        vertices,
        ..p.clone()
    }
}

/// `FSphere::FSphere(Points, Count)` (Core 10144960): centre of the bounding box, radius the farthest point times
/// 1.001.
pub fn sphere_of(pts: &[Vec3]) -> [f32; 4] {
    if pts.is_empty() {
        return [0.0; 4];
    }
    let (mut mn, mut mx) = (pts[0], pts[0]);
    for p in pts {
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
    }
    let c = [(mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, (mn[2] + mx[2]) * 0.5];
    let mut w = 0.0f32;
    for p in pts {
        let d = [c[0] - p[0], c[1] - p[1], c[2] - p[2]];
        let r = (d[1] as f64 * d[1] as f64 + d[0] as f64 * d[0] as f64) + d[2] as f64 * d[2] as f64;
        if r > w as f64 {
            w = r as f32;
        }
    }
    [c[0], c[1], c[2], ((w as f64).sqrt() * 1.001) as f32]
}

/// `UModel::PrecomputeSphereFilter` (Engine 103f6460): down the BSP from the root, a node the sphere lies wholly in front
/// of gets `NF_IsFront`, wholly behind `NF_IsBack` (both cleared first); a node it straddles sends the walk down both
/// sides.
pub fn precompute_sphere_filter(model: &mut Model, s: [f32; 4]) {
    fn walk(model: &mut Model, mut n: i32, s: [f32; 4]) {
        while n >= 0 {
            let nd = &mut model.nodes[n as usize];
            nd.node_flags &= 0x3f;
            let d = (s[0] * nd.plane[0] + nd.plane[1] * s[1] + nd.plane[2] * s[2]) - nd.plane[3];
            if d < -s[3] {
                nd.node_flags |= NF_IS_BACK;
                n = nd.i_back;
            } else {
                if d > s[3] {
                    nd.node_flags |= NF_IS_FRONT;
                } else if nd.i_back >= 0 {
                    let b = nd.i_back;
                    walk(model, b, s);
                }
                n = model.nodes[n as usize].i_front;
            }
        }
    }
    if !model.nodes.is_empty() {
        walk(model, 0, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(z: f32) -> Poly {
        Poly { base: [0.0, 0.0, z], normal: [0.0, 0.0, 1.0], texture_u: [1.0, 0.0, 0.0], texture_v: [0.0, 1.0, 0.0], vertices: vec![[0.0, 0.0, z], [10.0, 0.0, z], [10.0, 10.0, z], [0.0, 10.0, z]], poly_flags: 0, texture: 0, pan_u: 0, pan_v: 0, i_brush_poly: 0 }
    }

    #[test]
    fn split_classifies_and_cuts() {
        let p = quad(0.0);
        // x = 5 cuts it: vertices 0 and 3 behind (x - 5 < 0)
        match split_with_plane_fast(&p, [1.0, 0.0, 0.0, 5.0]) {
            Split::Split(f, b) => {
                // walking from the last vertex (0,10) behind: (0,0) behind, (10,0) front -> cut at (5,0) into both,
                // front gets (10,0); (10,10) front; (0,10) behind -> cut at (5,10), back gets (0,10)
                assert_eq!(f.vertices, vec![[5.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [5.0, 10.0, 0.0]]);
                assert_eq!(b.vertices, vec![[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [5.0, 10.0, 0.0], [0.0, 10.0, 0.0]]);
            }
            _ => panic!("not split"),
        }
        assert!(matches!(split_with_plane_fast(&p, [1.0, 0.0, 0.0, -1.0]), Split::Front));
        assert!(matches!(split_with_plane_fast(&p, [1.0, 0.0, 0.0, 11.0]), Split::Back));
        assert!(matches!(split_with_plane_fast(&p, [0.0, 0.0, 1.0, 0.0]), Split::Coplanar));
        // within 0.25 of the plane counts as on it
        assert!(matches!(split_with_plane_fast(&p, [1.0, 0.0, 0.0, 10.2]), Split::Back));
    }

    #[test]
    fn halves() {
        let mut p = quad(0.0);
        p.vertices = (0..6).map(|k| [k as f32, 0.0, 0.0]).collect();
        let h = split_in_half(&mut p);
        assert_eq!(p.vertices.len(), 4);
        assert_eq!(h.vertices, vec![[3.0, 0.0, 0.0], [4.0, 0.0, 0.0], [5.0, 0.0, 0.0], [0.0, 0.0, 0.0]]);
        assert!(p.poly_flags & PF_EDCUT != 0 && h.poly_flags & PF_EDCUT != 0);
    }

    #[test]
    fn sphere_of_points() {
        let s = sphere_of(&[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 3.0, 0.0]]);
        assert_eq!(&s[..3], &[1.0, 1.5, 0.0]);
        assert!((s[3] - (1.0f64 + 2.25).sqrt() as f32 * 1.001).abs() < 1e-5);
    }

    #[test]
    fn mirrored_transform_reverses_and_keeps_the_normal_outward() {
        // X mirrored: x -> -x; the polygon's winding flips, its normal (0,0,1) stays
        let (mut point, mut inv) = (Coords::UNIT, Coords::UNIT);
        point.mul_scale([-1.0, 1.0, 1.0], 0.0, 0);
        inv.div_scale([-1.0, 1.0, 1.0], 0.0, 0);
        let m = Mover { id: 1, location: [100.0, 0.0, 0.0], rotation: [0; 3], pre_pivot: [0.0; 3], point, vector: inv.transpose(), orientation: -1.0, special_lit: false };
        let t = transform(&quad(5.0), &m);
        assert_eq!(t.vertices[0], [100.0, 10.0, 5.0]);
        assert_eq!(t.vertices[3], [100.0, 0.0, 5.0]);
        assert_eq!(t.normal, [0.0, 0.0, 1.0]);
        assert_eq!(t.base, [100.0, 0.0, 5.0]);
    }
}

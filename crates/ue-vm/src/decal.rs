//! Decals (`Decal` actors: bullet holes, blood splats, burn marks): `ADecal::execAttachDecal` (Engine 103e7xxx, stub
//! 10302f9f) finds the BSP surface behind the decal and puts a square on it, spread over neighbouring surfaces up to
//! `MultiDecalLevel`; each surface entry lists the nodes of that surface it touches (`FUN_103e9340`). The renderer clips
//! the entries to those nodes (`URender::ClipDecal`) and stamps `LastRenderedTime` when it draws one.
use crate::collision::Vec3;
use crate::Vm;
use ue_assets::model::Model;

/// One surface a decal lies on: its four corners (world coordinates; the engine keeps them relative to the surface's
/// base point) and the nodes of the surface they touch.
#[derive(Debug, Clone, PartialEq)]
pub struct DecalSurf {
    pub surf: usize,
    pub verts: [Vec3; 4],
    pub nodes: Vec<usize>,
}

/// Where an attached decal lies: the point it hit, the hit surface's normal, and its surface entries (the first is
/// the hit surface). Decals restored from older saves have no entries.
#[derive(Debug, Clone, PartialEq)]
pub struct DecalPlace {
    pub center: Vec3,
    pub normal: Vec3,
    pub surfs: Vec<DecalSurf>,
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn size(a: Vec3) -> f32 {
    ((dot(a, a)) as f64).sqrt() as f32
}

/// `FVector::SafeNormal`-style normalisation as `AttachDecal` inlines it: only when the squared size is at least 1e-8,
/// scaled by `1 / sqrt` in double precision.
fn safe_normal(v: Vec3) -> Vec3 {
    let sq = dot(v, v);
    if sq >= 1e-8 {
        let k = 1.0 / (sq as f64).sqrt();
        [(v[0] as f64 * k) as f32, (v[1] as f64 * k) as f32, (v[2] as f64 * k) as f32]
    } else {
        v
    }
}

/// The edge plane of a node polygon edge `prev -> v` with surface normal `n`: `n x (v - prev)` (both helpers build it
/// component by component this way).
fn edge_normal(n: Vec3, prev: Vec3, v: Vec3) -> Vec3 {
    let d = sub(v, prev);
    [d[2] * n[1] - d[1] * n[2], d[0] * n[2] - d[2] * n[0], d[1] * n[0] - d[0] * n[1]]
}

fn node_points(m: &Model, node: usize) -> Vec<Vec3> {
    let n = &m.nodes[node];
    (0..n.num_vertices as usize)
        .map(|k| m.verts.get(n.i_vert_pool as usize + k).and_then(|v| m.points.get(v.p_vertex as usize)).copied().unwrap_or([0.0; 3]))
        .collect()
}

fn surf_normal(m: &Model, surf: usize) -> Vec3 {
    m.vectors.get(m.surfs[surf].v_normal as usize).copied().unwrap_or([0.0, 0.0, 1.0])
}

fn surf_base(m: &Model, surf: usize) -> Vec3 {
    m.points.get(m.surfs[surf].p_base as usize).copied().unwrap_or([0.0; 3])
}

/// `FUN_103e9090`: the surface of the first node, along `node`'s chain of coplanar nodes (`iPlane`), whose polygon
/// holds `p` (no change of side between consecutive edges; zero distances do not count); -1 when none does.
pub fn find_surf(m: &Model, mut node: i32, p: Vec3) -> i32 {
    while node >= 0 {
        let Some(nd) = m.nodes.get(node as usize) else { return -1 };
        if nd.num_vertices != 0 && nd.i_surf >= 0 {
            let n = surf_normal(m, nd.i_surf as usize);
            let pts = node_points(m, node as usize);
            let mut prev = *pts.last().unwrap();
            let mut last = 0.0f32;
            let mut inside = true;
            for &v in &pts {
                let e = edge_normal(n, prev, v);
                let d = (p[0] * e[0] + p[2] * e[2] + p[1] * e[1]) - (v[0] * e[0] + v[2] * e[2] + v[1] * e[1]);
                if (d < 0.0 && last > 0.0) || (d > 0.0 && last < 0.0) {
                    inside = false;
                    break;
                }
                prev = v;
                last = d;
            }
            if inside {
                return nd.i_surf;
            }
        }
        node = nd.i_plane;
    }
    -1
}

/// `FUN_103e9340`: the nodes of surface `surf` (in node order) that the polygon `verts` overlaps: the polygon clipped
/// against every edge plane of the node (keeping the side where `e . p - e . v >= 0`, crossings inserted) keeps a point.
pub fn touched_nodes(m: &Model, gen: u64, surf: usize, verts: &[Vec3; 4]) -> Vec<usize> {
    let n = surf_normal(m, surf);
    let mut out = Vec::new();
    // the surface's nodes in node order (an index of the model, made once: this runs for every decal placed)
    SURF_NODES.with(|cache| {
        let mut cache = cache.borrow_mut();
        let key = (gen, m as *const Model as usize, m.nodes.len());
        if cache.0 != key {
            let mut index: Vec<Vec<u32>> = vec![Vec::new(); m.surfs.len()];
            for (node, nd) in m.nodes.iter().enumerate() {
                if nd.i_surf >= 0 && nd.num_vertices != 0 {
                    if let Some(v) = index.get_mut(nd.i_surf as usize) {
                        v.push(node as u32);
                    }
                }
            }
            *cache = (key, index);
        }
        for &node in cache.1.get(surf).map(|v| v.as_slice()).unwrap_or(&[]) {
            if !clip_to_node(m, node as usize, n, verts.to_vec()).is_empty() {
                out.push(node as usize);
            }
        }
    });
    out
}

thread_local! {
    /// `touched_nodes`' index: the model it was made for (`Vm::world_model_gen`, address, node count) and each surface's
    /// nodes.
    static SURF_NODES: std::cell::RefCell<((u64, usize, usize), Vec<Vec<u32>>)> = std::cell::RefCell::new(((u64::MAX, 0, 0), Vec::new()));
}

/// The polygon `poly` clipped to node `node`'s polygon (surface normal `n`), as `FUN_103e9340` does it.
pub fn clip_to_node(m: &Model, node: usize, n: Vec3, mut poly: Vec<Vec3>) -> Vec<Vec3> {
    let pts = node_points(m, node);
    let mut prev = *pts.last().unwrap();
    for &v in &pts {
        let e = edge_normal(n, prev, v);
        let plane_d = v[0] * e[0] + v[2] * e[2] + v[1] * e[1];
        let mut dist: Vec<f32> = poly.iter().map(|p| (e[0] * p[0] + e[1] * p[1] + e[2] * p[2]) - plane_d).collect();
        let mut i = 0;
        while i < poly.len() {
            let j = (i + 1) % poly.len();
            let cross = (dist[i] < 0.0 && dist[j] > 0.0) || (dist[i] > 0.0 && dist[j] < 0.0);
            if cross {
                let d = sub(poly[j], poly[i]);
                let t = (plane_d - (e[0] * poly[i][0] + e[1] * poly[i][1] + e[2] * poly[i][2])) / (d[0] * e[0] + d[2] * e[2] + d[1] * e[1]);
                let x = add(poly[i], [d[0] * t, d[1] * t, d[2] * t]);
                poly.insert(i + 1, x);
                dist.insert(i + 1, 0.0);
                i += 2;
            } else {
                i += 1;
            }
        }
        let keep: Vec<(Vec3, f32)> = poly.into_iter().zip(dist).filter(|(_, d)| *d >= 0.0).collect();
        poly = keep.into_iter().map(|(p, _)| p).collect();
        if poly.is_empty() {
            return poly;
        }
        prev = v;
    }
    poly
}

impl Vm {
    /// The node a world trace from `start` to `end` hits (`UModel::LineCheck` with no extent: `Hit.Item`), if any.
    fn model_hit_node(&mut self, start: Vec3, end: Vec3) -> Option<i32> {
        let level = self.level;
        let hit = self.multi_line_check(start, end, [0.0; 3], true, false, 0).into_iter().find(|h| h.actor == level)?;
        (hit.item >= 0).then_some(hit.item)
    }

    /// `ADecal::execAttachDecal(TraceDistance, DecalDir)`: true when the decal now lies on a surface. `MultiDecalLevel`
    /// is clamped to 4. A trace from the decal along minus its rotation's X axis, `TraceDistance` long, must hit the
    /// world; the surface of the node holding the point where the trace meets the hit plane (`find_surf`) takes it
    /// unless the trace runs along it, the point is off its plane by more than 0.001 or the surface is flagged 0x600.
    /// The square is `sqrt(DrawScale^2 * USize^2 / 2)` from centre to corner: with a `DecalDir` its corners lie along
    /// `U +- U x N` (U = the direction projected on the surface; when that is too short, the direction to the surface's
    /// base point), without one along a random direction (three `FRand`s per try until inside the unit sphere) and
    /// its cross with N. Then a `MultiDecalLevel` x `MultiDecalLevel` grid of points over the square is traced 16 units
    /// each way: other surfaces found there whose normal is within `acos 0.7` get the square folded onto them.
    /// (Surfaces of movers: no mover's polygons are in the level model, so that branch never applies here.)
    pub fn attach_decal(&mut self, a: u32, trace_distance: f32, decal_dir: Vec3) -> bool {
        self.detach_decal(a);
        let texture = self.u32_at_prop(a, "Texture");
        if texture == 0 {
            return false;
        }
        let ml = self.ip(a, "MultiDecalLevel").min(4);
        if let Some((off, _)) = self.prop_at(a, "MultiDecalLevel") {
            self.set_u32_at(a, off, ml as u32);
        }
        let Some(model) = self.world_model.clone() else { return false };
        let m: &Model = &model;
        let o = self.offsets();
        let loc = self.vec_at(a, o.location);
        let fwd = crate::natives::rotate_vector([1.0, 0.0, 0.0], self.rot_of(a), false);
        let dir = mul(safe_normal(mul(fwd, -1.0)), trace_distance);
        let mut dd = decal_dir;
        let mut random = false;
        if dd == [0.0; 3] {
            loop {
                let x = self.frand() * 2.0 - 1.0;
                let y = self.frand() * 2.0 - 1.0;
                let z = self.frand() * 2.0 - 1.0;
                dd = [x, y, z];
                if dot(dd, dd) <= 1.0 {
                    break;
                }
            }
            // `UnsafeNormal`
            let k = 1.0 / (dot(dd, dd) as f64).sqrt();
            dd = [(dd[0] as f64 * k) as f32, (dd[1] as f64 * k) as f32, (dd[2] as f64 * k) as f32];
            random = true;
        }
        let end = add(loc, dir);
        let Some(node) = self.model_hit_node(loc, end) else { return false };
        let along = |base: Vec3, n: Vec3| add(loc, mul(sub(end, loc), dot(sub(base, loc), n) / dot(sub(end, loc), n)));
        let s0 = m.nodes[node as usize].i_surf;
        if s0 < 0 {
            return false;
        }
        let first = along(surf_base(m, s0 as usize), surf_normal(m, s0 as usize));
        let surf = find_surf(m, node, first);
        if surf < 0 {
            return false;
        }
        let surf = surf as usize;
        let n = surf_normal(m, surf);
        let base = surf_base(m, surf);
        let hit = along(base, n);
        if dot(fwd, n) == 0.0 {
            return false;
        }
        if dot(sub(base, hit), n).abs() > 0.001 {
            return false;
        }
        if m.surfs[surf].poly_flags & 0x600 != 0 {
            return false;
        }
        // corners
        let usize_ = self.texture_size(texture).0;
        let scale = self.fp(a, "DrawScale");
        let half = ((scale * scale * usize_ * usize_ * 0.5) as f64).sqrt() as f32;
        let (mut c0, mut c1);
        let mut use_random = random;
        if !random {
            let d = dot(dd, n);
            let u = sub(dd, mul(n, d));
            if u[0].abs() < 1e-4 && u[1].abs() < 1e-4 && u[2].abs() < 1e-4 {
                dd = sub(base, hit);
                use_random = true;
                c0 = [0.0; 3];
                c1 = [0.0; 3];
            } else {
                let v = [u[1] * n[2] - u[2] * n[1], u[2] * n[0] - u[0] * n[2], u[0] * n[1] - u[1] * n[0]];
                let (u, v) = (safe_normal(u), safe_normal(v));
                c0 = add(v, u);
                c1 = sub(u, v);
            }
        } else {
            c0 = [0.0; 3];
            c1 = [0.0; 3];
        }
        if use_random {
            let d = dot(dd, n);
            c0 = sub(dd, mul(n, d));
            c1 = [c0[1] * n[2] - c0[2] * n[1], c0[2] * n[0] - c0[0] * n[2], c0[0] * n[1] - c0[1] * n[0]];
        }
        let (c0, c1) = (mul(safe_normal(c0), half), mul(safe_normal(c1), half));
        let verts = [add(hit, c0), add(hit, c1), add(hit, mul(c0, -1.0)), add(hit, mul(c1, -1.0))];
        let nodes = touched_nodes(m, self.world_model_gen, surf, &verts);
        let mut entries = vec![DecalSurf { surf, verts, nodes }];
        // spread over neighbouring surfaces
        let nn = safe_normal(n);
        let off = mul(nn, -16.0);
        let (e1, e2) = (sub(verts[1], verts[0]), sub(verts[3], verts[0]));
        for i in 0..ml.max(0) {
            for j in 0..ml {
                let fi = (i as f32 + 1.0) / ml as f32;
                let fj = (j as f32 + 1.0) / ml as f32;
                let p = add(mul(e2, fj), add(mul(e1, fi), verts[0]));
                let (start, stop) = (sub(p, off), add(off, p));
                let Some(node2) = self.model_hit_node(start, stop) else { continue };
                let hs = m.nodes[node2 as usize].i_surf;
                if hs < 0 {
                    continue;
                }
                let (hb, hn) = (surf_base(m, hs as usize), surf_normal(m, hs as usize));
                let pt = add(start, mul(sub(stop, start), dot(sub(hb, start), hn) / dot(sub(stop, start), hn)));
                let s2 = find_surf(m, node2, pt);
                if s2 < 0 || entries.iter().any(|e| e.surf == s2 as usize) {
                    continue;
                }
                let s2 = s2 as usize;
                if m.surfs[s2].poly_flags & 0x600 != 0 {
                    continue;
                }
                let (b2, n2) = (surf_base(m, s2), surf_normal(m, s2));
                let cos = dot(n2, n) / (size(n) * size(n2));
                if cos.abs() <= 0.7 {
                    continue;
                }
                let mut folded = [[0.0f32; 3]; 4];
                for k in 0..4 {
                    let v = verts[k];
                    let a1 = sub(v, n);
                    let p = add(a1, mul(sub(v, a1), dot(sub(b2, a1), n2) / dot(sub(v, a1), n2)));
                    let a2 = sub(v, n2);
                    let b = add(a2, mul(sub(v, a2), dot(sub(b2, a2), n2) / dot(sub(v, a2), n2)));
                    let fb = size(sub(v, b)) / cos;
                    let fp = size(sub(v, p)) / cos;
                    let mut d = sub(b, p);
                    let sq = dot(d, d);
                    if sq > 1e-8 {
                        d = mul(d, 1.0 / sq.sqrt());
                    }
                    folded[k] = sub(b, mul(d, fp - fb));
                }
                let nodes = touched_nodes(m, self.world_model_gen, s2, &folded);
                entries.push(DecalSurf { surf: s2, verts: folded, nodes });
            }
        }
        // each surface keeps its decals in order: a new one goes before the first of the same texture
        for e in &entries {
            let same = {
                let list = self.surf_decals.get(&e.surf).cloned().unwrap_or_default();
                list.iter().position(|&d| self.u32_at_prop(d, "Texture") == texture)
            };
            let list = self.surf_decals.entry(e.surf).or_default();
            match same {
                Some(i) => list.insert(i, a),
                None => list.push(a),
            }
        }
        self.decals.insert(a, DecalPlace { center: hit, normal: n, surfs: entries });
        true
    }

    /// `DetachDecal`: off every surface.
    pub fn detach_decal(&mut self, a: u32) {
        if self.decals.remove(&a).is_some() {
            for list in self.surf_decals.values_mut() {
                list.retain(|&d| d != a);
            }
        }
    }

    /// `URender::ClipDecal` (10b1bda0) for every surface entry of decal `a` and every node it touches (all nodes of the
    /// surface when it lists none): the square clipped to the node's polygon, lifted 0.75 along the unit surface normal;
    /// texel coordinates from the square's first corner: `U = (P - V0) . unit(V1 - V0) / DrawScale`, `V` the same along
    /// `V3 - V0`. (The engine clips to the node's on-screen polygon each frame; the depth test does the rest here.)
    pub fn decal_pieces(&mut self, a: u32) -> Vec<Vec<(Vec3, [f32; 2])>> {
        let mut out = Vec::new();
        let Some(place) = self.decals.get(&a).cloned() else { return out };
        let Some(model) = self.world_model.clone() else { return out };
        let m: &Model = &model;
        let scale = self.fp(a, "DrawScale");
        if scale == 0.0 {
            return out;
        }
        for e in &place.surfs {
            let n = surf_normal(m, e.surf);
            let lift = mul(safe_normal(n), 0.75);
            let ua = safe_normal(sub(e.verts[1], e.verts[0]));
            let va = safe_normal(sub(e.verts[3], e.verts[0]));
            let nodes: Vec<usize> = if e.nodes.is_empty() {
                (0..m.nodes.len()).filter(|&i| m.nodes[i].i_surf == e.surf as i32 && m.nodes[i].num_vertices != 0).collect()
            } else {
                e.nodes.clone()
            };
            for node in nodes {
                let poly = clip_to_node(m, node, n, e.verts.to_vec());
                if poly.len() < 3 {
                    continue;
                }
                out.push(
                    poly.into_iter()
                        .map(|p| {
                            let r = sub(p, e.verts[0]);
                            (add(p, lift), [dot(r, ua) / scale, dot(r, va) / scale])
                        })
                        .collect(),
                );
            }
        }
        out
    }

    /// The decal's square as half-axes for drawing (right, up), from its first surface entry; decals without entries
    /// (older saves) lie on their normal, spun by the actor's roll, sized by the texture.
    pub fn decal_axes(&mut self, a: u32) -> Option<(Vec3, Vec3, Vec3)> {
        let place = self.decals.get(&a)?.clone();
        // `ClipDecal` lifts decals off their surface by 0.75 along its unit normal (10b2b304)
        let lift = mul(safe_normal(place.normal), 0.75);
        if let Some(e) = place.surfs.first() {
            let right = mul(sub(e.verts[1], e.verts[0]), 0.5);
            let up = mul(sub(e.verts[3], e.verts[0]), 0.5);
            return Some((add(place.center, lift), right, up));
        }
        let n = place.normal;
        let reference = if n[2].abs() > 0.99 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] };
        let cross = |a: Vec3, b: Vec3| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let r0 = safe_normal(cross(reference, n));
        let u0 = cross(n, r0);
        let roll = self.rot_of(a)[2];
        let (s, c) = ((roll & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0).sin_cos();
        let tex = self.u32_at_prop(a, "Texture");
        let (w, h) = self.texture_size(tex);
        let scale = self.fp(a, "DrawScale");
        let (hw, hh) = (w * scale * 0.5, h * scale * 0.5);
        let right = [0, 1, 2].map(|k| (r0[k] * c + u0[k] * s) * hw);
        let up = [0, 1, 2].map(|k| (u0[k] * c - r0[k] * s) * hh);
        Some((add(place.center, lift), right, up))
    }
}

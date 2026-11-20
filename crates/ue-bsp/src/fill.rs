//! Missing faces of a level's BSP (an Aerugo repair, not the original's behaviour). Some shipped maps have walls that
//! are solid in the BSP but have no polygon facing the open space next to them (the Wan Chai canal's wall under the
//! water); the original draws nothing there and the view runs on through the solid to whatever faces the camera far
//! behind. `find_missing_faces` finds those gaps and `fill_missing_faces` closes them with polygons that take the
//! texture and mapping of the nearest polygon on the same plane (or the nearest wall) and a lightmap of their own,
//! shadowed against the level once its lights are known. `fill_level` applies it under water only (see there).
//!
//! How: every node's plane, cut to the node's cell (its ancestors' half-spaces), is pushed down the node's front and
//! back subtrees, which split it into pieces that each end in a leaf on either side; the original's solidity rule
//! (`UModel::PointRegion`: a CSG node makes its front empty and its back solid) says which leaves are open. A piece
//! with open space on one side and solid on the other is a wall; what the polygons already on that plane (the node and
//! its coplanar nodes) do not cover is missing.

use ue_assets::model::{BspNode, LightMapIndex, Model, Vert};

type V = [f64; 3];

/// Plane side tolerance (units).
const EPS: f64 = 0.05;
/// Gaps smaller than this (square units) or thinner than `MIN_WIDTH` are cracks between polygons, left alone.
const MIN_AREA: f64 = 16.0;
const MIN_WIDTH: f64 = 1.0;
/// Half the side of the square a node's plane starts as (beyond the world's 65536 box).
const BIG: f64 = 262_144.0;
/// `NODE_NON_SOLID_FLAGS` of `UModel::PointRegion`: a node with these flags does not change solidity.
const NON_SOLID: u8 = 0x21;
/// Surface flags that make a polygon a poor model for a wall (invisible, see-through, sky, unlit light panels, portal,
/// mirror).
const NOT_A_WALL: u32 = 0x1 | 0x2 | 0x4 | 0x40 | 0x80 | 0x40_0000 | 0x0400_0000 | 0x0800_0000;

/// What the level's package says about where filling makes sense.
pub struct FillRules<'a> {
    /// Faces whose open side is in zone `z` may be filled.
    pub zone_ok: &'a dyn Fn(u8) -> bool,
    /// The zones `zone_ok` accepts as a mask (nodes whose subtree touches none of them are skipped).
    pub zone_mask: u64,
    /// The texture (object reference) may serve as a model (not an editor marker or a sky).
    pub texture_ok: &'a dyn Fn(i32) -> bool,
}

/// `fill_missing_faces` for a level loaded from `pkg`, under water only: gaps whose open side is a water zone (a
/// `WaterZone`). Elsewhere a missing face is often the designers' window from the playable area onto backdrop
/// buildings and sky beyond the level's solid hull (the city around the NYC streets, the Paris rooftops, the
/// intro): filling those would wall the view off. Under water there are no such views; what shows through a missing
/// face there is the far end of the canal or the sea floor seen through rock the player cannot pass. Marker and sky
/// textures never serve as models.
pub fn fill_level(m: &mut Model, pkg: &ue_package::Package) -> (usize, std::ops::Range<usize>) {
    use ue_package::ObjectRef;
    let water: Vec<bool> = m
        .zones
        .iter()
        .map(|z| match ObjectRef::from_index(z.zone_actor) {
            ObjectRef::Export(e) => pkg.export_class_name(e).to_ascii_lowercase().contains("water"),
            _ => false,
        })
        .collect();
    if !water.contains(&true) {
        return (0, m.light_map.len()..m.light_map.len());
    }
    let mut textures: std::collections::HashMap<i32, bool> = Default::default();
    for s in &m.surfs {
        textures.entry(s.texture).or_insert_with(|| {
            let path = pkg.object_path(ObjectRef::from_index(s.texture)).unwrap_or_default().to_ascii_lowercase();
            !path.is_empty() && !path.contains("marker") && !path.contains(".sky.")
        });
    }
    let zone_mask = water.iter().enumerate().filter(|(_, w)| **w).fold(0u64, |m, (z, _)| m | 1u64 << (z & 63));
    let rules = FillRules { zone_ok: &|z| water.get(z as usize).copied().unwrap_or(false), zone_mask, texture_ok: &|t| textures.get(&t).copied().unwrap_or(false) };
    fill_missing_faces(m, &rules)
}

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: V) -> f64 {
    dot(a, a).sqrt()
}

#[derive(Clone, Copy)]
struct Plane {
    n: V,
    d: f64,
}
impl Plane {
    fn of(p: [f32; 4]) -> Plane {
        Plane { n: [p[0] as f64, p[1] as f64, p[2] as f64], d: p[3] as f64 }
    }
    fn dist(&self, q: V) -> f64 {
        dot(self.n, q) - self.d
    }
    fn flipped(&self) -> Plane {
        Plane { n: [-self.n[0], -self.n[1], -self.n[2]], d: -self.d }
    }
}

/// `poly` cut by `pl`: the part in front, the part behind (either may be empty). A vertex within `EPS` of the plane
/// belongs to both.
fn split(poly: &[V], pl: &Plane) -> (Vec<V>, Vec<V>) {
    let (mut f, mut b) = (Vec::new(), Vec::new());
    let n = poly.len();
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let (dp, dq) = (pl.dist(p), pl.dist(q));
        if dp >= -EPS {
            f.push(p);
        }
        if dp <= EPS {
            b.push(p);
        }
        if (dp > EPS && dq < -EPS) || (dp < -EPS && dq > EPS) {
            let t = dp / (dp - dq);
            let m = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t, p[2] + (q[2] - p[2]) * t];
            f.push(m);
            b.push(m);
        }
    }
    (if f.len() >= 3 { f } else { Vec::new() }, if b.len() >= 3 { b } else { Vec::new() })
}

fn area(poly: &[V]) -> f64 {
    let mut s = [0.0; 3];
    for i in 1..poly.len().saturating_sub(1) {
        let c = cross(sub(poly[i], poly[0]), sub(poly[i + 1], poly[0]));
        s = [s[0] + c[0], s[1] + c[1], s[2] + c[2]];
    }
    len(s) / 2.0
}

fn worth_filling(poly: &[V]) -> bool {
    let a = area(poly);
    let longest = (0..poly.len()).map(|i| len(sub(poly[(i + 1) % poly.len()], poly[i]))).fold(0.0, f64::max);
    a >= MIN_AREA && a / longest.max(1e-6) >= MIN_WIDTH
}

fn is_csg(n: &BspNode) -> bool {
    n.num_vertices != 0 && n.node_flags & NON_SOLID == 0
}

/// `child_outside` of `UModel::PointRegion`: whether the space on `front`'s side of `n` is empty.
fn child_outside(n: &BspNode, front: bool, outside: bool) -> bool {
    if front {
        outside || is_csg(n)
    } else {
        outside && !is_csg(n)
    }
}

/// Where a piece of a plane ends up on one side: open space (with its zone and leaf) or solid.
#[derive(Clone, Copy, PartialEq, Debug)]
enum End {
    Open { zone: u8, leaf: i32 },
    Solid,
}

/// Pushes `poly` (on the plane `face`, looked at from the side `toward`) down from `node`; pieces land in `out`.
fn filter(m: &Model, node: usize, poly: Vec<V>, outside: bool, toward: V, out: &mut Vec<(Vec<V>, End)>) {
    let n = &m.nodes[node];
    let pl = Plane::of(n.plane);
    let ds: Vec<f64> = poly.iter().map(|&q| pl.dist(q)).collect();
    let (front, back) = if ds.iter().all(|d| d.abs() <= EPS) {
        // on this node's plane: the side the face looks into decides
        if dot(pl.n, toward) > 0.0 {
            (poly, Vec::new())
        } else {
            (Vec::new(), poly)
        }
    } else {
        split(&poly, &pl)
    };
    for (part, is_front) in [(front, true), (back, false)] {
        if part.is_empty() {
            continue;
        }
        let o = child_outside(n, is_front, outside);
        let child = if is_front { n.i_front } else { n.i_back };
        if child >= 0 {
            filter(m, child as usize, part, o, toward, out);
        } else {
            let side = is_front as usize;
            let end = if o { End::Open { zone: n.i_zone[side], leaf: n.i_leaf[side] } } else { End::Solid };
            out.push((part, end));
        }
    }
}

/// `piece` minus the convex polygon `c` (both on one plane with normal `n`): the parts outside `c`.
fn subtract(piece: Vec<V>, c: &[V], n: V) -> Vec<Vec<V>> {
    let centre = c.iter().fold([0.0; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]).map(|x| x / c.len() as f64);
    let mut out = Vec::new();
    let mut cur = piece;
    for i in 0..c.len() {
        let (a, b) = (c[i], c[(i + 1) % c.len()]);
        let e = sub(b, a);
        if len(e) < 1e-6 {
            continue;
        }
        let mut en = cross(e, n);
        let l = len(en);
        en = en.map(|x| x / l);
        let mut edge = Plane { n: en, d: dot(en, a) };
        // the edge plane's front is the polygon's inside
        if edge.dist(centre) < 0.0 {
            edge = edge.flipped();
        }
        let (inside, outside) = split(&cur, &edge);
        if !outside.is_empty() && area(&outside) > 1e-3 {
            out.push(outside);
        }
        if inside.is_empty() {
            return out;
        }
        cur = inside;
    }
    out
}

fn node_poly(m: &Model, i: usize) -> Vec<V> {
    let n = &m.nodes[i];
    (0..n.num_vertices as i32)
        .filter_map(|k| m.verts.get((n.i_vert_pool + k) as usize).and_then(|v| m.points.get(v.p_vertex as usize)))
        .map(|p| [p[0] as f64, p[1] as f64, p[2] as f64])
        .collect()
}

/// A missing face: its polygon (convex, wound as the level's polygons are for its normal), the plane it faces out of,
/// the open side's zone and leaf, the node whose plane it lies on, and a polygon to copy the look from.
#[derive(Clone, Debug)]
pub struct MissingFace {
    pub points: Vec<[f32; 3]>,
    pub plane: [f32; 4],
    pub zone: u8,
    pub leaf: i32,
    pub node: usize,
    pub model_node: Option<usize>,
}

/// Every missing face of a level's BSP (see the module).
pub fn find_missing_faces(m: &Model, rules: &FillRules) -> Vec<MissingFace> {
    let count = m.nodes.len();
    if count == 0 {
        return Vec::new();
    }
    // the tree: parent, which side, and whether each node's cell is empty space
    let mut parent: Vec<Option<(usize, bool)>> = vec![None; count];
    let mut outside = vec![m.root_outside; count];
    let mut order = vec![0usize];
    let mut k = 0;
    while k < order.len() {
        let i = order[k];
        k += 1;
        let n = &m.nodes[i];
        for (c, front) in [(n.i_front, true), (n.i_back, false)] {
            if c >= 0 && (c as usize) < count && parent[c as usize].is_none() && c as usize != 0 {
                parent[c as usize] = Some((i, front));
                outside[c as usize] = child_outside(n, front, outside[i]);
                order.push(c as usize);
            }
        }
    }
    // the winding the level's polygons use for their plane's normal
    let wind = (0..count)
        .find_map(|i| {
            let p = node_poly(m, i);
            (p.len() >= 3).then(|| dot(cross(sub(p[1], p[0]), sub(p[2], p[0])), Plane::of(m.nodes[i].plane).n).signum())
        })
        .unwrap_or(1.0);
    // polygons that can serve as a model: a solid-looking surface, with its centre, extent and normal
    let models: Vec<(usize, V, f64, V)> = (0..count)
        .filter(|&i| m.nodes[i].num_vertices >= 3 && m.nodes[i].i_surf >= 0 && m.surfs.get(m.nodes[i].i_surf as usize).is_some_and(|s| s.poly_flags & NOT_A_WALL == 0 && (rules.texture_ok)(s.texture)))
        .map(|i| {
            let q = node_poly(m, i);
            let c = q.iter().fold([0.0; 3], |a, x| [a[0] + x[0], a[1] + x[1], a[2] + x[2]]).map(|x| x / q.len() as f64);
            let r = q.iter().map(|x| len(sub(*x, c))).fold(0.0, f64::max);
            (i, c, r, Plane::of(m.nodes[i].plane).n)
        })
        .collect();
    // the level's extent: the box of all its polygons
    let (mut level_lo, mut level_hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for (_, c, r, _) in &models {
        for k in 0..3 {
            level_lo[k] = level_lo[k].min(c[k] - r);
            level_hi[k] = level_hi[k].max(c[k] + r);
        }
    }
    let mut found = Vec::new();
    for &i in &order {
        let n = &m.nodes[i];
        if n.zone_mask & rules.zone_mask == 0 {
            continue;
        }
        let pl = Plane::of(n.plane);
        // the node's plane cut to its cell
        let c = [pl.n[0] * pl.d, pl.n[1] * pl.d, pl.n[2] * pl.d];
        let helper = if pl.n[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
        let u = cross(pl.n, helper);
        let u = u.map(|x| x / len(u));
        let v = cross(pl.n, u);
        let mut face: Vec<V> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .iter()
            .map(|&(a, b)| [c[0] + (u[0] * a + v[0] * b) * BIG, c[1] + (u[1] * a + v[1] * b) * BIG, c[2] + (u[2] * a + v[2] * b) * BIG])
            .collect();
        let mut up = parent[i];
        while let Some((p, front)) = up {
            if face.is_empty() {
                break;
            }
            let pp = Plane::of(m.nodes[p].plane);
            let (f, b) = split(&face, &pp);
            face = if front { f } else { b };
            up = parent[p];
        }
        if face.len() < 3 {
            continue;
        }
        // both sides of every piece of it
        let mut fronts = Vec::new();
        let o = child_outside(n, true, outside[i]);
        if n.i_front >= 0 {
            filter(m, n.i_front as usize, face, o, pl.n, &mut fronts);
        } else {
            fronts.push((face, if o { End::Open { zone: n.i_zone[1], leaf: n.i_leaf[1] } } else { End::Solid }));
        }
        let back_dir = pl.flipped().n;
        let ob = child_outside(n, false, outside[i]);
        let mut walls: Vec<(Vec<V>, bool, u8, i32)> = Vec::new();
        for (piece, fe) in fronts {
            let mut backs = Vec::new();
            if n.i_back >= 0 {
                filter(m, n.i_back as usize, piece, ob, back_dir, &mut backs);
            } else {
                backs.push((piece, if ob { End::Open { zone: n.i_zone[0], leaf: n.i_leaf[0] } } else { End::Solid }));
            }
            for (p, be) in backs {
                match (fe, be) {
                    (End::Open { zone, leaf }, End::Solid) => walls.push((p, true, zone, leaf)),
                    (End::Solid, End::Open { zone, leaf }) => walls.push((p, false, zone, leaf)),
                    _ => {}
                }
            }
        }
        if walls.is_empty() {
            continue;
        }
        // what the node and its coplanar nodes already cover
        let mut covers = Vec::new();
        let mut s = i as i32;
        let mut guard = 0;
        while s >= 0 && guard < count {
            let p = node_poly(m, s as usize);
            if p.len() >= 3 {
                covers.push((s as usize, p));
            }
            s = m.nodes[s as usize].i_plane;
            guard += 1;
        }
        for (piece, faces_front, zone, leaf) in walls {
            let mut left = vec![piece];
            for (_, c) in &covers {
                left = left.into_iter().flat_map(|p| subtract(p, c, pl.n)).collect();
                if left.is_empty() {
                    break;
                }
            }
            if !(rules.zone_ok)(zone) {
                continue;
            }
            // open space that runs out of the level (a leak to the void) is not a room
            let inside = |p: &Vec<V>| p.iter().all(|q| (0..3).all(|k| q[k] >= level_lo[k] - 16.0 && q[k] <= level_hi[k] + 16.0));
            for mut p in left.into_iter().filter(|p| worth_filling(p) && inside(p)) {
                let out_plane = if faces_front { pl } else { pl.flipped() };
                // wind it as the level's polygons are for the plane it faces out of
                if dot(cross(sub(p[1], p[0]), sub(p[2], p[0])), out_plane.n).signum() != wind {
                    p.reverse();
                }
                let centre = p.iter().fold([0.0; 3], |a, q| [a[0] + q[0], a[1] + q[1], a[2] + q[2]]).map(|x| x / p.len() as f64);
                // a polygon to copy: one of the plane's own beside the gap facing the same way, else the nearest wall
                // (or floor/ceiling for a level gap) anywhere: the gap's distance to its edge
                let pk = [out_plane.n[0] as f32, out_plane.n[1] as f32, out_plane.n[2] as f32, out_plane.d as f32];
                let reach = p.iter().map(|q| len(sub(*q, centre))).fold(0.0, f64::max);
                let level = out_plane.n[2].abs() > 0.7;
                let own = covers
                    .iter()
                    .map(|(j, _)| *j)
                    .filter(|&j| dot(Plane::of(m.nodes[j].plane).n, out_plane.n) > 0.0 && m.nodes[j].i_surf >= 0 && m.surfs.get(m.nodes[j].i_surf as usize).is_some_and(|s| s.poly_flags & NOT_A_WALL == 0 && (rules.texture_ok)(s.texture)))
                    .map(|j| {
                        let q = node_poly(m, j);
                        let qc = q.iter().fold([0.0; 3], |a, x| [a[0] + x[0], a[1] + x[1], a[2] + x[2]]).map(|x| x / q.len() as f64);
                        (j, len(sub(qc, centre)))
                    })
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                let model_node = own.map(|x| x.0).or_else(|| {
                    models
                        .iter()
                        .filter(|(_, _, _, n)| (n[2].abs() > 0.7) == level)
                        .map(|&(j, c, r, _)| (j, (len(sub(c, centre)) - r - reach).max(0.0)))
                        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                        .map(|x| x.0)
                });
                found.push(MissingFace { points: p.iter().map(|q| q.map(|x| x as f32)).collect(), plane: pk, zone, leaf, node: i, model_node });
            }
        }
    }
    found
}

/// Closes the missing faces of a level's model: each becomes a coplanar node of the node whose plane it lies on, with a
/// surface copied from its model polygon (its texture and flags; its mapping when on the same plane, else the model's
/// texture axes laid onto the gap's plane) and a lightmap of its own lit by the model's lights. Faces without a polygon
/// to copy are left open. Returns how many were filled and the lightmaps made for them (their shadows are made later,
/// once the lights' places are known: `shade_filled_faces`).
pub fn fill_missing_faces(m: &mut Model, rules: &FillRules) -> (usize, std::ops::Range<usize>) {
    let faces = find_missing_faces(m, rules);
    let mut filled = 0;
    let first_light_map = m.light_map.len();
    // lights near each face: those of every lightmap of a polygon within `NEAR` of it
    const NEAR: f64 = 256.0;
    let lit: Vec<(V, f64, i32)> = (0..m.nodes.len())
        .filter(|&i| m.nodes[i].num_vertices >= 3 && m.nodes[i].i_surf >= 0)
        .filter_map(|i| {
            let lm = m.surfs.get(m.nodes[i].i_surf as usize)?.i_light_map;
            (lm >= 0).then(|| {
                let q = node_poly(m, i);
                let c = q.iter().fold([0.0; 3], |a, x| [a[0] + x[0], a[1] + x[1], a[2] + x[2]]).map(|x| x / q.len() as f64);
                (c, q.iter().map(|x| len(sub(*x, c))).fold(0.0, f64::max), lm)
            })
        })
        .collect();
    for f in faces {
        let Some(j) = f.model_node else { continue };
        let Some(model_surf) = m.surfs.get(m.nodes[j].i_surf as usize).cloned() else { continue };
        if f.points.len() < 3 || f.points.len() > 255 {
            continue;
        }
        let mut surf = model_surf.clone();
        let n = [f.plane[0] as f64, f.plane[1] as f64, f.plane[2] as f64];
        let get = |i: i32| m.vectors.get(i as usize).map(|v| [v[0] as f64, v[1] as f64, v[2] as f64]);
        let (Some(mu), Some(mv)) = (get(model_surf.v_texture_u), get(model_surf.v_texture_v)) else { continue };
        let on_plane = (m.nodes[j].plane[3] - f.plane[3]).abs() < 0.5 && dot(Plane::of(m.nodes[j].plane).n, n) > 0.999;
        if !on_plane {
            // the model's texture axes laid onto the gap's plane (as the editor aligns a texture to a face): each
            // axis loses its part along the normal; an axis left too short is replaced by the plane's own
            let flat = |a: V| sub(a, n.map(|x| x * dot(a, n)));
            let (su, sv) = (len(mu), len(mv));
            let mut u = flat(mu);
            if len(u) < 0.3 * su {
                let h = if n[2].abs() < 0.9 { cross([0.0, 0.0, 1.0], n) } else { [1.0, 0.0, 0.0] };
                u = flat(h);
            }
            u = u.map(|x| x * su / len(u));
            let mut v = flat(mv);
            if len(v) < 0.3 * sv || len(cross(u, v)) < 0.3 * su * sv {
                v = cross(n, u);
                if v[2] > 0.0 {
                    v = v.map(|x| -x);
                }
            }
            v = v.map(|x| x * sv / len(v));
            for (field, a) in [(&mut surf.v_texture_u, u), (&mut surf.v_texture_v, v)] {
                m.vectors.push(a.map(|x| x as f32));
                *field = m.vectors.len() as i32 - 1;
            }
            m.points.push(f.points[0]);
            surf.p_base = m.points.len() as i32 - 1;
            surf.pan_u = 0;
            surf.pan_v = 0;
        }
        m.vectors.push([f.plane[0], f.plane[1], f.plane[2]]);
        surf.v_normal = m.vectors.len() as i32 - 1;
        // its own lightmap, lit by the lights of the model's (every texel lit: no shadows are known for it), its zone's
        // ambient light where none reaches; a surface without a lightmap would be drawn at full brightness
        let centre = f.points.iter().fold([0.0; 3], |a, p| [a[0] + p[0] as f64, a[1] + p[1] as f64, a[2] + p[2] as f64]).map(|x| x / f.points.len() as f64);
        let reach = f.points.iter().map(|p| len(sub([p[0] as f64, p[1] as f64, p[2] as f64], centre))).fold(0.0, f64::max);
        let mut lights: Vec<i32> = Vec::new();
        for &(c, r, lm) in &lit {
            if len(sub(c, centre)) - r - reach > NEAR {
                continue;
            }
            let Some(l) = m.light_map.get(lm as usize).filter(|l| l.i_light_actors >= 0) else { continue };
            for &raw in m.lights[l.i_light_actors as usize..].iter().take_while(|&&r| r != 0) {
                if !lights.contains(&raw) {
                    lights.push(raw);
                }
            }
        }
        surf.i_light_map = new_lightmap(m, &surf, &f.points, model_surf.i_light_map, &lights);
        surf.i_brush_poly = -1;
        m.surfs.push(surf);
        let i_surf = m.surfs.len() as i32 - 1;
        let i_vert_pool = m.verts.len() as i32;
        for p in &f.points {
            m.points.push(*p);
            m.verts.push(Vert { p_vertex: m.points.len() as i32 - 1, i_side: -1 });
        }
        let host = &m.nodes[f.node];
        let node = BspNode {
            plane: f.plane,
            zone_mask: host.zone_mask | (1u64 << (f.zone & 63)),
            node_flags: 0x01,
            i_vert_pool,
            i_surf,
            i_back: -1,
            i_front: -1,
            i_plane: host.i_plane,
            i_collision_bound: -1,
            i_render_bound: -1,
            i_zone: [0, f.zone],
            num_vertices: f.points.len() as u8,
            i_leaf: [-1, f.leaf],
        };
        m.nodes.push(node);
        let new = m.nodes.len() as i32 - 1;
        m.nodes[f.node].i_plane = new;
        filled += 1;
    }
    (filled, first_light_map..m.light_map.len())
}

/// A lightmap for a filled face (surface `surf`, polygon `points`): texels laid over the polygon's texture-space extent
/// at the model lightmap's density (32 units without one), every texel lit by each of `lights` until
/// `shade_filled_faces` makes the shadows.
fn new_lightmap(m: &mut Model, surf: &ue_assets::model::BspSurf, points: &[[f32; 3]], model_lm: i32, lights: &[i32]) -> i32 {
    let get = |i: i32| m.vectors.get(i as usize).map(|v| [v[0] as f64, v[1] as f64, v[2] as f64]);
    let (Some(u), Some(v), Some(base)) = (get(surf.v_texture_u), get(surf.v_texture_v), m.points.get(surf.p_base as usize).map(|p| [p[0] as f64, p[1] as f64, p[2] as f64])) else {
        return -1;
    };
    let tex: Vec<(f64, f64)> = points.iter().map(|p| {
        let r = sub([p[0] as f64, p[1] as f64, p[2] as f64], base);
        (dot(r, u), dot(r, v))
    }).collect();
    let (lo_u, hi_u) = tex.iter().fold((f64::MAX, f64::MIN), |a, t| (a.0.min(t.0), a.1.max(t.0)));
    let (lo_v, hi_v) = tex.iter().fold((f64::MAX, f64::MIN), |a, t| (a.0.min(t.1), a.1.max(t.1)));
    let model = m.light_map.get(model_lm.max(0) as usize).filter(|_| model_lm >= 0).cloned();
    let mut scale = model.as_ref().map(|lm| (lm.u_scale as f64, lm.v_scale as f64)).unwrap_or((32.0, 32.0));
    // at most 256 texels a side
    let texels = |extent: f64, s: f64| (extent / s).ceil() as i64 + 1;
    while texels(hi_u - lo_u, scale.0) > 256 {
        scale.0 *= 2.0;
    }
    while texels(hi_v - lo_v, scale.1) > 256 {
        scale.1 *= 2.0;
    }
    let (w, h) = (texels(hi_u - lo_u, scale.0).max(2), texels(hi_v - lo_v, scale.1).max(2));
    // centred on the polygon
    let pan_u = lo_u - ((w - 1) as f64 * scale.0 - (hi_u - lo_u)) / 2.0;
    let pan_v = lo_v - ((h - 1) as f64 * scale.1 - (hi_v - lo_v)) / 2.0;
    let data_offset = m.light_bits.len() as i32;
    let per_light = (w as usize + 7) / 8 * h as usize;
    m.light_bits.extend(std::iter::repeat_n(0xFF, per_light * lights.len()));
    let i_light_actors = m.lights.len() as i32;
    m.lights.extend(lights.iter().copied());
    m.lights.push(0);
    m.light_map.push(LightMapIndex { data_offset, pan: [pan_u as f32, pan_v as f32, 0.0], u_clamp: w as i32, v_clamp: h as i32, u_scale: scale.0 as f32, v_scale: scale.1 as f32, i_light_actors });
    m.light_map.len() as i32 - 1
}

/// The shadows of the filled faces' lightmaps (`lights` from `fill_missing_faces`): a texel is lit by a light when the
/// line from the light to a point 2 units in front of the texel stays in empty space (`UModel::FastLineCheck`) and
/// lies within the light's radius. `light_at(k)` gives the place and radius of the light at index `k` of
/// `Model::lights`.
pub fn shade_filled_faces(m: &mut Model, light_maps: std::ops::Range<usize>, light_at: &dyn Fn(usize) -> Option<([f32; 3], f32)>) {
    if light_maps.is_empty() {
        return;
    }
    let bsp = crate::Bsp::from_model(m);
    let surf_of: std::collections::HashMap<i32, usize> = m.surfs.iter().enumerate().filter(|(_, s)| s.i_light_map >= light_maps.start as i32).map(|(i, s)| (s.i_light_map, i)).collect();
    for li in light_maps {
        let Some(&si) = surf_of.get(&(li as i32)) else { continue };
        let surf = m.surfs[si].clone();
        let lm = m.light_map[li].clone();
        let get = |i: i32| m.vectors.get(i as usize).map(|v| [v[0] as f64, v[1] as f64, v[2] as f64]);
        let (Some(u), Some(v), Some(n), Some(base)) = (get(surf.v_texture_u), get(surf.v_texture_v), get(surf.v_normal), m.points.get(surf.p_base as usize).map(|p| [p[0] as f64, p[1] as f64, p[2] as f64])) else { continue };
        // texture space to world: P = base + x U + y V with U . (P - base) = tu, V . (P - base) = tv
        let (uu, uv, vv) = (dot(u, u), dot(u, v), dot(v, v));
        let det = uu * vv - uv * uv;
        if det.abs() < 1e-9 {
            continue;
        }
        let (w, h) = (lm.u_clamp.max(0) as usize, lm.v_clamp.max(0) as usize);
        let row_bytes = (w + 7) / 8;
        let per_light = row_bytes * h;
        let mut k = 0usize;
        while let Some(&raw) = m.lights.get(lm.i_light_actors as usize + k) {
            if raw == 0 {
                break;
            }
            let at = light_at(lm.i_light_actors as usize + k);
            let off = lm.data_offset as usize + k * per_light;
            for j in 0..h {
                for i in 0..w {
                    let (tu, tv) = (lm.pan[0] as f64 + i as f64 * lm.u_scale as f64, lm.pan[1] as f64 + j as f64 * lm.v_scale as f64);
                    let (x, y) = ((tu * vv - tv * uv) / det, (tv * uu - tu * uv) / det);
                    let p = [base[0] + x * u[0] + y * v[0] + 2.0 * n[0], base[1] + x * u[1] + y * v[1] + 2.0 * n[1], base[2] + x * u[2] + y * v[2] + 2.0 * n[2]];
                    let pf = p.map(|c| c as f32);
                    let lit = at.is_some_and(|(lp, r)| {
                        let d = [lp[0] - pf[0], lp[1] - pf[1], lp[2] - pf[2]];
                        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < r && bsp.fast_line_check(lp, pf)
                    });
                    let byte = &mut m.light_bits[off + j * row_bytes + (i >> 3)];
                    if lit {
                        *byte |= 1 << (i & 7);
                    } else {
                        *byte &= !(1 << (i & 7));
                    }
                }
            }
            k += 1;
        }
    }
}

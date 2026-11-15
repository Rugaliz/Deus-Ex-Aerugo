//! `URender::DrawLodMesh` (Render.dll 10b0ea80) on the CPU, as the engine does it: the level of detail, the vertex
//! morph toward collapse targets, the faces of that level with their wedges collapsed (texture coordinates morphed),
//! vertex normals from the faces' unit normals, the facing test, `Fatness`, each used vertex lit by the light manager's
//! `Light` (10b027f0), and per triangle (10b0bab0) environment-mapped coordinates and the glow colour of unlit faces.
//!
//! The engine works in view space; this works in world space with the eye position, which gives the same values for
//! every formula built from differences and dot products. Cross products are the exception: the view axes
//! (`ViewCoords`: X = world Y, Y = -world Z, Z = world X) are a mirror image of the world's (determinant -1), so the
//! engine's face normal `(v0 - v1) x (v2 - v0)` in view space is `(v1 - v0) x (v2 - v0)` in world space (outward on
//! the shipped meshes). Outcodes (all-outside rejects) are left to the GPU's clipping: they only skip work.

use ue_assets::mesh::{LodMesh, Mesh};

pub const PF_INVISIBLE: u32 = 0x1;
pub const PF_MASKED: u32 = 0x2;
pub const PF_TRANSLUCENT: u32 = 0x4;
pub const PF_ENVIRONMENT: u32 = 0x10;
pub const PF_MODULATED: u32 = 0x40;
pub const PF_TWO_SIDED: u32 = 0x100;
pub const PF_NO_SMOOTH: u32 = 0x800;
pub const PF_FLAT: u32 = 0x4000;
pub const PF_UNLIT: u32 = 0x40_0000;

/// `URender`'s mesh LOD settings (`URender::Init` 10b13460: +0x40 0.25, +0x44 1.0, +0x48 mode 1; `MLOD`/`TLOD` are
/// console commands only).
const LOD_DIST: f32 = 0.25;
const LOD_SCALE: f32 = 1.0;
const LOD_MODE: i32 = 1;
/// `LodMesh::lod_params` in file order (model verts, special verts, `MeshScaleMax`, `LODHysteresis`, `LODStrength`,
/// `LODMinVerts`, `LODMorph`, `LODZDisplace`; checked on the island's meshes: 1.0, 10, 0.3, 0). `DrawLodMesh` reads
/// them at `+0x188` scale max, `+0x18c` strength, `+0x190` min verts, `+0x194` morph, `+0x198` Z displacement.
const MESH_SCALE_MAX: usize = 2;
const LOD_STRENGTH: usize = 4;
const LOD_MIN_VERTS: usize = 5;
const LOD_MORPH: usize = 6;
const LOD_ZDISPLACE: usize = 7;
/// The double at 10b2a5c4 (2^-15), the engine's "about zero".
const EPS: f64 = 3.0517578125e-05;

/// The camera as `DrawLodMesh` uses it.
#[derive(Debug, Clone, Copy)]
pub struct MeshView {
    pub eye: [f32; 3],
    /// The view's forward axis (depth of the actor for the level of detail).
    pub forward: [f32; 3],
    /// The viewport actor's `FovAngle` (degrees) and the frame's width in pixels (`FX`).
    pub fov_deg: f32,
    pub frame_x: f32,
}

/// The actor as `DrawLodMesh` uses it.
#[derive(Debug, Clone, Copy)]
pub struct MeshActor {
    pub location: [f32; 3],
    pub draw_scale: f32,
    pub lod_bias: f32,
    pub fatness: u8,
    /// The poly flags of the draw (`DrawActorSprite`'s 10b26230: style, `bNoSmooth`, `bMeshEnviroMap`, not
    /// `bMeshCurvy`, `PF_Unlit`).
    pub flags: u32,
    /// The glow colour unlit faces take (`ScaleGlow * 0.5 + AmbientGlow / 256`, clamped).
    pub glow: f32,
}

/// The actor's lights as `SetupForActor` left them: ambient (zone ambient and glow), `ScaleGlow * 1.4`, and the lights
/// that light vertices (position, colour of the moment, `1 / radius`).
#[derive(Debug, Clone, Default)]
pub struct MeshLights {
    pub ambient: [f32; 3],
    pub scale: f32,
    pub lights: Vec<([f32; 3], [f32; 3], f32)>,
    /// The actor's fog lights (`SetupForActor`, farthest first), positions relative to the eye: non-empty means
    /// `PF_RenderFog`, and each vertex gets `FLightManager::Fog` (10b02c00).
    pub fog: Vec<dx_light::manager::FogLight>,
}

/// One corner of a drawn triangle: world position, texture coordinates (in units of the texture: `byte / 256` for a
/// wedge, `(R + 1) / 2` for an environment-mapped vertex) and colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corner {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 3],
    /// The vertex fog colour D3DDrv adds as the specular colour (`DrawGouraudPolygon` 100056b0 with
    /// `SPECULARENABLE` set by `SetBlending` 10008270): only for `PF_RenderFog` faces that are neither translucent
    /// nor modulated; zero otherwise.
    pub fog: [f32; 3],
}

/// A triangle as 10b0bab0 gets it: material index (`DrawLodMesh`), mesh texture slot, poly flags (material or
/// triangle | draw | `PF_Flat`), corners.
#[derive(Debug, Clone, PartialEq)]
pub struct Tri {
    pub material: u16,
    pub slot: u16,
    pub flags: u32,
    pub corners: [Corner; 3],
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn f(bits: u32) -> f32 {
    f32::from_bits(bits)
}

/// The table `x^-0.5` of the light manager (`DrawLodMesh` normalises with it, `+ 0.001` against zero).
fn inv_len(v: [f32; 3]) -> f32 {
    dx_light::manager::fast_inv_sqrt(v[2] * v[2] + v[1] * v[1] + v[0] * v[0] + 0.001)
}

/// The level of detail (`DrawLodMesh` 10b0ea80 head): the number of model vertices drawn and the unrounded count the
/// morph range comes from. `None` when the mesh has no level of detail (all vertices).
pub fn lod_vertex_count(lod: &LodMesh, view: &MeshView, actor: &MeshActor) -> (usize, Option<f32>) {
    let model_verts = if lod.lod_params[0] == 0 { usize::MAX } else { lod.lod_params[0] as usize };
    let strength = f(lod.lod_params[LOD_STRENGTH]);
    if strength == 0.0 || lod.collapse_point_thus.is_empty() {
        return (model_verts, None);
    }
    let mut tan = ((view.fov_deg as f64 * 0.008726646259971648).tan()) as f32;
    if LOD_MODE > 2 {
        tan = 1.0;
    }
    let k_verts = model_verts as f32 * 0.003 + 0.25;
    let k_screen = (view.frame_x as f64 * 0.00109375 + 0.3) as f32;
    let d = sub(actor.location, view.eye);
    let z = (d[2] * view.forward[2] + d[1] * view.forward[1]) + d[0] * view.forward[0];
    let mut zz = z - f(lod.lod_params[LOD_ZDISPLACE]);
    if zz <= 1.0 {
        zz = 1.0;
    }
    let mut ratio = (f(lod.lod_params[MESH_SCALE_MAX]) * actor.draw_scale * actor.lod_bias * k_screen * 430.0) / (LOD_SCALE * LOD_DIST * strength * zz * k_verts * tan);
    ratio *= model_verts as f32;
    let min_verts = lod.lod_params[LOD_MIN_VERTS] as i32 as f32;
    if ratio <= min_verts {
        ratio = min_verts;
    }
    let n = (ratio as f64).round_ties_even() as i64;
    (n.clamp(0, model_verts as i64) as usize, Some(ratio))
}

/// `DrawLodMesh` for one draw: `pose` holds the frame's vertices in world space (special vertices first, as
/// `ULodMesh::GetFrame` gives them). Returns the triangles drawn, in face order.
pub fn build(lod: &LodMesh, pose: &[[f32; 3]], view: &MeshView, actor: &MeshActor, lights: Option<&MeshLights>) -> Vec<Tri> {
    let special = lod.special_verts();
    if pose.len() <= special {
        return Vec::new();
    }
    // a plain `Mesh` (no level-of-detail data) draws every vertex of the frame
    let model_verts = if lod.lod_params[0] == 0 { pose.len() - special } else { lod.lod_params[0] as usize };
    let (mut count, ratio) = lod_vertex_count(lod, view, actor);
    // `GetFrame` gives at most what the frame holds
    count = count.min(pose.len() - special);
    let mut verts: Vec<[f32; 3]> = pose[special..special + count].to_vec();
    let mut alpha = vec![0.0f32; count];
    // the morph: the vertices of the last `LODMorph` part of the range slide toward their collapse targets
    let lod_active = ratio.is_some();
    let lod_morph = f(lod.lod_params[LOD_MORPH]);
    let mut morph = lod_active && LOD_MODE & 1 != 0 && lod_morph as f64 > EPS;
    let mut a8 = 0usize;
    if morph {
        let ratio = ratio.unwrap();
        let start = (((1.0 - lod_morph) * ratio) as f64).round_ties_even() as i64;
        if count as i64 <= start {
            morph = false;
        } else {
            a8 = start.max(0) as usize;
            let step = 1.0 / (ratio * lod_morph);
            let mut i = count as i64 - 1;
            while (a8 as i64) < i {
                let iu = i as usize;
                let a = ((i - a8 as i64) as f32 * step).min(1.0);
                let t = lod.collapse_point_thus.get(iu).copied().unwrap_or(0) as usize;
                if t == 0 {
                    alpha[iu] = 0.0;
                } else if t < count {
                    let (p, q) = (verts[iu], verts[t]);
                    verts[iu] = [(q[0] - p[0]) * a + p[0], (q[1] - p[1]) * a + p[1], (q[2] - p[2]) * a + p[2]];
                    alpha[iu] = a;
                }
                i -= 1;
            }
        }
    }
    let full = !morph && model_verts <= count;
    // faces of this level, wedges collapsed (and their coordinates morphed), normals summed, facing tested
    let mut normals = vec![[0.0f32; 3]; count];
    let mut used = vec![false; count];
    let mut wedge_cache: Vec<Option<(usize, [u8; 2])>> = vec![None; lod.wedges.len()];
    let mut faces: Vec<(usize, [(usize, [u8; 2]); 3])> = Vec::new();
    for (fi, face) in lod.faces.iter().enumerate() {
        if !full && lod.face_level.get(fi).copied().unwrap_or(0) as usize > count {
            continue;
        }
        let mut corners = [(0usize, [0u8; 2]); 3];
        let mut ok = true;
        for k in 0..3 {
            let w0 = face.wedge[k] as usize;
            let c = match wedge_cache.get(w0).copied().flatten() {
                Some(c) => c,
                None => {
                    let Some(mut wed) = lod.wedges.get(w0).copied() else {
                        ok = false;
                        break;
                    };
                    let mut w = w0;
                    if !full {
                        let mut guard = 0;
                        while wed.vertex as usize >= count && guard < 65536 {
                            w = lod.collapse_wedge_thus.get(w).copied().unwrap_or(0) as usize;
                            wed = lod.wedges[w.min(lod.wedges.len() - 1)];
                            guard += 1;
                        }
                        let v = wed.vertex as usize;
                        let cw = lod.collapse_wedge_thus.get(w).copied().unwrap_or(w as u16) as usize;
                        if morph && a8 < v && v < count && alpha[v] != 0.0 && cw != w {
                            let t = lod.wedges[cw.min(lod.wedges.len() - 1)];
                            if t.uv != wed.uv {
                                let lerp = |a: u8, b: u8| (((b as f32 - a as f32) * alpha[v] + a as f32) as f64).round_ties_even() as u8;
                                wed.uv = [lerp(wed.uv[0], t.uv[0]), lerp(wed.uv[1], t.uv[1])];
                            }
                        }
                    }
                    let c = (wed.vertex as usize, wed.uv);
                    if let Some(slot) = wedge_cache.get_mut(w0) {
                        *slot = Some(c);
                    }
                    c
                }
            };
            if c.0 >= count {
                ok = false;
                break;
            }
            corners[k] = c;
        }
        if !ok {
            continue;
        }
        let (v0, v1, v2) = (verts[corners[0].0], verts[corners[1].0], verts[corners[2].0]);
        // the engine's (v0 - v1) x (v2 - v0) in view space: (v1 - v0) x (v2 - v0) in world space (see the module
        // notes), normalised with the table
        let a = sub(v1, v0);
        let b = sub(v2, v0);
        let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let s = inv_len(n);
        let un = [n[0] * s, n[1] * s, n[2] * s];
        for &(v, _) in &corners {
            for k in 0..3 {
                normals[v][k] += un[k];
            }
        }
        let material = face.material as usize;
        let mflags = lod.materials.get(material).map(|m| m[0]).unwrap_or(0) | actor.flags | PF_FLAT;
        let e = sub(v0, view.eye);
        let facing = (un[1] * e[1] + un[2] * e[2]) + un[0] * e[0];
        // the full-detail pass compares with 2^-15, the reduced one with 0
        let limit = if full { EPS as f32 } else { 0.0 };
        if mflags & PF_TWO_SIDED != 0 || facing < limit {
            for &(v, _) in &corners {
                used[v] = true;
            }
            faces.push((fi, corners));
        }
    }
    let (unit, lit, fog) = shade_vertices(&mut verts, &normals, &used, actor, lights, view, 0.001);
    faces
        .into_iter()
        .map(|(fi, c)| {
            let face = &lod.faces[fi];
            let [mflags, slot] = lod.materials.get(face.material as usize).copied().unwrap_or([0, 0]);
            let flags = mflags | actor.flags | PF_FLAT;
            let corners = std::array::from_fn(|k| corner(c[k].0, c[k].1, flags, &verts, &unit, &lit, &fog, actor, view));
            Tri { material: face.material, slot: slot as u16, flags, corners }
        })
        .collect()
}

/// The used vertices as both mesh routines finish them: unit normal (the summed face normals through the table,
/// `eps` added to the squared length: 0.001 in `DrawLodMesh`, none in `DrawMesh`), plane offset `W = N . P`, then
/// `Fatness` (`P += N * (Fatness / 16 - 8)` when not 128) and `Light`. Returns the unit normals and colours.
fn shade_vertices(verts: &mut [[f32; 3]], normals: &[[f32; 3]], used: &[bool], actor: &MeshActor, lights: Option<&MeshLights>, view: &MeshView, eps: f32) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let fat = if actor.fatness != 128 { Some(actor.fatness as f32 * 0.0625 - 8.0) } else { None };
    let mut lit = vec![[0.0f32; 3]; verts.len()];
    let mut fog = vec![[0.0f32; 3]; verts.len()];
    // the light manager's "misses the volume" shortcut flag, for this actor's vertices
    let mut skip = false;
    let mut unit = vec![[0.0f32; 3]; verts.len()];
    for v in 0..verts.len() {
        if !used[v] {
            continue;
        }
        let m = normals[v];
        let s = dx_light::manager::fast_inv_sqrt(m[2] * m[2] + m[1] * m[1] + m[0] * m[0] + eps);
        let n = [m[0] * s, m[1] * s, m[2] * s];
        unit[v] = n;
        let p0 = verts[v];
        let w = (p0[2] * n[2] + p0[1] * n[1]) + p0[0] * n[0];
        if let Some(k) = fat {
            verts[v] = [k * n[0] + p0[0], k * n[1] + p0[1], k * n[2] + p0[2]];
        }
        lit[v] = match lights {
            Some(l) => vertex_light(l, verts[v], n, w, view.eye),
            None => [1.0; 3],
        };
        if let Some(l) = lights.filter(|l| !l.fog.is_empty()) {
            let p = verts[v];
            let f = dx_light::manager::fog_at([p[0] - view.eye[0], p[1] - view.eye[1], p[2] - view.eye[2]], &l.fog, &mut skip);
            fog[v] = [f[0], f[1], f[2]];
        }
    }
    (unit, lit, fog)
}

/// One corner as 10b0bab0 gets it: texture coordinates `byte / 256` (environment-mapped for `PF_Environment`), the
/// vertex colour (the glow colour for `PF_Unlit`).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn corner(v: usize, uv: [u8; 2], flags: u32, verts: &[[f32; 3]], unit: &[[f32; 3]], lit: &[[f32; 3]], fog: &[[f32; 3]], actor: &MeshActor, view: &MeshView) -> Corner {
    // translucent (0x4) or modulated (0x40) faces get no fog pass
    let fogged = if flags & 0x44 == 0 { fog[v] } else { [0.0; 3] };
    let mut c = Corner { pos: verts[v], uv: [uv[0] as f32 / 256.0, uv[1] as f32 / 256.0], color: lit[v], fog: fogged };
    if flags & PF_ENVIRONMENT != 0 {
        c.uv = env_uv(verts[v], unit[v], view.eye);
    }
    if flags & PF_UNLIT != 0 {
        c.color = [actor.glow; 3];
    }
    c
}

/// `URender::DrawMesh` (Render 10b0d1a0) for a plain `Mesh` (`pose`: the frame's vertices in world space): every
/// triangle's unit normal (the table, `+ 0.001` in double precision), kept when not all outside one side and
/// two-sided, invisible or facing the eye (`N . v0` below 0); vertex normals summed over the vertex's `VertLinks` (every
/// triangle, hidden ones too); the used vertices shaded as in `DrawLodMesh`. Triangles flagged `PF_Invisible` (the
/// weapon triangle) are not drawn. No level of detail, no sorting on a non-span device.
pub fn build_plain(mesh: &Mesh, pose: &[[f32; 3]], view: &MeshView, actor: &MeshActor, lights: Option<&MeshLights>) -> Vec<Tri> {
    let n = pose.len();
    let mut verts = pose.to_vec();
    let mut face_normals = vec![[0.0f32; 3]; mesh.tris.len()];
    let mut used = vec![false; n];
    let mut kept: Vec<usize> = Vec::new();
    for (ti, t) in mesh.tris.iter().enumerate() {
        let [i0, i1, i2] = t.vertex.map(|i| i as usize);
        if i0 >= n || i1 >= n || i2 >= n {
            continue;
        }
        let (v0, v1, v2) = (verts[i0], verts[i1], verts[i2]);
        // the engine's (v0 - v1) x (v2 - v0) in view space is (v1 - v0) x (v2 - v0) in world space (module notes)
        let a = sub(v1, v0);
        let b = sub(v2, v0);
        let c = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let len2 = ((c[2] * c[2] + c[1] * c[1] + c[0] * c[0]) as f64 + 0.001) as f32;
        let s = dx_light::manager::fast_inv_sqrt(len2);
        let un = [s * c[0], s * c[1], s * c[2]];
        face_normals[ti] = un;
        let flags = t.poly_flags | actor.flags | PF_FLAT;
        let e = sub(v0, view.eye);
        let facing = un[0] * e[0] + un[1] * e[1] + un[2] * e[2];
        if flags & (PF_FLAT | PF_TWO_SIDED | PF_INVISIBLE) != PF_FLAT || facing < 0.0 {
            kept.push(ti);
            for i in [i0, i1, i2] {
                used[i] = true;
            }
        }
    }
    // vertex normals: the sum of the unit normals of the triangles linked to the vertex
    let mut normals = vec![[0.0f32; 3]; n];
    for (v, nv) in normals.iter_mut().enumerate() {
        if !used[v] {
            continue;
        }
        let Some(&[count, offset]) = mesh.connects.get(v) else { continue };
        for k in 0..count.max(0) {
            let Some(&ti) = mesh.vert_links.get((offset + k) as usize) else { continue };
            if let Some(f) = face_normals.get(ti as usize) {
                for j in 0..3 {
                    nv[j] += f[j];
                }
            }
        }
    }
    let (unit, lit, fog) = shade_vertices(&mut verts, &normals, &used, actor, lights, view, 0.0);
    kept.into_iter()
        .filter(|&ti| mesh.tris[ti].poly_flags & PF_INVISIBLE == 0)
        .map(|ti| {
            let t = &mesh.tris[ti];
            let flags = t.poly_flags | actor.flags | PF_FLAT;
            let corners = std::array::from_fn(|k| corner(t.vertex[k] as usize, t.uv[k], flags, &verts, &unit, &lit, &fog, actor, view));
            Tri { material: 0, slot: t.texture_index as u16, flags, corners }
        })
        .collect()
}

/// An environment-mapped vertex's coordinates (10b0bab0): the view direction reflected by the normal, in world axes,
/// `(R.x + 1) * 128` and `(R.y + 1) * 128` texels of 256.
fn env_uv(p: [f32; 3], n: [f32; 3], eye: [f32; 3]) -> [f32; 2] {
    let d = sub(p, eye);
    let l = ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) as f64).sqrt() as f32;
    let u = if l > 0.0 { [d[0] / l, d[1] / l, d[2] / l] } else { d };
    let dot = u[0] * n[0] + u[1] * n[1] + u[2] * n[2];
    let r = [u[0] - n[0] * (dot + dot), u[1] - n[1] * (dot + dot)];
    [(r[0] + 1.0) * 128.0 / 256.0, (r[1] + 1.0) * 128.0 / 256.0]
}

/// `FLightManager::Light` (10b027f0) for a lit vertex: for each light, `((d . N) / |d| + 1)^2 - 1.5` (none below 0) plus,
/// when the light's reflection in the vertex's tangent plane is seen in front, `6 s^2 / (|d|^2 |P - eye|^2)`, times
/// `1 - |d| / radius` (none when not positive), times its colour; the sum times `ScaleGlow * 1.4` plus the ambient,
/// each channel at most 1. Distances use the light manager's table square root.
pub fn vertex_light(l: &MeshLights, p: [f32; 3], n: [f32; 3], w: f32, eye: [f32; 3]) -> [f32; 3] {
    let e = sub(p, eye);
    let e2 = e[2] * e[2] + e[1] * e[1] + e[0] * e[0];
    let mut acc = [0.0f32; 3];
    for &(lp, color, inv_r) in &l.lights {
        let dz = lp[2] - p[2];
        let dy = lp[1] - p[1];
        let dx = lp[0] - p[0];
        let d2 = dz * dz + dy * dy + dx * dx;
        let dist = dx_light::manager::fast_sqrt(d2);
        let c = (dx * n[0] + dz * n[2] + dy * n[1]) / dist + 1.0;
        let mut v = c * c - 1.5;
        if v < 0.0 {
            v = 0.0;
        }
        // the light mirrored in the plane (N, w)
        let pd = (n[0] * lp[0] + n[1] * lp[1] + n[2] * lp[2]) - w;
        let r = [lp[0] - n[0] * (pd + pd), lp[1] - n[1] * (pd + pd), lp[2] - n[2] * (pd + pd)];
        let rv = sub(r, eye);
        let s = (rv[1] * e[1] + rv[2] * e[2] + rv[0] * e[0]) - e2;
        if s > 0.0 {
            v += (s * s * 6.0) / (d2 * e2);
        }
        let k = v * (1.0 - dist * inv_r);
        if k <= 0.0 {
            continue;
        }
        acc[0] += k * color[0];
        acc[1] += k * color[1];
        acc[2] += k * color[2];
    }
    std::array::from_fn(|i| {
        let x = acc[i] * l.scale + l.ambient[i];
        if x > 1.0 {
            1.0
        } else {
            x
        }
    })
}

/// The glow colour of unlit faces (`DrawLodMesh`: `ScaleGlow * 0.5 + AmbientGlow / 256`, below 2^-15 taken as 2^-15,
/// 1 or more as 1).
pub fn glow_colour(scale_glow: f32, ambient_glow: u8) -> f32 {
    let g = scale_glow * 0.5 + ambient_glow as f32 * 0.00390625;
    if (g as f64) < EPS {
        EPS as f32
    } else if g >= 1.0 {
        1.0
    } else {
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ue_assets::mesh::{Face, Mesh, MeshTri, Wedge};

    fn view(eye: [f32; 3]) -> MeshView {
        MeshView { eye, forward: [1.0, 0.0, 0.0], fov_deg: 90.0, frame_x: 800.0 }
    }

    fn actor(flags: u32) -> MeshActor {
        MeshActor { location: [0.0; 3], draw_scale: 1.0, lod_bias: 1.0, fatness: 128, flags, glow: 0.5 }
    }

    /// Two triangles in the plane x = 0: the first faces +x (toward an eye at x = 10), the second -x.
    fn two_faces(material_flags: u32) -> (LodMesh, Vec<[f32; 3]>) {
        let pose = vec![[0.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0], [0.0, -10.0, 0.0]];
        let wedges = (0..4).map(|v| Wedge { vertex: v, uv: [v as u8 * 10, 0] }).collect();
        let lod = LodMesh {
            mesh: Mesh::default(),
            collapse_point_thus: Vec::new(),
            face_level: Vec::new(),
            // face 0 faces +x: its world normal (v1 - v0) x (v2 - v0) is (100, 0, 0)
            faces: vec![Face { wedge: [0, 1, 2], material: 0 }, Face { wedge: [0, 2, 1], material: 0 }],
            collapse_wedge_thus: Vec::new(),
            wedges,
            materials: vec![[material_flags, 0]],
            special_faces: Vec::new(),
            lod_params: [0; 8],
            remap_anim_verts: Vec::new(),
            old_frame_verts: 0,
        };
        (lod, pose)
    }

    #[test]
    fn one_sided_faces_are_kept_by_the_facing_test_two_sided_ones_always() {
        let (lod, pose) = two_faces(0);
        let tris = build(&lod, &pose, &view([10.0, 0.0, 0.0]), &actor(0), None);
        assert_eq!(tris.len(), 1);
        // (v1 - v0) x (v2 - v0) of face 0: (0,10,0) x (0,0,10) = (100, 0, 0); v0 - eye = (-10, 0, 0): dot -1000 below
        // 2^-15, so face 0 is drawn and face 1 (normal -x, dot 1000) is the hidden one
        assert_eq!(tris[0].corners[1].pos, [0.0, 10.0, 0.0]);
        assert_eq!(tris[0].flags, PF_FLAT);
        let (lod, pose) = two_faces(PF_TWO_SIDED);
        assert_eq!(build(&lod, &pose, &view([10.0, 0.0, 0.0]), &actor(0), None).len(), 2);
    }

    #[test]
    fn unlit_faces_take_the_glow_and_environment_faces_reflected_coordinates() {
        let (lod, pose) = two_faces(PF_TWO_SIDED);
        let tris = build(&lod, &pose, &view([10.0, 0.0, 0.0]), &actor(PF_UNLIT), Some(&MeshLights { ambient: [0.2; 3], scale: 1.4, lights: Vec::new(), fog: Vec::new() }));
        assert!(tris.iter().all(|t| t.corners.iter().all(|c| c.color == [0.5; 3])));
        // only the face toward +x (both faces would cancel each other's normal at the shared vertices: the engine
        // sums the normals of every face of the level, hidden ones too)
        let (mut lod, pose) = two_faces(0);
        lod.faces.remove(1);
        let tris = build(&lod, &pose, &view([10.0, 0.0, 0.0]), &actor(PF_ENVIRONMENT), None);
        // the vertex at the origin, seen straight along -x with a normal along x: the view is reflected straight back
        // (R = (1, 0, 0)), so u = (1 + 1) / 2 = 1, v = (0 + 1) / 2
        let c = tris[0].corners[0];
        assert_eq!(c.pos, [0.0; 3]);
        // (to the table's precision: the normal is normalised twice with the 10-bit table)
        assert!((c.uv[0] - 1.0).abs() < 2e-3 && (c.uv[1] - 0.5).abs() < 1e-5, "{:?}", c.uv);
    }

    /// `Light` by hand: a light 4 above a vertex with normal +z (table square root exact on 16), red, radius 100; eye
    /// beside: diffuse (4/4 + 1)^2 - 1.5 = 2.5, the reflected light below the plane gives no specular, fade 1 - 4/100;
    /// 2.5 * 0.96 * 0.1 + 0.05 = 0.29 red.
    #[test]
    fn vertex_light_follows_the_light_manager() {
        let l = MeshLights { ambient: [0.05; 3], scale: 0.1, lights: vec![([0.0, 0.0, 4.0], [1.0, 0.0, 0.0], 0.01)], fog: Vec::new() };
        let c = vertex_light(&l, [0.0; 3], [0.0, 0.0, 1.0], 0.0, [0.0, 10.0, 0.0]);
        assert!((c[0] - 0.29).abs() < 1e-6, "{c:?}");
        assert!((c[1] - 0.05).abs() < 1e-7 && (c[2] - 0.05).abs() < 1e-7);
        // never above 1
        let l = MeshLights { ambient: [0.9; 3], scale: 1.0, lights: l.lights.clone(), fog: Vec::new() };
        assert_eq!(vertex_light(&l, [0.0; 3], [0.0, 0.0, 1.0], 0.0, [0.0, 10.0, 0.0])[0], 1.0);
    }

    /// The level of detail by hand: 100 model vertices, scale max 1, strength 1, at least 10, an 800-pixel frame at
    /// 90 degrees: `k_screen = 800 * 0.00109375 + 0.3 = 1.175`, `k_verts = 100 * 0.003 + 0.25 = 0.55`, so the count is
    /// `1.175 * 430 / (0.25 * z * 0.55) * 100`: 367 (all 100) at z = 1000, 36.7 -> 37 at 10000, 10 (the minimum) far.
    #[test]
    fn the_level_of_detail_follows_distance() {
        let (mut lod, _) = two_faces(0);
        lod.collapse_point_thus = vec![0; 100];
        lod.lod_params = [100, 0, 1.0f32.to_bits(), 0, 1.0f32.to_bits(), 10, 0.3f32.to_bits(), 0];
        let at = |lod: &LodMesh, x: f32| lod_vertex_count(lod, &view([0.0; 3]), &MeshActor { location: [x, 0.0, 0.0], ..actor(0) });
        assert_eq!(at(&lod, 1000.0).0, 100);
        let (n, r) = at(&lod, 10000.0);
        assert_eq!(n, 37);
        assert!((r.unwrap() - 36.745).abs() < 1e-2, "{r:?}");
        assert_eq!(at(&lod, 100000.0).0, 10);
        lod.lod_params[LOD_STRENGTH] = 0;
        assert_eq!(at(&lod, 100000.0), (100, None));
    }

    #[test]
    fn glow_is_half_scale_glow_plus_ambient_glow_clamped() {
        assert_eq!(glow_colour(1.0, 0), 0.5);
        assert_eq!(glow_colour(1.0, 64), 0.75);
        assert_eq!(glow_colour(3.0, 0), 1.0);
        assert_eq!(glow_colour(0.0, 0) as f64, EPS);
    }

    /// `DrawMesh` by hand: four triangles around the origin in the plane x = 0, the eye at x = 10. T0 (0,1,2) and T1
    /// (0,2,3) have world normals (v1 - v0) x (v2 - v0) = (100, 0, 0): drawn; T2 (0,2,1) turns away: hidden; T3 is the
    /// invisible weapon triangle: kept, not drawn. Vertex 0's links (all four) sum to +x, so with `PF_Environment` its
    /// reflected view is (1, 0, 0): u = 1, v = 0.5. Triangles carry their texture slot.
    #[test]
    fn plain_meshes_follow_draw_mesh() {
        let tri = |v: [u16; 3], flags: u32, slot: u32| MeshTri { vertex: v, uv: [[0, 0], [10, 0], [0, 10]], poly_flags: flags, texture_index: slot };
        let mesh = Mesh {
            tris: vec![tri([0, 1, 2], 0, 0), tri([0, 2, 3], 0, 1), tri([0, 2, 1], 0, 2), tri([0, 1, 2], PF_INVISIBLE, 3)],
            connects: vec![[4, 0], [3, 4], [4, 7], [1, 11]],
            vert_links: vec![0, 1, 2, 3, 0, 2, 3, 0, 1, 2, 3, 1],
            ..Mesh::default()
        };
        let pose = vec![[0.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0], [0.0, -10.0, 0.0]];
        let tris = build_plain(&mesh, &pose, &view([10.0, 0.0, 0.0]), &actor(PF_ENVIRONMENT), None);
        assert_eq!(tris.iter().map(|t| t.slot).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(tris[0].flags, PF_ENVIRONMENT | PF_FLAT);
        let c = tris[0].corners[0];
        assert!((c.uv[0] - 1.0).abs() < 2e-3 && (c.uv[1] - 0.5).abs() < 1e-5, "{:?}", c.uv);
        // texture coordinates are byte / 256 without the environment flag, and two-sided triangles are always kept
        let mut two = mesh.clone();
        two.tris[2].poly_flags = PF_TWO_SIDED;
        let tris = build_plain(&two, &pose, &view([10.0, 0.0, 0.0]), &actor(PF_UNLIT), None);
        assert_eq!(tris.iter().map(|t| t.slot).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(tris[0].corners[1].uv, [10.0 / 256.0, 0.0]);
        assert!(tris.iter().all(|t| t.corners.iter().all(|c| c.color == [0.5; 3])));
    }
}

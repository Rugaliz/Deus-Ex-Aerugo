//! Drawing a mesh and pictures in software: a free camera, textures as the game maps them (8-bit UVs, nearest texel,
//! masked texels left out), faces lit by their angle to the camera, and the open edges of the model on request.

use crate::catalog::{LoadedMesh, Picture};
use std::rc::Rc;

pub const PF_MASKED: u32 = 0x2;
pub const PF_TRANSLUCENT: u32 = 0x4;
pub const PF_MODULATED: u32 = 0x40;
pub const PF_TWO_SIDED: u32 = 0x100;
pub const PF_INVISIBLE: u32 = 0x1;

pub type V3 = [f32; 3];

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

/// A camera in the game's axes (x forward, y right, z up); `fov` is the horizontal angle in degrees.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub pos: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
}

impl Camera {
    pub fn basis(&self) -> (V3, V3, V3) {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (sp, cp) = self.pitch.to_radians().sin_cos();
        let fwd = [cp * cy, cp * sy, sp];
        let right = [-sy, cy, 0.0];
        let up = [-sp * cy, -sp * sy, cp];
        (fwd, right, up)
    }

    /// A camera looking at `centre` from `dist` away, from the direction `yaw`/`pitch` points to.
    pub fn orbit(centre: V3, dist: f32, yaw: f32, pitch: f32, fov: f32) -> Camera {
        let mut c = Camera { pos: [0.0; 3], yaw, pitch, fov };
        let (f, _, _) = c.basis();
        c.pos = [centre[0] - f[0] * dist, centre[1] - f[1] * dist, centre[2] - f[2] * dist];
        c
    }
}

/// A colour and depth target.
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
    pub depth: Vec<f32>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, rgba: vec![0; w * h * 4], depth: vec![f32::INFINITY; w * h] }
    }

    pub fn clear(&mut self, c: [u8; 3]) {
        for p in self.rgba.chunks_exact_mut(4) {
            p.copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
        self.depth.fill(f32::INFINITY);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: [u8; 4]) {
        for yy in y.max(0)..(y + h).min(self.h as i32) {
            for xx in x.max(0)..(x + w).min(self.w as i32) {
                self.blend(xx as usize, yy as usize, c);
            }
        }
    }

    pub fn blend(&mut self, x: usize, y: usize, c: [u8; 4]) {
        let i = (y * self.w + x) * 4;
        let a = c[3] as u32;
        for k in 0..3 {
            self.rgba[i + k] = ((self.rgba[i + k] as u32 * (255 - a) + c[k] as u32 * a) / 255) as u8;
        }
    }

    /// A picture scaled by `zoom` with its top-left corner at (x, y), over a checker board where it is transparent.
    pub fn picture(&mut self, p: &Picture, x: i32, y: i32, zoom: f32) {
        let (dw, dh) = ((p.w as f32 * zoom) as i32, (p.h as f32 * zoom) as i32);
        for yy in y.max(0)..(y + dh).min(self.h as i32) {
            for xx in x.max(0)..(x + dw).min(self.w as i32) {
                let (u, v) = ((((xx - x) as f32) / zoom) as u32, (((yy - y) as f32) / zoom) as u32);
                let t = &p.rgba[((v.min(p.h - 1) * p.w + u.min(p.w - 1)) * 4) as usize..][..4];
                let checker = if ((xx - x) / 8 + (yy - y) / 8) % 2 == 0 { 90 } else { 60 };
                let i = (yy as usize * self.w + xx as usize) * 4;
                self.rgba[i..i + 4].copy_from_slice(&[checker, checker, checker, 255]);
                self.blend(xx as usize, yy as usize, [t[0], t[1], t[2], if p.masked || t[3] < 255 { t[3] } else { 255 }]);
            }
        }
    }

    pub fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [u8; 4]) {
        let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize).clamp(1, 20000);
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            if x >= 0.0 && y >= 0.0 && (x as usize) < self.w && (y as usize) < self.h {
                self.blend(x as usize, y as usize, c);
            }
        }
    }
}

/// How to draw a mesh.
#[derive(Debug, Clone, Copy, Default)]
pub struct DrawOptions {
    /// Faces seen from behind drawn in a warning colour (the game does not draw them, except two-sided ones).
    pub back_faces: bool,
    /// Edges used by only one face drawn in red: where the model is open.
    pub open_edges: bool,
    pub wireframe: bool,
}

/// The vertices of a frame blend as the game places them (`ULodMesh::GetFrame` through `dx_vk::actors::posed_vertices`),
/// special vertices first: at `location`, turned by `rotation` (rotator units), scaled by `scale` (`DrawScale`).
pub fn pose_at(m: &LoadedMesh, frame_a: usize, frame_b: usize, t: f32, location: V3, rotation: [i32; 3], scale: f32) -> Vec<V3> {
    let d = dx_soft::ActorDraw { mesh: m.path.clone(), frame_a, frame_b, blend: t, location, rotation, scale, ..Default::default() };
    dx_vk::actors::posed_vertices(&m.lod, &d)
}

/// The pose at the origin, unturned, at scale 1.
pub fn pose(m: &LoadedMesh, frame_a: usize, frame_b: usize, t: f32) -> Vec<V3> {
    pose_at(m, frame_a, frame_b, t, [0.0; 3], [0; 3], 1.0)
}

/// The box around a pose.
pub fn bounds(p: &[V3]) -> (V3, V3) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in p {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    (lo, hi)
}

const NEAR: f32 = 0.05;

/// Draws the faces of `m` in pose `pos` (as `pose` gives it) with the textures per slot.
pub fn draw_mesh(c: &mut Canvas, cam: &Camera, m: &LoadedMesh, pos: &[V3], textures: &[Option<Rc<Picture>>], opt: DrawOptions) -> usize {
    let (fwd, right, up) = cam.basis();
    let focal = c.w as f32 * 0.5 / (cam.fov.to_radians() * 0.5).tan();
    let (cx, cy) = (c.w as f32 * 0.5, c.h as f32 * 0.5);
    let view: Vec<V3> = pos.iter().map(|&p| {
        let d = sub(p, cam.pos);
        [dot(d, right), dot(d, up), dot(d, fwd)]
    }).collect();
    let project = |v: V3| (cx + v[0] / v[2] * focal, cy - v[1] / v[2] * focal);
    let lod = &m.lod;
    let fallback = Picture { w: 2, h: 2, rgba: vec![150, 150, 160, 255, 110, 110, 120, 255, 110, 110, 120, 255, 150, 150, 160, 255], masked: false };
    let mut drawn = 0;
    let mut edges: std::collections::HashMap<(usize, usize), u32> = std::collections::HashMap::new();
    for face in &lod.faces {
        let [flags, slot] = lod.materials.get(face.material as usize).copied().unwrap_or([0, 0]);
        let idx: [usize; 3] = face.wedge.map(|w| lod.wedge_vertex(w as usize));
        if opt.open_edges {
            for k in 0..3 {
                let (a, b) = (idx[k], idx[(k + 1) % 3]);
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        if flags & PF_INVISIBLE != 0 {
            continue;
        }
        let (p0, p1, p2) = (pos[idx[0]], pos[idx[1]], pos[idx[2]]);
        // the game's front face: (v1 - v0) x (v2 - v0) points to the eye
        let n = cross(sub(p1, p0), sub(p2, p0));
        let facing = dot(n, sub(p0, cam.pos));
        let back = facing >= 0.0;
        if back && flags & PF_TWO_SIDED == 0 && !opt.back_faces {
            continue;
        }
        let tex = textures.get(slot as usize).cloned().flatten();
        let tex: &Picture = tex.as_deref().unwrap_or(&fallback);
        let shade = 0.35 + 0.65 * dot(norm(n), fwd).abs();
        let tint = if back && flags & PF_TWO_SIDED == 0 { Some([255u8, 0, 255]) } else { None };
        // the polygon in view space with its texture coordinates, cut at the near plane
        let mut poly: Vec<(V3, [f32; 2])> = (0..3)
            .map(|k| {
                let w = lod.wedges[face.wedge[k] as usize];
                (view[idx[k]], [w.uv[0] as f32 / 256.0 * tex.w as f32, w.uv[1] as f32 / 256.0 * tex.h as f32])
            })
            .collect();
        if poly.iter().any(|p| p.0[2] < NEAR) {
            poly = clip_near(&poly);
            if poly.len() < 3 {
                continue;
            }
        }
        let corners: Vec<[f32; 5]> = poly
            .iter()
            .map(|(v, uv)| {
                let (x, y) = project(*v);
                let iw = 1.0 / v[2];
                [x, y, iw, uv[0] * iw, uv[1] * iw]
            })
            .collect();
        let (cw, ch) = (c.w, c.h);
        let (rgba, depth) = (&mut c.rgba, &mut c.depth);
        for k in 1..corners.len() - 1 {
            dx_soft::raster_triangle(&corners[0], &corners[k], &corners[k + 1], cw, ch, &mut |x, y, iw, u, v| {
                let z = 1.0 / iw;
                let i = y * cw + x;
                if z >= depth[i] {
                    return;
                }
                let (tx, ty) = ((u.floor() as i64).rem_euclid(tex.w as i64) as usize, (v.floor() as i64).rem_euclid(tex.h as i64) as usize);
                let t = &tex.rgba[(ty * tex.w as usize + tx) * 4..][..4];
                if (flags & PF_MASKED != 0 || tex.masked) && t[3] < 128 {
                    return;
                }
                let mut col = [t[0] as f32 * shade, t[1] as f32 * shade, t[2] as f32 * shade];
                if let Some(tc) = tint {
                    col = [(col[0] + tc[0] as f32) * 0.5, (col[1] + tc[1] as f32) * 0.5, (col[2] + tc[2] as f32) * 0.5];
                }
                let o = i * 4;
                if flags & (PF_TRANSLUCENT | PF_MODULATED) != 0 {
                    for k in 0..3 {
                        rgba[o + k] = ((rgba[o + k] as f32 + col[k]).min(255.0)) as u8;
                    }
                    return;
                }
                depth[i] = z;
                for k in 0..3 {
                    rgba[o + k] = col[k].min(255.0) as u8;
                }
            });
        }
        drawn += 1;
    }
    let edge_line = |c: &mut Canvas, a: usize, b: usize, col: [u8; 4]| {
        let (mut va, mut vb) = (view[a], view[b]);
        if va[2] < NEAR && vb[2] < NEAR {
            return;
        }
        if va[2] < NEAR || vb[2] < NEAR {
            let t = (NEAR - va[2]) / (vb[2] - va[2]);
            let q: V3 = std::array::from_fn(|k| va[k] + (vb[k] - va[k]) * t);
            if va[2] < NEAR { va = q } else { vb = q }
        }
        c.line(project(va), project(vb), col);
    };
    if opt.wireframe {
        for face in &lod.faces {
            let idx: [usize; 3] = face.wedge.map(|w| lod.wedge_vertex(w as usize));
            for k in 0..3 {
                edge_line(c, idx[k], idx[(k + 1) % 3], [255, 255, 255, 70]);
            }
        }
    }
    if opt.open_edges {
        for (&(a, b), &n) in &edges {
            if n == 1 {
                edge_line(c, a, b, [255, 40, 40, 255]);
            }
        }
    }
    drawn
}

fn clip_near(poly: &[(V3, [f32; 2])]) -> Vec<(V3, [f32; 2])> {
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (ina, inb) = (a.0[2] >= NEAR, b.0[2] >= NEAR);
        if ina {
            out.push(a);
        }
        if ina != inb {
            let t = (NEAR - a.0[2]) / (b.0[2] - a.0[2]);
            let p: V3 = std::array::from_fn(|k| a.0[k] + (b.0[k] - a.0[k]) * t);
            let uv = [a.1[0] + (b.1[0] - a.1[0]) * t, a.1[1] + (b.1[1] - a.1[1]) * t];
            out.push((p, uv));
        }
    }
    out
}

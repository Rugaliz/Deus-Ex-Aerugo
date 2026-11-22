//! Drawing actor meshes into a world frame (colour + depth). Vertex-animated `LodMesh` models, shaded by facing only
//! (no light data yet); translucent materials are blended 50/50, modulated ones are skipped.

use crate::mesh::MeshData;
use crate::{camera_basis, raster_triangle, ActorDraw, Camera, Frame, MapView, Tex};
use std::rc::Rc;
use ue_assets::mesh::LodMesh;

const PF_MASKED: u32 = 0x2;
const PF_TRANSLUCENT: u32 = 0x4;
const PF_MODULATED: u32 = 0x40;

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `v >> rot`: rotates a mesh-space vector into the world (x forward, yaw about Z, pitch about Y, roll about X).
fn rotate(v: [f32; 3], r: [i32; 3]) -> [f32; 3] {
    let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
    let (sp, cp) = ang(r[0]).sin_cos();
    let (sy, cy) = ang(r[1]).sin_cos();
    let (sr, cr) = ang(r[2]).sin_cos();
    let x = [cp * cy, cp * sy, sp];
    let y = [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp];
    let z = [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp];
    std::array::from_fn(|i| v[0] * x[i] + v[1] * y[i] + v[2] * z[i])
}

impl MapView {
    pub fn mesh_data(&mut self, path: &str) -> Option<Rc<MeshData>> {
        if let Some(m) = self.meshes.get(path) {
            return m.clone();
        }
        let data = (|| {
            let (pkg_name, rest) = path.split_once('.')?;
            let pkg = self.files.package(pkg_name)?;
            let export = pkg.find(rest)?;
            let plain = pkg.pkg.export_class_name(export) == "Mesh";
            let lod = match pkg.pkg.export_class_name(export).as_str() {
                "LodMesh" => LodMesh::load(&pkg.pkg, export).ok()?,
                // meshes without level-of-detail data
                "Mesh" => LodMesh::from_plain(ue_assets::mesh::Mesh::load(&pkg.pkg, export).ok()?),
                _ => return None,
            };
            // a mod's replacement geometry (`Meshes/<Package>/<Name>.aemesh`)
            let lod = ue_assets::mesh_mod::with_mod(path, lod);
            let own_texture_paths: Vec<Option<String>> = lod
                .mesh
                .textures
                .iter()
                .map(|&slot| {
                    let (tp, te) = self.files.resolve(&pkg, slot)?;
                    // an export's path has no package part
                    tp.pkg.object_path(ue_package::ObjectRef::Export(te)).map(|p| format!("{}.{}", tp.name, p))
                })
                .collect();
            let own_textures = own_texture_paths.iter().map(|p| p.as_ref().and_then(|p| self.texture_by_path(p))).collect();
            Some(Rc::new(MeshData { lod: std::sync::Arc::new(lod), plain, own_textures, own_texture_paths }))
        })();
        self.meshes.insert(path.to_string(), data.clone());
        data
    }

    /// Draws one actor's mesh into `frame`. Returns false if the mesh could not be loaded.
    pub fn draw_actor(&mut self, frame: &mut Frame, cam: &Camera, d: &ActorDraw) -> bool {
        let Some(mesh) = self.mesh_data(&d.mesh) else { return false };
        let lod = &mesh.lod;
        let frames = lod.mesh.anim_frames.max(1) as usize;
        let (fa, fb) = (d.frame_a.min(frames - 1), d.frame_b.min(frames - 1));
        let pa = lod.frame_positions(fa);
        let pb = if fb != fa && d.blend > 0.0 { Some(lod.frame_positions(fb)) } else { None };
        // blend channels: weight * (frame - frame 0), added to the pose
        let blend_delta: Option<Vec<[f32; 3]>> = (!d.blends.is_empty()).then(|| {
            let base = lod.frame_positions(0);
            let mut acc = vec![[0.0f32; 3]; base.len()];
            for &(f, w) in &d.blends {
                let p = lod.frame_positions(f.min(frames - 1));
                for (i, a) in acc.iter_mut().enumerate() {
                    for k in 0..3 {
                        a[k] += w * (p[i][k] - base[i][k]);
                    }
                }
            }
            acc
        });
        let (fwd, right, up) = camera_basis(cam);
        let (w, h) = (frame.width as usize, frame.height as usize);
        let focal = (frame.width as f32 * 0.5) / crate::hor_plus_tan(cam.fov, frame.width as f32 / frame.height.max(1) as f32);
        let (cx, cy) = (frame.width as f32 * 0.5, frame.height as f32 * 0.5);
        // vertices in world space and then camera space
        let world: Vec<[f32; 3]> = (0..pa.len())
            .map(|i| {
                let mut v = pa[i];
                if let Some(pb) = &pb {
                    let q = pb[i];
                    v = [v[0] + (q[0] - v[0]) * d.blend, v[1] + (q[1] - v[1]) * d.blend, v[2] + (q[2] - v[2]) * d.blend];
                }
                if let Some(bd) = &blend_delta {
                    v = [v[0] + bd[i][0], v[1] + bd[i][1], v[2] + bd[i][2]];
                }
                let r = rotate([v[0] * d.scale, v[1] * d.scale, v[2] * d.scale], d.rotation);
                [r[0] + d.location[0], r[1] + d.location[1], r[2] + d.location[2]]
            })
            .collect();
        let camv: Vec<[f32; 3]> = world
            .iter()
            .map(|p| {
                let rel = [p[0] - cam.pos[0], p[1] - cam.pos[1], p[2] - cam.pos[2]];
                [dot(rel, right), dot(rel, up), dot(rel, fwd)]
            })
            .collect();
        // textures per material index
        let mut tex_cache: Vec<Option<Option<Rc<Tex>>>> = vec![None; 16];
        for face in &lod.faces {
            let mat = lod.materials[face.material as usize];
            let flags = mat[0];
            if flags & PF_MODULATED != 0 {
                continue;
            }
            let ti = mat[1] as usize;
            let tex = if ti < tex_cache.len() {
                match &tex_cache[ti] {
                    Some(t) => t.clone(),
                    None => {
                        let skin = d.skins.get(ti).cloned().flatten().and_then(|p| self.texture_by_path(&p));
                        let t = skin.or_else(|| mesh.own_textures.get(ti).cloned().flatten());
                        tex_cache[ti] = Some(t.clone());
                        t
                    }
                }
            } else {
                None
            };
            let Some(tex) = tex else { continue };
            let idx: Vec<usize> = face.wedge.iter().map(|&wi| lod.wedge_vertex(wi as usize)).collect();
            if idx.iter().any(|&i| i >= camv.len() || camv[i][2] < 1.0) {
                continue;
            }
            let corners: Vec<[f32; 5]> = face
                .wedge
                .iter()
                .zip(&idx)
                .map(|(&wi, &vi)| {
                    let c = camv[vi];
                    let iw = 1.0 / c[2];
                    let wedge = &lod.wedges[wi as usize];
                    let (u, v) = (wedge.uv[0] as f32 / 256.0 * tex.w as f32, wedge.uv[1] as f32 / 256.0 * tex.h as f32);
                    [cx + c[0] * iw * focal, cy - c[1] * iw * focal, iw, u * iw, v * iw]
                })
                .collect();
            // shade by how much the surface faces the viewer
            let (p0, p1, p2) = (world[idx[0]], world[idx[1]], world[idx[2]]);
            let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
            let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let nl = dot(n, n).sqrt().max(1e-9);
            let shade = 0.5 + 0.5 * (dot([n[0] / nl, n[1] / nl, n[2] / nl], [0.3, 0.5, 0.8])).abs();
            let masked = flags & PF_MASKED != 0;
            let translucent = flags & PF_TRANSLUCENT != 0;
            let (color, depth) = (&mut frame.color, &mut frame.depth);
            raster_triangle(&corners[0], &corners[1], &corners[2], w, h, &mut |x, y, iw, u, v| {
                let z = 1.0 / iw;
                let i = y * w + x;
                if z >= depth[i] {
                    return;
                }
                let (tx, ty) = ((u.floor() as i64).rem_euclid(tex.w as i64) as usize, (v.floor() as i64).rem_euclid(tex.h as i64) as usize);
                let t = &tex.rgba[(ty * tex.w as usize + tx) * 4..][..4];
                if (masked || translucent) && t[3] == 0 {
                    return;
                }
                if translucent {
                    for k in 0..3 {
                        color[i * 4 + k] = ((color[i * 4 + k] as f32 + t[k] as f32 * shade) * 0.5).min(255.0) as u8;
                    }
                } else {
                    depth[i] = z;
                    for k in 0..3 {
                        color[i * 4 + k] = (t[k] as f32 * shade).min(255.0) as u8;
                    }
                }
            });
        }
        true
    }
}

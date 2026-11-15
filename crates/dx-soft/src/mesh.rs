//! Software rendering of a `LodMesh` (one frame, orbit camera) to check vertex decoding, UVs and textures.

use crate::{raster_triangle, Image};
use std::path::Path;
use ue_assets::files::GameFiles;
use ue_assets::mesh::LodMesh;
use ue_assets::texture::{Palette, Texture};

const PF_MASKED: u32 = 0x2;

/// Renders `export` of `package` (for example `DeusExCharacters`, the `Rat` export) seen from `yaw_deg` around the
/// vertical axis. Returns the image and a one-line description.
pub fn render_lod_mesh(game: &Path, package: &str, export: usize, frame: usize, yaw_deg: f32, w: u32, h: u32) -> Result<(Image, String), String> {
    let files = GameFiles::new(game);
    let loaded = files.package(package).ok_or_else(|| format!("package {package} not found"))?;
    let mesh = LodMesh::load(&loaded.pkg, export).map_err(|e| e.to_string())?;
    let pos = mesh.frame_positions(frame);
    // bounds for framing
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &pos {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let centre: Vec<f32> = (0..3).map(|k| (lo[k] + hi[k]) * 0.5).collect();
    let radius = (0..3).map(|k| (hi[k] - lo[k]) * 0.5).fold(0.0f32, |a, b| a.max(b)).max(1e-3) * 1.8;

    // textures per material
    let mut tex: Vec<Option<(u32, u32, Vec<u8>)>> = Vec::new();
    for slot in &mesh.mesh.textures {
        tex.push((|| {
            let (pkg, exp) = files.resolve(&loaded, *slot)?;
            let t = Texture::load(&pkg.pkg, exp).ok()?;
            let (ppkg, pexp) = files.resolve(&pkg, t.palette)?;
            let pal = Palette::load(&ppkg.pkg, pexp).ok()?;
            Some((t.mips[0].u_size, t.mips[0].v_size, t.mip_rgba(0, &pal).ok()?))
        })());
    }

    let (wi, hi_) = (w as usize, h as usize);
    let mut color = vec![0u8; wi * hi_ * 4];
    for px in color.chunks_mut(4) {
        px.copy_from_slice(&[24, 28, 40, 255]);
    }
    let mut depth = vec![f32::INFINITY; wi * hi_];
    // camera on a circle around the model, looking at its centre; mesh space: x forward, y right, z up
    let (s, c) = yaw_deg.to_radians().sin_cos();
    let eye = [centre[0] + c * radius * 1.25, centre[1] + s * radius * 1.25, centre[2] + radius * 0.3];
    let fwd = {
        let d = [centre[0] - eye[0], centre[1] - eye[1], centre[2] - eye[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        [d[0] / l, d[1] / l, d[2] / l]
    };
    let right = {
        let r = [-fwd[1], fwd[0], 0.0];
        let l = (r[0] * r[0] + r[1] * r[1]).sqrt().max(1e-6);
        [r[0] / l, r[1] / l, 0.0]
    };
    let up = [right[1] * fwd[2] - right[2] * fwd[1], right[2] * fwd[0] - right[0] * fwd[2], right[0] * fwd[1] - right[1] * fwd[0]];
    let up = [-up[0], -up[1], -up[2]];
    let up = if up[2] < 0.0 { [-up[0], -up[1], -up[2]] } else { up };
    let focal = w as f32 * 0.5 / (45.0f32.to_radians()).tan();
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let proj = |p: [f32; 3]| {
        let d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
        let z = dot(d, fwd);
        (w as f32 * 0.5 + dot(d, right) / z * focal, h as f32 * 0.5 - dot(d, up) / z * focal, 1.0 / z)
    };
    let mut drawn = 0;
    for face in &mesh.faces {
        let mat = mesh.materials[face.material as usize];
        // characters get their skins from the actor (MultiSkins), not from the mesh: use a flat grey checker instead
        let fallback = (2u32, 2u32, vec![150, 150, 160, 255, 110, 110, 120, 255, 110, 110, 120, 255, 150, 150, 160, 255]);
        let (tw, th, rgba) = match tex.get(mat[1] as usize) {
            Some(Some((a, b, c))) => (a, b, c),
            _ => (&fallback.0, &fallback.1, &fallback.2),
        };
        let corners: Vec<[f32; 5]> = face
            .wedge
            .iter()
            .map(|&wi| {
                let wedge = &mesh.wedges[wi as usize];
                let p = pos[mesh.wedge_vertex(wi as usize)];
                let (x, y, iw) = proj(p);
                let (u, v) = (wedge.uv[0] as f32 / 256.0 * *tw as f32, wedge.uv[1] as f32 / 256.0 * *th as f32);
                [x, y, iw, u * iw, v * iw]
            })
            .collect();
        let masked = mat[0] & PF_MASKED != 0;
        // facing-ratio shading (no lighting data in the mesh)
        let corner_pos: Vec<[f32; 3]> = face.wedge.iter().map(|&wi| pos[mesh.wedge_vertex(wi as usize)]).collect();
        let e1 = [corner_pos[1][0] - corner_pos[0][0], corner_pos[1][1] - corner_pos[0][1], corner_pos[1][2] - corner_pos[0][2]];
        let e2 = [corner_pos[2][0] - corner_pos[0][0], corner_pos[2][1] - corner_pos[0][1], corner_pos[2][2] - corner_pos[0][2]];
        let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
        let shade = 0.35 + 0.65 * (dot([n[0] / nl, n[1] / nl, n[2] / nl], fwd)).abs();
        raster_triangle(&corners[0], &corners[1], &corners[2], wi, hi_, &mut |x, y, iw, u, v| {
            let z = 1.0 / iw;
            let idx = y * wi + x;
            if z >= depth[idx] {
                return;
            }
            let (tx, ty) = ((u.floor() as i64).rem_euclid(*tw as i64) as usize, (v.floor() as i64).rem_euclid(*th as i64) as usize);
            let t = &rgba[(ty * *tw as usize + tx) * 4..][..4];
            if masked && t[3] == 0 {
                return;
            }
            depth[idx] = z;
            for k in 0..3 {
                color[idx * 4 + k] = (t[k] as f32 * shade) as u8;
            }
        });
        drawn += 1;
    }
    let desc = format!(
        "{} faces ({} drawn), {} verts/frame, {} frames, scale {:?}, bbox {:?}..{:?}",
        mesh.faces.len(), drawn, mesh.mesh.frame_verts, mesh.mesh.anim_frames, mesh.mesh.scale, lo, hi
    );
    Ok((Image { width: w, height: h, rgba: color }, desc))
}

/// A mesh loaded for drawing actors: the geometry and the mesh's own textures (character skins come from the actor).
pub struct MeshData {
    pub lod: LodMesh,
    /// A plain `Mesh` (no level-of-detail data; `lod` is converted from it, `lod.mesh` holds it as loaded): the engine
    /// draws it with `DrawMesh`, not `DrawLodMesh`.
    pub plain: bool,
    pub own_textures: Vec<Option<std::rc::Rc<crate::Tex>>>,
    /// Object paths of the mesh's own texture slots (`None` when unresolved), for renderers that load textures themselves.
    pub own_texture_paths: Vec<Option<String>>,
}

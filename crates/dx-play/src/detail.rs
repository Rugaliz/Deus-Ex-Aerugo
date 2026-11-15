//! Detail textures as D3DDrv draws them (`DrawComplexSurface` 10003900, asm; WORKLOG 3.127). Render hands the device a
//! surface's `DetailTexture` (texture +0x58) unless the surface is a portal or the client's `TextureDetail` is not
//! `High`. The device then draws three layers over the surface just drawn: the polygon (as `ClipBspSurf` left it)
//! clipped to view depth `z < NearZ` (380, then divided by 4.223 per layer), its points faded by
//! `min(255, round(100 (NearZ / z - 1)))` (0 on the cut), grey 0x80 vertices, the detail texel blended over the grey by
//! that alpha and the result modulated into the frame. Texture coordinates: the surface's `U`, `V` (texels of its own
//! mapping) times the detail texture's `1 / (USize * UScale)` and the layer's scale (1, then times 4.223); the detail
//! texture's pan is 0 (`UTexture::Lock`). With a detail texture the device draws no fog map.

use crate::Session;
use dx_vis::frame::{Frame, Vec3};
use dx_vk::actors::DetailPoly;

/// The layers of one surface polygon: `pts` its points as `ClipBspSurf` clipped them (view space), `uv_of` the
/// surface's texel coordinates of a world point, `size` the detail texture's `USize * UScale`, `VSize * VScale`.
pub fn detail_layers(frame: &Frame, node: u32, pts: &[dx_vis::raster::TPoint], uv_of: &dyn Fn([f64; 3]) -> [f64; 2], texture: &str, size: [f32; 2]) -> Vec<DetailPoly> {
    let mut out = Vec::new();
    let n = pts.len();
    if n < 3 {
        return out;
    }
    // view space back to the world (the frame's axes are orthonormal)
    let world = |v: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|k| frame.origin[k] as f64 + v[0] * frame.x_axis[k] as f64 + v[1] * frame.y_axis[k] as f64 + v[2] * frame.z_axis[k] as f64)
    };
    let view = |p: &dx_vis::raster::TPoint| [p.view[0] as f64, p.view[1] as f64, p.view[2] as f64];
    let umult = [1.0 / size[0].max(1e-6) as f64, 1.0 / size[1].max(1e-6) as f64];
    let (mut scale, mut near) = (1.0f32, 380.0f32);
    for _ in 0..3 {
        let is_near: Vec<bool> = pts.iter().map(|p| p.view[2] < near).collect();
        if is_near.iter().any(|&b| b) {
            let tex = |w: [f64; 3]| -> [f32; 2] {
                let u = uv_of(w);
                [(u[0] * umult[0] * scale as f64) as f32, (u[1] * umult[1] * scale as f64) as f32]
            };
            let mut poly = Vec::new();
            let mut prev = n - 1;
            for cur in 0..n {
                if is_near[prev] != is_near[cur] {
                    // where the edge crosses z = NearZ: s cur + t prev, t = (z_cur - NearZ) / (z_cur - z_prev)
                    let (a, b) = (view(&pts[cur]), view(&pts[prev]));
                    let t = (a[2] - near as f64) / (a[2] - b[2]);
                    let s = 1.0 - t;
                    let w = world(std::array::from_fn(|k| s * a[k] + t * b[k]));
                    poly.push((w.map(|x| x as f32), tex(w), 0.0));
                }
                if is_near[cur] {
                    let w = world(view(&pts[cur]));
                    let a = ((((near as f64 / pts[cur].view[2] as f64) - 1.0) * 100.0) as f32).round_ties_even();
                    let a = if a > 255.0 || a < 0.0 { 255.0 } else { a };
                    poly.push((w.map(|x| x as f32), tex(w), a / 255.0));
                }
                prev = cur;
            }
            out.push(DetailPoly { node, texture: texture.to_string(), points: poly });
        }
        scale = (scale as f64 * 4.223f32 as f64) as f32;
        near = (near as f64 * 0.236_798_48f32 as f64) as f32;
    }
    out
}

impl Session {
    /// The detail layers of the main frame this engine frame: those drawn with the solid world (its solid list, and the
    /// third list's masked surfaces, which the renderer draws with them) and, by node, those of its first list (mirrors
    /// that opened a child frame) and of the third list's translucent and modulated surfaces (each drawn right after
    /// its surface). Like `actor_draws`, a view drawn before any step (or after a size change) runs the visibility
    /// pass here.
    pub fn detail_polys(&mut self, cam: &dx_soft::Camera) -> (Vec<DetailPoly>, std::collections::HashMap<u32, Vec<DetailPoly>>) {
        if self.vis_frames().is_empty() {
            self.mark_rendered(cam);
        }
        self.frame_details(0)
    }

    /// `detail_polys` for frame `fi` of the last visibility pass (0 the main view, then its sky and mirror frames), level
    /// and movers' surfaces alike (the level model the walk ran on, the tracker's nodes in it). A surface that gets a
    /// fog map in that frame gets no layers: `DrawComplexSurface` (1000395f) drops the detail texture when the surface
    /// has a fog map.
    pub fn frame_details(&mut self, fi: usize) -> (Vec<DetailPoly>, std::collections::HashMap<u32, Vec<DetailPoly>>) {
        let fogged: std::collections::HashSet<i32> = self.frame_fog_draws(fi).into_iter().map(|(si, _)| si as i32).collect();
        let Some(fv) = self.vis_frames().get(fi) else { return Default::default() };
        let frame = fv.frame.clone();
        let model = self.vm.world_model.as_deref().unwrap_or(&self.view.model);
        let fog_free = |node: u32| model.nodes.get(node as usize).map(|n| !fogged.contains(&n.i_surf)).unwrap_or(true);
        let listed: Vec<(u32, u32, u8)> = fv.vis.surfaces.iter().zip(&fv.vis.surface_lists).filter(|(&(node, _, _), _)| fog_free(node)).map(|(&(node, flags, _), &l)| (node, flags, l)).collect();
        // the first list, and the third list's translucent and modulated surfaces, are drawn one by one
        let by_node = |&(_, flags, l): &(u32, u32, u8)| l == 0 || (l == 2 && flags & 0x44 != 0);
        let solid: Vec<(u32, u32)> = listed.iter().filter(|x| !by_node(x)).map(|&(n, f, _)| (n, f)).collect();
        let single: Vec<(u32, u32)> = listed.iter().filter(|x| by_node(x)).map(|&(n, f, _)| (n, f)).collect();
        let solid = self.details_for(&frame, &solid);
        let mut by_node: std::collections::HashMap<u32, Vec<DetailPoly>> = Default::default();
        for p in self.details_for(&frame, &single) {
            by_node.entry(p.node).or_default().push(p);
        }
        (solid, by_node)
    }

    /// The detail layers of the surfaces `(node, draw flags)` of a frame drawn from `frame`.
    pub fn details_for(&mut self, frame: &Frame, surfaces: &[(u32, u32)]) -> Vec<DetailPoly> {
        // every surface's polygon and texture mapping first (the model is borrowed from the VM)
        struct Surf {
            node: u32,
            texture: i32,
            points: Vec<Vec3>,
            base: Vec3,
            tu: Vec3,
            tv: Vec3,
        }
        let mut todo: Vec<Surf> = Vec::new();
        {
            let model = self.vm.world_model.as_deref().unwrap_or(&self.view.model);
            let z = |p: &Vec3| (0..3).map(|k| (p[k] - frame.origin[k]) * frame.z_axis[k]).sum::<f32>();
            let get = |i: i32, v: &Vec<Vec3>| v.get(i.max(0) as usize).copied().unwrap_or([0.0; 3]);
            for &(node, flags) in surfaces {
                // portals get no detail texture
                if flags & 0x400_0000 != 0 {
                    continue;
                }
                let Some(nd) = model.nodes.get(node as usize) else { continue };
                let Some(surf) = model.surfs.get(nd.i_surf.max(0) as usize) else { continue };
                let points: Vec<Vec3> = (0..nd.num_vertices as usize)
                    .filter_map(|k| model.verts.get(nd.i_vert_pool as usize + k).and_then(|v| model.points.get(v.p_vertex as usize)).copied())
                    .collect();
                // nothing nearer than the first layer's NearZ: no layer at all
                if points.iter().all(|p| z(p) >= 380.0) {
                    continue;
                }
                todo.push(Surf { node, texture: surf.texture, points, base: get(surf.p_base, &model.points), tu: get(surf.v_texture_u, &model.vectors), tv: get(surf.v_texture_v, &model.vectors) });
            }
        }
        let mut out = Vec::new();
        for sf in todo {
            let detail = match self.surface_details.get(&sf.texture) {
                Some(d) => d.clone(),
                None => {
                    let d = self.view.surface_detail(sf.texture);
                    self.surface_details.insert(sf.texture, d.clone());
                    d
                }
            };
            let Some((texture, size)) = detail else { continue };
            let (base, tu, tv) = (sf.base, sf.tu, sf.tv);
            let uv_of = |w: [f64; 3]| -> [f64; 2] {
                let d = [w[0] - base[0] as f64, w[1] - base[1] as f64, w[2] - base[2] as f64];
                [d[0] * tu[0] as f64 + d[1] * tu[1] as f64 + d[2] * tu[2] as f64, d[0] * tv[0] as f64 + d[1] * tv[1] as f64 + d[2] * tv[2] as f64]
            };
            let pts = dx_vis::raster::clip_polygon(frame, &sf.points);
            out.extend(detail_layers(frame, sf.node, &pts, &uv_of, &texture, size));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame::new([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 90.0, 640, 480)
    }

    /// A floor strip from depth 50 to 500 (camera at the origin looking along +x): layer 0 cut at z = 380 (alpha
    /// min(255, round(100 (380 / 50 - 1))) = 255 at the near points, 0 on the cut), layer 1 at NearZ = 380 * 0.23679848
    /// (alpha round(100 (89.98 / 50 - 1)) = 80), layer 2 (NearZ 21.3) nothing. UVs: world (y, x) over 64 texels times
    /// the layer's scale.
    #[test]
    fn layers_cut_at_near_z_and_fade() {
        let f = frame();
        let world = [[50.0, -20.0, -20.0], [500.0, -20.0, -20.0], [500.0, 20.0, -20.0], [50.0, 20.0, -20.0]];
        let pts = dx_vis::raster::clip_polygon(&f, &world);
        assert_eq!(pts.len(), 4);
        let uv_of = |w: [f64; 3]| [w[1], w[0]];
        let layers = detail_layers(&f, 0, &pts, &uv_of, "D.T", [64.0, 64.0]);
        assert_eq!(layers.len(), 2);
        let l0 = &layers[0].points;
        assert_eq!(l0.len(), 4);
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-3);
        assert!(close(l0[0].0, [50.0, -20.0, -20.0]) && l0[0].2 == 1.0);
        assert!(close(l0[1].0, [380.0, -20.0, -20.0]) && l0[1].2 == 0.0);
        assert!(close(l0[2].0, [380.0, 20.0, -20.0]) && l0[2].2 == 0.0);
        assert!(close(l0[3].0, [50.0, 20.0, -20.0]) && l0[3].2 == 1.0);
        assert!((l0[1].1[0] - -20.0 / 64.0).abs() < 1e-6 && (l0[1].1[1] - 380.0 / 64.0).abs() < 1e-4);
        let near1 = (380.0f64 * 0.236_798_48f32 as f64) as f32;
        let l1 = &layers[1].points;
        assert!((l1[1].0[0] - near1).abs() < 1e-3);
        assert_eq!(l1[0].2, 80.0 / 255.0);
        // the second layer's coordinates are 4.223 times the first's
        assert!((l1[0].1[1] - 50.0 / 64.0 * 4.223).abs() < 1e-4);
    }

    /// A wall at depth 100 facing the camera: one layer, every point at full alpha (round(100 (380 / 100 - 1)) = 280,
    /// capped at 255); the next NearZ (89.98) is nearer than the wall.
    #[test]
    fn a_wall_inside_the_first_range_gets_one_full_layer() {
        let f = frame();
        let world = [[100.0, -10.0, 10.0], [100.0, 10.0, 10.0], [100.0, 10.0, -10.0], [100.0, -10.0, -10.0]];
        let pts = dx_vis::raster::clip_polygon(&f, &world);
        let layers = detail_layers(&f, 0, &pts, &|w| [w[1], w[2]], "D.T", [64.0, 64.0]);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].points.len(), 4);
        assert!(layers[0].points.iter().all(|p| p.2 == 1.0));
    }
}

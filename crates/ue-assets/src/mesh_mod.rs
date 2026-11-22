//! Replacement mesh geometry from the mods folder: `Meshes/<Package>/<Name>.aemesh` (`ue_package::mods`).
//!
//! A replacement changes only the model's vertices (in every animation frame) and its triangles. The mesh keeps
//! everything else of the original: its animation sequences and notifies (the VM reads those from the package), its
//! texture slots (so `Skin`/`MultiSkins` work as before), its scale, origin and `RotOrigin`, and its special
//! (attachment) vertices and faces, which place the weapon in a pawn's hand. The bounding boxes and spheres are made
//! again from the new vertices, so visibility tests see the new shape. The level of detail data is dropped: the
//! replacement is always drawn whole.
//!
//! ## The `.aemesh` format (version 1, little-endian)
//!
//! | Bytes | What |
//! |---|---|
//! | 8 | magic `AEMESH01` |
//! | 4 | `frames`: number of animation frames; must equal the original mesh's |
//! | 4 | `verts`: model vertices per frame (special vertices not counted, they stay the original's) |
//! | 4 | `tris`: number of triangles |
//! | `frames * verts * 12` | positions, frame after frame: x, y, z as f32 in the mesh's stored units (the integers the package holds, before `Origin`, `Scale` and `RotOrigin`); rounded to 16-bit integers on load |
//! | `tris * 18` | triangles: three u16 vertex indices (0 = first model vertex), three (u, v) pairs of u8 (texel = uv / 256 * texture size), u8 texture slot, u8 zero, u32 poly flags (as the mesh materials: 0 normal, 2 masked, 4 translucent, ...) |

use crate::mesh::{Face, LodMesh, Mesh, MeshTri, Wedge};
use crate::model::Bounds;

pub const MAGIC: &[u8; 8] = b"AEMESH01";

/// One triangle of a replacement mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoTri {
    pub vertex: [u16; 3],
    pub uv: [[u8; 2]; 3],
    pub slot: u8,
    pub flags: u32,
}

/// The geometry of a mesh: positions per frame (model vertices, stored units) and triangles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Geometry {
    pub frames: Vec<Vec<[f32; 3]>>,
    pub tris: Vec<GeoTri>,
}

impl Geometry {
    pub fn verts(&self) -> usize {
        self.frames.first().map_or(0, Vec::len)
    }

    /// The model vertices and faces of a mesh as stored (special vertices left out, wedges unfolded into corners).
    pub fn from_lod(lod: &LodMesh) -> Geometry {
        let special = lod.special_verts();
        let frames = (0..lod.mesh.anim_frames.max(1) as usize).map(|f| raw_frame(lod, f)[special..].to_vec()).collect();
        let tris = lod
            .faces
            .iter()
            .map(|face| {
                let [flags, slot] = lod.materials.get(face.material as usize).copied().unwrap_or([0, 0]);
                let w = face.wedge.map(|w| lod.wedges[w as usize]);
                GeoTri { vertex: w.map(|w| w.vertex), uv: w.map(|w| w.uv), slot: slot as u8, flags }
            })
            .collect();
        Geometry { frames, tris }
    }

    pub fn write(&self) -> Vec<u8> {
        let n = self.verts();
        let mut out = Vec::with_capacity(20 + self.frames.len() * n * 12 + self.tris.len() * 18);
        out.extend_from_slice(MAGIC);
        for v in [self.frames.len(), n, self.tris.len()] {
            out.extend_from_slice(&(v as u32).to_le_bytes());
        }
        for f in &self.frames {
            for p in f {
                for c in p {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
        }
        for t in &self.tris {
            for v in t.vertex {
                out.extend_from_slice(&v.to_le_bytes());
            }
            for uv in t.uv {
                out.extend_from_slice(&uv);
            }
            out.extend_from_slice(&[t.slot, 0]);
            out.extend_from_slice(&t.flags.to_le_bytes());
        }
        out
    }

    pub fn read(b: &[u8]) -> Result<Geometry, String> {
        if b.len() < 20 || &b[..8] != MAGIC {
            return Err("not an .aemesh file (version 1)".into());
        }
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
        let (nf, nv, nt) = (u32_at(8), u32_at(12), u32_at(16));
        if nf == 0 || nv == 0 || nv > 65535 || nf > 65535 {
            return Err(format!("bad counts: {nf} frames, {nv} vertices"));
        }
        let want = 20 + nf * nv * 12 + nt * 18;
        if b.len() != want {
            return Err(format!("size {} != {want} for {nf} frames, {nv} vertices, {nt} triangles", b.len()));
        }
        let f32_at = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let mut o = 20;
        let mut frames = Vec::with_capacity(nf);
        for _ in 0..nf {
            let mut f = Vec::with_capacity(nv);
            for _ in 0..nv {
                f.push([f32_at(o), f32_at(o + 4), f32_at(o + 8)]);
                o += 12;
            }
            frames.push(f);
        }
        let mut tris = Vec::with_capacity(nt);
        for _ in 0..nt {
            let v = |k: usize| u16::from_le_bytes([b[o + k * 2], b[o + k * 2 + 1]]);
            let vertex = [v(0), v(1), v(2)];
            if vertex.iter().any(|&i| i as usize >= nv) {
                return Err(format!("triangle {} uses vertex {vertex:?} of {nv}", tris.len()));
            }
            let uv = [[b[o + 6], b[o + 7]], [b[o + 8], b[o + 9]], [b[o + 10], b[o + 11]]];
            tris.push(GeoTri { vertex, uv, slot: b[o + 12], flags: u32_at(o + 14) as u32 });
            o += 18;
        }
        Ok(Geometry { frames, tris })
    }

    /// The original mesh with this geometry in place of its own (see the module notes).
    pub fn apply(&self, orig: &LodMesh) -> Result<LodMesh, String> {
        let frames = orig.mesh.anim_frames.max(1) as usize;
        if self.frames.len() != frames {
            return Err(format!("{} frames, the mesh has {frames}", self.frames.len()));
        }
        let n = self.verts();
        if self.frames.iter().any(|f| f.len() != n) {
            return Err("frames of different vertex counts".into());
        }
        let special = orig.special_verts();
        let pack = |p: [f32; 3]| {
            let q = p.map(|c| c.round().clamp(-32768.0, 32767.0) as i16 as u16 as u32);
            [q[0] | q[1] << 16, q[2]]
        };
        let mut verts = Vec::with_capacity(frames * (special + n));
        let mut boxes = Vec::with_capacity(frames);
        let mut spheres = Vec::with_capacity(frames);
        let (mut all_lo, mut all_hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for (f, new) in self.frames.iter().enumerate() {
            let old = raw_frame(orig, f);
            let frame: Vec<[f32; 3]> = old[..special].iter().chain(new.iter()).copied().collect();
            let packed: Vec<[u32; 2]> = frame.iter().map(|&p| pack(p)).collect();
            let stored: Vec<[f32; 3]> = packed.iter().map(|&p| crate::mesh::unpack_vertex(p).map(f32::from)).collect();
            let (b, s) = bounds_of(&stored);
            for k in 0..3 {
                all_lo[k] = all_lo[k].min(b.min[k]);
                all_hi[k] = all_hi[k].max(b.max[k]);
            }
            boxes.push(b);
            spheres.push(s);
            verts.extend(packed);
        }
        // wedges: one per distinct (vertex, uv); materials: the original's first, then any new (flags, slot) pair
        let mut wedges: Vec<Wedge> = Vec::new();
        let mut wedge_of = std::collections::HashMap::new();
        let mut materials = orig.materials.clone();
        let mut faces = Vec::with_capacity(self.tris.len());
        for t in &self.tris {
            let m = [t.flags, t.slot as u32];
            let material = materials.iter().position(|x| *x == m).unwrap_or_else(|| {
                materials.push(m);
                materials.len() - 1
            });
            let wedge = std::array::from_fn(|k| {
                *wedge_of.entry((t.vertex[k], t.uv[k])).or_insert_with(|| {
                    wedges.push(Wedge { vertex: t.vertex[k], uv: t.uv[k] });
                    (wedges.len() - 1) as u16
                })
            });
            faces.push(Face { wedge, material: material as u16 });
        }
        if wedges.len() > 65535 {
            return Err(format!("{} wedges (at most 65535)", wedges.len()));
        }
        let (bounding_box, bounding_sphere) = {
            let centre: [f32; 3] = std::array::from_fn(|k| (all_lo[k] + all_hi[k]) * 0.5);
            let r = spheres.iter().map(|s| dist(centre, [s[0], s[1], s[2]]) + s[3]).fold(0.0f32, f32::max);
            (Bounds { min: all_lo, max: all_hi, is_valid: true }, [centre[0], centre[1], centre[2], r])
        };
        // a plain `Mesh` is drawn from its triangle list and per-vertex triangle links
        let (tris, connects, vert_links) = if orig.mesh.tris.is_empty() {
            (Vec::new(), Vec::new(), Vec::new())
        } else {
            let tris: Vec<MeshTri> = self.tris.iter().map(|t| MeshTri { vertex: t.vertex.map(|v| v + special as u16), uv: t.uv, poly_flags: t.flags, texture_index: t.slot as u32 }).collect();
            let mut links: Vec<Vec<i32>> = vec![Vec::new(); special + n];
            for (i, t) in tris.iter().enumerate() {
                for v in t.vertex {
                    links[v as usize].push(i as i32);
                }
            }
            let mut connects = Vec::with_capacity(links.len());
            let mut flat = Vec::new();
            for l in links {
                connects.push([l.len() as i32, flat.len() as i32]);
                flat.extend(l);
            }
            (tris, connects, flat)
        };
        let mut lod_params = orig.lod_params;
        lod_params[0] = if orig.lod_params[0] == 0 { 0 } else { n as u32 };
        Ok(LodMesh {
            mesh: Mesh {
                bounding_box,
                bounding_sphere,
                verts,
                tris,
                connects,
                vert_links,
                bounding_boxes: boxes,
                bounding_spheres: spheres,
                frame_verts: (special + n) as i32,
                anim_frames: frames as i32,
                ..orig.mesh.clone()
            },
            collapse_point_thus: Vec::new(),
            face_level: Vec::new(),
            faces,
            collapse_wedge_thus: Vec::new(),
            wedges,
            materials,
            special_faces: orig.special_faces.clone(),
            lod_params,
            remap_anim_verts: Vec::new(),
            old_frame_verts: 0,
        })
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The box around points and the sphere around the box's centre.
fn bounds_of(p: &[[f32; 3]]) -> (Bounds, [f32; 4]) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in p {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    let c: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
    let r = p.iter().map(|&v| dist(c, v)).fold(0.0f32, f32::max);
    (Bounds { min: lo, max: hi, is_valid: true }, [c[0], c[1], c[2], r])
}

/// Every vertex of one frame (special ones first) in stored units, as `ULodMesh::GetFrame` reads them.
pub fn raw_frame(lod: &LodMesh, frame: usize) -> Vec<[f32; 3]> {
    let m = &lod.mesh;
    let n = m.frame_verts.max(0) as usize;
    let stride = if lod.remap_anim_verts.is_empty() { n } else { lod.old_frame_verts.max(0) as usize };
    let frame = frame.min((m.anim_frames.max(1) - 1) as usize);
    (0..n)
        .map(|j| {
            let src = if lod.remap_anim_verts.is_empty() { j } else { lod.remap_anim_verts.get(j).copied().unwrap_or(0) as usize };
            m.verts.get(frame * stride + src).map(|&p| crate::mesh::unpack_vertex(p).map(f32::from)).unwrap_or_default()
        })
        .collect()
}

/// `lod` (the mesh at object path `path`, `Package.Name`) with the mods folder's replacement geometry in place, if a
/// mod has one; a replacement that does not fit is reported and left out.
pub fn with_mod(path: &str, lod: LodMesh) -> LodMesh {
    let Some(source) = ue_package::mods::mesh(path) else { return lod };
    let applied = source.read().map_err(|e| e.to_string()).and_then(|b| Geometry::read(&b)).and_then(|g| g.apply(&lod));
    match applied {
        Ok(m) => m,
        Err(e) => {
            eprintln!("mods: {} not used for {path}: {e}", source.describe());
            lod
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_mesh() -> LodMesh {
        // two frames, one special vertex, a triangle of three model vertices
        let p = |x: i16, y: i16, z: i16| [x as u16 as u32 | (y as u16 as u32) << 16, z as u16 as u32];
        let mut lod = LodMesh::from_plain(Mesh::default());
        lod.mesh.verts = vec![p(9, 9, 9), p(0, 0, 0), p(10, 0, 0), p(0, 10, 0), p(8, 8, 8), p(0, 0, 1), p(10, 0, 1), p(0, 10, 1)];
        lod.mesh.frame_verts = 4;
        lod.mesh.anim_frames = 2;
        lod.mesh.textures = vec![0];
        lod.wedges = vec![Wedge { vertex: 0, uv: [0, 0] }, Wedge { vertex: 1, uv: [255, 0] }, Wedge { vertex: 2, uv: [0, 255] }];
        lod.faces = vec![Face { wedge: [0, 1, 2], material: 0 }];
        lod.materials = vec![[0, 0]];
        lod.special_faces = vec![Face { wedge: [0, 0, 0], material: 0 }];
        lod.lod_params[0] = 3;
        lod.lod_params[1] = 1;
        lod
    }

    #[test]
    fn geometry_round_trips_through_the_file() {
        let g = Geometry::from_lod(&tri_mesh());
        assert_eq!(g.frames[1], [[0.0, 0.0, 1.0], [10.0, 0.0, 1.0], [0.0, 10.0, 1.0]]);
        assert_eq!(Geometry::read(&g.write()).unwrap(), g);
        let back = g.apply(&tri_mesh()).unwrap();
        assert_eq!(back.mesh.verts, tri_mesh().mesh.verts);
        assert_eq!(Geometry::from_lod(&back), g);
    }

    #[test]
    fn a_new_face_moves_with_the_frames_and_keeps_the_special_vertices() {
        let mut g = Geometry::from_lod(&tri_mesh());
        for (f, frame) in g.frames.iter_mut().enumerate() {
            frame.push([10.0, 10.0, f as f32]);
        }
        g.tris.push(GeoTri { vertex: [1, 3, 2], uv: [[1, 1]; 3], slot: 0, flags: 2 });
        let m = g.apply(&tri_mesh()).unwrap();
        assert_eq!((m.mesh.frame_verts, m.lod_params[0], m.special_verts()), (5, 4, 1));
        assert_eq!(m.frame_local(1)[0], [8.0, 8.0, 8.0]);
        assert_eq!(m.frame_local(1)[4], [10.0, 10.0, 1.0]);
        assert_eq!(m.materials, [[0, 0], [2, 0]]);
        assert_eq!(m.faces.len(), 2);
        assert_eq!(m.mesh.bounding_boxes[1].max, [10.0, 10.0, 8.0]);
        assert!(Geometry { frames: vec![vec![[0.0; 3]]], tris: vec![] }.apply(&tri_mesh()).is_err(), "frame count must match");
    }
}

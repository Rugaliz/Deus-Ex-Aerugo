//! `Mesh` and `LodMesh` objects: vertex-animated models (spec: `docs/formats/mesh-format.md`).

use crate::model::{Bounds, Vec3};
use crate::payload;
use ue_package::{Error, Package, Reader, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Notify {
    pub time: f32,
    /// Name index of the script function to call.
    pub function: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnimSeq {
    /// Name indices (into the owning package's name table).
    pub name: i32,
    pub group: i32,
    pub start_frame: i32,
    pub num_frames: i32,
    pub notifys: Vec<Notify>,
    pub rate: f32,
}

/// One triangle: vertex indices, per-corner texture coordinates (0-255), surface flags and texture slot.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshTri {
    pub vertex: [u16; 3],
    pub uv: [[u8; 2]; 3],
    pub poly_flags: u32,
    pub texture_index: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub wedge: [u16; 3],
    pub material: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wedge {
    pub vertex: u16,
    pub uv: [u8; 2],
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub bounding_box: Bounds,
    pub bounding_sphere: [f32; 4],
    /// Packed vertices of all frames (two 32-bit words each; frame `f` starts at `f * frame_verts`).
    pub verts: Vec<[u32; 2]>,
    pub tris: Vec<MeshTri>,
    pub anim_seqs: Vec<AnimSeq>,
    /// (number of triangles using the vertex, offset into `vert_links`) per vertex.
    pub connects: Vec<[i32; 2]>,
    pub vert_links: Vec<i32>,
    /// Raw object references of the texture slots.
    pub textures: Vec<i32>,
    pub bounding_boxes: Vec<Bounds>,
    pub bounding_spheres: Vec<[f32; 4]>,
    pub frame_verts: i32,
    pub anim_frames: i32,
    pub and_flags: u32,
    pub or_flags: u32,
    pub scale: Vec3,
    pub origin: Vec3,
    pub rot_origin: [i32; 3],
    pub cur_poly: i32,
    pub cur_vertex: i32,
    /// Extra integer array present in package versions after 65.
    pub extra: Vec<i32>,
}

#[derive(Debug, Clone)]
pub struct LodMesh {
    pub mesh: Mesh,
    pub collapse_point_thus: Vec<u16>,
    pub face_level: Vec<u16>,
    pub faces: Vec<Face>,
    pub collapse_wedge_thus: Vec<u16>,
    pub wedges: Vec<Wedge>,
    /// (poly flags, texture index) per material.
    pub materials: Vec<[u32; 2]>,
    pub special_faces: Vec<Face>,
    /// Eight raw words in file order: model verts, special verts, then the LOD tuning values (floats).
    pub lod_params: [u32; 8],
    pub remap_anim_verts: Vec<u16>,
    pub old_frame_verts: i32,
}

fn vec3(r: &mut Reader) -> Result<Vec3> {
    Ok([r.f32()?, r.f32()?, r.f32()?])
}

fn bounds(r: &mut Reader) -> Result<Bounds> {
    Ok(Bounds { min: vec3(r)?, max: vec3(r)?, is_valid: r.u8()? != 0 })
}

fn count(r: &mut Reader, what: &str) -> Result<usize> {
    let n = r.compact_index()?;
    if !(0..=1 << 24).contains(&n) {
        return Err(Error::Invalid(format!("{what} count {n}")));
    }
    Ok(n as usize)
}

fn array<T>(r: &mut Reader, what: &str, mut f: impl FnMut(&mut Reader) -> Result<T>) -> Result<Vec<T>> {
    let n = count(r, what)?;
    (0..n).map(|_| f(r)).collect()
}

/// Lazy array (package version 62 and later): absolute end position, then the counted elements.
fn lazy<T>(r: &mut Reader, base: usize, what: &str, f: impl FnMut(&mut Reader) -> Result<T>) -> Result<Vec<T>> {
    let end = r.i32()?;
    let v = array(r, what, f)?;
    if end as usize != base + r.pos() {
        return Err(Error::Invalid(format!("{what}: end marker {end} != {}", base + r.pos())));
    }
    Ok(v)
}

fn mesh_part(pkg: &Package, export: usize, r: &mut Reader, base: usize) -> Result<Mesh> {
    // UPrimitive part
    let _ = (pkg, export);
    let bb0 = bounds(r)?;
    let bs0 = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
    let verts = lazy(r, base, "mesh vertex", |r| Ok([r.u32()?, r.u32()?]))?;
    let tris = lazy(r, base, "triangle", |r| {
        let vertex = [r.u16()?, r.u16()?, r.u16()?];
        let uv = [[r.u8()?, r.u8()?], [r.u8()?, r.u8()?], [r.u8()?, r.u8()?]];
        Ok(MeshTri { vertex, uv, poly_flags: r.u32()?, texture_index: r.u32()? })
    })?;
    let anim_seqs = array(r, "animation sequence", |r| {
        let (name, group) = (r.compact_index()?, r.compact_index()?);
        let (start_frame, num_frames) = (r.i32()?, r.i32()?);
        let notifys = array(r, "notify", |r| Ok(Notify { time: r.f32()?, function: r.compact_index()? }))?;
        Ok(AnimSeq { name, group, start_frame, num_frames, notifys, rate: r.f32()? })
    })?;
    let connects = lazy(r, base, "connect", |r| Ok([r.i32()?, r.i32()?]))?;
    // second bounding box and sphere (UMesh repeats them)
    let bounding_box = bounds(r)?;
    let bounding_sphere = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
    let _ = (bb0, bs0);
    let vert_links = lazy(r, base, "vertex link", |r| r.i32())?;
    let textures = array(r, "texture", |r| r.compact_index())?;
    let bounding_boxes = array(r, "bounding box", bounds)?;
    let bounding_spheres = array(r, "bounding sphere", |r| Ok([r.f32()?, r.f32()?, r.f32()?, r.f32()?]))?;
    let frame_verts = r.i32()?;
    let anim_frames = r.i32()?;
    let and_flags = r.u32()?;
    let or_flags = r.u32()?;
    let scale = vec3(r)?;
    let origin = vec3(r)?;
    let rot_origin = [r.i32()?, r.i32()?, r.i32()?];
    let cur_poly = r.i32()?;
    let cur_vertex = r.i32()?;
    let extra = if pkg.version == 65 {
        r.i32()?;
        Vec::new()
    } else if pkg.version > 65 {
        array(r, "extra", |r| r.i32())?
    } else {
        Vec::new()
    };
    Ok(Mesh {
        bounding_box,
        bounding_sphere,
        verts,
        tris,
        anim_seqs,
        connects,
        vert_links,
        textures,
        bounding_boxes,
        bounding_spheres,
        frame_verts,
        anim_frames,
        and_flags,
        or_flags,
        scale,
        origin,
        rot_origin,
        cur_poly,
        cur_vertex,
        extra,
    })
}

impl Mesh {
    /// Loads a plain `Mesh` object.
    pub fn load(pkg: &Package, export: usize) -> Result<Mesh> {
        if pkg.version < 62 {
            return Err(Error::Invalid("meshes older than package version 62 are not supported".into()));
        }
        let p = payload(pkg, export)?;
        let r = &mut Reader::new(p.data, p.native_start);
        let m = mesh_part(pkg, export, r, pkg.exports[export].serial_offset as usize)?;
        if r.pos() != p.data.len() {
            return Err(Error::Invalid(format!("mesh has {} trailing bytes", p.data.len() as i64 - r.pos() as i64)));
        }
        Ok(m)
    }
}

impl LodMesh {
    pub fn load(pkg: &Package, export: usize) -> Result<LodMesh> {
        if pkg.version < 62 {
            return Err(Error::Invalid("meshes older than package version 62 are not supported".into()));
        }
        let p = payload(pkg, export)?;
        let r = &mut Reader::new(p.data, p.native_start);
        let mesh = mesh_part(pkg, export, r, pkg.exports[export].serial_offset as usize)?;
        let words = |r: &mut Reader| array(r, "u16", |r| r.u16());
        let face = |r: &mut Reader| Ok(Face { wedge: [r.u16()?, r.u16()?, r.u16()?], material: r.u16()? });
        let collapse_point_thus = words(r)?;
        let face_level = words(r)?;
        let faces = array(r, "face", face)?;
        let collapse_wedge_thus = words(r)?;
        let wedges = array(r, "wedge", |r| Ok(Wedge { vertex: r.u16()?, uv: [r.u8()?, r.u8()?] }))?;
        let materials = array(r, "material", |r| Ok([r.u32()?, r.u32()?]))?;
        let special_faces = array(r, "special face", face)?;
        let mut lod_params = [0u32; 8];
        for w in &mut lod_params {
            *w = r.u32()?;
        }
        let remap_anim_verts = words(r)?;
        let old_frame_verts = r.i32()?;
        if r.pos() != p.data.len() {
            return Err(Error::Invalid(format!("lod mesh has {} trailing bytes", p.data.len() as i64 - r.pos() as i64)));
        }
        Ok(LodMesh {
            mesh,
            collapse_point_thus,
            face_level,
            faces,
            collapse_wedge_thus,
            wedges,
            materials,
            special_faces,
            lod_params,
            remap_anim_verts,
            old_frame_verts,
        })
    }
}

impl LodMesh {
    /// A plain `Mesh` (no level of detail data: the logo meshes of the DX level are such) as a `LodMesh` with one wedge
    /// per triangle corner and one material per distinct (flags, texture slot) pair, so the same drawing code serves both.
    pub fn from_plain(mesh: Mesh) -> LodMesh {
        let mut wedges = Vec::with_capacity(mesh.tris.len() * 3);
        let mut faces = Vec::with_capacity(mesh.tris.len());
        let mut materials: Vec<[u32; 2]> = Vec::new();
        for t in &mesh.tris {
            let m = [t.poly_flags, t.texture_index];
            let material = materials.iter().position(|x| *x == m).unwrap_or_else(|| {
                materials.push(m);
                materials.len() - 1
            });
            let base = wedges.len() as u16;
            for k in 0..3 {
                wedges.push(Wedge { vertex: t.vertex[k], uv: t.uv[k] });
            }
            faces.push(Face { wedge: [base, base + 1, base + 2], material: material as u16 });
        }
        LodMesh {
            mesh,
            collapse_point_thus: Vec::new(),
            face_level: Vec::new(),
            faces,
            collapse_wedge_thus: Vec::new(),
            wedges,
            materials,
            special_faces: Vec::new(),
            lod_params: [0; 8],
            remap_anim_verts: Vec::new(),
            old_frame_verts: 0,
        }
    }
}

/// Unpacks a stored vertex: three signed 16-bit coordinates (x, y, z) in mesh space; the upper half of the second
/// word is unused.
pub fn unpack_vertex(v: [u32; 2]) -> [i16; 3] {
    [v[0] as u16 as i16, (v[0] >> 16) as u16 as i16, v[1] as u16 as i16]
}

impl LodMesh {
    /// Number of "special" vertices (attachment points) that precede the model vertices in each frame.
    pub fn special_verts(&self) -> usize {
        self.lod_params[1] as usize
    }

    /// Index into a frame's vertex list of the vertex a wedge refers to (wedges count from the first model vertex,
    /// after the special vertices).
    pub fn wedge_vertex(&self, wedge: usize) -> usize {
        self.wedges[wedge].vertex as usize + self.special_verts()
    }

    /// Position of vertex `j` of one animation frame (as `frame_positions` gives it), e.g. a special vertex.
    pub fn frame_vertex(&self, frame: usize, j: usize) -> Vec3 {
        let m = &self.mesh;
        let n = m.frame_verts.max(0) as usize;
        let stride = if self.remap_anim_verts.is_empty() { n } else { self.old_frame_verts.max(0) as usize };
        let frame = frame.min((m.anim_frames.max(1) - 1) as usize);
        let src = if self.remap_anim_verts.is_empty() { j } else { self.remap_anim_verts.get(j).copied().unwrap_or(0) as usize };
        let Some(&packed) = m.verts.get(frame * stride + src) else { return [0.0; 3] };
        let raw = unpack_vertex(packed);
        let p: Vec3 = std::array::from_fn(|k| (raw[k] as f32 - m.origin[k]) * m.scale[k]);
        if m.rot_origin == [0, 0, 0] {
            return p;
        }
        let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
        let (sp, cp) = ang(m.rot_origin[0]).sin_cos();
        let (sy, cy) = ang(m.rot_origin[1]).sin_cos();
        let (sr, cr) = ang(m.rot_origin[2]).sin_cos();
        let axes = [
            [cp * cy, cp * sy, sp],
            [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp],
            [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp],
        ];
        std::array::from_fn(|k| p[0] * axes[0][k] + p[1] * axes[1][k] + p[2] * axes[2][k])
    }

    /// Positions of the `frame_verts` vertices of one animation frame, in mesh units: `(raw - origin) * scale`, then turned
    /// by `RotOrigin` (first-person weapons are modelled along +Y and turned to +X this way; it is zero for most meshes).
    /// (Whether the origin is subtracted before scaling is checked visually, see the spec.)
    /// The vertices of one animation frame as `ULodMesh::GetFrame` starts from them: the packed position minus the
    /// mesh's `Origin` (scale and `RotOrigin` are applied by the placement, `dx_vk::actors::mesh_coords`).
    pub fn frame_local(&self, frame: usize) -> Vec<Vec3> {
        let m = &self.mesh;
        let n = m.frame_verts.max(0) as usize;
        let stride = if self.remap_anim_verts.is_empty() { n } else { self.old_frame_verts.max(0) as usize };
        let frame = frame.min((m.anim_frames.max(1) - 1) as usize);
        (0..n)
            .map(|j| {
                let src = if self.remap_anim_verts.is_empty() { j } else { self.remap_anim_verts[j] as usize };
                let raw = m.verts.get(frame * stride + src).map(|&p| unpack_vertex(p)).unwrap_or_default();
                std::array::from_fn(|k| raw[k] as f32 - m.origin[k])
            })
            .collect()
    }

    pub fn frame_positions(&self, frame: usize) -> Vec<Vec3> {
        let m = &self.mesh;
        let n = m.frame_verts.max(0) as usize;
        let stride = if self.remap_anim_verts.is_empty() { n } else { self.old_frame_verts.max(0) as usize };
        let frame = frame.min((m.anim_frames.max(1) - 1) as usize);
        // `v >> rot` for the rotator units of the engine (1/65536 turn): pitch, yaw, roll
        let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
        let (sp, cp) = ang(m.rot_origin[0]).sin_cos();
        let (sy, cy) = ang(m.rot_origin[1]).sin_cos();
        let (sr, cr) = ang(m.rot_origin[2]).sin_cos();
        let axes = [
            [cp * cy, cp * sy, sp],
            [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp],
            [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp],
        ];
        let turn = m.rot_origin != [0, 0, 0];
        (0..n)
            .map(|j| {
                let src = if self.remap_anim_verts.is_empty() { j } else { self.remap_anim_verts[j] as usize };
                let raw = unpack_vertex(m.verts[frame * stride + src]);
                let p: Vec3 = std::array::from_fn(|k| (raw[k] as f32 - m.origin[k]) * m.scale[k]);
                if turn {
                    std::array::from_fn(|k| p[0] * axes[0][k] + p[1] * axes[1][k] + p[2] * axes[2][k])
                } else {
                    p
                }
            })
            .collect()
    }
}

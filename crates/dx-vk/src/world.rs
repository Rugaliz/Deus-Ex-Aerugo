//! The world BSP drawn with Vulkan: one vertex/index buffer for all visible faces, one draw per (texture, masked) batch.
use crate::texture::{GpuTexture, RectStaging, Staging};
use crate::{Context, Offscreen, Result, DEPTH_FORMAT};
use ash::vk;
use dx_soft::{Camera, MapView};
use ue_assets::model::Model;
use std::collections::HashMap;

const PF_INVISIBLE: u32 = 0x1;
const PF_MASKED: u32 = 0x2;
const PF_MIRRORED: u32 = 0x0800_0000;
const PF_AUTO_UPAN: u32 = 0x200;
const PF_AUTO_VPAN: u32 = 0x400;
const PF_SMALL_WAVY: u32 = 0x2000;
const PF_TRANSLUCENT: u32 = 0x4;
const PF_MODULATED: u32 = 0x40;
const PF_FAKEBACKDROP: u32 = 0x80;
const PF_TWOSIDED: u32 = 0x100;
const PF_UNLIT: u32 = 0x0040_0000;

pub const WORLD_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/world.vert.spv"));
pub const WORLD_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/world.frag.spv"));

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WorldVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// Texture coordinates in texels (the fragment shader divides by the texture size).
    pub uv: [f32; 2],
    /// Bit 0: two-sided. Bit 1: lit by the lightmap (otherwise full brightness). Bit 2: `PF_SmallWavy`. Bits 8..23: the
    /// zones on both sides.
    pub flags: u32,
    /// Position in the lightmap atlas, in texels.
    pub lightmap: [f32; 2],
    /// Zone ambient light (fraction of a byte; added to the lightmap before the 2x modulation).
    pub ambient: [f32; 3],
    /// Auto-pan speed (the zone's `TexUPanSpeed`/`TexVPanSpeed` on `PF_AutoUPan`/`PF_AutoVPan` surfaces), else 0.
    pub pan: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    view_proj: [f32; 16],
    cam_pos: [f32; 4],
    misc: [u32; 4],
    /// Points with `dot(xyz, p) < w` are not drawn (the camera side of a mirror is kept); all zero = no clipping.
    clip: [f32; 4],
    /// x, y: bit mask of the zones that may be drawn (zones 0..31, 32..63); z: 1 = use it.
    vis: [u32; 4],
}

/// Which batches a pass draws.
#[derive(Clone, Copy, PartialEq)]
pub enum Pass {
    All,
    Solid,
    Blended,
}

/// The inverse of a 3x3 matrix given by its columns (`None` when singular).
pub fn mat3_inverse(c: [[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let [a, b, d] = c;
    let cr = |u: [f32; 3], v: [f32; 3]| [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let (r0, r1, r2) = (cr(b, d), cr(d, a), cr(a, b));
    let det = a[0] * r0[0] + a[1] * r0[1] + a[2] * r0[2];
    if det.abs() < 1e-12 {
        return None;
    }
    // rows of the inverse are r0, r1, r2 over det; as columns:
    Some(std::array::from_fn(|col| [r0[col] / det, r1[col] / det, r2[col] / det]))
}

/// The inverse of an affine column-major 4x4 matrix.
pub fn affine_inverse(m: &[f32; 16]) -> [f32; 16] {
    let cols = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let Some(inv) = mat3_inverse(cols) else { return *m };
    let t = [m[12], m[13], m[14]];
    let mut out = [0.0f32; 16];
    for c in 0..3 {
        out[c * 4..c * 4 + 3].copy_from_slice(&inv[c]);
    }
    for r in 0..3 {
        out[12 + r] = -(inv[0][r] * t[0] + inv[1][r] * t[1] + inv[2][r] * t[2]);
    }
    out[15] = 1.0;
    out
}

/// An affine column-major 4x4 matrix applied to a point.
pub fn affine_point(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}

/// The view-projection and eye of a frame that sees the world through the affine map `pre` (a child frame: the
/// composition of its parents' reflections and sky transforms) from the main camera.
pub fn frame_view(cam: &Camera, aspect: f32, pre: &[f32; 16]) -> ([f32; 16], [f32; 3]) {
    (crate::actors::mat_mul(&view_proj(cam, aspect, 1.0, 65536.0), pre), affine_point(&affine_inverse(pre), cam.pos))
}

/// What a mirror frame keeps (`Push::clip`): the camera's side of plane `p`.
pub fn mirror_clip(p: [f32; 4]) -> [f32; 4] {
    [p[0], p[1], p[2], p[3] - 0.5]
}

/// The view-projection and eye of the main camera reflected in mirror plane `p`, `h` above it (a mirror frame: the
/// eye as the reflected scene sees it, for one-sided surfaces).
pub fn mirror_view(cam: &Camera, aspect: f32, h: f32, p: [f32; 4]) -> ([f32; 16], [f32; 3]) {
    let n = [p[0], p[1], p[2]];
    let vp = crate::actors::mat_mul(&view_proj(cam, aspect, 1.0, 65536.0), &reflect_matrix(n, p[3]));
    (vp, [cam.pos[0] - 2.0 * h * n[0], cam.pos[1] - 2.0 * h * n[1], cam.pos[2] - 2.0 * h * n[2]])
}

/// Reflection across the plane `dot(n, x) = d` as a column-major matrix.
pub fn reflect_matrix(n: [f32; 3], d: f32) -> [f32; 16] {
    let mut m = [0.0; 16];
    for c in 0..3 {
        for r in 0..3 {
            m[c * 4 + r] = if r == c { 1.0 } else { 0.0 } - 2.0 * n[r] * n[c];
        }
    }
    for r in 0..3 {
        m[12 + r] = 2.0 * d * n[r];
    }
    m[15] = 1.0;
    m
}

struct Batch {
    texture: usize,
    masked: bool,
    /// 0 opaque (and masked), 1 translucent, 2 modulated (blended surfaces are drawn after the solid ones), 3 fake
    /// backdrop (depth only, with the solid ones).
    blend: u8,
    first_index: u32,
    index_count: u32,
}

/// Column-major view-projection for the engine's camera conventions (x forward, y right, z up at zero rotation;
/// `fov` is horizontal). Clip +Y is up on screen because the viewport is flipped (see `Offscreen::begin`).
/// Half-width tangent of the view for a horizontal field of view `fov` (degrees) given for a 4:3 screen: wider screens see
/// more at the sides with the same vertical view ("Hor+", the usual widescreen behaviour); narrower ones keep the width.
pub fn hor_plus_tan(fov: f32, aspect: f32) -> f32 {
    (fov.to_radians() * 0.5).tan() * (aspect / (4.0 / 3.0)).max(1.0)
}

pub fn view_proj(cam: &Camera, aspect: f32, near: f32, far: f32) -> [f32; 16] {
    let (fwd, right, up) = cam.axes();
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let sx = 1.0 / hor_plus_tan(cam.fov, aspect);
    let sy_ = sx * aspect;
    let q = far / (far - near);
    // rows of clip = M * (p, 1)
    let rows = [
        [sx * right[0], sx * right[1], sx * right[2], -sx * dot(right, cam.pos)],
        [sy_ * up[0], sy_ * up[1], sy_ * up[2], -sy_ * dot(up, cam.pos)],
        [q * fwd[0], q * fwd[1], q * fwd[2], -q * dot(fwd, cam.pos) - q * near],
        [fwd[0], fwd[1], fwd[2], -dot(fwd, cam.pos)],
    ];
    let mut m = [0.0; 16];
    for r in 0..4 {
        for c in 0..4 {
            m[c * 4 + r] = rows[r][c];
        }
    }
    m
}

/// Where a brush model stands in the world: an affine frame, `world = origin + cols[0] * p.x + cols[1] * p.y +
/// cols[2] * p.z`. A mover's comes from `ABrush::ToWorld` (rotation, `MainScale`/`PostScale` with sheer, `PrePivot`):
/// the moving-brush tracker (Engine.dll 1037a400) puts every brush polygon through `ABrush::BuildCoords` and
/// `FPoly::Transform` (10387360), which is the same frame; texture vectors and normals go through its inverse, so the
/// model-space texture mapping and one-sided test hold unchanged.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub origin: [f32; 3],
    pub cols: [[f32; 3]; 3],
}

impl Placement {
    /// Location, rotation and pre-pivot only: `world = location + R * (p - pre_pivot)`.
    pub fn rigid(location: [f32; 3], rotation: [i32; 3], pre_pivot: [f32; 3]) -> Placement {
        let cols = crate::actors::rotation_axes(rotation);
        let origin = std::array::from_fn(|r| location[r] - (0..3).map(|c| cols[c][r] * pre_pivot[c]).sum::<f32>());
        Placement { origin, cols }
    }

    /// From an engine `FCoords` used with `TransformPointBy` (`world_i = (p - origin) . axis_i`).
    pub fn from_coords(origin: [f32; 3], x: [f32; 3], y: [f32; 3], z: [f32; 3]) -> Placement {
        let rows = [x, y, z];
        let cols: [[f32; 3]; 3] = std::array::from_fn(|c| std::array::from_fn(|r| rows[r][c]));
        let o = std::array::from_fn(|r| -(rows[r][0] * origin[0] + rows[r][1] * origin[1] + rows[r][2] * origin[2]));
        Placement { origin: o, cols }
    }

    /// Column-major model matrix.
    pub fn matrix(&self) -> [f32; 16] {
        let mut m = [0.0; 16];
        for (c, a) in self.cols.iter().enumerate() {
            m[c * 4..c * 4 + 3].copy_from_slice(a);
        }
        m[12..15].copy_from_slice(&self.origin);
        m[15] = 1.0;
        m
    }

    /// The inverse of the linear part (rows of the inverse), in double precision.
    fn inverse(&self) -> [[f64; 3]; 3] {
        let a: [[f64; 3]; 3] = std::array::from_fn(|r| std::array::from_fn(|c| self.cols[c][r] as f64));
        let cof = |r: usize, c: usize| {
            let (r1, r2, c1, c2) = ((r + 1) % 3, (r + 2) % 3, (c + 1) % 3, (c + 2) % 3);
            a[r1][c1] * a[r2][c2] - a[r1][c2] * a[r2][c1]
        };
        let det = a[0][0] * cof(0, 0) + a[0][1] * cof(0, 1) + a[0][2] * cof(0, 2);
        let det = if det.abs() < 1e-12 { 1e-12 } else { det };
        std::array::from_fn(|r| std::array::from_fn(|c| cof(c, r) / det))
    }

    /// A world direction in the model's own space (unit length).
    pub fn dir_to_local(&self, d: [f32; 3]) -> [f32; 3] {
        let inv = self.inverse();
        let v: [f64; 3] = std::array::from_fn(|r| inv[r][0] * d[0] as f64 + inv[r][1] * d[1] as f64 + inv[r][2] * d[2] as f64);
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let len = if len > 0.0 { len } else { 1.0 };
        std::array::from_fn(|k| (v[k] / len) as f32)
    }

    /// A world point in the model's own space.
    pub fn to_local(&self, p: [f32; 3]) -> [f32; 3] {
        let inv = self.inverse();
        let d: [f64; 3] = std::array::from_fn(|k| p[k] as f64 - self.origin[k] as f64);
        std::array::from_fn(|r| (inv[r][0] * d[0] + inv[r][1] * d[1] + inv[r][2] * d[2]) as f32)
    }
}

/// What drawing a surface needs besides its frame: texture slot, flags, its lightmap's place in the atlas and pans.
#[derive(Clone, Copy)]
struct SurfDraw {
    slot: usize,
    masked: bool,
    blend: u8,
    poly_flags: u32,
    sm: Option<dx_light::SurfaceMap>,
    pan: [i16; 2],
}

/// A piece of a mover's surface as the moving-brush tracker filtered it into the level (a dynamic BSP node): which
/// surface of the mover's model (brush polygon), the node's polygon and the surface's frame in world space, and the
/// node's zones.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub surf: usize,
    pub verts: Vec<[f32; 3]>,
    pub base: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
    pub normal: [f32; 3],
    pub zones: [u8; 2],
}

/// One fragment's draw: texture slot, masked, blend and its index range.
#[derive(Clone, Copy)]
struct FragDraw {
    slot: usize,
    masked: bool,
    blend: u8,
    first: u32,
    count: u32,
}

/// The fragments drawn this frame (a mover drawn through the lists) and their geometry.
struct Fragments {
    input: Vec<Fragment>,
    buffers: Option<(crate::Buffer, crate::Buffer)>,
    draws: Vec<FragDraw>,
}

/// The vertices of a polygon of surface `d` with frame `base`/`tu`/`tv`/`normal` in zones `zones` (the zone ambient and
/// auto-pan of the back zone's side, `zones[1]`).
#[allow(clippy::too_many_arguments)]
fn surface_vertices(points: &[[f32; 3]], d: &SurfDraw, base: [f32; 3], tu: [f32; 3], tv: [f32; 3], normal: [f32; 3], zones: [u8; 2], zone_ambient: &[[f32; 3]], zone_pan: &[[f32; 2]]) -> Vec<WorldVertex> {
    let sm = d.sm.filter(|_| d.poly_flags & PF_UNLIT == 0);
    // bits 8..23: the zones on both sides of the surface (for the sky pass, which draws the sky zone only)
    let flags = (d.poly_flags & PF_TWOSIDED != 0) as u32 | (sm.is_some() as u32) << 1 | ((d.poly_flags & PF_SMALL_WAVY != 0) as u32) << 2 | (zones[0] as u32) << 8 | (zones[1] as u32) << 16;
    let zone = zones[1] as usize;
    // the zone ambient is the start value of the lightmap texels (`FLightManager`); only faces without one add it
    let ambient = if sm.is_some() { [0.0; 3] } else { zone_ambient.get(zone).copied().unwrap_or([0.0; 3]) };
    let zp = zone_pan.get(zone).copied().unwrap_or([0.0; 2]);
    let pan = [if d.poly_flags & PF_AUTO_UPAN != 0 { zp[0] } else { 0.0 }, if d.poly_flags & PF_AUTO_VPAN != 0 { zp[1] } else { 0.0 }];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    points
        .iter()
        .map(|&p| {
            let rel = [p[0] - base[0], p[1] - base[1], p[2] - base[2]];
            let (a, b) = (dot(rel, tu), dot(rel, tv));
            let lightmap = sm.map(|s| s.atlas_pos(a, b)).unwrap_or([0.0; 2]);
            WorldVertex { pos: p, normal, uv: [a + d.pan[0] as f32, b + d.pan[1] as f32], flags, lightmap, ambient, pan }
        })
        .collect()
}

pub struct World {
    vertices: crate::Buffer,
    indices: crate::Buffer,
    batches: Vec<Batch>,
    textures: Vec<GpuTexture>,
    /// Per texture slot: a mod's replacement picture's scale (`replacement_scale`).
    uv_scale: Vec<u32>,
    /// The light blockers of this model for ray queries (its own space), with ray tracing.
    pub rt_blas: Option<crate::rt::Accel>,
    /// Animated surface textures: head slot -> the slots of its chain (head first), the head's key -> slot, and the
    /// frame each head shows now (index into its chain).
    anim: HashMap<usize, Vec<usize>>,
    anim_keys: HashMap<(String, usize), usize>,
    anim_frame: HashMap<usize, usize>,
    /// Procedural textures among them: slot, texture key, staging memory for the per-frame pixels.
    dynamic: Vec<(usize, (String, usize), Staging)>,
    sets: Vec<vk::DescriptorSet>,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sampler: vk::Sampler,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    /// Splitting planes, children and zones of the BSP nodes (to find the camera's zone).
    nodes: Vec<([f32; 4], i32, i32, [u8; 2])>,
    /// Per zone: the zones visible from it (`FZoneProperties::Visibility`).
    zone_visibility: Vec<u64>,
    /// Planes of the mirror surfaces (`PF_Mirrored`): `xyz` normal toward the visible side, `w` distance.
    pub mirror_planes: Vec<[f32; 4]>,
    /// Per mirror plane: the first index and the index count of its faces (drawn with `mask`).
    mirror_ranges: Vec<(u32, u32)>,
    /// The blended (translucent, modulated) faces by BSP node: batch index (texture, blend) and their index range.
    blended_nodes: HashMap<u32, (usize, u32, u32)>,
    /// The blended surfaces of the main view in the order `DrawFrame` draws its third list (the visibility walk's
    /// non-occluding pieces, newest first), as BSP nodes; `None` draws every blended batch (no walk result).
    pub blended_order: Option<Vec<u32>>,
    /// The main view's first list (mirrors that opened a child frame, drawn after them) in `DrawFrame`'s order.
    pub mirror_order: Option<Vec<u32>>,
    /// Translucent mirrors the walk drew solid (list 1 with `PF_Translucent` cleared, render device `ShinySurfaces`
    /// off): their faces drawn opaque with the solid world.
    pub solid_mirror_order: Option<Vec<u32>>,
    /// The main frame's level nodes from its walk (lists 1 and 2, movers left out): `DrawFrame` draws the solid and
    /// masked faces of these only, never the rest of the visible zones (surfaces behind a fake backdrop, such as the
    /// sky zone's own room, are not on them). `None`: no walk, the whole visible level.
    pub main_solid: Option<Vec<u32>>,
    mask: vk::Pipeline,
    /// The level's `TimeSeconds` for auto-panning surfaces (set by the caller before drawing).
    pub time: f32,
    translucent: vk::Pipeline,
    modulated: vk::Pipeline,
    backdrop: vk::Pipeline,
    pub face_count: usize,
    lightmap: Option<(GpuTexture, vk::DescriptorSet, vk::Sampler)>,
    /// Draw with lightmaps (default when they exist); false falls back to orientation shading.
    pub use_lightmaps: bool,
    /// The view's `RendMap` is not the lit one (5): `DrawFrame` sets up no light map for any surface (`SetupForSurf`
    /// only for `RendMap == 5`), so the device draws the plain textures (the `Matrix` cheat's `RMODE 6`).
    pub plain_textures: bool,
    /// Factor applied to lightmap + ambient (each as a fraction of 255) before it multiplies the texture. 4 is what the
    /// original OpenGL driver does: it doubles every byte of a 7-bit lightmap on upload and then modulates by 2
    /// (`GL_RGB_SCALE` = 2), so a texel value of 64 is neutral and 127 is 2x overbright.
    pub light_scale: f32,
    /// (atlas width, height, surfaces with a lightmap) when the map is lit.
    pub lightmap_info: Option<(u32, u32, usize)>,
    lightmap_pool: vk::DescriptorPool,
    /// Lightmap rectangles redone this frame (the light manager's per-frame lights), uploaded before drawing.
    lightmap_updates: Option<RectStaging>,
    /// The fog maps: an atlas laid out as the lightmap atlas (zero = no fog), its set (set 2) and pending rectangles.
    fog: (GpuTexture, vk::DescriptorSet),
    /// The fog atlases of child frames (by the walk's frame index): a frame's fog maps are made with its own camera.
    fog_frames: HashMap<usize, FogAtlas>,
    /// A fog texture without fog, for frames that have none.
    fog_zero: (GpuTexture, vk::DescriptorPool, vk::DescriptorSet),
    /// The fog atlas size (the lightmap atlas's).
    fog_size: (u32, u32),
    /// The fog set the draws bind now (`use_fog`).
    fog_bind: std::cell::Cell<vk::DescriptorSet>,
    fog_updates: Option<RectStaging>,
    /// Draw the fog maps (default on; off for comparisons).
    pub fog_on: bool,
    /// Per surface of the model: how to draw it (`None`: invisible or without a texture).
    surf_draws: Vec<Option<SurfDraw>>,
    zone_ambient: Vec<[f32; 3]>,
    zone_pan: Vec<[f32; 2]>,
    /// A mover drawn through the lists: the fragments the visibility walk drew this frame, instead of the whole model.
    fragments: Option<Fragments>,
    /// The opaque mirrors' faces (not in the solid pass): plane index and batch.
    opaque_mirrors: Vec<(usize, Batch)>,
    depth_far: vk::Pipeline,
    mask_always: vk::Pipeline,
    mask_equal: vk::Pipeline,
    /// The solid faces by BSP node (a child frame draws the nodes of its own lists).
    solid_nodes: HashMap<u32, Vec<NodeDraw>>,
}

/// One face's draw: texture slot, masked, blend (0 solid, 3 fake backdrop) and its index range.
#[derive(Clone, Copy)]
struct NodeDraw {
    slot: usize,
    masked: bool,
    blend: u8,
    first: u32,
    count: u32,
    /// An opaque mirror's plane (index into `mirror_planes`).
    plane: Option<usize>,
}

/// The faces through which a child frame is seen: a mirror plane's, or the fake backdrops (the sky zone).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Opening {
    Mirror([f32; 4]),
    Backdrop,
}

/// What drawing the opening faces does: mark the stencil (no colour, no depth), forget the depth under them, or
/// write their depth only (an opaque mirror or a backdrop drawn invisible over what it shows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FaceUse {
    Mask,
    ResetDepth,
    Depth,
    /// Like `Mask` but whatever the depth (to give the stencil area back after `Depth`).
    Unmask,
    /// Like `Mask` but also where the depth is the faces' own (marking openings already drawn into the depth).
    MaskEqual,
}

impl World {
    /// `lighting`: the map's lights and zone ambients (see `dx_light::collect`); without it the world is drawn unlit.
    pub fn new(cx: &Context, view: &MapView, lighting: Option<&dx_light::MapLighting>) -> Result<World> {
        World::from_model(cx, view, &view.model, lighting)
    }

    /// Any model of the map package: the world, or the brush of a mover (textures are looked up through `view`).
    pub fn from_model(cx: &Context, view: &MapView, model: &Model, lighting: Option<&dx_light::MapLighting>) -> Result<World> {
        let maps = lighting.map(|l| dx_light::generate(model, l));
        World::build(cx, view, model, lighting, maps)
    }

    /// As `from_model` with lightmaps made by the caller (`lighting` then only supplies the zone ambients).
    pub fn build(cx: &Context, view: &MapView, model: &Model, lighting: Option<&dx_light::MapLighting>, maps: Option<dx_light::Lightmaps>) -> Result<World> {
        // 1. geometry, grouped by texture
        let mut slots: HashMap<(String, usize), usize> = HashMap::new();
        let mut tex_raw: Vec<i32> = Vec::new();
        struct F {
            slot: usize,
            masked: bool,
            blend: u8,
            verts: Vec<WorldVertex>,
            node: usize,
            /// For a mirror surface: the index of its plane in `mirror_planes`.
            mirror: Option<usize>,
            /// An opaque mirror's plane: drawn only where no reflection is (`DrawFrame` draws an opaque mirror that
            /// opened a child frame invisible: depth only).
            opaque: Option<usize>,
        }
        let mut faces: Vec<F> = Vec::new();
        let mut mirror_planes: Vec<[f32; 4]> = Vec::new();
        let model_faces = model.faces();
        let mut surf_draws: Vec<Option<SurfDraw>> = vec![None; model.surfs.len()];
        let zone_ambient: Vec<[f32; 3]> = lighting.map(|l| l.zone_ambient.clone()).unwrap_or_default();
        let zone_pan: Vec<[f32; 2]> = lighting.map(|l| l.zone_pan.clone()).unwrap_or_default();
        for face in &model_faces {
            let surf = &model.surfs[face.surf];
            // zone portals are drawn unless they are also invisible (a water surface is a visible portal between the air
            // and the water zone; Render.dll draws a portal surface itself when PF_Invisible is clear)
            if surf.poly_flags & PF_INVISIBLE != 0 {
                continue;
            }
            let Some(key) = view.texture_key(surf.texture) else { continue };
            let slot = *slots.entry(key).or_insert_with(|| {
                tex_raw.push(surf.texture);
                tex_raw.len() - 1
            });
            let m = model;
            let normal = m.vectors.get(surf.v_normal as usize).copied().unwrap_or([0.0, 0.0, 1.0]);
            let base = m.points.get(surf.p_base as usize).copied().unwrap_or([0.0; 3]);
            let tu = m.vectors.get(surf.v_texture_u as usize).copied().unwrap_or([1.0, 0.0, 0.0]);
            let tv = m.vectors.get(surf.v_texture_v as usize).copied().unwrap_or([0.0, 1.0, 0.0]);
            let sm = maps.as_ref().and_then(|m| m.surfaces[face.surf]);
            let zones = m.nodes[face.node].i_zone;
            // a fake backdrop shows the sky drawn before the world and hides what lies behind it: depth only
            let blend = if surf.poly_flags & PF_FAKEBACKDROP != 0 {
                3
            } else if surf.poly_flags & PF_TRANSLUCENT != 0 {
                1
            } else if surf.poly_flags & PF_MODULATED != 0 {
                2
            } else {
                0
            };
            let d = SurfDraw { slot, masked: surf.poly_flags & PF_MASKED != 0, blend, poly_flags: surf.poly_flags, sm, pan: [surf.pan_u, surf.pan_v] };
            surf_draws[face.surf] = Some(d);
            let verts = surface_vertices(&face.vertices, &d, base, tu, tv, normal, zones, &zone_ambient, &zone_pan);
            let mut mirror = None;
            if surf.poly_flags & PF_MIRRORED != 0 {
                // the plane of the surface, facing its visible side
                let dist = normal[0] * base[0] + normal[1] * base[1] + normal[2] * base[2];
                let plane = [normal[0], normal[1], normal[2], dist];
                mirror = Some(match mirror_planes.iter().position(|q| (0..4).all(|k| (q[k] - plane[k]).abs() < 1e-3)) {
                    Some(i) => i,
                    None => {
                        mirror_planes.push(plane);
                        mirror_planes.len() - 1
                    }
                });
            }
            faces.push(F { slot, masked: d.masked, blend, verts, node: face.node, mirror, opaque: mirror.filter(|_| blend == 0) });
        }
        // the light blockers for ray queries (ray-traced shadows and ambient occlusion): opaque, unmasked faces
        let rt_blas = if cx.quality().ray_traced() {
            let (mut pos, mut idx): (Vec<[f32; 3]>, Vec<u32>) = (Vec::new(), Vec::new());
            for f in faces.iter().filter(|f| f.blend == 0 && !f.masked && f.verts.len() >= 3) {
                let base = pos.len() as u32;
                pos.extend(f.verts.iter().map(|v| v.pos));
                for k in 1..f.verts.len() as u32 - 1 {
                    idx.extend_from_slice(&[base, base + k, base + k + 1]);
                }
            }
            crate::rt::static_blas(cx, &pos, &idx)?
        } else {
            None
        };
        let face_count = faces.len();
        faces.sort_by_key(|f| (f.opaque.is_some(), f.opaque, f.blend, f.slot, f.masked));
        let mut vertices: Vec<WorldVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut batches: Vec<Batch> = Vec::new();
        let mut blended_nodes: HashMap<u32, (usize, u32, u32)> = HashMap::new();
        let mut opaque_mirrors: Vec<(usize, Batch)> = Vec::new();
        let mut solid_nodes: HashMap<u32, Vec<NodeDraw>> = HashMap::new();
        for f in &faces {
            if f.verts.len() < 3 {
                continue;
            }
            // every solid face (opaque mirrors drawn plain) by node, for a child frame's own lists
            if f.blend == 0 || f.blend == 3 {
                solid_nodes.entry(f.node as u32).or_default().push(NodeDraw { slot: f.slot, masked: f.masked, blend: f.blend, first: indices.len() as u32, count: (f.verts.len() as u32 - 2) * 3, plane: f.opaque });
            }
            if let Some(plane) = f.opaque {
                if opaque_mirrors.last().map(|(p, b)| (*p, b.texture, b.masked) != (plane, f.slot, f.masked)).unwrap_or(true) {
                    opaque_mirrors.push((plane, Batch { texture: f.slot, masked: f.masked, blend: 0, first_index: indices.len() as u32, index_count: 0 }));
                }
                let base = vertices.len() as u32;
                vertices.extend_from_slice(&f.verts);
                for k in 1..f.verts.len() as u32 - 1 {
                    indices.extend_from_slice(&[base, base + k, base + k + 1]);
                }
                let b = &mut opaque_mirrors.last_mut().unwrap().1;
                b.index_count = indices.len() as u32 - b.first_index;
                continue;
            }
            if batches.last().map(|b| (b.texture, b.masked, b.blend) != (f.slot, f.masked, f.blend)).unwrap_or(true) {
                batches.push(Batch { texture: f.slot, masked: f.masked, blend: f.blend, first_index: indices.len() as u32, index_count: 0 });
            }
            if f.blend == 1 || f.blend == 2 {
                blended_nodes.insert(f.node as u32, (batches.len() - 1, indices.len() as u32, (f.verts.len() as u32 - 2) * 3));
            }
            let base = vertices.len() as u32;
            vertices.extend_from_slice(&f.verts);
            for k in 1..f.verts.len() as u32 - 1 {
                indices.extend_from_slice(&[base, base + k, base + k + 1]);
            }
            batches.last_mut().unwrap().index_count = indices.len() as u32 - batches.last().unwrap().first_index;
        }
        // the triangles of each mirror plane's faces again, after everything else (to mark where reflections show)
        let mut mirror_ranges = Vec::new();
        for plane in 0..mirror_planes.len() {
            let first = indices.len() as u32;
            let mut base = 0u32;
            for f in &faces {
                let n = f.verts.len() as u32;
                if n >= 3 && f.mirror == Some(plane) {
                    for k in 1..n - 1 {
                        indices.extend_from_slice(&[base, base + k, base + k + 1]);
                    }
                }
                if n >= 3 {
                    base += n;
                }
            }
            mirror_ranges.push((first, indices.len() as u32 - first));
        }

        // 2. textures; an undecodable one becomes a 1x1 magenta so it is visible
        let mut textures = Vec::new();
        let mut dynamic = Vec::new();
        // per texture slot: a mod's replacement picture's size over the texture's own, as f32 bits (0: the same)
        let mut uv_scale: Vec<u32> = Vec::new();
        for &raw in &tex_raw {
            // a procedural texture: one level at its own size, refreshed every frame (`update_procedurals`)
            if let Some(key) = view.texture_key(raw).filter(|k| view.is_procedural(k)) {
                if let Some((w, h, rgba)) = view.procedural_initial(&key) {
                    dynamic.push((textures.len(), key, Staging::new(cx, w, h)?));
                    textures.push(GpuTexture::new(cx, &[(w, h, rgba)])?);
                    continue;
                }
            }
            // keep the mip chain up to the first level whose pixel data is missing or the wrong size
            let mips = view
                .texture_mips(raw)
                .map(|m| m.into_iter().take_while(|(w, h, px)| px.len() == (*w * *h * 4) as usize).collect::<Vec<_>>())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| vec![(1, 1, vec![255, 0, 255, 255])]);
            uv_scale.resize(textures.len(), 0);
            uv_scale.push(replacement_scale(mips[0].0, view.texture_size(raw)));
            textures.push(GpuTexture::new(cx, &mips)?);
        }
        // animated textures (`AnimNext` chains): every frame after a surface texture gets a slot of its own; the
        // caller picks the frame shown (`set_anim_frame`, from the texture's `Get` on the viewport clock)
        let mut anim: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut anim_keys: HashMap<(String, usize), usize> = HashMap::new();
        for (slot, &raw) in tex_raw.iter().enumerate() {
            let chain = view.anim_chain(raw);
            if chain.is_empty() {
                continue;
            }
            let mut slots = vec![slot];
            for key in &chain {
                let mips = view
                    .texture_mips_by_key(key)
                    .map(|m| m.into_iter().take_while(|(w, h, px)| px.len() == (*w * *h * 4) as usize).collect::<Vec<_>>())
                    .filter(|m| !m.is_empty())
                    .unwrap_or_else(|| vec![(1, 1, vec![255, 0, 255, 255])]);
                slots.push(textures.len());
                uv_scale.resize(textures.len(), 0);
                uv_scale.push(replacement_scale(mips[0].0, view.texture_size_by_key(key)));
                textures.push(GpuTexture::new(cx, &mips)?);
            }
            if let Some(k) = view.texture_key(raw) {
                anim_keys.insert(k, slot);
            }
            anim.insert(slot, slots);
        }

        unsafe {
            let sampler = cx.device.create_sampler(
                &cx.texture_sampler_info(false),
                None,
            )?;
            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = cx.device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None)?;
            let n = textures.len().max(1) as u32;
            let sizes = [vk::DescriptorPoolSize::default().ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(n)];
            let pool = cx.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(n).pool_sizes(&sizes), None)?;
            let layouts = vec![set_layout; textures.len()];
            let sets = if textures.is_empty() {
                Vec::new()
            } else {
                cx.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts))?
            };
            for (set, tex) in sets.iter().zip(&textures) {
                let info = [vk::DescriptorImageInfo::default()
                    .sampler(sampler)
                    .image_view(tex.view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                cx.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default().dst_set(*set).dst_binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&info)],
                    &[],
                );
            }

            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(std::mem::size_of::<Push>() as u32)];
            // set 1: the lightmap atlas (always bound; a 1x1 black texture when the map has none)
            let lm_sampler = cx.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;
            let (lm_tex, lightmap_info) = match &maps {
                Some(m) => (GpuTexture::new(cx, &[(m.width, m.height, m.rgba.clone())])?, Some((m.width, m.height, m.surfaces.iter().flatten().count()))),
                None => (GpuTexture::new(cx, &[(1, 1, vec![0, 0, 0, 255])])?, None),
            };
            // set 2: the fog atlas (same size and places as the lightmap atlas), all zero until a surface has fog
            let fog_tex = match &maps {
                Some(m) => GpuTexture::new(cx, &[(m.width, m.height, vec![0; (m.width * m.height * 4) as usize])])?,
                None => GpuTexture::new(cx, &[(1, 1, vec![0; 4])])?,
            };
            let lm_pool_sizes = [vk::DescriptorPoolSize::default().ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(2)];
            let lm_pool = cx.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(2).pool_sizes(&lm_pool_sizes), None)?;
            let lm_layouts = [set_layout, set_layout];
            let lm_sets = cx.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(lm_pool).set_layouts(&lm_layouts))?;
            let (lm_set, fog_set) = (lm_sets[0], lm_sets[1]);
            let lm_info = [vk::DescriptorImageInfo::default().sampler(lm_sampler).image_view(lm_tex.view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let fog_info = [vk::DescriptorImageInfo::default().sampler(lm_sampler).image_view(fog_tex.view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            cx.device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default().dst_set(lm_set).dst_binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&lm_info),
                    vk::WriteDescriptorSet::default().dst_set(fog_set).dst_binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&fog_info),
                ],
                &[],
            );
            let fog_zero_tex = GpuTexture::new(cx, &[(1, 1, vec![0; 4])])?;
            let (fog_zero_pool, fog_zero_set) = make_fog_set(cx, set_layout, lm_sampler, &fog_zero_tex)?;
            let fog_size = maps.as_ref().map(|m| (m.width, m.height)).unwrap_or((1, 1));
            let set_layouts = [set_layout, set_layout, set_layout];
            let layout = cx.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts).push_constant_ranges(&push), None)?;
            let vs = cx.shader_module(WORLD_VERT)?;
            let fs = cx.shader_module(WORLD_FRAG)?;
            // the picture options as specialization constants; the opaque pipeline with multisampling also draws masked
            // texels by alpha-to-coverage
            let q = cx.quality();
            let a2c = q.msaa > 1;
            let spec_map = crate::SCENE_SPEC;
            let (data, data_a2c) = (crate::scene_spec_data(q, false), crate::scene_spec_data(q, true));
            let spec = vk::SpecializationInfo::default().map_entries(&spec_map).data(&data);
            let spec_a2c = vk::SpecializationInfo::default().map_entries(&spec_map).data(&data_a2c);
            let stages = [
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vs).name(c"main"),
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fs).name(c"main").specialization_info(&spec),
            ];
            let stages_a2c = [stages[0], stages[1].specialization_info(&spec_a2c)];
            let ms_a2c = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(cx.samples()).alpha_to_coverage_enable(true);
            let bindings = [vk::VertexInputBindingDescription::default().stride(std::mem::size_of::<WorldVertex>() as u32).input_rate(vk::VertexInputRate::VERTEX)];
            let attrs = [
                vk::VertexInputAttributeDescription::default().location(0).format(vk::Format::R32G32B32_SFLOAT).offset(0),
                vk::VertexInputAttributeDescription::default().location(1).format(vk::Format::R32G32B32_SFLOAT).offset(12),
                vk::VertexInputAttributeDescription::default().location(2).format(vk::Format::R32G32_SFLOAT).offset(24),
                vk::VertexInputAttributeDescription::default().location(3).format(vk::Format::R32_UINT).offset(32),
                vk::VertexInputAttributeDescription::default().location(4).format(vk::Format::R32G32_SFLOAT).offset(36),
                vk::VertexInputAttributeDescription::default().location(5).format(vk::Format::R32G32B32_SFLOAT).offset(44),
                vk::VertexInputAttributeDescription::default().location(6).format(vk::Format::R32G32_SFLOAT).offset(56),
            ];
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&bindings).vertex_attribute_descriptions(&attrs);
            let input_asm = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
            let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
            let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(cx.samples());
            // blended surfaces use the drivers' factors and do not write depth
            let make_full = |factors: Option<(vk::BlendFactor, vk::BlendFactor)>, color: bool, depth_write: bool, compare: vk::CompareOp, a2c: bool| -> Result<vk::Pipeline> {
                let (stages, ms) = if a2c { (&stages_a2c, &ms_a2c) } else { (&stages, &ms) };
                let ds = vk::PipelineDepthStencilStateCreateInfo::default()
                    .depth_test_enable(true)
                    .depth_write_enable(depth_write)
                    .depth_compare_op(compare);
                let mut att = vk::PipelineColorBlendAttachmentState::default().color_write_mask(if color { vk::ColorComponentFlags::RGBA } else { vk::ColorComponentFlags::empty() });
                if let Some((src, dst)) = factors {
                    att = att
                        .blend_enable(true)
                        .src_color_blend_factor(src)
                        .dst_color_blend_factor(dst)
                        .color_blend_op(vk::BlendOp::ADD)
                        .src_alpha_blend_factor(vk::BlendFactor::ZERO)
                        .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                        .alpha_blend_op(vk::BlendOp::ADD);
                }
                let blend_att = [att];
                let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_att);
                let dyn_states = crate::SCENE_DYNAMIC_STATES;
                let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
                let color_formats = [cx.color_format()];
                let mut rendering = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats).depth_attachment_format(DEPTH_FORMAT).stencil_attachment_format(DEPTH_FORMAT);
                let info = vk::GraphicsPipelineCreateInfo::default()
                    .stages(stages)
                    .vertex_input_state(&vertex_input)
                    .input_assembly_state(&input_asm)
                    .viewport_state(&viewport)
                    .rasterization_state(&raster)
                    .multisample_state(ms)
                    .depth_stencil_state(&ds)
                    .color_blend_state(&blend)
                    .dynamic_state(&dynamic)
                    .layout(layout)
                    .push_next(&mut rendering);
                Ok(cx.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None).map_err(|(_, e)| e)?[0])
            };
            let make_with = |factors, color, depth_write, compare| make_full(factors, color, depth_write, compare, false);
            let make = |factors: Option<(vk::BlendFactor, vk::BlendFactor)>, color: bool, depth_write: bool| make_with(factors, color, depth_write, vk::CompareOp::LESS);
            let pipeline = make_full(None, true, true, vk::CompareOp::LESS, a2c)?;
            let translucent = make(Some((vk::BlendFactor::ONE, vk::BlendFactor::ONE_MINUS_SRC_COLOR)), true, false)?;
            let modulated = make(Some((vk::BlendFactor::DST_COLOR, vk::BlendFactor::SRC_COLOR)), true, false)?;
            let backdrop = make(None, false, true)?;
            // mirror faces into the stencil only: no colour, no depth
            let mask = make(None, false, false)?;
            // depth only, whatever is there (with a projection that puts everything at depth 1: `ClearZ` under a mirror)
            let depth_far = make_with(None, false, true, vk::CompareOp::ALWAYS)?;
            // no colour, no depth, whatever is there (giving a child frame's stencil area back)
            let mask_always = make_with(None, false, false, vk::CompareOp::ALWAYS)?;
            // no colour, no depth, where the faces are in front or are what the depth holds (the ray-traced passes'
            // openings)
            let mask_equal = make_with(None, false, false, vk::CompareOp::LESS_OR_EQUAL)?;
            cx.device.destroy_shader_module(vs, None);
            cx.device.destroy_shader_module(fs, None);

            let vb = cx.upload(&vertices, vk::BufferUsageFlags::VERTEX_BUFFER)?;
            let ib = cx.upload(&indices, vk::BufferUsageFlags::INDEX_BUFFER)?;
            Ok(World { anim, anim_keys, anim_frame: HashMap::new(), mirror_ranges, blended_nodes, blended_order: None, mirror_order: None, solid_mirror_order: None, main_solid: None, mask, vertices: vb, indices: ib, batches, textures, uv_scale, rt_blas, dynamic, sets, set_layout, pool, sampler, layout, pipeline, backdrop, mirror_planes, nodes: model.nodes.iter().map(|n| (n.plane, n.i_front, n.i_back, n.i_zone)).collect(), zone_visibility: model.zones.iter().map(|z| z.visibility).collect(), time: 0.0, translucent, modulated, face_count, lightmap: Some((lm_tex, lm_set, lm_sampler)), use_lightmaps: maps.is_some(), plain_textures: false, light_scale: 4.0, lightmap_info, lightmap_pool: lm_pool, lightmap_updates: if maps.is_some() { Some(RectStaging::new(cx, 8 << 20)?) } else { None }, fog: (fog_tex, fog_set), fog_frames: HashMap::new(), fog_zero: (fog_zero_tex, fog_zero_pool, fog_zero_set), fog_size, fog_bind: std::cell::Cell::new(fog_set), fog_updates: if maps.is_some() { Some(RectStaging::new(cx, 4 << 20)?) } else { None }, fog_on: true, surf_draws, zone_ambient, zone_pan, fragments: None, opaque_mirrors, depth_far, mask_always, mask_equal, solid_nodes })
        }
    }

    pub fn vertex_count(&self) -> u64 {
        self.vertices.size / std::mem::size_of::<WorldVertex>() as u64
    }
    pub fn batch_count(&self) -> usize {
        self.batches.len()
    }
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Records the world draw inside an active rendering scope (viewport and scissor already set).
    pub fn draw(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32) {
        self.draw_pass(cx, cb, cam, aspect, None, Pass::All);
    }

    /// The shaders' lighting mode (`Push::misc` y): 1 lightmaps, 0 orientation shading, 2 plain textures.
    fn light_mode(&self) -> u32 {
        if self.plain_textures {
            2
        } else {
            self.use_lightmaps as u32
        }
    }

    /// The zone a point lies in (`UModel::PointRegion`): descend the BSP by the side of each splitting plane.
    pub fn zone_of(&self, p: [f32; 3]) -> u8 {
        let mut zone = 0u8;
        let mut node = 0i32;
        while node >= 0 {
            let Some((plane, front_child, back_child, zones)) = self.nodes.get(node as usize) else { break };
            let front = plane[0] * p[0] + plane[1] * p[1] + plane[2] * p[2] - plane[3] >= 0.0;
            zone = zones[front as usize];
            node = if front { *front_child } else { *back_child };
        }
        zone
    }

    /// Zones that may be seen from a point (the original draws only the zones visible from the view's zone).
    pub fn visible_zones(&self, p: [f32; 3]) -> Option<u64> {
        let z = self.zone_of(p) as usize;
        self.zone_visibility.get(z).copied().filter(|&m| m != 0)
    }

    /// The world as seen from another place (the sky pass: the sky zone's own view, with the zones visible from it).
    pub fn draw_from(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32) {
        let vp = view_proj(cam, aspect, 1.0, 65536.0);
        self.record_zone(cx, cb, vp, cam.pos, [0.0; 4], Pass::All, self.visible_zones(cam.pos));
    }

    /// Like `draw`, for a model that stands somewhere else (a mover's brush).
    pub fn draw_placed(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, placement: Option<&Placement>) {
        self.draw_pass(cx, cb, cam, aspect, placement, Pass::All);
    }

    /// The solid or the blended batches only (blended surfaces are drawn after the actors).
    pub fn draw_pass(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, placement: Option<&Placement>, pass: Pass) {
        let (vp, eye) = match placement {
            Some(p) => (crate::actors::mat_mul(&view_proj(cam, aspect, 1.0, 65536.0), &p.matrix()), p.to_local(cam.pos)),
            None => (view_proj(cam, aspect, 1.0, 65536.0), cam.pos),
        };
        // the level itself: only the zones visible from the camera's zone; a placed model (mover) is always drawn
        let zones = if placement.is_none() { self.visible_zones(cam.pos) } else { None };
        if let (Pass::Blended, None, Some(order)) = (pass, placement, &self.blended_order) {
            self.record_nodes(cx, cb, vp, eye, order);
            return;
        }
        self.record_zone(cx, cb, vp, eye, [0.0; 4], pass, zones);
    }

    /// The blended faces of `nodes`, one by one in that order (`DrawFrame`'s third list).
    fn record_nodes(&self, cx: &Context, cb: vk::CommandBuffer, vp: [f32; 16], eye: [f32; 3], nodes: &[u32]) {
        let mut push = Push { view_proj: vp, cam_pos: [eye[0], eye[1], eye[2], self.light_scale], misc: [0, self.light_mode(), self.time.to_bits(), self.fog_on as u32], clip: [0.0; 4], vis: [0; 4] };
        unsafe {
            cx.device.cmd_bind_vertex_buffers(cb, 0, &[self.vertices.buffer], &[0]);
            cx.device.cmd_bind_index_buffer(cb, self.indices.buffer, 0, vk::IndexType::UINT32);
            if let Some((_, set, _)) = &self.lightmap {
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 1, &[*set], &[]);
            }
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 2, &[self.fog_bind.get()], &[]);
            let (mut bound, mut blend) = (usize::MAX, u8::MAX);
            for node in nodes {
                let Some(&(bi, first, count)) = self.blended_nodes.get(node) else { continue };
                let b = &self.batches[bi];
                if b.blend != blend {
                    blend = b.blend;
                    let p = if blend == 1 { self.translucent } else { self.modulated };
                    cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, p);
                }
                if b.texture != bound {
                    cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.sets[self.frame_slot(b.texture)]], &[]);
                    bound = b.texture;
                }
                push.misc[0] = b.masked as u32;
                push.vis[3] = self.uv_scale_of(b.texture);
                let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
                cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                cx.device.cmd_draw_indexed(cb, count, 1, first, 0, 0);
            }
        }
    }

    /// The faces of blended level nodes `nodes` drawn opaque: translucent mirrors the walk put on the solid list with
    /// their translucency cleared (`OccludeBsp`, `ShinySurfaces` off).
    pub fn draw_nodes_opaque(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, nodes: &[u32]) {
        let vp = view_proj(cam, aspect, 1.0, 65536.0);
        let mut push = Push { view_proj: vp, cam_pos: [cam.pos[0], cam.pos[1], cam.pos[2], self.light_scale], misc: [0, self.light_mode(), self.time.to_bits(), self.fog_on as u32], clip: [0.0; 4], vis: [0; 4] };
        unsafe {
            cx.device.cmd_bind_vertex_buffers(cb, 0, &[self.vertices.buffer], &[0]);
            cx.device.cmd_bind_index_buffer(cb, self.indices.buffer, 0, vk::IndexType::UINT32);
            if let Some((_, set, _)) = &self.lightmap {
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 1, &[*set], &[]);
            }
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 2, &[self.fog_bind.get()], &[]);
            cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let mut bound = usize::MAX;
            for node in nodes {
                let Some(&(bi, first, count)) = self.blended_nodes.get(node) else { continue };
                let b = &self.batches[bi];
                if b.texture != bound {
                    cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.sets[self.frame_slot(b.texture)]], &[]);
                    bound = b.texture;
                }
                push.misc[0] = b.masked as u32;
                push.vis[3] = self.uv_scale_of(b.texture);
                let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
                cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                cx.device.cmd_draw_indexed(cb, count, 1, first, 0, 0);
            }
        }
    }

    /// The blended faces of level nodes `nodes`, one by one in that order (part of `DrawFrame`'s third list).
    pub fn draw_nodes(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, nodes: &[u32]) {
        self.record_nodes(cx, cb, view_proj(cam, aspect, 1.0, 65536.0), cam.pos, nodes);
    }

    /// Draws the model through the lists from now on (`Some`: the fragments the walk drew this frame, in world space)
    /// or whole again (`None`). Only call while no submitted frame still reads the old geometry.
    pub fn set_fragments(&mut self, cx: &Context, frags: Option<Vec<Fragment>>) -> Result<()> {
        let Some(frags) = frags else {
            if let Some(f) = self.fragments.take() {
                if let Some((vb, ib)) = f.buffers {
                    cx.destroy_buffer(vb);
                    cx.destroy_buffer(ib);
                }
            }
            return Ok(());
        };
        if self.fragments.as_ref().map(|f| f.input == frags).unwrap_or(false) {
            return Ok(());
        }
        let mut vertices: Vec<WorldVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut draws = Vec::with_capacity(frags.len());
        for f in &frags {
            let first = indices.len() as u32;
            let d = self.surf_draws.get(f.surf).copied().flatten().filter(|_| f.verts.len() >= 3);
            let Some(d) = d else {
                draws.push(FragDraw { slot: 0, masked: false, blend: 0, first, count: 0 });
                continue;
            };
            let base = vertices.len() as u32;
            vertices.extend(surface_vertices(&f.verts, &d, f.base, f.u, f.v, f.normal, f.zones, &self.zone_ambient, &self.zone_pan));
            for k in 1..f.verts.len() as u32 - 1 {
                indices.extend_from_slice(&[base, base + k, base + k + 1]);
            }
            draws.push(FragDraw { slot: d.slot, masked: d.masked, blend: d.blend, first, count: indices.len() as u32 - first });
        }
        let buffers = if indices.is_empty() { None } else { Some((cx.upload(&vertices, vk::BufferUsageFlags::VERTEX_BUFFER)?, cx.upload(&indices, vk::BufferUsageFlags::INDEX_BUFFER)?)) };
        let _ = self.set_fragments(cx, None);
        self.fragments = Some(Fragments { input: frags, buffers, draws });
        Ok(())
    }

    /// Is the model drawn through the lists (`set_fragments`)?
    pub fn listed(&self) -> bool {
        self.fragments.is_some()
    }

    /// Fragments `idxs` seen through `vp` from `eye`: the solid ones (opaque, masked, fake backdrop) with `solid`, the
    /// blended ones otherwise.
    pub fn draw_fragments_vp(&self, cx: &Context, cb: vk::CommandBuffer, vp: [f32; 16], eye: [f32; 3], idxs: &[usize], solid: bool) {
        if let Some(f) = &self.fragments {
            let draws: Vec<FragDraw> = idxs.iter().filter_map(|&i| f.draws.get(i).copied()).filter(|d| (d.blend == 0 || d.blend == 3) == solid).collect();
            self.record_fragments(cx, cb, vp, eye, &draws);
        }
    }

    /// The solid (and masked) faces of level nodes `nodes` (a child frame's) through `vp` from `eye`, `clip` as in
    /// `Push::clip`; grouped by texture (with a depth buffer their order does not show).
    /// `skip`: opaque mirror planes the frame opened children for (drawn invisible over their reflection instead).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_solid_nodes_vp(&self, cx: &Context, cb: vk::CommandBuffer, vp: [f32; 16], eye: [f32; 3], clip: [f32; 4], nodes: &[u32], skip: &[[f32; 4]]) {
        let skipped = |d: &NodeDraw| d.plane.map(|i| skip.contains(&self.mirror_planes[i])).unwrap_or(false);
        let mut draws: Vec<NodeDraw> = nodes.iter().filter_map(|n| self.solid_nodes.get(n)).flatten().copied().filter(|d| !skipped(d)).collect();
        if draws.is_empty() || self.sets.is_empty() {
            return;
        }
        draws.sort_by_key(|d| (d.blend, d.slot, d.masked, d.first));
        let mut push = Push { view_proj: vp, cam_pos: [eye[0], eye[1], eye[2], self.light_scale], misc: [0, self.light_mode(), self.time.to_bits(), self.fog_on as u32], clip, vis: [0; 4] };
        unsafe {
            cx.device.cmd_bind_vertex_buffers(cb, 0, &[self.vertices.buffer], &[0]);
            cx.device.cmd_bind_index_buffer(cb, self.indices.buffer, 0, vk::IndexType::UINT32);
            if let Some((_, set, _)) = &self.lightmap {
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 1, &[*set], &[]);
            }
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 2, &[self.fog_bind.get()], &[]);
            let (mut bound, mut blend) = (usize::MAX, u8::MAX);
            let mut k = 0;
            while k < draws.len() {
                let d = draws[k];
                // runs of adjacent ranges of one texture go in one call
                let mut count = d.count;
                let mut j = k + 1;
                while j < draws.len() && (draws[j].blend, draws[j].slot, draws[j].masked) == (d.blend, d.slot, d.masked) && draws[j].first == d.first + count {
                    count += draws[j].count;
                    j += 1;
                }
                if d.blend != blend {
                    blend = d.blend;
                    cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, if blend == 3 { self.backdrop } else { self.pipeline });
                }
                if d.slot != bound {
                    cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.sets[self.frame_slot(d.slot)]], &[]);
                    bound = d.slot;
                }
                push.misc[0] = d.masked as u32;
                push.vis[3] = self.uv_scale_of(d.slot);
                let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
                cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                cx.device.cmd_draw_indexed(cb, count, 1, d.first, 0, 0);
                k = j;
            }
        }
    }

    /// The blended faces of level nodes `nodes`, in that order, seen through `vp` from `eye`.
    pub fn draw_nodes_vp(&self, cx: &Context, cb: vk::CommandBuffer, vp: [f32; 16], eye: [f32; 3], nodes: &[u32]) {
        self.record_nodes(cx, cb, vp, eye, nodes);
    }

    fn record_fragments(&self, cx: &Context, cb: vk::CommandBuffer, vp: [f32; 16], eye: [f32; 3], draws: &[FragDraw]) {
        let Some((vb, ib)) = self.fragments.as_ref().and_then(|f| f.buffers.as_ref()) else { return };
        let mut push = Push { view_proj: vp, cam_pos: [eye[0], eye[1], eye[2], self.light_scale], misc: [0, self.light_mode(), self.time.to_bits(), self.fog_on as u32], clip: [0.0; 4], vis: [0; 4] };
        unsafe {
            cx.device.cmd_bind_vertex_buffers(cb, 0, &[vb.buffer], &[0]);
            cx.device.cmd_bind_index_buffer(cb, ib.buffer, 0, vk::IndexType::UINT32);
            if let Some((_, set, _)) = &self.lightmap {
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 1, &[*set], &[]);
            }
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 2, &[self.fog_bind.get()], &[]);
            let (mut bound, mut blend) = (usize::MAX, u8::MAX);
            for d in draws.iter().filter(|d| d.count > 0) {
                if d.blend != blend {
                    blend = d.blend;
                    let p = match blend {
                        0 => self.pipeline,
                        1 => self.translucent,
                        2 => self.modulated,
                        _ => self.backdrop,
                    };
                    cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, p);
                }
                if d.slot != bound {
                    cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.sets[self.frame_slot(d.slot)]], &[]);
                    bound = d.slot;
                }
                push.misc[0] = d.masked as u32;
                push.vis[3] = self.uv_scale_of(d.slot);
                let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
                cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                cx.device.cmd_draw_indexed(cb, d.count, 1, d.first, 0, 0);
            }
        }
    }

    /// The reflection in one mirror plane the camera is in front of (`PF_Mirrored`, as the original's child views): the
    /// solid world mirrored across the plane, only what lies on the camera's side of it. `h` is the camera's height
    /// above the plane (see `mirrors_facing`). Drawn with the stencil limited to the plane's faces (`draw_mirror_mask`).
    pub fn draw_mirror(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, h: f32, p: [f32; 4]) {
        let (vp, eye) = mirror_view(cam, aspect, h, p);
        self.record_zone(cx, cb, vp, eye, mirror_clip(p), Pass::Solid, self.visible_zones(cam.pos));
    }

    /// Does mirror plane `p` have opaque faces (drawn invisible under their reflection)?
    /// The main frame's solid and masked faces of `nodes` (its walk lists); opaque mirror faces are left to the
    /// mirror passes.
    pub fn draw_main_solid(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, nodes: &[u32]) {
        let skip = self.mirror_planes.clone();
        self.draw_solid_nodes_vp(cx, cb, view_proj(cam, aspect, 1.0, 65536.0), cam.pos, [0.0; 4], nodes, &skip);
    }

    pub fn is_opaque_mirror(&self, p: [f32; 4]) -> bool {
        self.mirror_planes.iter().position(|q| *q == p).map(|i| self.opaque_mirrors.iter().any(|(k, _)| *k == i)).unwrap_or(false)
    }

    /// A child frame's opening faces seen through `vp` from `eye` (the stencil state is the caller's).
    pub fn opening_faces(&self, cx: &Context, cb: vk::CommandBuffer, vp: [f32; 16], eye: [f32; 3], which: Opening, how: FaceUse) {
        let mut vp = vp;
        let pipeline = match how {
            FaceUse::Mask => self.mask,
            FaceUse::Unmask => self.mask_always,
            FaceUse::MaskEqual => self.mask_equal,
            FaceUse::Depth => self.backdrop,
            FaceUse::ResetDepth => {
                // depth row = w row: every fragment at depth 1
                for c in 0..4 {
                    vp[c * 4 + 2] = vp[c * 4 + 3];
                }
                self.depth_far
            }
        };
        match which {
            Opening::Mirror(p) => self.record_mirror_faces(cx, cb, vp, eye, p, pipeline),
            Opening::Backdrop => {
                let backdrops: Vec<&Batch> = self.batches.iter().filter(|b| b.blend == 3).collect();
                if backdrops.is_empty() || self.sets.is_empty() {
                    return;
                }
                let push = Push { view_proj: vp, cam_pos: [eye[0], eye[1], eye[2], self.light_scale], misc: [0, self.light_mode(), self.time.to_bits(), self.fog_on as u32], clip: [0.0; 4], vis: [0; 4] };
                unsafe {
                    cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
                    cx.device.cmd_bind_vertex_buffers(cb, 0, &[self.vertices.buffer], &[0]);
                    cx.device.cmd_bind_index_buffer(cb, self.indices.buffer, 0, vk::IndexType::UINT32);
                    cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.sets[0]], &[]);
                    if let Some((_, set, _)) = &self.lightmap {
                        cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 1, &[*set], &[]);
                    }
                    cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 2, &[self.fog_bind.get()], &[]);
                    let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
                    cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                    for b in backdrops {
                        cx.device.cmd_draw_indexed(cb, b.index_count, 1, b.first_index, 0, 0);
                    }
                }
            }
        }
    }

    /// Mirror plane `p`'s faces with `pipeline` and view-projection `vp` (the stencil state is the caller's).
    fn record_mirror_faces(&self, cx: &Context, cb: vk::CommandBuffer, vp: [f32; 16], eye: [f32; 3], p: [f32; 4], pipeline: vk::Pipeline) {
        let Some(i) = self.mirror_planes.iter().position(|q| *q == p) else { return };
        let (first, count) = self.mirror_ranges[i];
        if count == 0 || self.sets.is_empty() {
            return;
        }
        let push = Push { view_proj: vp, cam_pos: [eye[0], eye[1], eye[2], self.light_scale], misc: [0, self.light_mode(), self.time.to_bits(), self.fog_on as u32], clip: [0.0; 4], vis: [0; 4] };
        unsafe {
            cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
            cx.device.cmd_bind_vertex_buffers(cb, 0, &[self.vertices.buffer], &[0]);
            cx.device.cmd_bind_index_buffer(cb, self.indices.buffer, 0, vk::IndexType::UINT32);
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.sets[0]], &[]);
            if let Some((_, set, _)) = &self.lightmap {
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 1, &[*set], &[]);
            }
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 2, &[self.fog_bind.get()], &[]);
            let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
            cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
            cx.device.cmd_draw_indexed(cb, count, 1, first, 0, 0);
        }
    }

    /// Forgets the depth where mirror plane `p` shows (use with the stencil limited to it): the faces drawn at depth 1
    /// whatever lies there (the reflected world must not be hidden by what is behind the mirror; `DrawFrame` draws the
    /// child frame before its parent, after a `ClearZ`).
    pub fn reset_mirror_depth(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, p: [f32; 4]) {
        let mut vp = view_proj(cam, aspect, 1.0, 65536.0);
        for c in 0..4 {
            vp[c * 4 + 2] = vp[c * 4 + 3];
        }
        self.record_mirror_faces(cx, cb, vp, cam.pos, p, self.depth_far);
    }

    /// Mirror plane `p`'s faces into the depth buffer only, over its reflection (an opaque mirror drawn invisible).
    pub fn draw_mirror_depth(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, p: [f32; 4]) {
        self.record_mirror_faces(cx, cb, view_proj(cam, aspect, 1.0, 65536.0), cam.pos, p, self.backdrop);
    }

    /// The opaque mirror faces of the planes not in `reflected`, drawn as plain surfaces (no reflection this frame).
    pub fn draw_unreflected_mirrors(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, reflected: &[[f32; 4]]) {
        let draws: Vec<&Batch> = self.opaque_mirrors.iter().filter(|(i, _)| !reflected.contains(&self.mirror_planes[*i])).map(|(_, b)| b).collect();
        if draws.is_empty() {
            return;
        }
        let mut push = Push { view_proj: view_proj(cam, aspect, 1.0, 65536.0), cam_pos: [cam.pos[0], cam.pos[1], cam.pos[2], self.light_scale], misc: [0, self.light_mode(), self.time.to_bits(), self.fog_on as u32], clip: [0.0; 4], vis: [0; 4] };
        unsafe {
            cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            cx.device.cmd_bind_vertex_buffers(cb, 0, &[self.vertices.buffer], &[0]);
            cx.device.cmd_bind_index_buffer(cb, self.indices.buffer, 0, vk::IndexType::UINT32);
            if let Some((_, set, _)) = &self.lightmap {
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 1, &[*set], &[]);
            }
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 2, &[self.fog_bind.get()], &[]);
            for b in draws {
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.sets[self.frame_slot(b.texture)]], &[]);
                push.misc[0] = b.masked as u32;
                push.vis[3] = self.uv_scale_of(b.texture);
                let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
                cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                cx.device.cmd_draw_indexed(cb, b.index_count, 1, b.first_index, 0, 0);
            }
        }
    }

    /// The world's own mirror plane closest to `p` (same facing, within half a unit), as `mirror_planes` holds it.
    pub fn mirror_plane_near(&self, p: [f32; 4]) -> Option<[f32; 4]> {
        self.mirror_planes.iter().copied().find(|q| q[0] * p[0] + q[1] * p[1] + q[2] * p[2] > 0.999 && (q[3] - p[3]).abs() < 0.5)
    }

    /// Like `draw_from`, one pass only (a sky frame draws its solid surfaces, then its sprites and its blended list).
    pub fn draw_from_pass(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, pass: Pass) {
        let vp = view_proj(cam, aspect, 1.0, 65536.0);
        self.record_zone(cx, cb, vp, cam.pos, [0.0; 4], pass, self.visible_zones(cam.pos));
    }

    /// Draws the faces of mirror plane `p` where they are visible (depth test against the solid world), without colour
    /// or depth: with `Stencil::Mark` this marks where its reflection may show.
    pub fn draw_mirror_mask(&self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, p: [f32; 4]) {
        let Some(i) = self.mirror_planes.iter().position(|q| *q == p) else { return };
        let (first, count) = self.mirror_ranges[i];
        if count == 0 || self.sets.is_empty() {
            return;
        }
        let zones = self.visible_zones(cam.pos);
        let vis = match zones {
            Some(m) => [m as u32, (m >> 32) as u32, 1, 0],
            None => [0; 4],
        };
        let push = Push { view_proj: view_proj(cam, aspect, 1.0, 65536.0), cam_pos: [cam.pos[0], cam.pos[1], cam.pos[2], self.light_scale], misc: [0, self.light_mode(), self.time.to_bits(), self.fog_on as u32], clip: [0.0; 4], vis };
        unsafe {
            cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.mask);
            cx.device.cmd_bind_vertex_buffers(cb, 0, &[self.vertices.buffer], &[0]);
            cx.device.cmd_bind_index_buffer(cb, self.indices.buffer, 0, vk::IndexType::UINT32);
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.sets[0]], &[]);
            if let Some((_, set, _)) = &self.lightmap {
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 1, &[*set], &[]);
            }
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 2, &[self.fog_bind.get()], &[]);
            let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
            cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
            cx.device.cmd_draw_indexed(cb, count, 1, first, 0, 0);
        }
    }

    /// The (at most 2) nearest mirror planes the camera is in front of, with the camera's height above each.
    pub fn mirrors_facing(&self, cam: &Camera) -> Vec<(f32, [f32; 4])> {
        let mut planes: Vec<(f32, [f32; 4])> = self
            .mirror_planes
            .iter()
            .map(|p| (p[0] * cam.pos[0] + p[1] * cam.pos[1] + p[2] * cam.pos[2] - p[3], *p))
            .filter(|(h, _)| *h > 0.0)
            .collect();
        planes.sort_by(|a, b| a.0.total_cmp(&b.0));
        planes.truncate(2);
        planes
    }

    /// `zones`: the zones that may be drawn (bit per zone), `None` = all.
    fn record_zone(&self, cx: &Context, cb: vk::CommandBuffer, vp: [f32; 16], eye: [f32; 3], clip: [f32; 4], pass: Pass, zones: Option<u64>) {
        let vis = match zones {
            Some(m) => [m as u32, (m >> 32) as u32, 1, 0],
            None => [0; 4],
        };
        let mut push = Push { view_proj: vp, cam_pos: [eye[0], eye[1], eye[2], self.light_scale], misc: [0, self.light_mode(), self.time.to_bits(), self.fog_on as u32], clip, vis };
        unsafe {
            cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            cx.device.cmd_bind_vertex_buffers(cb, 0, &[self.vertices.buffer], &[0]);
            cx.device.cmd_bind_index_buffer(cb, self.indices.buffer, 0, vk::IndexType::UINT32);
            if let Some((_, set, _)) = &self.lightmap {
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 1, &[*set], &[]);
            }
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 2, &[self.fog_bind.get()], &[]);
            let mut bound = usize::MAX;
            let mut blend = 0u8;
            for b in &self.batches {
                let solid = b.blend == 0 || b.blend == 3;
                if (pass == Pass::Solid && !solid) || (pass == Pass::Blended && solid) {
                    continue;
                }
                if b.blend != blend {
                    blend = b.blend;
                    let p = match blend {
                        0 => self.pipeline,
                        1 => self.translucent,
                        2 => self.modulated,
                        _ => self.backdrop,
                    };
                    cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, p);
                }
                if b.texture != bound {
                    cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.sets[self.frame_slot(b.texture)]], &[]);
                    bound = b.texture;
                }
                push.misc[0] = b.masked as u32;
                push.vis[3] = self.uv_scale_of(b.texture);
                let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
                cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                cx.device.cmd_draw_indexed(cb, b.index_count, 1, b.first_index, 0, 0);
            }
        }
    }

    /// The animated surface textures: (head key, number of frames in its chain).
    pub fn animated_textures(&self) -> Vec<((String, usize), usize)> {
        self.anim_keys.iter().map(|(k, &slot)| (k.clone(), self.anim.get(&slot).map(|v| v.len()).unwrap_or(1))).collect()
    }

    /// Shows frame `index` of an animated surface texture's chain (0 = the texture itself).
    pub fn set_anim_frame(&mut self, key: &(String, usize), index: usize) {
        if let Some(&slot) = self.anim_keys.get(key) {
            self.anim_frame.insert(slot, index);
        }
    }

    /// The replacement scale of the texture shown in `slot` (see `replacement_scale`), for the push constants.
    fn uv_scale_of(&self, slot: usize) -> u32 {
        self.uv_scale.get(self.frame_slot(slot)).copied().unwrap_or(0)
    }

    fn frame_slot(&self, slot: usize) -> usize {
        match (self.anim.get(&slot), self.anim_frame.get(&slot)) {
            (Some(chain), Some(&i)) => chain.get(i).copied().unwrap_or(slot),
            _ => slot,
        }
    }

    /// Steps the procedural textures for a frame drawn at `time` (the viewport clock, seconds) and stages their new
    /// pixels. Call after the previous frame finished on the GPU, then `record_procedural_uploads` before drawing.
    pub fn update_procedurals(&mut self, view: &mut MapView, time: f64) {
        for (_, key, staging) in &mut self.dynamic {
            if let Some((_, _, rgba)) = view.procedural_frame(key, time) {
                staging.write(rgba);
            }
        }
    }

    pub fn record_procedural_uploads(&mut self, cx: &Context, cb: vk::CommandBuffer) {
        for (slot, _, staging) in &mut self.dynamic {
            staging.record_upload(cx, cb, &self.textures[*slot]);
        }
        if let (Some(st), Some((tex, _, _))) = (&mut self.lightmap_updates, &self.lightmap) {
            st.record_upload(cx, cb, tex);
        }
        if let Some(st) = &mut self.fog_updates {
            st.record_upload(cx, cb, &self.fog.0);
        }
        for a in self.fog_frames.values_mut() {
            a.staging.record_upload(cx, cb, &a.tex);
        }
    }

    /// Makes the draws use the fog atlas of frame `frame` of the walk (0 the main view's; a child frame without fog
    /// maps of its own gets none).
    pub fn use_fog(&self, frame: usize) {
        let set = match frame {
            0 => self.fog.1,
            _ => self.fog_frames.get(&frame).map(|a| a.set).unwrap_or(self.fog_zero.2),
        };
        self.fog_bind.set(set);
    }

    /// Stages fog rectangles into the atlas of frame `frame` (0: the main view's, `update_fog`), making a child
    /// frame's atlas the first time it has fog.
    pub fn update_frame_fog(&mut self, cx: &Context, frame: usize, rects: &[(u32, u32, u32, u32, Vec<u8>)]) -> Result<()> {
        if frame == 0 {
            self.update_fog(rects);
            return Ok(());
        }
        let Some((_, _, sampler)) = self.lightmap.as_ref() else { return Ok(()) };
        if self.fog_updates.is_none() {
            return Ok(());
        }
        if !self.fog_frames.contains_key(&frame) {
            let (w, h) = self.fog_size;
            let tex = GpuTexture::new(cx, &[(w, h, vec![0; (w * h * 4) as usize])])?;
            let (pool, set) = make_fog_set(cx, self.set_layout, *sampler, &tex)?;
            let staging = RectStaging::new(cx, 4 << 20)?;
            self.fog_frames.insert(frame, FogAtlas { tex, pool, set, staging });
        }
        let a = self.fog_frames.get_mut(&frame).unwrap();
        for (x, y, w, h, rgba) in rects {
            if !a.staging.push(*x, *y, *w, *h, rgba) {
                break;
            }
        }
        Ok(())
    }

    /// Stages fog atlas rectangles (RGBA8 as the device holds them, see `dx_light::fog_atlas_rect`).
    pub fn update_fog(&mut self, rects: &[(u32, u32, u32, u32, Vec<u8>)]) {
        if let Some(st) = &mut self.fog_updates {
            for (x, y, w, h, rgba) in rects {
                if !st.push(*x, *y, *w, *h, rgba) {
                    break;
                }
            }
        }
    }

    /// Stages new texels (RGBA8) for atlas rectangles; call where the procedural textures are stepped.
    pub fn update_lightmap(&mut self, rects: &[(u32, u32, u32, u32, Vec<u8>)]) {
        if let Some(st) = &mut self.lightmap_updates {
            for (x, y, w, h, rgba) in rects {
                if !st.push(*x, *y, *w, *h, rgba) {
                    break;
                }
            }
        }
    }

    pub fn destroy(mut self, cx: &Context) {
        unsafe {
            let _ = cx.device.device_wait_idle();
        }
        let _ = self.set_fragments(cx, None);
        if let Some(a) = self.rt_blas.take() {
            a.destroy(cx);
        }
        unsafe {
            cx.device.destroy_pipeline(self.pipeline, None);
            cx.device.destroy_pipeline(self.translucent, None);
            cx.device.destroy_pipeline(self.modulated, None);
            cx.device.destroy_pipeline(self.backdrop, None);
            cx.device.destroy_pipeline(self.depth_far, None);
            cx.device.destroy_pipeline(self.mask_always, None);
            cx.device.destroy_pipeline(self.mask_equal, None);
            cx.device.destroy_pipeline(self.mask, None);
            cx.device.destroy_pipeline_layout(self.layout, None);
            cx.device.destroy_descriptor_pool(self.pool, None);
            cx.device.destroy_descriptor_set_layout(self.set_layout, None);
            cx.device.destroy_sampler(self.sampler, None);
            cx.device.destroy_descriptor_pool(self.lightmap_pool, None);
            if let Some((_, _, smp)) = &self.lightmap {
                cx.device.destroy_sampler(*smp, None);
            }
        }
        if let Some((t, _, _)) = self.lightmap {
            t.destroy(cx);
        }
        for t in self.textures {
            t.destroy(cx);
        }
        if let Some(st) = self.lightmap_updates {
            st.destroy(cx);
        }
        self.fog.0.destroy(cx);
        if let Some(st) = self.fog_updates {
            st.destroy(cx);
        }
        for (_, a) in self.fog_frames {
            unsafe { cx.device.destroy_descriptor_pool(a.pool, None) };
            a.tex.destroy(cx);
            a.staging.destroy(cx);
        }
        unsafe { cx.device.destroy_descriptor_pool(self.fog_zero.1, None) };
        self.fog_zero.0.destroy(cx);
        for (_, _, s) in self.dynamic {
            s.destroy(cx);
        }
        cx.destroy_buffer(self.vertices);
        cx.destroy_buffer(self.indices);
    }
}

/// A child frame's own fog atlas (same size and places as the lightmap atlas).
struct FogAtlas {
    tex: GpuTexture,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    staging: RectStaging,
}

/// A descriptor set of its own pool sampling `tex` (a fog texture, set 2).
fn make_fog_set(cx: &Context, layout: vk::DescriptorSetLayout, sampler: vk::Sampler, tex: &GpuTexture) -> Result<(vk::DescriptorPool, vk::DescriptorSet)> {
    unsafe {
        let sizes = [vk::DescriptorPoolSize::default().ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(1)];
        let pool = cx.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(1).pool_sizes(&sizes), None)?;
        let layouts = [layout];
        let set = cx.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts))?[0];
        let info = [vk::DescriptorImageInfo::default().sampler(sampler).image_view(tex.view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        cx.device.update_descriptor_sets(&[vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&info)], &[]);
        Ok((pool, set))
    }
}

/// Renders the world from `cam` into an offscreen target and returns RGBA8 pixels (clear colour matches `dx-soft`).
pub fn render_world(cx: &Context, world: &World, cam: &Camera, width: u32, height: u32) -> Result<Vec<u8>> {
    let target = Offscreen::new(cx, width, height)?;
    let rb = target.readback_buffer(cx)?;
    cx.one_shot(|cb| {
        target.begin(cx, cb, [24.0 / 255.0, 28.0 / 255.0, 40.0 / 255.0, 1.0]);
        world.draw(cx, cb, cam, width as f32 / height as f32);
        target.end_and_copy(cx, cb, &rb);
    })?;
    let px = target.read_pixels(cx, &rb)?;
    cx.destroy_buffer(rb);
    target.destroy(cx);
    Ok(px)
}

#[cfg(test)]
mod tests {

    #[test]
    fn affine_inverse_undoes_a_reflection_and_a_sky_map() {
        // a reflection is its own inverse
        let r = reflect_matrix([0.0, 1.0, 0.0], 1294.0);
        let p = [10.0, 1300.0, -5.0];
        let q = affine_point(&r, p);
        assert!((q[1] - 1288.0).abs() < 1e-3);
        let back = affine_point(&affine_inverse(&r), q);
        assert!((0..3).all(|k| (back[k] - p[k]).abs() < 1e-3));
        // a rotation (yaw 90) with a translation: the inverse maps back, and the 3x3 inverse is the transpose
        let rot = [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let inv = mat3_inverse(rot).unwrap();
        assert!((0..3).all(|c| (0..3).all(|r| (inv[c][r] - rot[r][c]).abs() < 1e-6)));
        let mut m = [0.0f32; 16];
        for c in 0..3 {
            m[c * 4..c * 4 + 3].copy_from_slice(&rot[c]);
        }
        m[12..15].copy_from_slice(&[100.0, -50.0, 7.0]);
        m[15] = 1.0;
        let q = affine_point(&m, p);
        let back = affine_point(&affine_inverse(&m), q);
        assert!((0..3).all(|k| (back[k] - p[k]).abs() < 1e-2), "{back:?}");
        assert!(mat3_inverse([[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
    }
    use super::*;

    fn project(m: &[f32; 16], p: [f32; 3]) -> [f32; 4] {
        let mut o = [0.0; 4];
        for r in 0..4 {
            o[r] = m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r];
        }
        o
    }

    #[test]
    fn placement_maps_local_points_to_the_world_and_back() {
        let pl = Placement::rigid([100.0, 200.0, -50.0], [0, 16384, 0], [4.0, 0.0, -8.0]);
        let m = pl.matrix();
        let local = [10.0, 20.0, 30.0];
        // world = location + R * (local - pre_pivot); a quarter turn of yaw sends +X to +Y and +Y to -X
        let world: [f32; 3] = std::array::from_fn(|r| m[r] * local[0] + m[4 + r] * local[1] + m[8 + r] * local[2] + m[12 + r]);
        let d = [local[0] - 4.0, local[1], local[2] + 8.0];
        let expect = [100.0 - d[1], 200.0 + d[0], -50.0 + d[2]];
        for k in 0..3 {
            assert!((world[k] - expect[k]).abs() < 1e-3, "{world:?} vs {expect:?}");
        }
        let back = pl.to_local(world);
        for k in 0..3 {
            assert!((back[k] - local[k]).abs() < 1e-3, "{back:?}");
        }
        let dir = pl.dir_to_local([0.0, 1.0, 0.0]); // world +Y is local +X
        assert!((dir[0] - 1.0).abs() < 1e-5 && dir[1].abs() < 1e-5);
    }

    #[test]
    fn placement_from_engine_coords_with_a_mirror() {
        // an `FCoords` for `TransformPointBy`: world_i = (p - origin) . axis_i; X axis mirrored, a quarter turn of yaw
        // (world x = -(p.y - o.y), world y = -(p.x - o.x), world z = p.z - o.z)
        let (o, x, y, z) = ([10.0, 20.0, 30.0], [0.0, -1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let pl = Placement::from_coords(o, x, y, z);
        let m = pl.matrix();
        let p = [3.0, -5.0, 7.0];
        let world: [f32; 3] = std::array::from_fn(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r]);
        assert_eq!(world, [-(p[1] - o[1]), -(p[0] - o[0]), p[2] - o[2]]);
        let back = pl.to_local(world);
        for k in 0..3 {
            assert!((back[k] - p[k]).abs() < 1e-4, "{back:?}");
        }
        // the linear part is a reflection (negative determinant): world +X is local -Y
        assert_eq!(pl.dir_to_local([1.0, 0.0, 0.0]), [0.0, -1.0, 0.0]);
        // and the rigid form is the same frame as before
        let r = Placement::rigid([100.0, 0.0, 0.0], [0, 0, 0], [1.0, 2.0, 3.0]);
        assert_eq!(r.origin, [99.0, -2.0, -3.0]);
    }

    #[test]
    fn projection_follows_engine_axes() {
        // At zero rotation the camera looks along +X with +Y to the right and +Z up (90 degree horizontal fov at 4:3).
        let cam = Camera { pos: [10.0, 0.0, 0.0], pitch: 0, yaw: 0, fov: 90.0, roll: 0 };
        let m = view_proj(&cam, 4.0 / 3.0, 1.0, 1000.0);
        let c = project(&m, [110.0, 100.0, 75.0]); // 100 ahead, 100 right, 75 up
        assert_eq!(c[3], 100.0);
        assert!((c[0] / c[3] - 1.0).abs() < 1e-5, "right edge of a 90 degree view at 45 degrees");
        assert!((c[1] / c[3] - 1.0).abs() < 1e-5, "top edge: 75/100 * aspect 4/3");
        // a wider screen keeps the same vertical view and sees more at the sides (Hor+)
        let wide = view_proj(&cam, 16.0 / 9.0, 1.0, 1000.0);
        let c = project(&wide, [110.0, 0.0, 75.0]);
        assert!((c[1] / c[3] - 1.0).abs() < 1e-5, "16:9 keeps the 4:3 vertical view");
        let c = project(&wide, [110.0, 100.0, 0.0]);
        assert!((c[0] / c[3] - 0.75).abs() < 1e-5, "16:9 shows 4/3 times as wide");
        let near = project(&m, [11.0, 0.0, 0.0]);
        assert!((near[2] / near[3]).abs() < 1e-5, "near plane -> depth 0");
        let far = project(&m, [1010.0, 0.0, 0.0]);
        assert!((far[2] / far[3] - 1.0).abs() < 1e-5, "far plane -> depth 1");
        // A quarter turn of yaw (16384 units) points the camera along +Y.
        let cam = Camera { pos: [0.0; 3], pitch: 0, yaw: 16384, fov: 90.0, roll: 0 };
        let c = project(&view_proj(&cam, 1.0, 1.0, 1000.0), [0.0, 100.0, 0.0]);
        assert!(c[0].abs() < 1e-3 && c[3] > 99.0);
    }
}

/// A mod's replacement picture `width` texels wide for a texture the game knows as `own` (`MapView::texture_size`):
/// how much bigger it is, as f32 bits for the shader (0: the texture's own picture). World texture coordinates count
/// in the game's texels, so the shader divides by the picture's size over this.
fn replacement_scale(width: u32, own: Option<(u32, u32)>) -> u32 {
    match own {
        Some((w, _)) if w > 0 && w != width => (width as f32 / w as f32).to_bits(),
        _ => 0,
    }
}

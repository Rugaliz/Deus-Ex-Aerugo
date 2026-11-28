//! The front-to-back BSP walk of `URender::OccludeBsp` (Render.dll 10b173d0) with the actor sprites carried through
//! it (`FDynamicSprite` chunks 10b23e30, final pieces 10b24740). See `docs/render-visibility.md`.
//!
//! Child frames (`URender::OccludeFrame` 10b19fc0, `CreateChildFrame` 10b15a40): fake backdrops open the sky zone
//! seen from the `SkyZone` actor, mirrors open the mirrored camera. Warp zones (`WarpZoneInfo`) open frames too in
//! the engine; no Deus Ex map has one, so they are not modelled.
//!
//! Not yet here: volumetric leaf lighting, the engine's frame-to-frame shortcuts (they do not change the result).

use crate::bound::bound_visible;
use crate::frame::{Frame, Vec3};
use crate::raster::{clip_polygon, rasterize};
use crate::span::{Span, SpanBuffer};
use ue_assets::model::Model;

pub const PF_INVISIBLE: u32 = 0x1;
pub const PF_TRANSLUCENT: u32 = 0x4;
pub const PF_FAKE_BACKDROP: u32 = 0x80;
pub const PF_TWO_SIDED: u32 = 0x100;
pub const PF_PORTAL: u32 = 0x400_0000;
pub const PF_MIRRORED: u32 = 0x800_0000;
/// Flags that let what is behind show through (invisible, masked, translucent, modulated).
const PF_SEE_THROUGH: u32 = 0x47;
/// Set by the walk on surfaces it put on a draw list through the mirror path.
const PF_FLAG_MIRROR_DRAWN: u32 = 0x8000_0000;
/// Child frames stop at this depth (`Recursion < 3`; mirrors `<= 2`).
const MAX_RECURSION: i32 = 3;

/// A zone's `SkyZone` actor: its zone, location and rotation (pitch, yaw, roll).
#[derive(Debug, Clone, Copy)]
pub struct SkyZone {
    pub zone: u8,
    pub location: Vec3,
    pub rotation: [i32; 3],
}

/// What the walk reads besides the frame: the level's BSP, the surfaces' flags, the zones' sky zones and a few
/// viewport and render device settings.
pub struct World<'a> {
    pub model: &'a Model,
    /// Poly flags of surface `i` with its texture's `PolyFlags` merged in.
    pub surf_flags: &'a dyn Fn(usize) -> u32,
    /// The `SkyZone` of zone `z` (`ZoneInfo.SkyZone`), if any.
    pub sky_zone: &'a dyn Fn(u8) -> Option<SkyZone>,
    /// `ShowFlags & SHOW_PlayerCtrl` of the viewing player (set in game views): sky and mirrors are opened only then,
    /// and invisible surfaces stay invisible.
    pub player_ctrl: bool,
    /// Render device `SpanBased` (software renderer): opaque mirrors are then not drawn at all.
    pub span_based: bool,
    /// Render device `ShinySurfaces`: translucent mirrors open a mirror frame too.
    pub shiny_surfaces: bool,
    /// Volumetric lighting this frame (`VolumetricLighting` and fog maps on the device, the viewing player in a fog
    /// zone): the leaves' volumetric lights gather as the walk goes.
    pub volumetrics: bool,
    /// The stored volumetric light at index `i` of `UModel::Lights`.
    pub vol_light: &'a dyn Fn(usize) -> Option<VolLight>,
    /// The moving-brush tracker's `SurfIsDynamic` threshold (`usize::MAX` without movers): a node of such a surface
    /// is a mover's and uses `iRenderBound` as a list link, so the walk skips its bound test (10b17ae5).
    pub first_dynamic_surf: usize,
    /// Repaired render bounds per node (`bound::repaired_render_bounds`), used instead of the stored ones when given.
    pub render_bounds: Option<&'a [Option<(Vec3, Vec3)>]>,
}
/// An actor's sprite as `FDynamicSprite::Setup` leaves it: screen box (rows `y1..y2`, columns `x1..x2`) and depth.
#[derive(Debug, Clone, Copy)]
pub struct Sprite {
    /// The caller's id (the actor).
    pub id: u32,
    /// The caller's item number (a render iterator's index; `u32::MAX` for an actor drawn as itself).
    pub item: u32,
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
    pub z: f32,
}

/// A dynamic light item (`SetupDynamics`): a light filtered down the BSP to the surfaces its sphere crosses.
#[derive(Debug, Clone, Copy)]
pub struct DynLight {
    /// The caller's id (the actor).
    pub id: u32,
    pub pos: Vec3,
    /// `WorldLightRadius`.
    pub radius: f32,
    /// `bSpecialLit`: such lights reach only `PF_SpecialLit` surfaces, the others only the rest.
    pub special_lit: bool,
    /// `VolumeRadius`, and whether the item is volumetric (`SetupDynamics`: in a fog zone, a volume radius and
    /// brightness, the volume touching the view).
    pub volume_radius: u8,
    pub volumetric: bool,
}

/// A volumetric light of a leaf's stored list (`FLeaf::iVolumetric` into `UModel::Lights`).
#[derive(Debug, Clone, Copy)]
pub struct VolLight {
    pub id: u32,
    pub pos: Vec3,
    pub volume_radius: u8,
}

/// What a frame is walked with: the actors' sprites and the dynamic lights (`SetupDynamics` runs per frame).
#[derive(Debug, Clone, Default)]
pub struct FrameInput {
    pub sprites: Vec<Sprite>,
    pub lights: Vec<DynLight>,
}

/// The light manager's cap on surfaces with a dynamic light list per frame (`NumDynLightSurfs < 0x800`).
const MAX_DYN_LIGHT_SURFS: usize = 0x800;
const PF_SPECIAL_LIT: u32 = 0x10_0000;

/// What the walk found.
#[derive(Debug, Clone, Default)]
pub struct Visibility {
    /// Per input sprite: drawn (its actor gets `LastRenderTime`).
    pub sprites: Vec<bool>,
    /// Zones the camera saw (the camera zone and every zone reached through a portal): bit per zone.
    pub zones: u64,
    /// Surfaces put on the frame's draw lists: node, the poly flags they are drawn with and the zone (the side of
    /// the node they are seen from). The engine keeps one draw list per surface and zone (`FBspDrawList`).
    pub surfaces: Vec<(u32, u32, u8)>,
    /// Per entry of `surfaces`: the frame list its draw list was linked into (`FSceneNode+0x98 + 4 * list`, newest
    /// first): 0 for a mirror opening a child frame (or an opaque mirror drawn invisible), 1 for solid surfaces (and a
    /// translucent mirror drawn solid), 2 for those that let what is behind show (`flags & 0x47`). `DrawFrame` draws
    /// list 0 reversed (walk order), list 1 sorted by its key, list 2 as linked (back to front).
    pub surface_lists: Vec<u8>,
    /// The sprites in the order they joined the frame's sprite list (first visible piece, 10b247ec: linked at the
    /// head of `FSceneNode+0xa4`); `DrawFrame` walks that list, so it draws them in the reverse of this order.
    pub sprite_order: Vec<usize>,
    /// The dynamic lights reaching each surface (`URender::SurfLights`): surface index and light id.
    pub surf_lights: Vec<(u32, u32)>,
    /// The volumetric lights whose volume a drawn surface's view pyramid reaches (the fog lights of the draw list,
    /// `FBspDrawList+0x40`): surface index, zone of the draw and light id.
    pub surf_vol_lights: Vec<(u32, u8, u32)>,
    /// Per input sprite: its fog lights (`FDynamicSprite+0xb4`, `Volumetrics`): every fog light of the running list
    /// when one of its pieces was drawn, linked at the head (list order, head first).
    pub sprite_vols: Vec<Vec<u32>>,
    /// Per leaf: the dynamic lights the light filter linked to it (`URender::LeafLights`, head first). A drawn sprite
    /// takes the list of its actor's leaf (`Region.iLeaf`) as its `LeafLights` (`OccludeFrame`, after the walk).
    pub leaf_lights: std::collections::HashMap<i32, Vec<u32>>,
}

/// One frame's result: the frame (camera, screen box) and what it saw.
#[derive(Debug, Clone)]
pub struct FrameVis {
    pub frame: Frame,
    /// The actors' sprites the frame was walked with (`vis.sprites` says which were drawn).
    pub sprites: Vec<Sprite>,
    /// The dynamic lights the frame was walked with.
    pub lights: Vec<DynLight>,
    pub vis: Visibility,
    /// Index of the parent frame in the list (`None` for the main view).
    pub parent: Option<usize>,
}

/// `URender::OccludeFrame`: the walk of `frame`, then, depth first, of every child frame it opened (the newest
/// first, as the engine links them). `sprites` gives the actors' sprites for a frame (`SetupDynamics` runs per
/// frame). Returns every frame in walk order.
pub fn occlude(world: &World, frame: Frame, sprites: &mut dyn FnMut(&Frame) -> FrameInput) -> Vec<FrameVis> {
    let mut out = Vec::new();
    occlude_into(world, frame, None, sprites, &mut out);
    out
}

fn occlude_into(world: &World, frame: Frame, parent: Option<usize>, sprites: &mut dyn FnMut(&Frame) -> FrameInput, out: &mut Vec<FrameVis>) {
    let input = sprites(&frame);
    let (vis, children) = walk_lit(world, &frame, &input.sprites, &input.lights);
    let me = out.len();
    out.push(FrameVis { frame, sprites: input.sprites, lights: input.lights, vis, parent });
    for child in children.into_iter().rev() {
        occlude_into(world, child, Some(me), sprites, out);
    }
}

/// The zones seen by any frame (each frame stamps its camera zone and the zones its portals open): bit per zone.
pub fn zones_seen(frames: &[FrameVis]) -> u64 {
    frames.iter().fold(0, |m, f| m | f.vis.zones)
}

/// `CreateChildFrame`: a child with the same surface, zone and near clip plane takes the new spans in and widens its
/// side slopes; otherwise a new child.
fn create_child(children: &mut Vec<Frame>, parent: &Frame, span: &SpanBuffer, zone: u8, i_surf: i32, mirror: f32, near_clip: [f32; 4], coords: [Vec3; 4], bounds: Option<[f32; 4]>) {
    if let Some(c) = children.iter_mut().find(|c| c.i_surf == i_surf && c.near_clip == near_clip && c.zone == zone) {
        c.span.merge_with(span);
        if let Some(b) = bounds {
            c.set_bounds(b, true);
        }
        return;
    }
    let mut c = Frame::child(parent, zone, i_surf, mirror, near_clip, coords, bounds);
    c.span.merge_with(span);
    children.push(c);
}

/// `GMath.SinTab` / `CosTab`: the engine's 16384-entry sine table.
fn sin_tab(a: i32) -> f32 {
    use std::sync::OnceLock;
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    let t = T.get_or_init(|| (0..16384).map(|i| (i as f64 * 0.000_383_495_196_971_410_3).sin() as f32).collect());
    t[((a >> 2) & 16383) as usize]
}

fn cos_tab(a: i32) -> f32 {
    sin_tab(a.wrapping_add(16384))
}

/// `FCoords *= FCoords` for a transform with no origin: every axis re-expressed in the transform's axes.
fn coords_mul(c: &mut [Vec3; 4], t: [Vec3; 3]) {
    for k in 0..4 {
        let v = c[k];
        c[k] = [dot(v, t[0]), dot(v, t[1]), dot(v, t[2])];
    }
}

/// The sky frame's camera: the view's axes turned by the inverse of the sky zone's rotation (`FCoords /= FRotator`:
/// roll, then pitch, then yaw), placed at the sky zone actor.
fn sky_coords(f: &Frame, sky: &SkyZone) -> [Vec3; 4] {
    let mut c = [[0.0; 3], f.x_axis, f.y_axis, f.z_axis];
    let [pitch, yaw, roll] = sky.rotation;
    let (sr, cr) = (sin_tab(roll), cos_tab(roll));
    coords_mul(&mut c, [[1.0, 0.0, 0.0], [0.0, cr, sr], [0.0, -sr, cr]]);
    let (sp, cp) = (sin_tab(pitch), cos_tab(pitch));
    coords_mul(&mut c, [[cp, 0.0, -sp], [0.0, 1.0, 0.0], [sp, 0.0, cp]]);
    let (sy, cy) = (sin_tab(yaw), cos_tab(yaw));
    coords_mul(&mut c, [[cy, -sy, 0.0], [sy, cy, 0.0], [0.0, 0.0, 1.0]]);
    c[0] = [c[0][0] + sky.location[0], c[0][1] + sky.location[1], c[0][2] + sky.location[2]];
    c
}

/// A mirror frame's camera and near clip plane: the camera mirrored by the node plane, and the plane in the parent's
/// view space, negated so that what lies on the camera's side of the mirror is kept.
fn mirror_coords(f: &Frame, plane: [f32; 4]) -> ([Vec3; 4], [f32; 4]) {
    let n = [plane[0], plane[1], plane[2]];
    let reflect = |v: Vec3| {
        let d = 2.0 * dot(v, n);
        [v[0] - n[0] * d, v[1] - n[1] * d, v[2] - n[2] * d]
    };
    let o = f.origin;
    let pd = 2.0 * (dot(o, n) - plane[3]);
    let origin = [o[0] - n[0] * pd, o[1] - n[1] * pd, o[2] - n[2] * pd];
    let coords = [origin, reflect(f.x_axis), reflect(f.y_axis), reflect(f.z_axis)];
    let nv = [dot(f.x_axis, n), dot(f.y_axis, n), dot(f.z_axis, n)];
    let ov = [dot(o, f.x_axis), dot(o, f.y_axis), dot(o, f.z_axis)];
    let w = plane[3] - dot(ov, nv);
    (coords, [-nv[0], -nv[1], -nv[2], -w])
}

/// A piece of a sprite: one span per row from `y1`.
#[derive(Debug, Clone)]
struct Piece {
    sprite: usize,
    y1: i32,
    spans: Vec<Span>,
}

#[derive(Debug, Clone, Copy)]
struct Rec {
    node: usize,
    far: i32,
    far_outside: bool,
    outside: bool,
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The world point under screen position `(sx, sy)` at view depth `z`.
fn deproject(f: &Frame, sx: f32, sy: f32, z: f32) -> Vec3 {
    let (x, y) = ((sx - f.fx15) * z / f.proj_z, (sy - f.fy15) * z / f.proj_z);
    let mut p = f.origin;
    for k in 0..3 {
        p[k] += f.x_axis[k] * x + f.y_axis[k] * y + f.z_axis[k] * z;
    }
    p
}

/// Splits a piece by a node plane at the sprite's depth (the chunk filter): the plane distance over the sprite's
/// screen box is linear in the screen position; the four box corners decide whether it is cut (beyond +-0.01), and a
/// cut row is split where the distance crosses zero.
fn split(f: &Frame, s: &Sprite, p: Piece, plane: [f32; 4]) -> (Option<Piece>, Option<Piece>) {
    let d = |sx: f32, sy: f32| dot(deproject(f, sx, sy, s.z), [plane[0], plane[1], plane[2]]) - plane[3];
    let corners = [(s.x1 as f32, s.y1 as f32), (s.x2 as f32, s.y1 as f32), (s.x2 as f32, s.y2 as f32), (s.x1 as f32, s.y2 as f32)];
    let ds = corners.map(|(x, y)| d(x, y));
    let front = ds.iter().filter(|&&v| v > 0.01).count();
    let back = ds.iter().filter(|&&v| v < -0.01).count();
    // a piece wholly on one side moves on as it is
    if back == 0 {
        return (Some(p), None);
    }
    if front == 0 {
        return (None, Some(p));
    }
    let (c, ax, ay) = (d(0.0, 0.0), d(1.0, 0.0), d(0.0, 1.0));
    let (ax, ay) = (ax - c, ay - c);
    let mut fp = Piece { sprite: p.sprite, y1: p.y1, spans: Vec::with_capacity(p.spans.len()) };
    let mut bp = fp.clone();
    for (i, &(a, b)) in p.spans.iter().enumerate() {
        let y = (p.y1 + i as i32) as f32 + 0.5;
        if a >= b {
            fp.spans.push((0, 0));
            bp.spans.push((0, 0));
            continue;
        }
        let at = |x: i32| c + ax * (x as f32 + 0.5) + ay * y;
        if ax.abs() < 1e-12 {
            if at(a) > 0.0 {
                fp.spans.push((a, b));
                bp.spans.push((0, 0));
            } else {
                fp.spans.push((0, 0));
                bp.spans.push((a, b));
            }
            continue;
        }
        // crossing column: distance zero
        let xc = (-(c + ay * y) / ax - 0.5).ceil() as i32;
        let cut = xc.clamp(a, b);
        if ax > 0.0 {
            bp.spans.push((a, cut));
            fp.spans.push((cut, b));
        } else {
            fp.spans.push((a, cut));
            bp.spans.push((cut, b));
        }
    }
    let empty = |q: &Piece| q.spans.iter().all(|&(a, b)| a >= b);
    (Some(fp).filter(|q| !empty(q)), Some(bp).filter(|q| !empty(q)))
}

/// Runs the walk (`OccludeBsp`) over the level from frame `f` (its zone is the camera's: 0 when the level has no
/// zones) with the actors' `sprites`. Returns what it saw and the child frames it opened, in creation order.
pub fn walk(world: &World, f: &Frame, sprites: &[Sprite]) -> (Visibility, Vec<Frame>) {
    walk_lit(world, f, sprites, &[])
}

/// The walk with dynamic light items: each is filtered down the BSP from the root (callback 10b24a10) at the nodes
/// the walk visits; where its sphere crosses a node plane (`-R < distance < R`) the node's surface and its coplanar
/// ones (with a lightmap, `PF_SpecialLit` matching the light's `bSpecialLit`) get it on their light list.
pub fn walk_lit(world: &World, f: &Frame, sprites: &[Sprite], lights: &[DynLight]) -> (Visibility, Vec<Frame>) {
    let model = world.model;
    let surf_flags = world.surf_flags;
    let camera_zone = f.zone;
    let mut children: Vec<Frame> = Vec::new();
    let mut vis = Visibility { sprites: vec![false; sprites.len()], sprite_vols: vec![Vec::new(); sprites.len()], ..Default::default() };
    if model.nodes.is_empty() {
        return (vis, children);
    }
    let zones = camera_zone != 0;
    let zmask: u8 = if zones { 0xff } else { 0 };
    let mut seen_mask: u64 = 1u64 << camera_zone;
    vis.zones = seen_mask;
    // one span buffer per zone; the camera zone sees the whole screen
    // dynamic lists per node: [0] chunks being filtered, then final pieces for the front [1] and back [2] side
    let n = model.nodes.len();
    // the per-node lists come from the previous walk's (emptied here, allocations kept)
    let mut scratch = SCRATCH.with(|s| std::mem::take(&mut *s.borrow_mut()));
    scratch.reset(n);
    let mut span: Vec<SpanBuffer> = (0..64).map(|_| SpanBuffer::alloc(0, 0)).collect();
    // the camera zone's buffer, a copy of the frame's: the last walk's rows are reused
    let mut cam_span = std::mem::take(&mut scratch.cam_span);
    cam_span.copy_from(&f.span);
    span[camera_zone as usize] = cam_span;
    let (mut chunks, mut finals, mut light_lists) = (std::mem::take(&mut scratch.chunks), std::mem::take(&mut scratch.finals), std::mem::take(&mut scratch.light_lists));
    // the nodes whose lists got something (the next walk empties only those)
    let mut touched: Vec<usize> = std::mem::take(&mut scratch.touched);
    touched.push(0);
    let mut lit_surfs: std::collections::HashSet<u32> = Default::default();
    // per light item: (light, volumetric, linked to a leaf already)
    light_lists[0].extend((0..lights.len()).map(|li| (li, lights[li].volumetric, false)));
    // dynamic volumetric links per leaf (`URender::LeafLights`): light id, view-space position, flag
    let mut leaf_links: std::collections::HashMap<i32, Vec<(u32, Vec3, bool)>> = Default::default();
    // the volumetric lights gathered so far (`URender+0x30`), newest last; stamped once per frame
    let mut running: Vec<(u32, Vec3, f32)> = Vec::new();
    let mut stamped: std::collections::HashSet<u32> = Default::default();
    let mut surf_vol: std::collections::HashSet<(u32, u8, u32)> = Default::default();
    let planes = f.view_planes();
    let view_of = |p: Vec3| {
        let d = [p[0] - f.origin[0], p[1] - f.origin[1], p[2] - f.origin[2]];
        let ax = |a: Vec3| d[2] * a[2] + d[1] * a[1] + d[0] * a[0];
        [ax(f.x_axis), ax(f.y_axis), ax(f.z_axis)]
    };
    // `URender::LeafVolumetricLighting` (10b170e0)
    let leaf_vol = |leaf: i32, running: &mut Vec<(u32, Vec3, f32)>, stamped: &mut std::collections::HashSet<u32>, leaf_links: &std::collections::HashMap<i32, Vec<(u32, Vec3, bool)>>| {
        if !world.volumetrics || leaf < 0 {
            return;
        }
        if let Some(lf) = model.leaves.get(leaf as usize) {
            if lf.i_volumetric >= 0 {
                let mut i = lf.i_volumetric as usize;
                while model.lights.get(i).copied().unwrap_or(0) != 0 {
                    if let Some(vl) = (world.vol_light)(i) {
                        if stamped.insert(vl.id) {
                            let d = [vl.pos[0] - f.origin[0], vl.pos[1] - f.origin[1], vl.pos[2] - f.origin[2]];
                            let rv = (vl.volume_radius as i32 + 1) as f32 * 25.0;
                            let mut show = true;
                            if rv * rv < (d[0] * d[0] + d[1] * d[1]) + d[2] * d[2] {
                                let lim = (vl.volume_radius as i32 + 1) as f32 * -25.0;
                                show = planes.iter().all(|p| (vl.pos[1] * p[1] + p[2] * vl.pos[2] + p[0] * vl.pos[0]) - p[3] >= lim);
                            }
                            if show {
                                running.push((vl.id, view_of(vl.pos), rv));
                            }
                        }
                    }
                    i += 1;
                }
            }
        }
        for &(id, view, flag) in leaf_links.get(&leaf).map(|v| v.as_slice()).unwrap_or(&[]).iter().rev() {
            if flag && stamped.insert(id) {
                let rv = lights.iter().find(|l| l.id == id).map(|l| (l.volume_radius as i32 + 1) as f32 * 25.0).unwrap_or(25.0);
                running.push((id, view, rv));
            }
        }
    };
    for (i, s) in sprites.iter().enumerate() {
        if s.y2 > s.y1 && s.x2 > s.x1 {
            chunks[0].push(Piece { sprite: i, y1: s.y1, spans: vec![(s.x1, s.x2); (s.y2 - s.y1) as usize] });
        }
    }
    let eye = f.origin;
    let side_of = |node: usize| {
        let p = model.nodes[node].plane;
        (dot(eye, [p[0], p[1], p[2]]) - p[3]) > 0.0
    };
    let csg = |node: usize| {
        let nd = &model.nodes[node];
        nd.num_vertices != 0 && nd.node_flags & 0x21 == 0
    };
    // the four corner rays of the view (world directions), for the surface facing test
    let rays: Vec<Vec3> = [(-1.0f32, -1.0f32), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)]
        .iter()
        .map(|&(sx, sy)| {
            let v = [sx * f.fx2, sy * f.fy2, f.proj_z];
            let l = dot(v, v).sqrt();
            let v = [v[0] / l, v[1] / l, v[2] / l];
            [f.x_axis[0] * v[0] + f.y_axis[0] * v[1] + f.z_axis[0] * v[2], f.x_axis[1] * v[0] + f.y_axis[1] * v[1] + f.z_axis[1] * v[2], f.x_axis[2] * v[0] + f.y_axis[2] * v[1] + f.z_axis[2] * v[2]]
        })
        .collect();

    // final piece visit (10b24740): a visible piece draws the sprite (a later one merges into its span) and links the
    // running fog lights to it
    let visit_finals = |node: usize, list: usize, z: usize, finals: &mut Vec<[Vec<Piece>; 2]>, span: &Vec<SpanBuffer>, vis: &mut Visibility, running: &Vec<(u32, Vec3, f32)>| {
        if span[z].is_empty() {
            return;
        }
        for p in std::mem::take(&mut finals[node][list]) {
            if span[z].overlaps_raster(p.y1, p.y1 + p.spans.len() as i32, &p.spans) {
                if !vis.sprites[p.sprite] {
                    vis.sprite_order.push(p.sprite);
                }
                vis.sprites[p.sprite] = true;
                let vols = &mut vis.sprite_vols[p.sprite];
                for &(id, _, _) in running.iter().rev() {
                    if !vols.contains(&id) {
                        vols.insert(0, id);
                    }
                }
            }
        }
    };

    let mut stack: Vec<Rec> = Vec::new();
    // per surface: its points and what of it is drawn (reused, not allocated each time)
    let mut poly: Vec<Vec3> = Vec::new();
    let mut drawn_buf = SpanBuffer::default();
    let mut node = 0usize;
    let mut outside = model.root_outside;
    'walk: loop {
        // ---- first visit of `node`
        let nd = &model.nodes[node];
        let mut skip = zones && nd.zone_mask & seen_mask == 0;
        let bound = match world.render_bounds {
            Some(rb) => rb.get(node).copied().flatten(),
            None if nd.i_render_bound >= 0 && !(nd.i_surf >= 0 && nd.i_surf as usize >= world.first_dynamic_surf) => {
                let b = &model.bounds[nd.i_render_bound as usize];
                Some((b.min, b.max))
            }
            None => None,
        };
        if let Some((bmin, bmax)) = bound.filter(|_| !skip) {
            match bound_visible(f, bmin, bmax, if zones { None } else { Some(&span[0]) }) {
                None => skip = true,
                Some(sb) if zones => {
                    let (x1, y1, x2, y2) = (sb.min_x as i32, sb.min_y as i32, sb.max_x as i32, sb.max_y as i32);
                    let any = (0..64).filter(|&z| seen_mask & (1u64 << z) != 0 && nd.zone_mask & (1u64 << z) != 0).any(|z| span[z].box_is_visible(x1, y1, x2, y2));
                    if !any {
                        skip = true;
                    }
                }
                _ => {}
            }
        }
        let mut rec = None;
        if !skip {
            // dynamic lights hanging here: the surfaces their sphere crosses, then down the sides it reaches
            for (li, volumetric, linked) in std::mem::take(&mut light_lists[node]) {
                let l = &lights[li];
                let dist = (nd.plane[0] * l.pos[0] + l.pos[1] * nd.plane[1] + l.pos[2] * nd.plane[2]) - nd.plane[3];
                let r = l.radius;
                let rv = (l.volume_radius as i32 + 1) as f64;
                let mut back_linked = linked;
                if -r < dist {
                    let mut front_linked = linked;
                    if !linked && nd.i_leaf[1] >= 0 {
                        let flag = volumetric && dist as f64 > rv * -25.0;
                        leaf_links.entry(nd.i_leaf[1]).or_default().push((l.id, view_of(l.pos), flag));
                        front_linked = true;
                    }
                    if nd.i_front >= 0 {
                        light_lists[nd.i_front as usize].push((li, volumetric && dist as f64 > rv * -25.0, front_linked));
                        touched.push(nd.i_front as usize);
                    }
                    if dist >= r {
                        continue;
                    }
                    let mut c = node as i32;
                    while c >= 0 {
                        let cn = &model.nodes[c as usize];
                        if cn.i_surf >= 0 {
                            let si = cn.i_surf as u32;
                            let surf = &model.surfs[si as usize];
                            let room = lit_surfs.contains(&si) || lit_surfs.len() < MAX_DYN_LIGHT_SURFS;
                            if surf.i_light_map >= 0 && room && (surf.poly_flags & PF_SPECIAL_LIT != 0) == l.special_lit && !vis.surf_lights.contains(&(si, l.id)) {
                                lit_surfs.insert(si);
                                vis.surf_lights.push((si, l.id));
                            }
                        }
                        c = cn.i_plane;
                    }
                }
                if dist < r {
                    if !back_linked && nd.i_leaf[0] >= 0 {
                        let flag = volumetric && rv * 25.0 > dist as f64;
                        leaf_links.entry(nd.i_leaf[0]).or_default().push((l.id, view_of(l.pos), flag));
                        back_linked = true;
                    }
                    if nd.i_back >= 0 {
                        light_lists[nd.i_back as usize].push((li, volumetric && rv * 25.0 > dist as f64, back_linked));
                        touched.push(nd.i_back as usize);
                    }
                }
            }
            // chunks hanging here move down (or become final pieces where a side has no child)
            for p in std::mem::take(&mut chunks[node]) {
                let (front, back) = split(f, &sprites[p.sprite], p, nd.plane);
                if let Some(q) = front {
                    if nd.i_front >= 0 {
                        chunks[nd.i_front as usize].push(q);
                        touched.push(nd.i_front as usize);
                    } else if outside || csg(node) {
                        finals[node][0].push(q);
                        touched.push(node);
                    }
                }
                if let Some(q) = back {
                    if nd.i_back >= 0 {
                        chunks[nd.i_back as usize].push(q);
                        touched.push(nd.i_back as usize);
                    } else if outside && !csg(node) {
                        finals[node][1].push(q);
                        touched.push(node);
                    }
                }
            }
            let front_side = side_of(node);
            let c = csg(node);
            let (near, far) = if front_side { (nd.i_front, nd.i_back) } else { (nd.i_back, nd.i_front) };
            let near_outside = if front_side { outside || c } else { outside && !c };
            let far_outside = if front_side { outside && !c } else { outside || c };
            let r = Rec { node, far, far_outside, outside };
            if near >= 0 {
                stack.push(r);
                node = near as usize;
                outside = near_outside;
                continue 'walk;
            }
            rec = Some(r);
        }
        // ---- back from the near side (or no near side): the node itself, then the far side
        let mut cur = rec;
        loop {
            let r = match cur.take() {
                Some(r) => r,
                None => match stack.pop() {
                    Some(r) => r,
                    None => break 'walk,
                },
            };
            let nd = &model.nodes[r.node];
            if zones && nd.zone_mask & seen_mask == 0 {
                continue;
            }
            let front_side = side_of(r.node);
            let c = csg(r.node);
            let near_list = if front_side { 0 } else { 1 };
            let near_solid = if front_side { !r.outside && !c } else { !r.outside || c };
            let zn = (nd.i_zone[front_side as usize] & zmask) as usize;
            leaf_vol(nd.i_leaf[front_side as usize], &mut running, &mut stamped, &leaf_links);
            if !near_solid {
                visit_finals(r.node, near_list, zn, &mut finals, &span, &mut vis, &running);
            }
            // the node's surface and its coplanar surfaces
            let mut s_node = r.node as i32;
            while s_node >= 0 {
                let sn = &model.nodes[s_node as usize];
                let fs = side_of(s_node as usize);
                let sign = if fs { 1.0 } else { -1.0 };
                let np = [sn.plane[0], sn.plane[1], sn.plane[2]];
                // only the node's own surface gets the facing test (coplanar ones skip it in the engine)
                let facing = s_node as usize != r.node || rays.iter().any(|ray| dot(np, *ray) * sign <= 0.0);
                let dist = dot(eye, np) - sn.plane[3];
                if facing && sn.i_surf >= 0 {
                    // outside game views (editor) invisible surfaces are drawn
                    let mut flags = surf_flags(sn.i_surf as usize) & if world.player_ctrl { !0 } else { !PF_INVISIBLE };
                    if (fs || dist >= -1.0 || flags & (PF_TWO_SIDED | PF_PORTAL) != 0) && (flags & PF_PORTAL == 0 || zones) {
                        let z = (sn.i_zone[fs as usize] & zmask) as usize;
                        if !span[z].is_empty() {
                            poly.clear();
                            poly.extend((0..sn.num_vertices as usize).filter_map(|k| model.verts.get(sn.i_vert_pool as usize + k)).filter_map(|v| model.points.get(v.p_vertex as usize).copied()));
                            let mut pts = clip_polygon(f, &poly);
                            if pts.len() >= 3 {
                                if (f.mirror == -1.0) != (!fs && flags & (PF_TWO_SIDED | PF_PORTAL) != 0) {
                                    pts.reverse();
                                }
                                if let Some(ras) = rasterize(&mut pts, f.y, Some(&span[z])) {
                                    drawn_buf.reset(ras.y1, ras.y2);
                                    let drawn = &mut drawn_buf;
                                    let solid = flags & PF_SEE_THROUGH == 0 || flags & (PF_PORTAL | PF_INVISIBLE) == PF_PORTAL | PF_INVISIBLE || flags & PF_MIRRORED != 0;
                                    let open = if solid {
                                        let src = &mut span[z];
                                        drawn.copy_from_raster_update(src, ras.y1, ras.y2, &ras.spans)
                                    } else {
                                        drawn.copy_from_raster(&span[z], ras.y1, ras.y2, &ras.spans)
                                    };
                                    if open {
                                        let see_through = flags & PF_SEE_THROUGH != 0;
                                        let bounds = Some([ras.min_x as f32, ras.y1 as f32, ras.max_x as f32, ras.y2 as f32]);
                                        let sky = if flags & PF_FAKE_BACKDROP != 0 && f.recursion < MAX_RECURSION && world.player_ctrl { (world.sky_zone)(z as u8) } else { None };
                                        let draw_it;
                                        // the frame list (OccludeBsp 10b18208..10b195fc): see `Visibility::surface_lists`
                                        let mut list = (flags & PF_SEE_THROUGH != 0) as u8 + 1;
                                        if let Some(sky) = sky {
                                            // the sky zone seen through the backdrop, from the sky zone actor; the
                                            // surface itself is not drawn
                                            let coords = sky_coords(f, &sky);
                                            create_child(&mut children, f, &drawn, sky.zone, 0, f.mirror, f.near_clip, coords, bounds);
                                            draw_it = false;
                                        } else if flags & PF_MIRRORED != 0 && f.recursion < MAX_RECURSION && world.player_ctrl {
                                            if flags & PF_TRANSLUCENT == 0 || world.shiny_surfaces {
                                                let (coords, near) = mirror_coords(f, sn.plane);
                                                create_child(&mut children, f, &drawn, z as u8, 0, -f.mirror, near, coords, bounds);
                                                list = 0;
                                                if !see_through {
                                                    // an opaque mirror: nothing of it is drawn (software), or drawn
                                                    // invisible
                                                    if world.span_based {
                                                        s_node = sn.i_plane;
                                                        continue;
                                                    }
                                                    flags |= PF_INVISIBLE;
                                                }
                                            } else {
                                                // a translucent mirror without shiny surfaces: drawn solid
                                                flags &= !PF_TRANSLUCENT;
                                                list = 1;
                                            }
                                            flags |= PF_FLAG_MIRROR_DRAWN;
                                            draw_it = true;
                                        } else {
                                            draw_it = flags & PF_INVISIBLE == 0;
                                            if flags & PF_PORTAL != 0 && see_through {
                                                // a zone portal: the zone behind sees through it
                                                let zb = (sn.i_zone[!fs as usize] & zmask) as usize;
                                                if zb != 0 {
                                                    seen_mask |= 1u64 << zb;
                                                    vis.zones |= 1u64 << zb;
                                                    span[zb].merge_with(&drawn);
                                                }
                                            }
                                        }
                                        if draw_it && !running.is_empty() {
                                            // the surface's fog lights: those whose volume reaches into the view
                                            // pyramid through the polygon (edge planes from the clipped points)
                                            let mut normals: Vec<Vec3> = Vec::new();
                                            let m = pts.len();
                                            for k in 0..m {
                                                let (p, c) = (pts[(k + m - 1) % m].view, pts[k].view);
                                                let n = [p[2] * c[1] - c[2] * p[1], c[2] * p[0] - c[0] * p[2], c[0] * p[1] - p[0] * c[1]];
                                                let q = ue_assets::light::fast_inv_sqrt((n[2] * n[2] + n[1] * n[1]) + n[0] * n[0]);
                                                normals.push([q * n[0], q * n[1], q * n[2]]);
                                            }
                                            for &(id, v, rv) in running.iter().rev() {
                                                if normals.iter().all(|n| rv >= (v[0] * n[0] + v[1] * n[1]) + v[2] * n[2]) {
                                                    surf_vol.insert((sn.i_surf as u32, z as u8, id));
                                                }
                                            }
                                        }
                                        if draw_it {
                                            vis.surfaces.push((s_node as u32, flags, z as u8));
                                            vis.surface_lists.push(list);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                s_node = sn.i_plane;
            }
            // the far side's final pieces
            let far_list = if front_side { 1 } else { 0 };
            let far_solid = if front_side { !r.outside || c } else { !r.outside && !c };
            let zf = (nd.i_zone[!front_side as usize] & zmask) as usize;
            leaf_vol(nd.i_leaf[!front_side as usize], &mut running, &mut stamped, &leaf_links);
            if !far_solid {
                visit_finals(r.node, far_list, zf, &mut finals, &span, &mut vis, &running);
            }
            if r.far >= 0 {
                node = r.far as usize;
                outside = r.far_outside;
                continue 'walk;
            }
        }
    }
    vis.surf_vol_lights = surf_vol.into_iter().collect();
    vis.surf_vol_lights.sort_unstable();
    vis.leaf_lights = leaf_links.into_iter().map(|(leaf, v)| (leaf, v.iter().rev().map(|&(id, _, _)| id).collect())).collect();
    let cam_span = std::mem::take(&mut span[camera_zone as usize]);
    SCRATCH.with(|s| *s.borrow_mut() = Scratch { chunks, finals, light_lists, touched, cam_span });
    (vis, children)
}

/// The walk's per-node lists, kept between walks so that each does not allocate (and free) three of them the size of
/// the level's node count: [0] chunks being filtered, final pieces per side, dynamic light items.
#[derive(Default)]
struct Scratch {
    chunks: Vec<Vec<Piece>>,
    finals: Vec<[Vec<Piece>; 2]>,
    light_lists: Vec<Vec<(usize, bool, bool)>>,
    /// The nodes whose lists the last walk put something in (only those can hold leftovers).
    touched: Vec<usize>,
    /// The camera zone's span buffer (its rows' allocations kept).
    cam_span: SpanBuffer,
}

impl Scratch {
    fn reset(&mut self, n: usize) {
        let clear = |s: &mut Scratch, i: usize| {
            s.chunks[i].clear();
            s.finals[i][0].clear();
            s.finals[i][1].clear();
            s.light_lists[i].clear();
        };
        if self.chunks.len() == n && self.finals.len() == n && self.light_lists.len() == n {
            let touched = std::mem::take(&mut self.touched);
            for &i in &touched {
                if i < n {
                    clear(self, i);
                }
            }
            self.touched = touched;
            self.touched.clear();
            return;
        }
        self.chunks.resize_with(n, Vec::new);
        self.finals.resize_with(n, Default::default);
        self.light_lists.resize_with(n, Vec::new);
        for i in 0..n {
            clear(self, i);
        }
        self.touched.clear();
    }
}

thread_local! {
    static SCRATCH: std::cell::RefCell<Scratch> = Default::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        let mut f = Frame::new([0.0; 3], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0], 90.0, 100, 80);
        f.zone = 3;
        f
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
    }

    #[test]
    fn child_frame_bounds_and_reuse() {
        let p = frame();
        let mut children = Vec::new();
        let mut span = SpanBuffer::alloc(10, 12);
        span.merge_with(&{
            let mut s = SpanBuffer::for_screen(100, 80);
            s.close_rect(0, 0, 100, 10);
            s.close_rect(0, 12, 100, 80);
            s.close_rect(0, 0, 20, 80);
            s.close_rect(40, 0, 100, 80);
            s
        });
        let coords = [p.origin, p.x_axis, p.y_axis, p.z_axis];
        create_child(&mut children, &p, &span, 5, 0, 1.0, [0.0; 4], coords, Some([20.0, 10.0, 40.0, 12.0]));
        assert_eq!(children.len(), 1);
        let c = &children[0];
        assert_eq!((c.recursion, c.zone, c.i_surf), (1, 5, 0));
        // pixel 20 is 30 left of the centre (50): slope 30 / proj_z
        assert!((c.prj_xm - (50.0 - 20.0) / p.proj_z).abs() < 1e-6);
        assert!((c.prj_xp - (40.0 - 50.0) / p.proj_z).abs() < 1e-6);
        assert!((c.prj_ym - (40.0 - 10.0) / p.proj_z).abs() < 1e-6);
        assert_eq!(c.span.row(11), &[(20, 40)]);
        // a second surface for the same zone widens the box and merges its spans; another zone is a new child
        let mut span2 = SpanBuffer::alloc(30, 31);
        span2.merge_with(&{
            let mut s = SpanBuffer::for_screen(100, 80);
            s.close_rect(0, 0, 100, 30);
            s.close_rect(0, 31, 100, 80);
            s.close_rect(0, 0, 60, 80);
            s
        });
        create_child(&mut children, &p, &span2, 5, 0, 1.0, [0.0; 4], coords, Some([60.0, 30.0, 100.0, 31.0]));
        assert_eq!(children.len(), 1);
        let c = &children[0];
        assert!((c.prj_xm - 30.0 / p.proj_z).abs() < 1e-6 && (c.prj_xp - 50.0 / p.proj_z).abs() < 1e-6);
        assert!((c.prj_yp - (31.0 - 40.0) / p.proj_z).abs() < 1e-6 && (c.prj_ym - 30.0 / p.proj_z).abs() < 1e-6);
        assert_eq!(c.span.row(30), &[(60, 100)]);
        create_child(&mut children, &p, &span2, 6, 0, 1.0, [0.0; 4], coords, None);
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn mirror_camera_and_clip_plane() {
        let mut p = frame();
        p.origin = [2.0, 3.0, 4.0];
        // mirror plane x = 10, normal towards the camera
        let plane = [-1.0, 0.0, 0.0, -10.0];
        let (coords, near) = mirror_coords(&p, plane);
        assert!(close(coords[0], [18.0, 3.0, 4.0]));
        let mut c = Frame::child(&p, 3, 0, -1.0, near, coords, None);
        c.near_clip = near;
        let reflect = |q: Vec3| [20.0 - q[0], q[1], q[2]];
        for q in [[1.0f32, 5.0, -2.0], [9.5, -3.0, 7.0], [12.0, 0.0, 0.0], [30.0, 1.0, 1.0]] {
            // the mirrored view of a point is the parent's view of its reflection
            assert!(close(c.to_view(q), p.to_view(reflect(q))), "{q:?}");
            // the clip plane keeps what lies on the camera's side of the mirror
            let v = c.to_view(q);
            let d = near[0] * v[0] + near[1] * v[1] + near[2] * v[2] - near[3];
            assert_eq!(d >= 0.0, q[0] <= 10.0, "{q:?} {d}");
        }
    }

    #[test]
    fn sky_camera() {
        let p = frame();
        let sky = SkyZone { zone: 9, location: [100.0, -50.0, 2000.0], rotation: [0, 0, 0] };
        let c = sky_coords(&p, &sky);
        assert!(close(c[0], sky.location) && close(c[1], p.x_axis) && close(c[2], p.y_axis) && close(c[3], p.z_axis));
        // a turned sky zone: every axis turned by roll about X, then pitch about Y, then yaw about Z
        let (pitch, yaw, roll) = (2000i32, 16384i32, -3000i32);
        let sky = SkyZone { rotation: [pitch, yaw, roll], ..sky };
        let c = sky_coords(&p, &sky);
        let rad = |u: i32| u as f64 * std::f64::consts::TAU / 65536.0;
        let turn = |a: Vec3| {
            let a = [a[0] as f64, a[1] as f64, a[2] as f64];
            let (s, k) = rad(roll).sin_cos();
            let a = [a[0], a[1] * k + a[2] * s, a[2] * k - a[1] * s];
            let (s, k) = rad(pitch).sin_cos();
            let a = [a[0] * k - a[2] * s, a[1], a[0] * s + a[2] * k];
            let (s, k) = rad(yaw).sin_cos();
            let a = [a[0] * k - a[1] * s, a[0] * s + a[1] * k, a[2]];
            [a[0] as f32, a[1] as f32, a[2] as f32]
        };
        assert!(close(c[0], sky.location));
        for k in 0..3 {
            let want = turn([p.x_axis, p.y_axis, p.z_axis][k]);
            assert!(c[k + 1].iter().zip(want).all(|(a, b)| (a - b).abs() < 2e-3), "axis {k}: {:?} {want:?}", c[k + 1]);
        }
    }
}

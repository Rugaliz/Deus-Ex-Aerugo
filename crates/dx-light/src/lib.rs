//! Static lightmaps. The map file does not store light colours: per surface it stores a rectangle of texels and, for each
//! light that reaches the surface, one shadow bit per texel. The engine computes the colour of every texel when the surface
//! is first drawn; this crate does the same once per map (spec: `docs/formats/lightmap-format.md`).

pub mod manager;

use std::collections::HashMap;
use ue_assets::model::Model;
use ue_package::ObjectRef;
use ue_registry::Registry;

pub use ue_assets::light::hsv_to_rgb;

/// A light as the lightmap builder needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    pub pos: [f32; 3],
    pub brightness: u8,
    pub hue: u8,
    pub saturation: u8,
    pub radius_byte: u8,
    pub light_type: u8,
    pub effect: u8,
    /// `LightCone` (spot lights) and the actor's `Rotation` (the spot's direction), rotator units.
    pub cone: u8,
    pub rotation: [i32; 3],
    /// Direction of a spot light (the rotation's X axis); in the space the light is expressed in.
    pub dir: [f32; 3],
    /// `bStatic`, `bNoDelete`, `bDynamicLight` as the actor's flag bits (`manager::F_*`).
    pub flags: u32,
    /// `LightPeriod` and `LightPhase` (time-dependent effects).
    pub period: u8,
    pub phase: u8,
    /// `bSpecialLit` (Actor `0x1ec` bit 0).
    pub special_lit: bool,
}

impl Light {
    /// World radius: `25 * (LightRadius + 1)`.
    pub fn radius(&self) -> f32 {
        25.0 * (self.radius_byte as f32 + 1.0)
    }
}

/// Lights of a model in the order of `Model::lights` (`None` for the separators and for objects that could not be read).
pub struct MapLighting {
    pub lights: Vec<Option<Light>>,
    /// Zone ambient as the lightmap texel value (0..1 of a byte: `round(64 * rgb) / 255`), per zone index.
    pub zone_ambient: Vec<[f32; 3]>,
    /// `TexUPanSpeed`, `TexVPanSpeed` per zone index (auto-panning surfaces).
    pub zone_pan: Vec<[f32; 2]>,
    pub level_brightness: f32,
    /// Whether the lights use the stored shadow bits (level surfaces, movers' own lightmaps) or none (a
    /// `bDynamicLightMover`'s leaf lights: mask 127 everywhere).
    pub shadowed: bool,
    /// Whether the cached map holds the static lights only (the others merged every frame on top, as for level
    /// surfaces) or every light at its base brightness (movers until their per-frame merge is ported).
    pub static_only: bool,
}

fn vec3_at(reg: &Registry, inst: &ue_registry::Instance, name: &str) -> Option<[f32; 3]> {
    let p = reg.find_prop(inst.class, name)?;
    let o = p.offset as usize;
    let b = inst.data.get(o..o + 12)?;
    Some(std::array::from_fn(|i| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())))
}

/// Reads the light actors, zone ambients and the level brightness of the map loaded as package `pi` of `reg`.
pub fn collect(reg: &mut Registry, pi: usize, model: &Model) -> MapLighting {
    let instance = |reg: &mut Registry, raw: i32| -> Option<ue_registry::Instance> {
        let ObjectRef::Export(e) = ObjectRef::from_index(raw) else { return None };
        let class = reg.class_of_export(pi, e)?;
        reg.instantiate_object(class, pi, e).ok().map(|(i, _)| i)
    };
    let mut cache: HashMap<i32, Option<Light>> = HashMap::new();
    let mut lights = Vec::with_capacity(model.lights.len());
    for &raw in &model.lights {
        if raw == 0 {
            lights.push(None);
            continue;
        }
        let l = cache
            .entry(raw)
            .or_insert_with(|| {
                let inst = instance(reg, raw)?;
                Some(Light {
                    pos: vec3_at(reg, &inst, "Location")?,
                    brightness: reg.get_u8(&inst, "LightBrightness", 0)?,
                    hue: reg.get_u8(&inst, "LightHue", 0)?,
                    saturation: reg.get_u8(&inst, "LightSaturation", 0)?,
                    radius_byte: reg.get_u8(&inst, "LightRadius", 0)?,
                    light_type: reg.get_u8(&inst, "LightType", 0)?,
                    effect: reg.get_u8(&inst, "LightEffect", 0)?,
                    cone: reg.get_u8(&inst, "LightCone", 0)?,
                    rotation: {
                        let o = reg.find_prop(inst.class, "Rotation")?.offset as usize;
                        let b = inst.data.get(o..o + 12)?;
                        std::array::from_fn(|i| i32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()))
                    },
                    dir: [0.0; 3],
                    flags: [("bStatic", manager::F_STATIC), ("bNoDelete", manager::F_NO_DELETE), ("bDynamicLight", manager::F_DYNAMIC_LIGHT)]
                        .iter()
                        .filter(|(n, _)| reg.get_bool(&inst, n).unwrap_or(false))
                        .fold(0, |a, (_, b)| a | b),
                    period: reg.get_u8(&inst, "LightPeriod", 0).unwrap_or(0),
                    phase: reg.get_u8(&inst, "LightPhase", 0).unwrap_or(0),
                    special_lit: reg.get_bool(&inst, "bSpecialLit").unwrap_or(false),
                })
            })
            .clone();
        lights.push(l.map(|mut l| {
            // the X axis of `GMath.UnitCoords /= Rotation` (engine sine table)
            let (sp, cp) = (manager::sin_tab(l.rotation[0]), manager::sin_tab(l.rotation[0].wrapping_add(16384)));
            let (sy, cy) = (manager::sin_tab(l.rotation[1]), manager::sin_tab(l.rotation[1].wrapping_add(16384)));
            l.dir = [cp * cy, cp * sy, sp];
            l
        }));
    }
    // Zones without an actor of their own (the engine falls back to actor 0) use the level's `LevelInfo`.
    let level_info = (0..reg.packages[pi].pkg.exports.len()).find(|&e| reg.packages[pi].pkg.export_class_name(e) == "LevelInfo");
    let mut zone_ambient = Vec::new();
    let mut zone_pan = Vec::new();
    let mut level_brightness = 1.0;
    if let Some(inst) = level_info.and_then(|e| instance(reg, e as i32 + 1)) {
        level_brightness = reg.get_f32(&inst, "Brightness", 0).unwrap_or(1.0);
    }
    for z in &model.zones {
        let raw = if z.zone_actor == 0 { level_info.map(|e| e as i32 + 1).unwrap_or(0) } else { z.zone_actor };
        let amb = instance(reg, raw).and_then(|inst| {
            let (b, h, s) = (reg.get_u8(&inst, "AmbientBrightness", 0)?, reg.get_u8(&inst, "AmbientHue", 0)?, reg.get_u8(&inst, "AmbientSaturation", 0)?);
            Some(hsv_to_rgb(h, s, b))
        });
        zone_ambient.push(manager::ambient_texel(amb.unwrap_or([0.0; 3])).map(|b| b as f32 / 255.0));
        let pan = instance(reg, raw).map(|inst| [reg.get_f32(&inst, "TexUPanSpeed", 0).unwrap_or(0.0), reg.get_f32(&inst, "TexVPanSpeed", 0).unwrap_or(0.0)]);
        zone_pan.push(pan.unwrap_or([0.0; 2]));
    }
    MapLighting { lights, zone_ambient, zone_pan, level_brightness, shadowed: true, static_only: true }
}

impl MapLighting {
    /// The same lights seen from another coordinate frame (a mover's brush model): `point` maps a world point into that frame,
    /// `dir` maps a world direction.
    pub fn transformed(&self, point: impl Fn([f32; 3]) -> [f32; 3], dir: impl Fn([f32; 3]) -> [f32; 3]) -> MapLighting {
        MapLighting {
            lights: self.lights.iter().map(|l| l.map(|l| Light { pos: point(l.pos), dir: dir(l.dir), ..l })).collect(),
            zone_ambient: self.zone_ambient.clone(),
            zone_pan: self.zone_pan.clone(),
            level_brightness: self.level_brightness,
            shadowed: self.shadowed,
            static_only: self.static_only,
        }
    }
}

/// Where a surface's texels live in the atlas, and how texture-space coordinates map onto them.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceMap {
    /// Top-left texel of the surface's rectangle in the atlas (one replicated border texel surrounds it).
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Texture-space origin and texel size: texel `(i, j)` sits at `(pan.0 + i * scale.0, pan.1 + j * scale.1)` where
    /// texture space is `(P - pBase) . vTextureU` and `. vTextureV` (before the surface's own PanU/PanV).
    pub pan: [f32; 2],
    pub scale: [f32; 2],
}

impl SurfaceMap {
    /// Atlas position in texels (not yet divided by the atlas size) of the point with texture-space coordinates `(tu, tv)`,
    /// for bilinear sampling with the texel centres at `+0.5`.
    pub fn atlas_pos(&self, tu: f32, tv: f32) -> [f32; 2] {
        let u = ((tu - self.pan[0]) / self.scale[0]).clamp(-0.5, self.w as f32 - 0.5);
        let v = ((tv - self.pan[1]) / self.scale[1]).clamp(-0.5, self.h as f32 - 0.5);
        [self.x as f32 + u + 0.5, self.y as f32 + v + 0.5]
    }
}

/// The atlas rectangle of a surface with its replicated border (x, y, w, h, RGBA8) for `texels` (`w * h`, row-major).
pub fn atlas_rect(sm: &SurfaceMap, texels: &[[u8; 3]]) -> (u32, u32, u32, u32, Vec<u8>) {
    let (w, h) = (sm.w as i32, sm.h as i32);
    let mut rgba = Vec::with_capacity(((w + 2) * (h + 2) * 4) as usize);
    for j in -1..=h {
        for i in -1..=w {
            let t = texels[(j.clamp(0, h - 1) * w + i.clamp(0, w - 1)) as usize];
            rgba.extend_from_slice(&[t[0], t[1], t[2], 255]);
        }
    }
    (sm.x - 1, sm.y - 1, sm.w + 2, sm.h + 2, rgba)
}

/// The fog atlas rectangle of a surface (same place as its lightmap) for its 7-bit RGBA fog map, as the Direct3D
/// driver uploads it: every byte doubled (D3DDrv's RGBA7 to A8R8G8B8 conversion, 10002120), border replicated.
/// `None` writes zeros: the fog pass then leaves the pixel unchanged (blend `ONE, INVSRCALPHA`).
pub fn fog_atlas_rect(sm: &SurfaceMap, texels: Option<&[[u8; 4]]>) -> (u32, u32, u32, u32, Vec<u8>) {
    let (w, h) = (sm.w as i32, sm.h as i32);
    let mut rgba = Vec::with_capacity(((w + 2) * (h + 2) * 4) as usize);
    for j in -1..=h {
        for i in -1..=w {
            let t = texels.map(|t| t[(j.clamp(0, h - 1) * w + i.clamp(0, w - 1)) as usize]).unwrap_or([0; 4]);
            rgba.extend_from_slice(&t.map(|c| c << 1));
        }
    }
    (sm.x - 1, sm.y - 1, sm.w + 2, sm.h + 2, rgba)
}

impl Lightmaps {
    /// The texels of surface `si` as stored in the atlas.
    pub fn texels(&self, si: usize) -> Option<Vec<[u8; 3]>> {
        let sm = self.surfaces.get(si)?.as_ref()?;
        let mut out = Vec::with_capacity((sm.w * sm.h) as usize);
        for j in 0..sm.h {
            for i in 0..sm.w {
                let o = (((sm.y + j) * self.width + sm.x + i) * 4) as usize;
                out.push([self.rgba[o], self.rgba[o + 1], self.rgba[o + 2]]);
            }
        }
        Some(out)
    }
}

pub struct Lightmaps {
    pub width: u32,
    pub height: u32,
    /// RGBA8; colour channels are the sum of the lights' contributions, at most 127 each (127 = full brightness).
    pub rgba: Vec<u8>,
    /// Per surface of the model (`None` when it has no lightmap).
    pub surfaces: Vec<Option<SurfaceMap>>,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}



/// Texel positions of a surface in the world: solves `x . U = tu`, `x . V = tv`, `x . N = 0` for `x = P - pBase`.
fn plane_solver(u: [f32; 3], v: [f32; 3], n: [f32; 3]) -> Option<[[f32; 3]; 3]> {
    let (c0, c1, c2) = (cross(v, n), cross(n, u), cross(u, v));
    let det = dot(u, c0);
    if det.abs() < 1e-9 {
        return None;
    }
    // x = (tu * c0 + tv * c1 + 0 * c2) / det
    let _ = c2;
    Some([c0.map(|c| c / det), c1.map(|c| c / det), [0.0; 3]])
}

struct Built {
    surf: usize,
    w: usize,
    h: usize,
    texels: Vec<[u8; 3]>,
}

/// The light manager's view of surface `si`: frame and lightmap size (`None` without a usable lightmap).
pub fn surface_of(model: &Model, si: usize) -> Option<manager::Surface> {
    surface_in(model, si, model)
}

/// Surface `si` of `frame` lit with `maps`' lightmap `iLightMap` (a mover's surface in the level model: the moving-brush
/// tracker's world-space frame, its brush model's lightmaps).
pub fn surface_in(frame: &Model, si: usize, maps: &Model) -> Option<manager::Surface> {
    let surf = frame.surfs.get(si)?;
    if surf.i_light_map < 0 {
        return None;
    }
    let lm = maps.light_map.get(surf.i_light_map as usize)?;
    let (w, h) = (lm.u_clamp.max(0) as usize, lm.v_clamp.max(0) as usize);
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return None;
    }
    Some(manager::Surface {
        origin: *frame.points.get(surf.p_base as usize)?,
        u: *frame.vectors.get(surf.v_texture_u as usize)?,
        v: *frame.vectors.get(surf.v_texture_v as usize)?,
        n: *frame.vectors.get(surf.v_normal as usize)?,
        pan: [lm.pan[0], lm.pan[1]],
        scale: [lm.u_scale, lm.v_scale],
        w,
        h,
    })
}

/// The stored light list of surface `si`: index into `Model::lights` and the light's shadow bits (the bits follow the
/// list, one bitmap per entry).
pub fn surface_lights(model: &Model, si: usize) -> Vec<(usize, Option<&[u8]>)> {
    match model.surfs.get(si) {
        Some(surf) if surf.i_light_map >= 0 => lightmap_lights(model, surf.i_light_map as usize),
        _ => Vec::new(),
    }
}

/// The stored light list of lightmap `i_light_map` of `model` (see `surface_lights`).
pub fn lightmap_lights(model: &Model, i_light_map: usize) -> Vec<(usize, Option<&[u8]>)> {
    let mut out = Vec::new();
    let Some(lm) = model.light_map.get(i_light_map) else { return out };
    if lm.i_light_actors < 0 {
        return out;
    }
    let per_light = (lm.u_clamp.max(0) as usize + 7) / 8 * lm.v_clamp.max(0) as usize;
    let mut k = 0usize;
    while let Some(&raw) = model.lights.get(lm.i_light_actors as usize + k) {
        if raw == 0 {
            break;
        }
        let off = lm.data_offset.max(0) as usize + k * per_light;
        out.push((lm.i_light_actors as usize + k, model.light_bits.get(off..off + per_light)));
        k += 1;
    }
    out
}

/// "Add light" for the stored list: a type and a brightness; `OmniBumpMap` lights do not light the map.
pub fn adds_light(l: &Light) -> bool {
    l.light_type != 0 && l.brightness != 0 && l.effect != 15
}

fn build_surface(model: &Model, si: usize, zone: usize, lighting: &MapLighting) -> Option<Built> {
    let s = surface_of(model, si)?;
    // the cached static map holds the static lights only; the others are merged every frame on top of it
    let mut lights: Vec<(&Light, Option<&[u8]>)> = Vec::new();
    for (at, bits) in surface_lights(model, si) {
        let Some(Some(light)) = lighting.lights.get(at) else { continue };
        if !adds_light(light) || (lighting.static_only && !manager::is_static_light(light)) {
            continue;
        }
        // (a list whose bits are cut short loses that light, not the surface: `layout` relies on it)
        let bits = match (lighting.shadowed, bits) {
            (false, _) => None,
            (true, Some(b)) => Some(b),
            (true, None) => continue,
        };
        lights.push((light, bits));
    }
    let ambient = lighting.zone_ambient.get(zone).copied().unwrap_or([0.0; 3]).map(|c| (c * 255.0).round() as u8);
    let texels = manager::surface_lightmap(&s, &lights, ambient, lighting.level_brightness);
    Some(Built { surf: si, w: s.w, h: s.h, texels })
}

/// Builds the lightmap atlas of a model (uses all CPU cores).
pub fn generate(model: &Model, lighting: &MapLighting) -> Lightmaps {
    let n = model.surfs.len();
    // the zone a surface is lit in: the front zone of the first node drawing it
    let mut zone_of = vec![0usize; n];
    let mut seen = vec![false; n];
    for nd in &model.nodes {
        if nd.i_surf >= 0 && (nd.i_surf as usize) < n && !seen[nd.i_surf as usize] {
            seen[nd.i_surf as usize] = true;
            zone_of[nd.i_surf as usize] = nd.i_zone[1] as usize;
        }
    }
    let zone_of = &zone_of;
    let threads = std::thread::available_parallelism().map(|t| t.get()).unwrap_or(4).min(16);
    let mut built: Vec<Built> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                s.spawn(move || (t..n).step_by(threads).filter_map(|si| build_surface(model, si, zone_of[si], lighting)).collect::<Vec<_>>())
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    built.sort_by_key(|b| b.surf);
    let (width, height, placed) = pack(&built.iter().map(|b| (b.w, b.h)).collect::<Vec<_>>());
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let mut surfaces = vec![None; n];
    for (bi, px, py) in placed {
        let b = &built[bi];
        let lm = &model.light_map[model.surfs[b.surf].i_light_map as usize];
        surfaces[b.surf] = Some(SurfaceMap { x: px, y: py, w: b.w as u32, h: b.h as u32, pan: [lm.pan[0], lm.pan[1]], scale: [lm.u_scale, lm.v_scale] });
        for j in -1..=b.h as i32 {
            for i in -1..=b.w as i32 {
                let t = b.texels[(j.clamp(0, b.h as i32 - 1) as usize) * b.w + i.clamp(0, b.w as i32 - 1) as usize];
                let o = (((py as i32 + j) as u32 * width + (px as i32 + i) as u32) * 4) as usize;
                rgba[o..o + 4].copy_from_slice(&[t[0], t[1], t[2], 255]);
            }
        }
    }
    Lightmaps { width, height, rgba, surfaces }
}

/// Shelf packing of lightmap rectangles `(w, h)` (in surface order), tallest first, one replicated texel of border around
/// every rectangle: atlas width, height and each rectangle's index and top-left texel.
fn pack(sizes: &[(usize, usize)]) -> (u32, u32, Vec<(usize, u32, u32)>) {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&k| std::cmp::Reverse((sizes[k].1, sizes[k].0)));
    let width = 4096u32;
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    let mut placed = Vec::new();
    for k in order {
        let (pw, ph) = (sizes[k].0 as u32 + 2, sizes[k].1 as u32 + 2);
        if x + pw > width {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        placed.push((k, x + 1, y + 1));
        x += pw;
        shelf = shelf.max(ph);
    }
    (width, (y + shelf).max(1).next_power_of_two(), placed)
}

/// Where `generate` puts each surface of `model` in its atlas (`None` without a lightmap), without lighting anything.
pub fn layout(model: &Model) -> Vec<Option<SurfaceMap>> {
    let surfs: Vec<(usize, manager::Surface)> = (0..model.surfs.len()).filter_map(|si| surface_of(model, si).map(|s| (si, s))).collect();
    let (_, _, placed) = pack(&surfs.iter().map(|(_, s)| (s.w, s.h)).collect::<Vec<_>>());
    let mut out = vec![None; model.surfs.len()];
    for (k, x, y) in placed {
        let (si, s) = &surfs[k];
        out[*si] = Some(SurfaceMap { x, y, w: s.w as u32, h: s.h as u32, pan: s.pan, scale: s.scale });
    }
    out
}

/// World positions of the texels of surface `si` (row-major, `w * h`) and its unit normal. For checking the layout against the
/// BSP; `generate` computes the same positions.
pub fn texel_positions(model: &Model, si: usize) -> Option<(usize, usize, Vec<[f32; 3]>, [f32; 3])> {
    let surf = model.surfs.get(si)?;
    let lm = model.light_map.get(usize::try_from(surf.i_light_map).ok()?)?;
    let (w, h) = (lm.u_clamp.max(0) as usize, lm.v_clamp.max(0) as usize);
    let base = *model.points.get(surf.p_base as usize)?;
    let (u, v) = (*model.vectors.get(surf.v_texture_u as usize)?, *model.vectors.get(surf.v_texture_v as usize)?);
    let n = *model.vectors.get(surf.v_normal as usize)?;
    let nl = dot(n, n).sqrt();
    let n_unit = n.map(|c| c / nl);
    let inv = plane_solver(u, v, n_unit)?;
    let pos = (0..w * h)
        .map(|k| {
            let (tu, tv) = (lm.pan[0] + (k % w) as f32 * lm.u_scale, lm.pan[1] + (k / w) as f32 * lm.v_scale);
            let x = [0, 1, 2].map(|c| tu * inv[0][c] + tv * inv[1][c]);
            [base[0] + x[0], base[1] + x[1], base[2] + x[2]]
        })
        .collect();
    Some((w, h, pos, n_unit))
}

/// The shadow bit of texel `(i, j)` of the `k`-th light of surface `si` (`true` = lit), as stored in the map.
pub fn shadow_bit(model: &Model, si: usize, k: usize, i: usize, j: usize) -> Option<bool> {
    let lm = model.light_map.get(usize::try_from(model.surfs.get(si)?.i_light_map).ok()?)?;
    let (w, h) = (lm.u_clamp.max(0) as usize, lm.v_clamp.max(0) as usize);
    let row_bytes = (w + 7) / 8;
    let byte = model.light_bits.get(lm.data_offset.max(0) as usize + k * row_bytes * h + j * row_bytes + (i >> 3))?;
    Some((byte >> (i & 7)) & 1 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ue_assets::model::{LightMapIndex, Poly};

    /// A 64 x 64 square in the plane x = `x` facing `nx` (+1 or -1), lightmap `lm`.
    fn square(x: f32, nx: f32, lm: i32, flags: u32) -> Poly {
        Poly {
            base: [x, 0.0, 0.0],
            normal: [nx, 0.0, 0.0],
            texture_u: [0.0, 1.0, 0.0],
            texture_v: [0.0, 0.0, 1.0],
            vertices: vec![[x, 0.0, 0.0], [x, 64.0, 0.0], [x, 64.0, 64.0], [x, 0.0, 64.0]],
            poly_flags: flags,
            actor: 0,
            texture: 7,
            item_name: 0,
            i_link: 0,
            i_brush_poly: lm,
            pan_u: 3,
            pan_v: 4,
        }
    }

    fn brush_with_lightmaps(n: usize) -> Model {
        let lm = LightMapIndex { data_offset: 0, pan: [0.0; 3], u_clamp: 4, v_clamp: 4, u_scale: 16.0, v_scale: 16.0, i_light_actors: 0 };
        Model { light_map: vec![lm; n], light_bits: vec![0xff; 8], lights: vec![5, 0], ..Model::default() }
    }

    #[test]
    fn mover_model_makes_a_surface_per_polygon() {
        let brush = brush_with_lightmaps(2);
        // the second polygon is invisible and has no lightmap; flag 0x4000_0000 is cleared by the tracker's mask
        let m = mover_model(&brush, &[square(0.0, 1.0, 1, 0x4000_0002), square(10.0, -1.0, -1, 0x21)], true, 3);
        assert_eq!(m.surfs.len(), 2);
        assert_eq!(m.surfs[0].i_light_map, 1);
        assert_eq!(m.surfs[0].poly_flags, 0x10_0002);
        assert_eq!((m.surfs[0].texture, m.surfs[0].pan_u, m.surfs[0].pan_v), (7, 3, 4));
        assert_eq!(m.surfs[1].i_light_map, -1);
        let faces = m.faces();
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[1].vertices[2], [10.0, 64.0, 64.0]);
        assert_eq!(m.nodes[1].plane, [-1.0, 0.0, 0.0, -10.0]);
        assert_eq!(m.nodes[0].i_zone, [3, 3]);
        assert_eq!((m.lights.clone(), m.light_map.len()), (vec![5, 0], 2));
        let s = surface_of(&m, 0).unwrap();
        assert_eq!((s.w, s.h, s.n), (4, 4, [1.0, 0.0, 0.0]));
        assert!(surface_of(&m, 1).is_none());
    }

    #[test]
    fn layout_places_surfaces_where_generate_does() {
        // lightmaps of different sizes, one polygon without a lightmap
        let mut brush = brush_with_lightmaps(3);
        brush.light_map[1].v_clamp = 9;
        brush.light_map[2].u_clamp = 7;
        let polys = [square(0.0, 1.0, 0, 0), square(10.0, -1.0, -1, 0), square(20.0, 1.0, 1, 0), square(30.0, -1.0, 2, 0)];
        let m = mover_model(&brush, &polys, false, 0);
        let lighting = MapLighting { lights: vec![None; 2], zone_ambient: vec![[0.0; 3]], zone_pan: vec![[0.0; 2]], level_brightness: 1.0, shadowed: false, static_only: true };
        let from_generate: Vec<_> = generate(&m, &lighting).surfaces.iter().map(|s| s.map(|s| (s.x, s.y, s.w, s.h))).collect();
        let from_layout: Vec<_> = layout(&m).iter().map(|s| s.map(|s| (s.x, s.y, s.w, s.h))).collect();
        assert_eq!(from_layout, from_generate);
        // tallest first, then widest; a border texel around each
        assert_eq!(from_layout, vec![Some((16, 1, 4, 4)), None, Some((1, 1, 4, 9)), Some((7, 1, 7, 4))]);
    }

    #[test]
    fn leaf_lights_are_those_in_front_with_the_same_special_lit() {
        let brush = brush_with_lightmaps(2);
        let mut m = mover_model(&brush, &[square(0.0, 1.0, 0, 0), square(10.0, -1.0, 1, 0)], false, 0);
        let l = |x: f32, special_lit: bool| Light { pos: [x, 32.0, 32.0], brightness: 255, hue: 0, saturation: 255, radius_byte: 10, light_type: 1, effect: 0, cone: 0, rotation: [0; 3], dir: [1.0, 0.0, 0.0], flags: 0, period: 0, phase: 0, special_lit };
        // first square faces +x at x = 0, second faces -x at x = 10: x = 5 lights both, -5 the second, 0 (on the
        // first's plane) the second, and the special-lit one neither
        let table = leaf_light_lists(&mut m, &[l(5.0, false), l(-5.0, false), l(0.0, false), l(20.0, true)], false);
        assert_eq!(m.lights, vec![1, 0, 1, 1, 1, 0]);
        assert_eq!(m.light_map[0].i_light_actors, 0);
        assert_eq!(m.light_map[1].i_light_actors, 2);
        let xs: Vec<Option<f32>> = table.iter().map(|t| t.map(|l| l.pos[0])).collect();
        assert_eq!(xs, vec![Some(5.0), None, Some(5.0), Some(-5.0), Some(0.0), None]);
    }

    #[test]
    fn hsv_matches_the_engine_function() {
        // value 255: 1.4 -> 0.7 * 1.4 / (sqrt(1.4) + 0.01) = 0.8213; hue 0 is red, saturation 255 is white.
        let red = hsv_to_rgb(0, 0, 255);
        assert!((red[0] - 0.8213).abs() < 1e-3 && red[1] == 0.0 && red[2] == 0.0, "{red:?}");
        let white = hsv_to_rgb(0, 255, 255);
        assert!(white.iter().all(|&c| (c - 0.8213).abs() < 1e-3), "{white:?}");
        // hue 85 is green, hue 170 blue
        assert!(hsv_to_rgb(85, 0, 255)[1] > 0.82);
        assert!(hsv_to_rgb(170, 0, 255)[2] > 0.82);
        // value 0 is black, and the tone curve is concave: half the value gives much more than half the brightness
        assert_eq!(hsv_to_rgb(0, 255, 0), [0.0; 3]);
        assert!(hsv_to_rgb(0, 255, 64)[0] > 0.4);
    }

    #[test]
    fn plane_solver_inverts_the_texture_axes() {
        let (u, v, n) = ([1.0, 0.0, 0.0], [0.5, 1.0, 0.0], [0.0, 0.0, 1.0]);
        let inv = plane_solver(u, v, n).unwrap();
        let (tu, tv) = (3.0, 2.0);
        let x = [0, 1, 2].map(|k| tu * inv[0][k] + tv * inv[1][k]);
        assert!((dot(x, u) - tu).abs() < 1e-5 && (dot(x, v) - tv).abs() < 1e-5 && dot(x, n).abs() < 1e-5);
    }

    #[test]
    fn fog_rectangles_hold_the_doubled_bytes_with_a_border() {
        let m = SurfaceMap { x: 10, y: 20, w: 2, h: 1, pan: [0.0; 2], scale: [16.0; 2] };
        let (x, y, w, h, rgba) = fog_atlas_rect(&m, Some(&[[1, 2, 3, 127], [64, 0, 127, 10]]));
        assert_eq!((x, y, w, h), (9, 19, 4, 3));
        // every row: border copy of the first texel, both texels, border copy of the last
        let row = [2, 4, 6, 254, 2, 4, 6, 254, 128, 0, 254, 20, 128, 0, 254, 20];
        assert_eq!(rgba, row.repeat(3));
        // no fog: zeros, which the blend leaves as they are
        let (_, _, _, _, z) = fog_atlas_rect(&m, None);
        assert!(z.len() == 48 && z.iter().all(|&b| b == 0));
    }

    #[test]
    fn atlas_position_puts_texel_centres_at_half_integers() {
        let m = SurfaceMap { x: 10, y: 20, w: 4, h: 4, pan: [100.0, 200.0], scale: [16.0, 8.0] };
        assert_eq!(m.atlas_pos(100.0, 200.0), [10.5, 20.5]);
        assert_eq!(m.atlas_pos(100.0 + 3.0 * 16.0, 200.0 + 8.0), [13.5, 21.5]);
        // outside the rectangle the position is held half a texel past the edge (the replicated border)
        assert_eq!(m.atlas_pos(-1e6, 1e6), [10.0, 24.0]);
    }
}

/// The level surfaces the moving-brush tracker gives a mover (Engine 1037b5c0), kept in the brush's own space: one
/// surface and one node per brush polygon, flags `& 0x3cffffff` (`PF_SpecialLit` added for the mover's `bSpecialLit`),
/// the polygon's texture, pans and vectors, and `iLightMap = iBrushPoly`. That index is into the brush model's own
/// lightmaps, light list and shadow bits (Render's `SetupForSurf` reads `Surf.Actor->Brush` for a dynamic surface),
/// which come along. The nodes stand in zone `zone` (the engine's dynamic nodes take the zone of the level node side
/// they land in).
pub fn mover_model(brush: &Model, polys: &[ue_assets::model::Poly], special_lit: bool, zone: u8) -> Model {
    use ue_assets::model::{BspNode, BspSurf, Vert};
    let mut m = Model { light_map: brush.light_map.clone(), light_bits: brush.light_bits.clone(), lights: brush.lights.clone(), ..Model::default() };
    for (k, p) in polys.iter().enumerate() {
        let mut vec = |v: [f32; 3]| {
            m.vectors.push(v);
            (m.vectors.len() - 1) as i32
        };
        let (v_normal, v_texture_u, v_texture_v) = (vec(p.normal), vec(p.texture_u), vec(p.texture_v));
        m.points.push(p.base);
        let p_base = (m.points.len() - 1) as i32;
        let mut poly_flags = p.poly_flags & 0x3cff_ffff;
        if special_lit {
            poly_flags |= 0x10_0000;
        }
        m.surfs.push(BspSurf { texture: p.texture, poly_flags, p_base, v_normal, v_texture_u, v_texture_v, i_light_map: p.i_brush_poly, i_brush_poly: k as i32, pan_u: p.pan_u, pan_v: p.pan_v, actor: 0 });
        let i_vert_pool = m.verts.len() as i32;
        for &v in &p.vertices {
            m.points.push(v);
            m.verts.push(Vert { p_vertex: (m.points.len() - 1) as i32, i_side: -1 });
        }
        let w = p.normal[0] * p.base[0] + p.normal[1] * p.base[1] + p.normal[2] * p.base[2];
        m.nodes.push(BspNode {
            plane: [p.normal[0], p.normal[1], p.normal[2], w],
            zone_mask: 0,
            node_flags: 0,
            i_vert_pool,
            i_surf: k as i32,
            i_back: -1,
            i_front: -1,
            i_plane: -1,
            i_collision_bound: -1,
            i_render_bound: -1,
            i_zone: [zone, zone],
            num_vertices: p.vertices.len().min(255) as u8,
            i_leaf: [-1, -1],
        });
    }
    m
}

/// A `bDynamicLightMover`'s lights (`SetupForSurf`, Render 10b06c90): not its stored list but every light permeating
/// the mover's leaf (`Region.iLeaf`) that stands in front of the surface (`(Location - Base) . Normal > 0`) and has the
/// mover's `bSpecialLit`, unshadowed. `lights` must be in the model's space (the side test is unchanged by the
/// brush's frame). Writes a list per lit surface into `model.lights` and points the surface's lightmap at it; returns
/// the light table that goes with it (`MapLighting::lights`, `None` at the separators).
pub fn leaf_light_lists(model: &mut Model, lights: &[Light], special_lit: bool) -> Vec<Option<Light>> {
    let mut table = Vec::new();
    model.lights.clear();
    for si in 0..model.surfs.len() {
        let Some(s) = surface_of(model, si) else { continue };
        let start = model.lights.len() as i32;
        for l in lights {
            let d = [l.pos[0] - s.origin[0], l.pos[1] - s.origin[1], l.pos[2] - s.origin[2]];
            if 0.0 < d[0] * s.n[0] + d[2] * s.n[2] + d[1] * s.n[1] && l.special_lit == special_lit {
                model.lights.push(1);
                table.push(Some(*l));
            }
        }
        model.lights.push(0);
        table.push(None);
        let lm = model.surfs[si].i_light_map as usize;
        model.light_map[lm].i_light_actors = start;
    }
    table
}

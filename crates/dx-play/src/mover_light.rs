//! Movers' lightmaps per frame: Render.dll `FLightManager::SetupForSurf` (10b06c90) for a surface the moving-brush
//! tracker calls dynamic. Such a surface is lit in the level model's frame (the tracker writes the brush polygon's
//! current world-space base and vectors into it) with the lightmap `iLightMap = iBrushPoly` of the mover's own brush
//! model: that model's light list and shadow bits, or for a `bDynamicLightMover` the lights permeating its leaf.
//!
//! - The static part (the static lights over the zone's ambient) is a cache item keyed by brush model, lightmap and
//!   zone that also stores the mover's `Region.iLeaf`, `Location` and `Rotation`; when any of them differs it is made
//!   again, so a door's static light follows it as it moves.
//! - Per-frame lights merge on top every frame as on level surfaces. A light's illumination is a cache item keyed by
//!   light and surface index only, so a moving mover keeps the illumination its surface got when that light was first
//!   merged (time-dependent effects excepted: they run every frame). The engine's cache can evict items; the port's
//!   never does.

use crate::Session;
use dx_light::manager::{self, Illumination, LightKind};
use std::collections::{HashMap, HashSet};

type Rect = (u32, u32, u32, u32, Vec<u8>);

/// What a mover surface's static cache item was made for: the mover's `Region.iLeaf`, `Location`, `Rotation` (kept in
/// the item) and the zone (part of its key).
type Tag = (i32, [f32; 3], [i32; 3], u8);

struct BrushLight {
    model: ue_assets::model::Model,
    /// The live light of each entry of the brush model's light list (0: none).
    actors: Vec<u32>,
    /// Where each brush polygon sits in the mover's lightmap atlas (`dx_light::generate` of `dx_light::mover_model`).
    layout: Vec<Option<dx_light::SurfaceMap>>,
}

#[derive(Default)]
pub(crate) struct MoverLight {
    /// Per brush model export (`None`: not lightable).
    brushes: HashMap<usize, Option<BrushLight>>,
    /// Level actors by object path (resolves brush light lists).
    by_path: Option<HashMap<String, u32>>,
    /// Per dynamic surface: brush export, the static cache item's tag and texels.
    statics: HashMap<usize, (usize, Tag, Vec<[u8; 3]>)>,
    /// Per (dynamic surface, light): the illumination of a per-frame light.
    cache: HashMap<(usize, u32), Option<Illumination>>,
    /// Dynamic surfaces whose atlas texels differ from their static part.
    changed: HashSet<usize>,
    /// Atlas rectangles for the movers' worlds: brush export and rectangle.
    pub pending: Vec<(usize, Rect)>,
    /// Brush polygons (frame index, export, polygon) whose rectangle in that frame's fog atlas of the brush's world
    /// holds fog, and the fog rectangles to upload (export, frame index, rectangle).
    fogged: HashSet<(usize, usize, usize)>,
    pub fog_pending: Vec<(usize, usize, Rect)>,
}

impl MoverLight {
    /// Brush `export` got a fresh world in the renderer: its surfaces are sent again when next drawn.
    pub fn forget_brush(&mut self, export: usize) {
        let gone: Vec<usize> = self.statics.iter().filter(|(_, v)| v.0 == export).map(|(&k, _)| k).collect();
        for si in gone {
            self.statics.remove(&si);
            self.changed.remove(&si);
        }
        // a fresh world's fog atlas is empty
        self.fogged.retain(|&(_, e, _)| e != export);
    }

    /// The brush polygons fogged this frame are `now` (per frame index): the others that held fog get their rectangle
    /// cleared.
    pub fn clear_fog_except(&mut self, now: HashSet<(usize, usize, usize)>) {
        for &(fi, export, k) in self.fogged.difference(&now) {
            if let Some(sm) = self.brushes.get(&export).and_then(|b| b.as_ref()).and_then(|b| b.layout.get(k).copied().flatten()) {
                self.fog_pending.push((export, fi, dx_light::fog_atlas_rect(&sm, None)));
            }
        }
        self.fogged = now;
    }
}

impl Session {
    /// The brush model export (map package) of mover `a`.
    pub(crate) fn mover_brush_export(&mut self, a: u32) -> Option<usize> {
        let brush = self.vm.op(a, "Brush");
        let path = self.vm.reg.obj_path(ue_registry::ObjId(brush)).to_string();
        let (pi, export) = self.vm.reg.export_by_path(&path)?;
        (pi == self.map_pkg).then_some(export)
    }

    fn ensure_brush_light(&mut self, ml: &mut MoverLight, export: usize) {
        if ml.brushes.contains_key(&export) {
            return;
        }
        let entry = self.view.model_at(export).map(|model| {
            let by_path = ml.by_path.get_or_insert_with(|| self.vm.level_paths.iter().map(|(&a, p)| (p.clone(), a)).collect());
            let actors = model.lights.iter().map(|&raw| if raw == 0 { 0 } else { self.vm.reg.ref_path(self.map_pkg, raw).and_then(|p| by_path.get(&p).copied()).unwrap_or(0) }).collect();
            let polys = self.view.brush_polys(export).unwrap_or_default();
            let layout = dx_light::layout(&dx_light::mover_model(&model, &polys, false, 0));
            BrushLight { model, actors, layout }
        });
        ml.brushes.insert(export, entry);
    }

    /// Dynamic surface `si` of the level model as `SetupForSurf` lights it: its mover's brush export, brush polygon,
    /// place in that brush's lightmap atlas and the surface (the tracker's world-space frame, the brush's lightmap).
    pub(crate) fn mover_surface(&mut self, ml: &mut MoverLight, si: usize) -> Option<(usize, usize, dx_light::SurfaceMap, manager::Surface)> {
        let (a, k) = self.brush_tracker.as_ref()?.1.surf_owner(si)?;
        if !self.vm.is_live(a) {
            return None;
        }
        let export = self.mover_brush_export(a)?;
        self.ensure_brush_light(ml, export);
        let b = ml.brushes.get(&export)?.as_ref()?;
        let sm = b.layout.get(k).copied().flatten()?;
        let surf = dx_light::surface_in(self.vm.world_model.as_deref()?, si, &b.model)?;
        Some((export, k, sm, surf))
    }

    /// The lightmaps of the movers' surfaces drawn this frame (`drawn`: dynamic surface and the zone of its first draw;
    /// `filed`: the dynamic lights the visibility walk filed per surface). Rectangles go to `ml.pending`.
    pub(crate) fn update_mover_light(&mut self, ml: &mut MoverLight, lighting: &dx_light::MapLighting, drawn: &[(usize, u8)], filed: &HashMap<usize, Vec<u32>>, time: f32) {
        let Some(world) = self.vm.world_model.clone() else { return };
        let owners: Vec<(usize, u8, u32, usize)> = match &self.brush_tracker {
            Some((_, t, _)) => drawn.iter().filter_map(|&(si, z)| t.surf_owner(si).map(|(a, k)| (si, z, a, k))).collect(),
            None => return,
        };
        let level = self.vm.level;
        let level_brightness = self.vm.fp(level, "Brightness");
        for (si, zone, a, k) in owners {
            if !self.vm.is_live(a) {
                continue;
            }
            let Some(export) = self.mover_brush_export(a) else { continue };
            self.ensure_brush_light(ml, export);
            let Some(Some(b)) = ml.brushes.get(&export) else { continue };
            let Some(sm) = b.layout.get(k).copied().flatten() else { continue };
            let Some(surf) = dx_light::surface_in(&world, si, &b.model) else { continue };
            let dynamic_mover = self.vm.bp(a, "bDynamicLightMover");
            let special_lit = self.vm.bp(a, "bSpecialLit");
            let leaf = self.vm.region_leaf(a);
            // the surface's light list: the brush's stored list with its shadow bits, or the lights permeating the
            // mover's leaf that stand in front of the surface and share its bSpecialLit, unshadowed
            let mut list: Vec<(u32, Option<&[u8]>)> = Vec::new();
            if !dynamic_mover {
                for (at, bits) in dx_light::lightmap_lights(&b.model, world.surfs[si].i_light_map as usize) {
                    list.push((b.actors.get(at).copied().unwrap_or(0), bits));
                }
            } else if let Some(l) = usize::try_from(leaf).ok().and_then(|l| world.leaves.get(l)) {
                if l.i_permeating >= 0 {
                    let mut at = l.i_permeating as usize;
                    while world.lights.get(at).copied().unwrap_or(0) != 0 {
                        let la = self.vm.model_light_actors.get(at).copied().unwrap_or(0);
                        if la != 0 && self.vm.is_live(la) {
                            let p = self.vm.location(la);
                            let d = [p[0] - surf.origin[0], p[1] - surf.origin[1], p[2] - surf.origin[2]];
                            if 0.0 < d[0] * surf.n[0] + d[2] * surf.n[2] + d[1] * surf.n[1] && self.vm.bp(la, "bSpecialLit") == special_lit {
                                list.push((la, None));
                            }
                        }
                        at += 1;
                    }
                }
            }
            // "add light" (10b08b30) sorts them: static ones into the cached map, the others merged every frame
            let mut statics: Vec<(dx_light::Light, Option<&[u8]>)> = Vec::new();
            let mut per_frame: Vec<(u32, dx_light::Light, Option<&[u8]>)> = Vec::new();
            let mut fresh: Vec<(Illumination, [f32; 3])> = Vec::new();
            for &(la, bits) in &list {
                if la == 0 || la == a || !self.vm.is_live(la) {
                    continue;
                }
                let l = self.live_light(la);
                match manager::light_kind(&l) {
                    LightKind::None => {}
                    LightKind::Static => statics.push((l, bits)),
                    LightKind::PerFrame => per_frame.push((la, l, bits)),
                    LightKind::Dynamic => {
                        let br = self.light_brightness_now(la, &l, time);
                        fresh.push((manager::illumination_at(&l, &surf, None, time), manager::light_colour(&l, br, level_brightness)));
                    }
                }
            }
            for &la in filed.get(&si).map(|v| v.as_slice()).unwrap_or(&[]) {
                if la == a || !self.vm.is_live(la) {
                    continue;
                }
                let l = self.live_light(la);
                // a bDynamicLightMover takes every filed light of its bSpecialLit as dynamic
                let dynamic = if dynamic_mover { l.light_type != 0 && l.brightness != 0 && l.special_lit == special_lit } else { manager::light_kind(&l) == LightKind::Dynamic };
                if dynamic {
                    let br = self.light_brightness_now(la, &l, time);
                    fresh.push((manager::illumination_at(&l, &surf, None, time), manager::light_colour(&l, br, level_brightness)));
                }
            }
            // the static part: made again when the mover's leaf, location or rotation (or the zone) changed
            let tag: Tag = (leaf, self.vm.location(a), self.vm.rotation(a), zone);
            let rebuilt = ml.statics.get(&si).map(|s| s.1 != tag).unwrap_or(true);
            if rebuilt {
                let ambient = lighting.zone_ambient.get(zone as usize).copied().unwrap_or([0.0; 3]).map(|c| (c * 255.0).round() as u8);
                let refs: Vec<(&dx_light::Light, Option<&[u8]>)> = statics.iter().map(|(l, bits)| (l, *bits)).collect();
                let texels = manager::surface_lightmap(&surf, &refs, ambient, lighting.level_brightness);
                ml.statics.insert(si, (export, tag, texels));
            }
            let mut lights: Vec<(u32, [f32; 3])> = Vec::new();
            for (la, l, bits) in per_frame {
                if manager::EFFECT_ANIMATED.get(l.effect as usize).copied().unwrap_or(false) {
                    ml.cache.remove(&(si, la));
                }
                let entry = ml.cache.entry((si, la)).or_insert_with(|| Some(manager::illumination_at(&l, &surf, bits, time)));
                if entry.is_some() {
                    let br = self.light_brightness_now(la, &l, time);
                    lights.push((la, manager::light_colour(&l, br, level_brightness)));
                }
            }
            let static_texels = &ml.statics[&si].2;
            if lights.is_empty() && fresh.is_empty() {
                if ml.changed.remove(&si) || rebuilt {
                    ml.pending.push((export, dx_light::atlas_rect(&sm, static_texels)));
                }
                continue;
            }
            let mut with: Vec<(&Illumination, [f32; 3])> = lights.iter().filter_map(|(la, c)| ml.cache.get(&(si, *la)).and_then(|e| e.as_ref()).map(|e| (e, *c))).collect();
            with.extend(fresh.iter().map(|(e, c)| (e, *c)));
            let texels = manager::frame_lightmap(&surf, static_texels, &with, &self.vm.randoms);
            ml.pending.push((export, dx_light::atlas_rect(&sm, &texels)));
            ml.changed.insert(si);
        }
    }
}

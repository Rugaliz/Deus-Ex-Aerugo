//! The per-frame part of the world's lightmaps (Render.dll `FLightManager::SetupForSurf`): every frame, each drawn
//! surface whose stored light list has lights that are not baked into the static map (animated light types, waver
//! effects, lights that are not static) gets its static texels plus those lights merged with their brightness of the
//! moment (`GlobalLighting`). Their illumination is computed once and cached, as the engine does. The new texels go to
//! the renderer's lightmap atlas.

use crate::Session;
use dx_light::manager::{self, Illumination};
use std::collections::{HashMap, HashSet};

pub struct WorldLight {
    pkg: usize,
    lighting: dx_light::MapLighting,
    maps: dx_light::Lightmaps,
    cache: HashMap<(usize, usize), Option<Illumination>>,
    /// Per surface: its stored light list (index into `Model::lights`, range of its shadow bits in `Model::light_bits`).
    lists: HashMap<usize, Vec<(usize, Option<(usize, usize)>)>>,
    /// Surfaces whose atlas texels differ from the static map (to restore them when their lights stop).
    changed: HashSet<usize>,
    pub pending: Vec<(u32, u32, u32, u32, Vec<u8>)>,
    /// Per frame index (0 the main view, then its child frames): the surfaces whose rectangle in that frame's fog atlas
    /// holds fog (cleared when their fog stops); the fog rectangles to upload, the main view's and the child frames'.
    fogged: HashMap<usize, HashSet<usize>>,
    pub fog_pending: Vec<(u32, u32, u32, u32, Vec<u8>)>,
    pub child_fog_pending: Vec<(usize, (u32, u32, u32, u32, Vec<u8>))>,
    /// The movers' surfaces (the brush tracker's dynamic ones).
    pub(crate) movers: crate::mover_light::MoverLight,
}

impl Session {
    /// A light as the light manager reads it from the live actor.
    pub(crate) fn live_light(&mut self, a: u32) -> dx_light::Light {
        let vm = &mut self.vm;
        let rot = vm.rotation(a);
        let (sp, cp) = (manager::sin_tab(rot[0]), manager::sin_tab(rot[0].wrapping_add(16384)));
        let (sy, cy) = (manager::sin_tab(rot[1]), manager::sin_tab(rot[1].wrapping_add(16384)));
        let flag = |vm: &mut ue_vm::Vm, n: &'static str, b: u32| if vm.bp(a, n) { b } else { 0 };
        dx_light::Light {
            pos: vm.location(a),
            brightness: vm.u8p(a, "LightBrightness"),
            hue: vm.u8p(a, "LightHue"),
            saturation: vm.u8p(a, "LightSaturation"),
            radius_byte: vm.u8p(a, "LightRadius"),
            light_type: vm.u8p(a, "LightType"),
            effect: vm.u8p(a, "LightEffect"),
            cone: vm.u8p(a, "LightCone"),
            rotation: rot,
            dir: [cp * cy, cp * sy, sp],
            flags: flag(vm, "bStatic", manager::F_STATIC) | flag(vm, "bNoDelete", manager::F_NO_DELETE) | flag(vm, "bDynamicLight", manager::F_DYNAMIC_LIGHT),
            period: vm.u8p(a, "LightPeriod"),
            phase: vm.u8p(a, "LightPhase"),
            special_lit: vm.bp(a, "bSpecialLit"),
        }
    }

    /// Per drawn frame (after the visibility pass): the renderer's random tables step on, then the per-frame lightmaps
    /// of the surfaces drawn are redone (only once a renderer with lightmaps exists).
    pub(crate) fn update_world_light(&mut self) {
        let time = self.level_time();
        let mut randoms = std::mem::take(&mut self.vm.randoms);
        randoms.tick(time, &mut || self.vm.frand());
        self.vm.randoms = randoms;
        if !self.wants_world_light {
            return;
        }
        if self.world_light.as_ref().map(|w| w.pkg != self.map_pkg).unwrap_or(true) {
            let lighting = dx_light::collect(&mut self.vm.reg, self.map_pkg, &self.view.model);
            let maps = dx_light::generate(&self.view.model, &lighting);
            self.world_light = Some(WorldLight { pkg: self.map_pkg, lighting, maps, cache: HashMap::new(), lists: HashMap::new(), changed: HashSet::new(), pending: Vec::new(), fogged: HashMap::new(), fog_pending: Vec::new(), child_fog_pending: Vec::new(), movers: Default::default() });
        }
        // the surfaces drawn by any frame of the visibility pass, and the dynamic lights the walk filed onto them
        // (the walk runs on the level model with the movers filtered in: the tracker's surfaces go to the movers)
        let mut surfs: Vec<usize> = Vec::new();
        let mut mover_surfs: Vec<(usize, u8)> = Vec::new();
        let mut dyn_lights: HashMap<usize, Vec<u32>> = HashMap::new();
        let first_dynamic = self.brush_tracker.as_ref().map(|t| t.1.first_dynamic_surf).unwrap_or(usize::MAX);
        if let Some((_, frames)) = &self.vis_cache {
            let model = self.vm.world_model.as_deref().unwrap_or(&self.view.model);
            let mut seen = HashSet::new();
            for fv in frames {
                for &(node, _, z) in &fv.vis.surfaces {
                    let s = model.nodes.get(node as usize).map(|n| n.i_surf).unwrap_or(-1);
                    if s >= 0 && seen.insert(s as usize) {
                        if s as usize >= first_dynamic {
                            mover_surfs.push((s as usize, z));
                        } else {
                            surfs.push(s as usize);
                        }
                    }
                }
                for &(si, id) in &fv.vis.surf_lights {
                    let v = dyn_lights.entry(si as usize).or_default();
                    if !v.contains(&id) {
                        v.push(id);
                    }
                }
            }
        }
        let level = self.vm.level;
        let level_brightness = self.vm.fp(level, "Brightness");
        let mut wl = self.world_light.take().unwrap();
        for si in surfs {
            let Some(sm) = wl.maps.surfaces.get(si).copied().flatten() else { continue };
            let Some(surf) = dx_light::surface_of(&self.view.model, si) else { continue };
            let model = &self.view.model;
            let list = wl
                .lists
                .entry(si)
                .or_insert_with(|| {
                    let base = model.light_bits.as_ptr() as usize;
                    dx_light::surface_lights(model, si).into_iter().map(|(at, b)| (at, b.map(|b| (b.as_ptr() as usize - base, b.len())))).collect()
                })
                .clone();
            let mut lights: Vec<(usize, [f32; 3])> = Vec::new();
            // lights merged without a cache this frame (dynamic: unshadowed, computed every frame)
            let mut fresh: Vec<(Illumination, [f32; 3])> = Vec::new();
            for (at, bits) in &list {
                let a = self.vm.model_light_actors.get(*at).copied().unwrap_or(0);
                if a == 0 || !self.vm.is_live(a) {
                    continue;
                }
                let l = self.live_light(a);
                match manager::light_kind(&l) {
                    manager::LightKind::None | manager::LightKind::Static => continue,
                    manager::LightKind::Dynamic => {
                        let b = self.light_brightness_now(a, &l, time);
                        fresh.push((manager::illumination_at(&l, &surf, None, time), manager::light_colour(&l, b, level_brightness)));
                        continue;
                    }
                    manager::LightKind::PerFrame => {}
                }
                let model = &self.view.model;
                // time-dependent effects run their spatial function every frame; the others are cached
                let animated = manager::EFFECT_ANIMATED.get(l.effect as usize).copied().unwrap_or(false);
                if animated {
                    wl.cache.remove(&(si, *at));
                }
                let entry = wl.cache.entry((si, *at)).or_insert_with(|| bits.map(|(o, n)| manager::illumination_at(&l, &surf, Some(&model.light_bits[o..o + n]), time)));
                if entry.is_none() {
                    continue;
                }
                let b = self.light_brightness_now(a, &l, time);
                lights.push((*at, manager::light_colour(&l, b, level_brightness)));
            }
            for &a in dyn_lights.get(&si).map(|v| v.as_slice()).unwrap_or(&[]) {
                if !self.vm.is_live(a) {
                    continue;
                }
                let l = self.live_light(a);
                if manager::light_kind(&l) == manager::LightKind::Dynamic {
                    let b = self.light_brightness_now(a, &l, time);
                    fresh.push((manager::illumination_at(&l, &surf, None, time), manager::light_colour(&l, b, level_brightness)));
                }
            }
            // a surface no light changes keeps its static map: put back only if a light changed it before (the texels
            // are decoded only when needed)
            if lights.is_empty() && fresh.is_empty() {
                if wl.changed.remove(&si) {
                    if let Some(static_texels) = wl.maps.texels(si) {
                        wl.pending.push(dx_light::atlas_rect(&sm, &static_texels));
                    }
                }
                continue;
            }
            let Some(static_texels) = wl.maps.texels(si) else { continue };
            let mut with: Vec<(&Illumination, [f32; 3])> = lights.iter().filter_map(|(at, c)| wl.cache.get(&(si, *at)).and_then(|e| e.as_ref()).map(|e| (e, *c))).collect();
            with.extend(fresh.iter().map(|(e, c)| (e, *c)));
            let texels = manager::frame_lightmap(&surf, &static_texels, &with, &self.vm.randoms);
            wl.pending.push(dx_light::atlas_rect(&sm, &texels));
            wl.changed.insert(si);
        }
        let mut movers = std::mem::take(&mut wl.movers);
        self.update_mover_light(&mut movers, &wl.lighting, &mover_surfs, &dyn_lights, time);
        wl.movers = movers;
        self.world_light = Some(wl);
    }

    /// A light's brightness now (`GlobalLighting`'s type function, shared randoms and strobe).
    pub(crate) fn light_brightness_now(&mut self, a: u32, l: &dx_light::Light, time: f32) -> f32 {
        let mut strobe = self.vm.strobe;
        let b = ue_assets::light::light_type_brightness(l.light_type, l.brightness as f32 * (1.0 / 255.0f32), l.period, l.phase, a as u8, time, &self.vm.randoms, &mut strobe);
        self.vm.strobe = strobe;
        b
    }

    /// The main view's draws that get a fog map: `frame_fog_draws(0)`.
    pub fn fog_draws(&mut self) -> Vec<(usize, Vec<u32>)> {
        self.frame_fog_draws(0)
    }

    /// The draws of frame `fi` of the last visibility pass (0 the main view, then its mirror and sky frames) that get a
    /// fog map (`SetupForSurf`, 10b06c90, with that frame's camera): one draw list per surface and zone (`FBspDrawList`,
    /// its poly flags those of its first piece) of a surface with a lightmap (movers' surfaces too: theirs is the
    /// brush's), whose zone is a fog zone, that has fog lights and is not translucent. Per surface (index in the level
    /// model the walk ran on) the first such draw (an atlas holds one fog map per surface) and its fog lights.
    pub fn frame_fog_draws(&mut self, fi: usize) -> Vec<(usize, Vec<u32>)> {
        let mut cands: Vec<(usize, u8, Vec<u32>)> = Vec::new();
        {
            let Some(f) = self.vis_frames().get(fi) else { return Vec::new() };
            let model = self.vm.world_model.as_deref().unwrap_or(&self.view.model);
            let mut seen = HashSet::new();
            for &(node, flags, z) in &f.vis.surfaces {
                let si = model.nodes.get(node as usize).map(|n| n.i_surf).unwrap_or(-1);
                if si < 0 || !seen.insert((si, z)) || model.surfs.get(si as usize).map(|s| s.i_light_map < 0).unwrap_or(true) {
                    continue;
                }
                let ids: Vec<u32> = f.vis.surf_vol_lights.iter().filter(|&&(s, sz, _)| s == si as u32 && sz == z).map(|&(_, _, id)| id).collect();
                if !ids.is_empty() && flags & dx_vis::walk::PF_TRANSLUCENT == 0 {
                    cands.push((si as usize, z, ids));
                }
            }
        }
        let mut out: Vec<(usize, Vec<u32>)> = Vec::new();
        for (si, z, ids) in cands {
            let zone = self.vm.zone_actors.get(z as usize).copied().filter(|&a| a != 0).unwrap_or(self.vm.level);
            if zone != 0 && self.vm.bp(zone, "bFogZone") && !out.iter().any(|(s, _)| *s == si) {
                out.push((si, ids));
            }
        }
        out
    }

    /// The main view's fog map of surface `si`: `frame_fog_texels(0, si)`.
    pub fn fog_texels(&mut self, si: usize) -> Option<Vec<[u8; 4]>> {
        self.frame_fog_texels(0, si)
    }

    /// The fog map of surface `si` in frame `fi` of the last visibility pass (`SetupForSurf`'s volumetric part): the
    /// frame's camera and the fog lights of its draw; `None` when the surface gets none there or has no lightmap.
    /// 7-bit RGBA in the lightmap's `w * h` (a mover's surface: its brush lightmap's).
    pub fn frame_fog_texels(&mut self, fi: usize, si: usize) -> Option<Vec<[u8; 4]>> {
        let ids = self.frame_fog_draws(fi).into_iter().find(|(s, _)| *s == si)?.1;
        let surf = if si < self.first_dynamic_surf() {
            dx_light::surface_of(&self.view.model, si)?
        } else {
            let mut ml = self.world_light.as_mut().map(|w| std::mem::take(&mut w.movers)).unwrap_or_default();
            let found = self.mover_surface(&mut ml, si);
            if let Some(w) = self.world_light.as_mut() {
                w.movers = ml;
            }
            found?.3
        };
        Some(self.fog_map_with(fi, &surf, &ids))
    }

    /// The level model's first dynamic surface (the moving-brush tracker's), `usize::MAX` without a tracker.
    pub(crate) fn first_dynamic_surf(&self) -> usize {
        self.brush_tracker.as_ref().map(|t| t.1.first_dynamic_surf).unwrap_or(usize::MAX)
    }

    fn fog_map_with(&mut self, fi: usize, surf: &manager::Surface, ids: &[u32]) -> Vec<[u8; 4]> {
        let Some(frame) = self.vis_frames().get(fi).map(|f| f.frame.clone()) else { return vec![[0; 4]; surf.w * surf.h] };
        let time = self.level_time();
        let level = self.vm.level;
        let level_brightness = self.vm.fp(level, "Brightness");
        let mut lights = Vec::new();
        let mut seen = HashSet::new();
        for &a in ids {
            if !seen.insert(a) || !self.vm.is_live(a) {
                continue;
            }
            let l = self.live_light(a);
            let b = self.light_brightness_now(a, &l, time);
            let d = [l.pos[0] - frame.origin[0], l.pos[1] - frame.origin[1], l.pos[2] - frame.origin[2]];
            let ax = |v: [f32; 3]| (d[2] * v[2] + d[1] * v[1]) + d[0] * v[0];
            lights.push(manager::FogLight {
                view: [ax(frame.x_axis), ax(frame.y_axis), ax(frame.z_axis)],
                volume_radius: self.vm.u8p(a, "VolumeRadius"),
                volume_brightness: self.vm.u8p(a, "VolumeBrightness"),
                volume_fog: self.vm.u8p(a, "VolumeFog"),
                brightness: b,
                colour: manager::light_colour(&l, b, level_brightness),
            });
        }
        let mut skip = self.fog_skip;
        let out = manager::fog_map(surf, [frame.origin, frame.x_axis, frame.y_axis, frame.z_axis], &lights, &mut skip);
        self.fog_skip = skip;
        out
    }

    /// Per drawn frame, after the per-frame lightmaps: the fog maps of every frame's fog draws (they depend on the
    /// frame's camera, so they are redone every frame), each frame into its own fog atlas (the main view's, or the one
    /// the renderer keeps for that child frame index), a mover's surface's into its brush's world; surfaces that had
    /// fog in a frame and have none now are cleared there. The frames are made in the walk's order, so the light
    /// manager's "misses the volume" shortcut flag carries over from one to the next as in the engine.
    pub(crate) fn update_fog(&mut self) {
        let Some(mut wl) = self.world_light.take() else { return };
        let first_dynamic = self.first_dynamic_surf();
        let frames = self.vis_frames().len();
        let mut now: HashMap<usize, HashSet<usize>> = HashMap::new();
        let mut now_movers = HashSet::new();
        for fi in 0..frames {
            for (si, ids) in self.frame_fog_draws(fi) {
                if si < first_dynamic {
                    let Some(sm) = wl.maps.surfaces.get(si).copied().flatten() else { continue };
                    let Some(surf) = dx_light::surface_of(&self.view.model, si) else { continue };
                    let texels = self.fog_map_with(fi, &surf, &ids);
                    let rect = dx_light::fog_atlas_rect(&sm, Some(&texels));
                    if fi == 0 {
                        wl.fog_pending.push(rect);
                    } else {
                        wl.child_fog_pending.push((fi, rect));
                    }
                    now.entry(fi).or_default().insert(si);
                } else if let Some((export, k, sm, surf)) = self.mover_surface(&mut wl.movers, si) {
                    let texels = self.fog_map_with(fi, &surf, &ids);
                    wl.movers.fog_pending.push((export, fi, dx_light::fog_atlas_rect(&sm, Some(&texels))));
                    now_movers.insert((fi, export, k));
                }
            }
        }
        for (&fi, had) in &wl.fogged {
            let empty = HashSet::new();
            for &si in had.difference(now.get(&fi).unwrap_or(&empty)) {
                if let Some(sm) = wl.maps.surfaces.get(si).copied().flatten() {
                    let rect = dx_light::fog_atlas_rect(&sm, None);
                    if fi == 0 {
                        wl.fog_pending.push(rect);
                    } else {
                        wl.child_fog_pending.push((fi, rect));
                    }
                }
            }
        }
        wl.fogged = now;
        wl.movers.clear_fog_except(now_movers);
        self.world_light = Some(wl);
    }

    /// The fog atlas rectangles made since the last draw, for the renderer.
    pub(crate) fn take_fog_updates(&mut self) -> Vec<(u32, u32, u32, u32, Vec<u8>)> {
        self.world_light.as_mut().map(|w| std::mem::take(&mut w.fog_pending)).unwrap_or_default()
    }

    /// Takes the pending fog rectangles (tests, tools).
    pub fn fog_updates(&mut self) -> Vec<(u32, u32, u32, u32, Vec<u8>)> {
        self.take_fog_updates()
    }

    /// The lightmap rectangles redone since the last draw, for the renderer.
    pub(crate) fn take_lightmap_updates(&mut self) -> Vec<(u32, u32, u32, u32, Vec<u8>)> {
        self.world_light.as_mut().map(|w| std::mem::take(&mut w.pending)).unwrap_or_default()
    }

    /// The movers' lightmap rectangles redone since the last draw (brush export, rectangle), for the renderer.
    pub(crate) fn take_mover_lightmap_updates(&mut self) -> Vec<(usize, (u32, u32, u32, u32, Vec<u8>))> {
        self.world_light.as_mut().map(|w| std::mem::take(&mut w.movers.pending)).unwrap_or_default()
    }

    /// The movers' fog rectangles made since the last draw (brush export, frame index, rectangle), for the renderer.
    pub(crate) fn take_mover_fog_updates(&mut self) -> Vec<(usize, usize, (u32, u32, u32, u32, Vec<u8>))> {
        self.world_light.as_mut().map(|w| std::mem::take(&mut w.movers.fog_pending)).unwrap_or_default()
    }

    /// Takes the pending mover fog rectangles (tests, tools).
    pub fn mover_fog_updates(&mut self) -> Vec<(usize, usize, (u32, u32, u32, u32, Vec<u8>))> {
        self.take_mover_fog_updates()
    }

    /// The child frames' fog rectangles made since the last draw (frame index, rectangle), for the renderer.
    pub(crate) fn take_child_fog_updates(&mut self) -> Vec<(usize, (u32, u32, u32, u32, Vec<u8>))> {
        self.world_light.as_mut().map(|w| std::mem::take(&mut w.child_fog_pending)).unwrap_or_default()
    }

    /// Takes the pending child frame fog rectangles (tests, tools).
    pub fn child_fog_updates(&mut self) -> Vec<(usize, (u32, u32, u32, u32, Vec<u8>))> {
        self.take_child_fog_updates()
    }

    /// Takes the pending mover lightmap rectangles (tests, tools).
    pub fn mover_lightmap_updates(&mut self) -> Vec<(usize, (u32, u32, u32, u32, Vec<u8>))> {
        self.take_mover_lightmap_updates()
    }

    /// Keeps the per-frame lightmaps up to date even without a renderer (tests, tools).
    pub fn enable_world_light(&mut self) {
        self.wants_world_light = true;
    }

    /// Takes the pending lightmap rectangles (tests, tools).
    pub fn lightmap_updates(&mut self) -> Vec<(u32, u32, u32, u32, Vec<u8>)> {
        self.take_lightmap_updates()
    }

    /// The lighting the world light uses (for tests and tools).
    pub fn world_light_lighting(&self) -> Option<&dx_light::MapLighting> {
        self.world_light.as_ref().map(|w| &w.lighting)
    }
}

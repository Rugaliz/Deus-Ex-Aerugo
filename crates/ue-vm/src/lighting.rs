//! How a mesh actor is lit: Render.dll's `FLightManager::SetupForActor` (10b08c70), spec `docs/actor-lighting.md`.
//!
//! Per drawn actor the light manager gathers lights from three places (the actor's cached list of last time, the lights
//! permeating its BSP leaf, the dynamic lights the visibility walk linked to that leaf), sorts them by influence, keeps
//! the strongest within the engine's limits and fades each light in or out over a few frames. It also takes the
//! sprite's fog lights. The per-vertex formula runs in the mesh shader (`FLightManager::Light`, 10b027f0).

use crate::Vm;
use ue_assets::light::{fast_sqrt, hsv_to_rgb};

/// One light of an actor's entries that lights vertices: world position, colour (brightness of the moment, the level's
/// `Brightness` and the light's fade applied) and world radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshLight {
    pub pos: [f32; 3],
    pub color: [f32; 3],
    pub radius: f32,
}

/// The lighting of one mesh actor: `vertex = min(1, sum(lights) * scale + ambient)`, plus its fog lights.
#[derive(Debug, Clone, PartialEq)]
pub struct ActorLighting {
    pub ambient: [f32; 3],
    /// `ScaleGlow * 1.4`.
    pub scale: f32,
    pub lights: Vec<MeshLight>,
    /// `PF_RenderFog`: the sprite had fog lights (its polygons get the fog pass).
    pub render_fog: bool,
    /// The fog lights (volumetric entries) in entry order; the caller sorts them by view depth (farthest first).
    pub fog_lights: Vec<u32>,
}

/// What `SetupForActor` needs from the frame and the visibility walk.
#[derive(Debug, Clone, Copy, Default)]
pub struct ActorFrame<'a> {
    /// The viewport's frame counter (`UViewport+0x84`): a cached visibility of a static light is re-traced when the
    /// low 4 bits of `counter ^ light index` are zero.
    pub frame_count: u32,
    /// This frame's fade step: `(CurrentTime - LastUpdateTime) * 768`, truncated, 0..255.
    pub fade_step: i32,
    /// The viewport's `CurrentTime` (the `AmbientGlow` 255 pulse).
    pub viewport_time: f64,
    /// The level's model has leaves (`UModel::Leaves.Num`); without them actors get no lights.
    pub has_leaves: bool,
    /// The view's `RendMap` is the lit one (5); any other gives ambient only.
    pub lit_view: bool,
    /// The lights permeating the actor's leaf (`FLeaf::iPermeating` into `UModel::Lights`), in list order.
    pub permeating: &'a [u32],
    /// The sprite's `LeafLights`: dynamic lights linked to the actor's leaf by the walk, head first.
    pub leaf_lights: &'a [u32],
    /// The sprite's `Volumetrics`: its fog lights from the walk, head first.
    pub volumetrics: &'a [u32],
}

const LE_CYLINDER: u8 = 17;
/// Gathered lights at most (`0x78`).
const MAX_GATHERED: usize = 120;
/// Light entries at most (the entry array of `FLightManager`).
const MAX_ENTRIES: usize = 256;

/// One light entry (`FLightManager` light info): the light, whether it lights vertices (not kind 3) and whether it is a
/// fog light.
struct Entry {
    light: u32,
    lights_vertices: bool,
    volumetric: bool,
}

impl Vm {
    /// The BSP leaf the actor is in (`Region.iLeaf`; -1 outside the world).
    pub fn region_leaf(&mut self, a: u32) -> i32 {
        let ro = self.region_offsets();
        self.u32_at(a, ro.region + ro.leaf) as i32
    }

    /// A light's brightness at this moment (`URender::GlobalLighting`'s `LightType` functions on `LightBrightness /
    /// 255`), clamped to 0..1. Uses the renderer's frame randoms and strobe state.
    pub fn light_brightness_of(&mut self, l: u32) -> f32 {
        let time = self.fp(self.level, "TimeSeconds");
        let b = self.u8p(l, "LightBrightness") as f32 * (1.0 / 255.0f32);
        let (ty, period, phase) = (self.u8p(l, "LightType"), self.u8p(l, "LightPeriod"), self.u8p(l, "LightPhase"));
        let mut strobe = self.strobe;
        let v = ue_assets::light::light_type_brightness(ty, b, period, phase, l as u8, time, &self.randoms, &mut strobe);
        self.strobe = strobe;
        v
    }

    /// `URender::GlobalLighting` on a given brightness `b` with actor `a`'s light type (the audio driver's ambient
    /// sound modulation): the type's function at this moment, clamped to 0..1.
    pub fn light_type_scale(&mut self, a: u32, b: f32) -> f32 {
        let time = self.fp(self.level, "TimeSeconds");
        let (ty, period, phase) = (self.u8p(a, "LightType"), self.u8p(a, "LightPeriod"), self.u8p(a, "LightPhase"));
        let mut strobe = self.strobe;
        let v = ue_assets::light::light_type_brightness(ty, b, period, phase, a as u8, time, &self.randoms, &mut strobe);
        self.strobe = strobe;
        v
    }

    /// The index of an actor in the level's actor list (`Actor.Index`), `None` for a non-actor.
    pub fn actor_index(&self, a: u32) -> Option<usize> {
        let i = crate::ref_instance(a)?;
        self.actor_slots.iter().position(|&s| s == i)
    }

    /// The light's colour as a light entry computes it: `FGetHSV(LightHue, LightSaturation, 255) * Level.Brightness *
    /// brightness`.
    pub fn light_entry_colour(&mut self, l: u32) -> [f32; 3] {
        let b = self.light_brightness_of(l);
        let f = self.fp(self.level, "Brightness") * b;
        hsv_to_rgb(self.u8p(l, "LightHue"), self.u8p(l, "LightSaturation"), 255).map(|c| c * f)
    }

    /// `FLightManager::SetupForActor` for actor `a` (10b08c70): ambient, the lights with their fades, the fog lights.
    /// Updates the actor's cached light list and the lights' `LightingTag`/`ExtraTag`/`SpecialTag`, as the engine does.
    pub fn setup_for_actor(&mut self, a: u32, fr: &ActorFrame) -> ActorLighting {
        // ambient: the actor's zone (the level's when none) plus its glow
        let zone_ref = {
            let ro = self.region_offsets();
            self.u32_at(a, ro.region)
        };
        let zone = if zone_ref != 0 { zone_ref } else { self.level };
        let glow_byte = self.u8p(a, "AmbientGlow");
        let glow = if glow_byte == 255 { ((fr.viewport_time * 8.0).sin() * 0.2 + 0.25) as f32 } else { (glow_byte as f64 * (1.0 / 255.0)) as f32 };
        let zrgb = hsv_to_rgb(self.u8p(zone, "AmbientHue"), self.u8p(zone, "AmbientSaturation"), self.u8p(zone, "AmbientBrightness"));
        let ambient = zrgb.map(|c| glow + c);
        let scale = self.fp(a, "ScaleGlow") * 1.4;
        let mut out = ActorLighting { ambient, scale, lights: Vec::new(), render_fog: false, fog_lights: Vec::new() };
        let leaf = self.region_leaf(a);
        if self.bp(a, "bUnlit") || leaf == -1 || !fr.has_leaves || !fr.lit_view {
            return out;
        }
        let loc = self.location(a);
        let special = self.bp(a, "bSpecialLit");
        let step = fr.fade_step;
        let cached = self.actor_light_cache.get(&a).cloned();
        let fresh = cached.is_none();
        self.light_stamp = self.light_stamp.wrapping_add(1);
        let stamp = self.light_stamp;

        // gather: last time's lights with their fades, the leaf's permeating lights (fade 0), the walk's leaf lights
        // (full)
        let mut list: Vec<u32> = Vec::new();
        let gather = |vm: &mut Vm, l: u32, fade: i32, list: &mut Vec<u32>| {
            if !vm.is_live(l) || vm.ip(l, "ExtraTag") == stamp {
                return;
            }
            let lp = vm.location(l);
            let d = [lp[0] - loc[0], lp[1] - loc[1], lp[2] - loc[2]];
            let d2 = (d[0] * d[0] + d[1] * d[1]) + d[2] * d[2];
            let mut r = (vm.u8p(l, "LightRadius") as f32 + 1.0) * 25.0;
            if vm.bp(l, "bSpecialLit") != special || !(d2 < r * r) {
                return;
            }
            let mut key = vm.u8p(l, "LightBrightness") as f32;
            if vm.u8p(l, "LightEffect") == LE_CYLINDER {
                key *= 0.75;
                r *= 0.75;
            }
            let tag = ((1.0 - fast_sqrt(d2) / r) * key * 1024.0).round_ties_even() as i32;
            vm.set_ip(l, "SpecialTag", fade);
            vm.set_ip(l, "LightingTag", tag);
            vm.set_ip(l, "ExtraTag", stamp);
            list.push(l);
        };
        for &(l, fade) in cached.as_deref().unwrap_or(&[]) {
            gather(self, l, fade as i32, &mut list);
        }
        for &l in fr.permeating {
            if list.len() >= MAX_GATHERED {
                break;
            }
            gather(self, l, 0, &mut list);
        }
        for &l in fr.leaf_lights {
            if list.len() >= MAX_GATHERED {
                break;
            }
            gather(self, l, 0xff, &mut list);
        }
        let tags: std::collections::HashMap<u32, i32> = list.iter().map(|&l| (l, self.ip(l, "LightingTag"))).collect();
        ue_assets::sort::engine_sort(&mut list, |x, y| if tags[y] < tags[x] { -1 } else { 1 });

        // choose: within an eighth of the strongest kept one, at most 8 static lights and 8 in all for the others,
        // static and fixed lights must see the actor; a refused light fades out
        let mut entries: Vec<Entry> = Vec::new();
        let add_light = |vm: &mut Vm, l: u32, entries: &mut Vec<Entry>| {
            if entries.len() < MAX_ENTRIES && vm.u8p(l, "LightType") != 0 && vm.u8p(l, "LightBrightness") != 0 && l != a {
                entries.push(Entry { light: l, lights_vertices: true, volumetric: false });
            }
        };
        let (mut threshold, mut n_static, mut n_all) = (-1i32, 0, 0);
        for &l in &list {
            let tag = tags[&l];
            let is_static = self.bp(l, "bStatic");
            let mut keep = tag >= threshold && (if is_static { n_static } else { n_all }) <= 7;
            if keep && (is_static || !self.bp(l, "bMovable")) {
                let index = crate::ref_instance(l).unwrap_or(0) as u32;
                keep = if !fresh && (index ^ fr.frame_count) & 0xf != 0 {
                    self.ip(l, "SpecialTag") & 1 != 0
                } else {
                    let lp = self.location(l);
                    let clear = self.bsp.as_ref().map(|b| b.line_check(loc, lp, [0.0; 3], 0).is_none()).unwrap_or(true);
                    if clear && fresh {
                        self.set_ip(l, "SpecialTag", 0xff);
                    }
                    clear
                };
            }
            if !keep {
                let st = self.ip(l, "SpecialTag") - step;
                let st = if st < 0 { 0 } else { st & !1 };
                self.set_ip(l, "SpecialTag", st);
                if st > 0 {
                    add_light(self, l, &mut entries);
                }
                continue;
            }
            n_all += 1;
            if is_static {
                n_static += 1;
            }
            if threshold == -1 {
                threshold = tag / 8;
            }
            add_light(self, l, &mut entries);
            let st = (self.ip(l, "SpecialTag") + step).min(0xff) | 1;
            self.set_ip(l, "SpecialTag", st);
        }
        // the cached list: lights still faded in, at most 16
        let mut keep_list = Vec::new();
        for &l in &list {
            let st = self.ip(l, "SpecialTag");
            if st > 1 && keep_list.len() < 16 {
                keep_list.push((l, st as u8));
            }
        }
        self.actor_light_cache.insert(a, keep_list);

        // the sprite's fog lights: an entry each (existing ones are marked), and the fog pass for its polygons
        for &v in fr.volumetrics {
            if entries.len() >= MAX_ENTRIES {
                break;
            }
            out.render_fog = true;
            match entries.iter_mut().find(|e| e.light == v) {
                Some(e) => e.volumetric = true,
                None => entries.push(Entry { light: v, lights_vertices: false, volumetric: true }),
            }
        }
        // entry set-up: colour of the moment, scaled by the light's fade (fog colours are taken before the fade)
        for e in &entries {
            let colour = self.light_entry_colour(e.light);
            let fade = (self.ip(e.light, "SpecialTag") as f64 * (1.0 / 255.0)) as f32;
            if e.lights_vertices {
                let radius = (self.u8p(e.light, "LightRadius") as f32 + 1.0) * 25.0;
                out.lights.push(MeshLight { pos: self.location(e.light), color: colour.map(|c| fade * c), radius });
            }
            if e.volumetric {
                out.fog_lights.push(e.light);
            }
        }
        out
    }
}

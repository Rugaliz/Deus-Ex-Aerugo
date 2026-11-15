//! Coronas (`URender::DrawFrame`, 10b1b4e9..10b1ba5e, and the light visit 10b1bc00): glows drawn over every master
//! frame (the main view and viewport windows) for `bCorona` lights that reach the viewer's leaf and can be seen from
//! the frame's eye.

use crate::Session;
use dx_soft::Camera;
use ue_vm::Value;

/// The engine keeps 32 coronas (a `GCache` item of 32 x 12 bytes: light, its actor index, brightness).
pub const MAX_CORONAS: usize = 32;

/// A corona's colour from its light's `LightHue` and `LightSaturation` (DrawFrame: hue in three 85-wide segments,
/// 1/85 and, in the last, 1/84 steps; then each channel pulled toward 1 by `saturation / 255`).
pub fn corona_color(hue: u8, saturation: u8) -> [f32; 3] {
    let k = 0.011_764_706_f32; // 10b2b2e4
    let k2 = 0.011_904_762_f32; // 10b2b2e0
    let h = hue as i32;
    let base = if hue < 0x56 {
        [(0x55 - h) as f32 * k, h as f32 * k, 0.0]
    } else if hue < 0xab {
        [0.0, (0xaa - h) as f32 * k, (h - 0x55) as f32 * k]
    } else {
        [(h - 0xaa) as f32 * k, 0.0, (0xff - h) as f32 * k2]
    };
    let f = (saturation as f64 * (1.0 / 255.0)) as f32; // 10b2a930 (double 1/255)
    [(1.0 - base[0]) * f + base[0], base[1] + (1.0 - base[1]) * f, base[2] + (1.0 - base[2]) * f]
}

impl Session {
    /// The corona pass of one engine frame (main view only; needs the viewer in a leaf and a zone, a drawn scene frame
    /// and the device's `Coronas`). Every corona fades by `3 * real seconds since the last pass`, and is dropped below
    /// 0; then each light permeating the viewer's leaf (the model's list, then the walk's dynamic lights of that leaf)
    /// that is clear from the eye (`SingleLineCheck` level and movers, then pawns and others, where only the viewer
    /// may be hit) and has `bCorona`, a `Skin` and no `bLightChanged` brightens by twice that amount (at most 1), or
    /// takes the next free slot. Each corona's `Skin` steps its animation (`Get`), and the light keeps the frame.
    pub(crate) fn update_coronas(&mut self, cam: &Camera) {
        if self.scene_view(cam).is_none() {
            return;
        }
        let leaf = self.vm.region_leaf(self.player);
        let walked = self.vis_cache.as_ref().and_then(|(_, f)| f.first()).and_then(|fv| fv.vis.leaf_lights.get(&leaf).cloned()).unwrap_or_default();
        self.corona_pass(cam.pos, walked);
    }

    /// One master frame's corona pass (`DrawFrame` runs it for every frame without a parent: the main view and each
    /// viewport window's, on one table and one clock): the lights reaching the viewport actor's (the player's) leaf,
    /// `walked` the frame's own `LeafLights` there, tested from the frame's eye `eye`.
    pub(crate) fn corona_pass(&mut self, eye: [f32; 3], walked: Vec<u32>) {
        let now = self.viewport_time();
        let player = self.player;
        let leaf = self.vm.region_leaf(player);
        let has_zone = self.vm.zone_of_actor(player) != 0;
        if leaf < 0 || !has_zone || !self.vm.render_device_flag("Coronas") {
            return;
        }
        let last = self.corona_time.replace(now).unwrap_or(now);
        let delta = ((now - last) * 3.0) as f32; // 10b2a890 (double 3.0)
        for c in self.coronas.iter_mut() {
            if c.0 != 0 {
                c.1 -= delta;
                if c.1 < 0.0 {
                    c.0 = 0;
                }
            }
        }
        // the lights reaching the viewer's leaf
        let mut lights = Vec::new();
        if let Some(lf) = self.view.model.leaves.get(leaf as usize) {
            if lf.i_permeating >= 0 {
                let mut i = lf.i_permeating as usize;
                while self.view.model.lights.get(i).copied().unwrap_or(0) != 0 {
                    if let Some(&l) = self.vm.model_light_actors.get(i) {
                        lights.push(l);
                    }
                    i += 1;
                }
            }
        }
        lights.extend(walked);
        let mut next = 0usize;
        for l in lights {
            if l == 0 || !self.vm.is_live(l) {
                continue;
            }
            let at = self.vm.location(l);
            if self.vm.single_line_check(0, at, eye, 0x06, [0.0; 3], 0).actor != 0 {
                continue;
            }
            let hit = self.vm.single_line_check(0, at, eye, 0x11, [0.0; 3], 0).actor;
            if hit != 0 && hit != player {
                continue;
            }
            if !self.vm.bp(l, "bCorona") || self.vm.op(l, "Skin") == 0 || self.vm.bp(l, "bLightChanged") {
                continue;
            }
            if let Some(c) = self.coronas.iter_mut().find(|c| c.0 == l) {
                c.1 = (c.1 + delta * 2.0).min(1.0);
                continue;
            }
            while next < MAX_CORONAS && self.coronas[next].0 != 0 {
                next += 1;
            }
            if next < MAX_CORONAS {
                self.coronas[next] = (l, (delta * 2.0).min(1.0));
            }
        }
        // the drawing pass steps each corona's texture and leaves the frame in the light's `Skin`
        for i in 0..MAX_CORONAS {
            let l = self.coronas[i].0;
            if l == 0 || !self.vm.is_live(l) {
                continue;
            }
            let skin = self.vm.op(l, "Skin");
            if skin != 0 {
                let frame = self.vm.texture_get(skin, now);
                let _ = self.vm.set_prop(l, "Skin", 0, Value::Obj(frame));
            }
        }
    }

    /// The coronas as screen tiles for the frame drawn with `cam` in the scene rectangle `rect` (target pixels): in
    /// front of the eye only, centred on the light's projection, `width * DrawScale * 0.2` wide and high, the whole
    /// `Skin`, coloured by hue and saturation times the brightness, translucent and two-sided (`DrawIcon`, 0x104).
    pub fn corona_quads(&mut self, cam: &Camera, rect: [i32; 4]) -> Vec<dx_vk::ui::UiQuad> {
        let mut out = Vec::new();
        if rect[2] <= 0 || rect[3] <= 0 {
            return out;
        }
        let f = Session::vis_frame(cam, rect[2], rect[3]);
        for i in 0..MAX_CORONAS {
            let (l, bright) = self.coronas[i];
            if l == 0 || !self.vm.is_live(l) {
                continue;
            }
            let v = f.to_view(self.vm.location(l));
            if v[2] <= 0.0 {
                continue;
            }
            let skin = self.vm.op(l, "Skin");
            if skin == 0 {
                continue;
            }
            let (tw, th) = self.vm.texture_size(skin);
            let r = f.proj_z / v[2];
            let (sx, sy) = (f.fx2 + r * v[0], f.fy2 + r * v[1]);
            let size = rect[2] as f32 * self.vm.fp(l, "DrawScale") * 0.2;
            let c = corona_color(self.vm.u8p(l, "LightHue"), self.vm.u8p(l, "LightSaturation"));
            let path = self.vm.reg.obj_path(ue_registry::ObjId(skin)).to_string();
            out.push(dx_vk::ui::UiQuad {
                texture: path,
                rect: [rect[0] as f32 + sx - size * 0.5, rect[1] as f32 + sy - size * 0.5, size, size],
                uv: [0.0, 0.0, tw, th],
                color: [c[0] * bright, c[1] * bright, c[2] * bright],
                poly_flags: 0x104,
            });
        }
        out
    }

    /// The live coronas: (light, brightness) (tests and tools).
    pub fn coronas(&self) -> Vec<(u32, f32)> {
        self.coronas.iter().copied().filter(|c| c.0 != 0).collect()
    }
}

//! Procedural textures as the renderers see them: the simulation state of each water and fire texture in use, stepped when the
//! texture is drawn (the engine steps a texture when a renderer locks it), turned into RGBA with its palette.

use crate::MapView;
use dx_fractal::water::{wet_source, Water};
use dx_fractal::{AnimClock, Drop, Fire, Ice};
use ue_assets::procedural::{FireParams, IceParams, ProceduralTexture, WaterParams};
use ue_assets::texture::{Palette, Texture};

/// What generates the pixels.
enum Kind {
    /// Water; `source` is a wet texture's picture at its own size (`None`: a wave texture, whose pixels are light levels).
    Water { water: Water, source: Option<Vec<u8>> },
    Fire(Fire),
    /// Ice: the glass and source pictures at the texture's size (the stored first mip of each; a procedural source
    /// or glass is not stepped by the ice texture here). `real_time`: `TimeMethod` 1, drawn with the frame's time.
    Ice { ice: Ice, glass: Option<Vec<u8>>, source: Option<Vec<u8>>, real_time: bool, last: Option<f64> },
}

pub(crate) struct Procedural {
    kind: Kind,
    clock: AnimClock,
    palette: Vec<[u8; 4]>,
    masked: bool,
    rgba: Vec<u8>,
}

impl Procedural {
    fn size(&self) -> (usize, usize) {
        match &self.kind {
            Kind::Water { water, .. } => water.size(),
            Kind::Fire(f) => f.size(),
            Kind::Ice { ice, .. } => (ice.u_size, ice.v_size),
        }
    }

    fn step(&mut self, shared: &mut dx_fractal::Shared) {
        let (u, v) = self.size();
        if u < 8 || v < 8 {
            return;
        }
        match &mut self.kind {
            Kind::Water { water, source } => {
                water.redraw_drops(shared);
                water.calculate();
                if let Some(src) = source {
                    water.apply_wet(src);
                }
            }
            Kind::Fire(f) => f.step(shared),
            // `ConstantTimeTick` (Fire 10509f30): a fixed 1/120 s
            Kind::Ice { ice, glass, source, .. } => {
                ice.render(1.0 / 120.0, glass.as_deref(), source.as_deref());
            }
        }
    }

    fn refresh_rgba(&mut self) {
        let bitmap = match &self.kind {
            Kind::Water { water, .. } => &water.bitmap,
            Kind::Fire(f) => &f.bitmap,
            Kind::Ice { ice, .. } => &ice.bitmap,
        };
        for (px, &i) in self.rgba.chunks_mut(4).zip(bitmap) {
            let c = if self.masked && i == 0 { [0, 0, 0, 0] } else { self.palette.get(i as usize).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0, 0, 0, 255]) };
            px.copy_from_slice(&c);
        }
    }
}

impl MapView {
    /// The texture behind an object path, if it is a procedural texture the port generates (water and fire).
    pub fn procedural_key_by_path(&self, path: &str) -> Option<(String, usize)> {
        if self.runtime.contains_key(path) {
            return Some((path.to_string(), usize::MAX));
        }
        let (pkg_name, rest) = path.split_once('.')?;
        let pkg = self.files.package(pkg_name)?;
        let export = pkg.find(rest)?;
        let key = (pkg.name.clone(), export);
        self.is_procedural(&key).then_some(key)
    }

    /// Whether a texture (by `texture_key`) is generated at run time; its pixels then come from `procedural_frame`.
    pub fn is_procedural(&self, key: &(String, usize)) -> bool {
        if let Some(p) = self.procedurals.get(key) {
            return p.is_some();
        }
        self.files.package(&key.0).map(|p| matches!(p.pkg.export_class_name(key.1).as_str(), "WetTexture" | "WaveTexture" | "FireTexture" | "IceTexture")).unwrap_or(false)
    }

    /// The current pixels of a procedural texture drawn at `time` (seconds, the viewport clock): steps the simulation
    /// as the texture's clock asks, and returns `(width, height, rgba)`. `None` if the texture cannot be generated (a wet
    /// texture without a usable source stays as stored, like in the engine).
    pub fn procedural_frame(&mut self, key: &(String, usize), time: f64) -> Option<(u32, u32, &[u8])> {
        if key.1 == usize::MAX {
            return self.runtime.get(&key.0).map(|(w, h, rgba)| (*w, *h, rgba.as_slice()));
        }
        if !self.procedurals.contains_key(key) {
            let p = self.make_procedural(key);
            self.procedurals.insert(key.clone(), p);
        }
        let p = self.procedurals.get_mut(key)?.as_mut()?;
        // an ice texture in real time (`UIceTexture::Tick` 1050a020, `TimeMethod` 1): drawn with the time since the
        // last draw, not through the texture's clock
        if let Kind::Ice { ice, glass, source, real_time: true, last } = &mut p.kind {
            let dt = last.map(|l| (time - l) as f32).unwrap_or(0.0);
            *last = Some(time);
            if ice.render(dt, glass.as_deref(), source.as_deref()) {
                p.refresh_rgba();
            }
            let (u, v) = p.size();
            return Some((u as u32, v as u32, &p.rgba));
        }
        let steps = p.clock.update(time);
        for _ in 0..steps {
            p.step(&mut self.fractal);
        }
        if steps > 0 {
            p.refresh_rgba();
        }
        let (u, v) = p.size();
        Some((u as u32, v as u32, &p.rgba))
    }

    /// The first frame of a procedural texture (`width, height, rgba`) without touching the running state, for creating
    /// GPU textures from a shared view.
    pub fn procedural_initial(&self, key: &(String, usize)) -> Option<(u32, u32, Vec<u8>)> {
        if key.1 == usize::MAX {
            return self.runtime.get(&key.0).cloned();
        }
        let p = self.make_procedural(key)?;
        let (u, v) = p.size();
        Some((u as u32, v as u32, p.rgba))
    }

    fn make_procedural(&self, key: &(String, usize)) -> Option<Procedural> {
        let pkg = self.files.package(&key.0)?;
        let class = pkg.pkg.export_class_name(key.1);
        if class == "FireTexture" {
            return self.make_fire(&pkg, key.1);
        }
        if class == "IceTexture" {
            return self.make_ice(&pkg, key.1);
        }
        if !matches!(class.as_str(), "WetTexture" | "WaveTexture") {
            return None;
        }
        let t = Texture::load(&pkg.pkg, key.1).ok()?;
        let params = WaterParams::load(&pkg.pkg, key.1).ok()?;
        let (u, v) = (t.u_size as usize, t.v_size as usize);
        if !u.is_power_of_two() || !v.is_power_of_two() || u < 2 || v < 2 {
            return None;
        }
        let drops = params.drops.iter().map(|d| Drop { kind: d.kind, depth: d.depth, x: d.x, y: d.y, a: d.a, b: d.b, c: d.c, d: d.d }).collect();
        let mut water = Water::new(u, v, drops);
        let (source, palette_ref, palette_pkg) = if class == "WetTexture" {
            let (spkg, sexp) = self.files.resolve(&pkg, params.source_texture)?;
            let s = Texture::load_any(&spkg.pkg, sexp).ok()?;
            let m = s.mips.first()?;
            let src = wet_source(u, v, m.u_size as usize, m.v_size as usize, &m.data)?;
            water.set_refraction(params.wave_amp);
            // until the first step the texture shows its source undistorted
            water.bitmap.copy_from_slice(&src);
            (Some(src), s.palette, spkg)
        } else {
            water.set_wave_light(params.wave_amp, params.bump_map_light, params.bump_map_angle, params.phong_range, params.phong_size);
            (None, t.palette, pkg.clone())
        };
        let (ppkg, pexp) = self.files.resolve(&palette_pkg, palette_ref)?;
        let palette = Palette::load(&ppkg.pkg, pexp).ok()?.colors;
        let mut p = Procedural {
            kind: Kind::Water { water, source },
            clock: AnimClock::new(params.min_frame_rate, params.max_frame_rate, params.prime_count),
            palette,
            masked: t.masked,
            rgba: vec![0; u * v * 4],
        };
        p.refresh_rgba();
        Some(p)
    }

    /// An ice texture as `UIceTexture::PostLoad` (Fire 10509b20) leaves it: below 8x8 no source or glass; a glass of
    /// another size is dropped; a source of another size (or no glass) is scaled up into the texture itself, which
    /// then never changes (a larger source is dropped); the palette is the source's.
    fn make_ice(&self, pkg: &std::rc::Rc<ue_assets::files::Loaded>, export: usize) -> Option<Procedural> {
        let t = Texture::load_any(&pkg.pkg, export).ok()?;
        let params = IceParams::load(&pkg.pkg, export).ok()?;
        let (u, v) = (t.u_size as usize, t.v_size as usize);
        if !u.is_power_of_two() || !v.is_power_of_two() || u < 2 || v < 2 {
            return None;
        }
        let picture = |r: i32| -> Option<(usize, usize, Vec<u8>, i32, std::rc::Rc<ue_assets::files::Loaded>)> {
            if r == 0 {
                return None;
            }
            let (spkg, sexp) = self.files.resolve(pkg, r)?;
            let s = Texture::load_any(&spkg.pkg, sexp).ok()?;
            let m = s.mips.first()?;
            Some((m.u_size as usize, m.v_size as usize, m.data.clone(), s.palette, spkg))
        };
        let mut ice = Ice {
            u_size: u,
            v_size: v,
            u_bits: u.trailing_zeros(),
            panning_style: params.panning_style,
            time_method: params.time_method,
            horiz_pan_speed: params.horiz_pan_speed,
            vert_pan_speed: params.vert_pan_speed,
            frequency: params.frequency,
            amplitude: params.amplitude,
            move_ice: params.move_ice,
            master_count: params.master_count,
            u_displace: params.u_displace,
            v_displace: params.v_displace,
            u_position: params.u_position,
            v_position: params.v_position,
            force_refresh: true,
            bitmap: t.mips.first().map(|m| m.data.clone()).unwrap_or_default(),
            ..Default::default()
        };
        ice.bitmap.resize(u * v, 0);
        let mut source = picture(params.source_texture);
        let mut glass = picture(params.glass_texture);
        if source.is_some() {
            if u < 8 || v < 8 {
                source = None;
                glass = None;
            } else {
                let (su, sv) = source.as_ref().map(|s| (s.0, s.1)).unwrap();
                let source_differs = su != u || sv != v;
                let glass_differs = match &glass {
                    Some(g) if g.0 == u && g.1 == v => false,
                    Some(_) => {
                        glass = None;
                        true
                    }
                    None => true,
                };
                if source_differs || glass_differs {
                    let (ub, vb) = (u.trailing_zeros() as i32, v.trailing_zeros() as i32);
                    let (sub, svb) = (su.trailing_zeros() as i32, sv.trailing_zeros() as i32);
                    if ub - sub < 0 || vb - svb < 0 {
                        source = None;
                    } else {
                        let s = source.as_ref().unwrap();
                        ice.bitmap = Ice::scale_source(u, v, &s.2, sub as u32, (ub - sub) as u32, (vb - svb) as u32);
                        ice.local_source = true;
                    }
                }
            }
        }
        // the palette: the source's when there is one, else the texture's own
        let (palette_pkg, palette_ref) = match &source {
            Some(s) => (s.4.clone(), s.3),
            None => (pkg.clone(), t.palette),
        };
        let (ppkg, pexp) = self.files.resolve(&palette_pkg, palette_ref)?;
        let palette = Palette::load(&ppkg.pkg, pexp).ok()?.colors;
        let real_time = params.time_method != 0;
        let mut p = Procedural {
            kind: Kind::Ice { ice, glass: glass.map(|g| g.2), source: source.map(|s| s.2), real_time, last: None },
            clock: AnimClock::new(params.min_frame_rate, params.max_frame_rate, params.prime_count),
            palette,
            masked: t.masked,
            rgba: vec![0; u * v * 4],
        };
        p.refresh_rgba();
        Some(p)
    }

    fn make_fire(&self, pkg: &std::rc::Rc<ue_assets::files::Loaded>, export: usize) -> Option<Procedural> {
        let t = ProceduralTexture::load(&pkg.pkg, export).ok()?;
        let params = FireParams::load(&pkg.pkg, export).ok()?;
        let (u, v) = (t.texture.u_size as usize, t.texture.v_size as usize);
        if !u.is_power_of_two() || !v.is_power_of_two() || u < 2 || v < 2 {
            return None;
        }
        let sparks: Vec<[u8; 8]> = t.sparks.iter().map(|s| [s.kind, s.heat, s.x, s.y, s.params[0], s.params[1], s.params[2], s.params[3]]).collect();
        let mut fire = Fire::new(u, v, &sparks, params.sparks_limit, params.render_heat, params.rising, params.frequency);
        fire.num_sparks = fire.num_sparks.min(params.num_sparks.max(0) as usize);
        let (ppkg, pexp) = self.files.resolve(pkg, t.texture.palette)?;
        let palette = Palette::load(&ppkg.pkg, pexp).ok()?.colors;
        let mut p = Procedural {
            kind: Kind::Fire(fire),
            clock: AnimClock::new(params.min_frame_rate, params.max_frame_rate, params.prime_count),
            palette,
            // the engine clears a fire texture's masked flag when it loads
            masked: false,
            rgba: vec![0; u * v * 4],
        };
        p.refresh_rgba();
        Some(p)
    }
}

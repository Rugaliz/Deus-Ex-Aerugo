//! The viewer window: a search box and result list on the left, the model (animated) or texture on the right.
//!
//! Search: type to filter (every word must appear in the object path), Up/Down/PageUp/PageDown to pick, Enter to
//! open, Tab or Esc to go to the view. View: left mouse drag looks around, right drag turns around the model, wheel
//! moves forward and back, W/A/S/D/Q/E fly (Shift faster), R resets the camera; Space plays or pauses,
//! Left/Right step frames, Up/Down change the sequence, K changes the skin set (the game's classes using the mesh),
//! F shows the model in hand as the game places it (first-person weapons), M switches the mods folder's replacement
//! on and off, B back faces, O open edges, L wireframe, +/- field of view; X exports (glTF with textures for a model,
//! PNG at the mod path for a texture); Tab or / goes back to the search.

use crate::catalog::{self, Catalog, Kind, LoadedMesh, Picture, SkinSet};
use crate::font::Text;
use crate::render::{self, Camera, Canvas, DrawOptions, V3};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

const PANEL: i32 = 380;

enum Shown {
    None,
    Mesh(Box<MeshView>),
    Texture { path: String, pic: Picture, zoom: f32 },
}

struct MeshView {
    m: LoadedMesh,
    pics: Vec<Option<Rc<Picture>>>,
    slot_names: Vec<Option<String>>,
    sets: Vec<SkinSet>,
    skin: Option<usize>,
    seq: usize,
    /// Position in the sequence, in frames.
    t: f32,
    playing: bool,
    with_mod: bool,
    /// First-person placement (offset from the eye, scale) when shown in hand.
    held: Option<(V3, f32, String)>,
    centre: V3,
    radius: f32,
}

struct Viewer {
    cat: Catalog,
    out: PathBuf,
    query: String,
    hits: Vec<usize>,
    sel: usize,
    top: usize,
    searching: bool,
    shown: Shown,
    cam: Camera,
    opt: DrawOptions,
    status: String,
    text: Option<Text>,
    small: Option<Text>,
}

pub fn run(game: &Path, args: &mut Vec<String>) -> Result<(), String> {
    let shot = crate::take_value(args, "--shot");
    let frames: u32 = crate::take_value(args, "--frames").and_then(|v| v.parse().ok()).unwrap_or(if shot.is_some() { 3 } else { u32::MAX });
    let size = crate::take_value(args, "--size").and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))).unwrap_or((1400, 860));
    let out = crate::take_value(args, "--out").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("ModKit"));
    let fp = crate::take_flag(args, "--fp");
    let with_mod = crate::take_flag(args, "--mod");
    let cat = Catalog::open(game)?;
    let text = crate::font::Text::load(&cat, &["FontMenuSmall", "TechMedium", "SmallFont"]);
    let small = crate::font::Text::load(&cat, &["FontTiny", "FontMenuSmall", "SmallFont"]);
    let mut v = Viewer {
        cat,
        out,
        query: args.join(" "),
        hits: Vec::new(),
        sel: 0,
        top: 0,
        searching: true,
        shown: Shown::None,
        cam: Camera { pos: [-100.0, 0.0, 0.0], yaw: 0.0, pitch: 0.0, fov: 60.0 },
        opt: DrawOptions::default(),
        status: "Type to search the game's models and textures; Enter opens one".into(),
        text,
        small,
    };
    v.refilter();
    // a name given on the command line that names one entry opens it
    if !v.query.is_empty() && !v.hits.is_empty() {
        let exact = v.hits.iter().position(|&i| v.cat.entries[i].path.to_ascii_lowercase().ends_with(&format!(".{}", v.query.to_ascii_lowercase())));
        if v.hits.len() == 1 || exact.is_some() {
            v.sel = exact.unwrap_or(0);
            v.open();
            v.searching = false;
            if let Shown::Mesh(mv) = &mut v.shown {
                mv.with_mod = with_mod;
            }
            if with_mod {
                v.reload_mesh();
            }
            if fp {
                v.toggle_held();
            }
        }
    }

    let sdl = sdl3::init().map_err(|e| e.to_string())?;
    let video = sdl.video().map_err(|e| e.to_string())?;
    let window = video.window("Deus Ex Aerugo mod kit", size.0, size.1).position_centered().resizable().build().map_err(|e| e.to_string())?;
    video.text_input().start(&window);
    let mut canvas = window.into_canvas();
    let creator = canvas.texture_creator();
    let mut events = sdl.event_pump().map_err(|e| e.to_string())?;
    let (mut tw, mut th) = (0u32, 0u32);
    let mut texture: Option<sdl3::render::Texture> = None;
    let mut last = Instant::now();
    let mut drag: Option<sdl3::mouse::MouseButton> = None;
    let mut frame = 0u32;
    'run: loop {
        for e in events.poll_iter() {
            use sdl3::event::Event;
            use sdl3::keyboard::Keycode as K;
            match e {
                Event::Quit { .. } => break 'run,
                Event::TextInput { text, .. } if v.searching => {
                    v.query.push_str(&text);
                    v.refilter();
                }
                Event::KeyDown { keycode: Some(k), keymod, .. } => {
                    let shift = keymod.intersects(sdl3::keyboard::Mod::LSHIFTMOD | sdl3::keyboard::Mod::RSHIFTMOD);
                    if v.searching {
                        match k {
                            K::Backspace => {
                                v.query.pop();
                                v.refilter();
                            }
                            K::Up => v.sel = v.sel.saturating_sub(1),
                            K::Down => v.sel = (v.sel + 1).min(v.hits.len().saturating_sub(1)),
                            K::PageUp => v.sel = v.sel.saturating_sub(20),
                            K::PageDown => v.sel = (v.sel + 20).min(v.hits.len().saturating_sub(1)),
                            K::Return | K::KpEnter => {
                                v.open();
                                v.searching = false;
                            }
                            K::Tab | K::Escape => v.searching = false,
                            _ => {}
                        }
                    } else {
                        v.key(k, shift);
                        if k == K::Tab || k == K::Slash {
                            v.searching = true;
                        }
                    }
                }
                Event::MouseButtonDown { mouse_btn, x, .. } => {
                    if (x as i32) < PANEL {
                        v.searching = true;
                    } else {
                        v.searching = false;
                        drag = Some(mouse_btn);
                    }
                }
                Event::MouseButtonUp { .. } => drag = None,
                Event::MouseMotion { xrel, yrel, .. } => match drag {
                    Some(sdl3::mouse::MouseButton::Left) => {
                        v.cam.yaw += xrel * 0.25;
                        v.cam.pitch = (v.cam.pitch - yrel * 0.25).clamp(-89.0, 89.0);
                    }
                    Some(sdl3::mouse::MouseButton::Right) => v.orbit(xrel * 0.4, -yrel * 0.4),
                    _ => {}
                },
                Event::MouseWheel { y, mouse_x, .. } => {
                    if (mouse_x as i32) < PANEL {
                        v.sel = (v.sel as i64 - y as i64 * 3).clamp(0, v.hits.len().saturating_sub(1) as i64) as usize;
                    } else {
                        v.wheel(y);
                    }
                }
                _ => {}
            }
        }
        let dt = last.elapsed().as_secs_f32().min(0.1);
        last = Instant::now();
        if !v.searching {
            let ks = events.keyboard_state();
            use sdl3::keyboard::Scancode as S;
            let mut mv = [0.0f32; 3];
            for (sc, d) in [(S::W, [1.0, 0.0, 0.0]), (S::S, [-1.0, 0.0, 0.0]), (S::D, [0.0, 1.0, 0.0]), (S::A, [0.0, -1.0, 0.0]), (S::E, [0.0, 0.0, 1.0]), (S::Q, [0.0, 0.0, -1.0])] {
                if ks.is_scancode_pressed(sc) {
                    for k in 0..3 {
                        mv[k] += d[k];
                    }
                }
            }
            let fast = if ks.is_scancode_pressed(S::LShift) { 4.0 } else { 1.0 };
            v.fly(mv, dt * fast);
        }
        v.advance(dt);
        let (w, h) = canvas.window().size_in_pixels();
        let (w, h) = (w.max(PANEL as u32 + 64), h.max(200));
        let img = v.draw(w as usize, h as usize);
        if texture.is_none() || (tw, th) != (w, h) {
            texture = Some(creator.create_texture_streaming(sdl3::pixels::PixelFormat::RGBA32, w, h).map_err(|e| e.to_string())?);
            (tw, th) = (w, h);
        }
        let t = texture.as_mut().unwrap();
        t.update(None, &img.rgba, w as usize * 4).map_err(|e| e.to_string())?;
        canvas.clear();
        canvas.copy(t, None, None).map_err(|e| e.to_string())?;
        canvas.present();
        frame += 1;
        if frame >= frames {
            if let Some(s) = &shot {
                crate::write_png(Path::new(s), w, h, &img.rgba)?;
                println!("{s}");
            }
            break;
        }
    }
    Ok(())
}

impl Viewer {
    fn refilter(&mut self) {
        let q: Vec<String> = self.query.split_whitespace().map(str::to_ascii_lowercase).collect();
        self.hits = (0..self.cat.entries.len())
            .filter(|&i| {
                let p = self.cat.entries[i].path.to_ascii_lowercase();
                q.iter().all(|w| p.contains(w.as_str()))
            })
            .collect();
        self.sel = self.sel.min(self.hits.len().saturating_sub(1));
    }

    fn open(&mut self) {
        let Some(&i) = self.hits.get(self.sel) else { return };
        let e = self.cat.entries[i].clone();
        match e.kind {
            Kind::Texture => match self.cat.picture(&e.path) {
                Ok(pic) => {
                    self.status = format!("{} ({}x{}) - X exports it as a mod's replacement picture", e.path, pic.w, pic.h);
                    self.shown = Shown::Texture { path: e.path, pic, zoom: 0.0 };
                }
                Err(err) => self.status = err,
            },
            Kind::Mesh => match self.cat.mesh(&e.path, false) {
                Ok(m) => {
                    let sets = self.cat.skins_for(&m.path);
                    let skin = (!sets.is_empty()).then_some(0);
                    let (pics, slot_names, _) = crate::slot_pictures(&mut self.cat, &m, skin);
                    let mut mv = MeshView { m, pics, slot_names, sets, skin, seq: 0, t: 0.0, playing: true, with_mod: false, held: None, centre: [0.0; 3], radius: 1.0 };
                    // start on an idle sequence, else one that moves
                    let idle = mv.m.seqs.iter().position(|s| s.2 > 1 && s.0.to_ascii_lowercase().starts_with("idle"));
                    if let Some(k) = idle.or_else(|| mv.m.seqs.iter().position(|s| s.2 > 1 && !s.0.eq_ignore_ascii_case("All"))) {
                        mv.seq = k;
                    }
                    self.status = format!("{} - {} frames, {} faces", e.path, mv.m.lod.mesh.anim_frames, mv.m.lod.faces.len());
                    self.shown = Shown::Mesh(Box::new(mv));
                    self.frame_camera();
                }
                Err(err) => self.status = err,
            },
        }
    }

    fn reload_mesh(&mut self) {
        let Shown::Mesh(mv) = &mut self.shown else { return };
        match self.cat.mesh(&mv.m.path, mv.with_mod) {
            Ok(m) => mv.m = m,
            Err(e) => self.status = e,
        }
    }

    fn mesh_frames(mv: &MeshView) -> (usize, usize, f32) {
        let nf = mv.m.lod.mesh.anim_frames.max(1) as usize;
        let (start, n) = mv.m.seqs.get(mv.seq).map(|s| (s.1, s.2)).unwrap_or((0, nf));
        let t = mv.t.rem_euclid(n as f32);
        let a = (start + t as usize).min(nf - 1);
        let b = if n > 1 { (start + (t as usize + 1) % n).min(nf - 1) } else { a };
        (a, b, t.fract())
    }

    fn pose(mv: &MeshView) -> Vec<V3> {
        let (a, b, t) = Self::mesh_frames(mv);
        match &mv.held {
            Some((o, s, _)) => render::pose_at(&mv.m, a, b, t, *o, [0; 3], *s),
            None => render::pose(&mv.m, a, b, t),
        }
    }

    /// The camera looking at the whole model (or from the eye, for a model in hand).
    fn frame_camera(&mut self) {
        let Shown::Mesh(mv) = &mut self.shown else { return };
        if mv.held.is_some() {
            self.cam = Camera { pos: [0.0; 3], yaw: 0.0, pitch: 0.0, fov: self.cam.fov };
            return;
        }
        // the bounds over the whole sequence's frames
        let (start, n) = mv.m.seqs.get(mv.seq).map(|s| (s.1, s.2)).unwrap_or((0, 1));
        let mut pts = Vec::new();
        for f in start..start + n.min(8) {
            pts.extend(render::pose(&mv.m, f, f, 0.0));
        }
        let (lo, hi) = render::bounds(&pts);
        mv.centre = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
        let d = render::sub(hi, lo);
        mv.radius = (render::dot(d, d).sqrt() * 0.5).max(1.0);
        let dist = mv.radius / (self.cam.fov.to_radians() * 0.5).tan() * 1.15;
        self.cam = Camera::orbit(mv.centre, dist, 150.0, -15.0, self.cam.fov);
    }

    fn orbit(&mut self, dyaw: f32, dpitch: f32) {
        let Shown::Mesh(mv) = &self.shown else { return };
        let c = if mv.held.is_some() { [0.0; 3] } else { mv.centre };
        let d = render::sub(self.cam.pos, c);
        let dist = render::dot(d, d).sqrt();
        self.cam.yaw += dyaw;
        self.cam.pitch = (self.cam.pitch + dpitch).clamp(-89.0, 89.0);
        self.cam = Camera::orbit(c, dist, self.cam.yaw, self.cam.pitch, self.cam.fov);
    }

    fn speed(&self) -> f32 {
        match &self.shown {
            Shown::Mesh(mv) => mv.radius.max(2.0),
            _ => 10.0,
        }
    }

    fn fly(&mut self, dir: V3, dt: f32) {
        if dir == [0.0; 3] || !matches!(self.shown, Shown::Mesh(_)) {
            return;
        }
        let (f, r, _) = self.cam.basis();
        let s = self.speed() * dt;
        for k in 0..3 {
            self.cam.pos[k] += (f[k] * dir[0] + r[k] * dir[1]) * s + if k == 2 { dir[2] * s } else { 0.0 };
        }
    }

    fn wheel(&mut self, y: f32) {
        match &mut self.shown {
            Shown::Texture { zoom, .. } => *zoom = (if *zoom <= 0.0 { 1.0 } else { *zoom } * if y > 0.0 { 1.25 } else { 0.8 }).clamp(0.125, 32.0),
            Shown::Mesh(_) => {
                let (f, _, _) = self.cam.basis();
                let s = self.speed() * 0.15 * y;
                for k in 0..3 {
                    self.cam.pos[k] += f[k] * s;
                }
            }
            Shown::None => {}
        }
    }

    fn advance(&mut self, dt: f32) {
        if let Shown::Mesh(mv) = &mut self.shown {
            if mv.playing {
                let rate = mv.m.seqs.get(mv.seq).map(|s| s.3).filter(|r| *r > 0.0).unwrap_or(10.0);
                mv.t += dt * rate;
            }
        }
    }

    fn toggle_held(&mut self) {
        let game = self.cat.game.clone();
        let Shown::Mesh(mv) = &mut self.shown else { return };
        if mv.held.is_some() {
            mv.held = None;
            self.status = "free view".into();
        } else {
            // the classes holding this mesh in first person
            let class = mv.sets.iter().find(|s| s.source.ends_with(".PlayerViewMesh")).and_then(|s| s.source.split('.').nth(1).map(str::to_string));
            match class.map(|c| (catalog::first_person(&game, &c), c)) {
                Some((Ok((_, o, s)), c)) => {
                    self.status = format!("in hand as {c} holds it (F: free view; +/-: field of view, now {:.0})", self.cam.fov);
                    mv.held = Some((o, s, c));
                }
                Some((Err(e), _)) => self.status = e,
                None => self.status = "no class holds this mesh in first person".into(),
            }
        }
        self.frame_camera();
    }

    fn key(&mut self, k: sdl3::keyboard::Keycode, shift: bool) {
        use sdl3::keyboard::Keycode as K;
        let _ = shift;
        match k {
            K::R => self.frame_camera(),
            K::B => self.opt.back_faces = !self.opt.back_faces,
            K::O => self.opt.open_edges = !self.opt.open_edges,
            K::L => self.opt.wireframe = !self.opt.wireframe,
            K::Equals | K::Plus | K::KpPlus => self.cam.fov = (self.cam.fov + 5.0).min(170.0),
            K::Minus | K::KpMinus => self.cam.fov = (self.cam.fov - 5.0).max(10.0),
            K::F => self.toggle_held(),
            K::X => self.export(),
            K::M => {
                if let Shown::Mesh(mv) = &mut self.shown {
                    mv.with_mod = !mv.with_mod;
                    let on = mv.with_mod;
                    let has = ue_package::mods::mesh(&mv.m.path).is_some();
                    self.reload_mesh();
                    self.status = match (on, has) {
                        (true, true) => "with the mods folder's replacement".into(),
                        (true, false) => "no mod replaces this mesh".into(),
                        _ => "the game's own mesh".into(),
                    };
                }
            }
            _ => {}
        }
        let Shown::Mesh(mv) = &mut self.shown else { return };
        match k {
            K::Space => mv.playing = !mv.playing,
            K::Right => {
                mv.playing = false;
                mv.t = mv.t.floor() + 1.0;
            }
            K::Left => {
                mv.playing = false;
                mv.t = mv.t.ceil() - 1.0;
            }
            K::Up | K::Down if !mv.m.seqs.is_empty() => {
                let n = mv.m.seqs.len();
                mv.seq = if k == K::Down { (mv.seq + 1) % n } else { (mv.seq + n - 1) % n };
                mv.t = 0.0;
            }
            K::K if !mv.sets.is_empty() => {
                mv.skin = match mv.skin {
                    None => Some(0),
                    Some(i) if i + 1 < mv.sets.len() => Some(i + 1),
                    _ => None,
                };
                let skin = mv.skin;
                let m = std::mem::replace(&mut mv.m, LoadedMesh { path: String::new(), lod: ue_assets::mesh::LodMesh::from_plain(Default::default()), plain: true, slots: vec![], seqs: vec![] });
                let (pics, names, src) = crate::slot_pictures(&mut self.cat, &m, skin);
                let Shown::Mesh(mv) = &mut self.shown else { return };
                mv.m = m;
                mv.pics = pics;
                mv.slot_names = names;
                self.status = format!("skins: {}", src.unwrap_or_else(|| "the mesh's own textures".into()));
            }
            _ => {}
        }
    }

    fn export(&mut self) {
        let r = match &self.shown {
            Shown::Mesh(mv) => crate::gltf::export(&mv.m, &mv.pics, &mv.slot_names, &self.out),
            Shown::Texture { path, pic, .. } => {
                let f = crate::texture_file(&self.out, path);
                crate::write_png(&f, pic.w, pic.h, &pic.rgba).map(|_| f)
            }
            Shown::None => return,
        };
        self.status = match r {
            Ok(f) => format!("exported {}", f.display()),
            Err(e) => e,
        };
    }

    fn draw(&mut self, w: usize, h: usize) -> Canvas {
        let mut c = Canvas::new(w, h);
        c.clear([24, 27, 36]);
        let (vx, vw) = (PANEL as usize, w - PANEL as usize);
        // the view, drawn into its own canvas and copied in
        let mut view = Canvas::new(vw, h);
        view.clear([28, 32, 44]);
        match &self.shown {
            Shown::Mesh(mv) => {
                let pos = Self::pose(mv);
                let mut cam = self.cam;
                if mv.held.is_some() {
                    // Hor+ as the game: the field of view is the 4:3 one, widened with the view's shape
                    let ht = (cam.fov.to_radians() * 0.5).tan() * ((vw as f32 / h as f32) / (4.0 / 3.0)).max(1.0);
                    cam.fov = 2.0 * ht.atan().to_degrees();
                }
                render::draw_mesh(&mut view, &cam, &mv.m, &pos, &mv.pics, self.opt);
            }
            Shown::Texture { pic, zoom, .. } => {
                let fit = ((vw as f32 - 40.0) / pic.w as f32).min((h as f32 - 120.0) / pic.h as f32).max(0.05);
                let z = if *zoom > 0.0 { *zoom } else { fit.min(4.0) };
                let (x, y) = ((vw as f32 - pic.w as f32 * z) / 2.0, (h as f32 - 80.0 - pic.h as f32 * z) / 2.0);
                view.picture(pic, x as i32, y as i32, z);
            }
            Shown::None => {}
        }
        for y in 0..h {
            let s = y * vw * 4;
            let d = (y * w + vx) * 4;
            c.rgba[d..d + vw * 4].copy_from_slice(&view.rgba[s..s + vw * 4]);
        }
        // the search panel
        let Some(text) = &self.text else { return c };
        let small = self.small.as_ref().unwrap_or(text);
        let lh = text.height + 2;
        c.fill_rect(0, 0, PANEL, h as i32, [16, 18, 24, 255]);
        c.fill_rect(8, 8, PANEL - 16, lh + 8, if self.searching { [40, 60, 40, 255] } else { [34, 36, 44, 255] });
        let cursor = if self.searching { "_" } else { "" };
        text.draw(&mut c, 14, 12, &format!("Search: {}{cursor}", self.query), [220, 230, 220, 255]);
        let meshes = self.hits.iter().filter(|&&i| self.cat.entries[i].kind == Kind::Mesh).count();
        small.draw(&mut c, 14, lh + 20, &format!("{meshes} models (M), {} textures (T); Enter opens", self.hits.len() - meshes), [150, 160, 150, 255]);
        let list_y = lh * 2 + 28;
        let rows = ((h as i32 - list_y - 8) / lh).max(1) as usize;
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + rows {
            self.top = self.sel + 1 - rows;
        }
        for (r, &i) in self.hits.iter().enumerate().skip(self.top).take(rows) {
            let y = list_y + (r - self.top) as i32 * lh;
            if r == self.sel {
                c.fill_rect(4, y - 1, PANEL - 8, lh, [60, 70, 100, 255]);
            }
            let e = &self.cat.entries[i];
            let (tag, col) = if e.kind == Kind::Mesh { ("M", [140, 200, 255, 255]) } else { ("T", [230, 200, 120, 255]) };
            text.draw(&mut c, 10, y, tag, col);
            text.draw(&mut c, 28, y, &e.path, [210, 210, 210, 255]);
        }
        // the status lines over the view
        let mut lines: Vec<String> = Vec::new();
        if let Shown::Mesh(mv) = &self.shown {
            let (a, _, _) = Self::mesh_frames(mv);
            let seq = mv.m.seqs.get(mv.seq).map(|s| format!("{} ({} frames at {}/s)", s.0, s.2, s.3)).unwrap_or_default();
            lines.push(format!("{}   sequence {seq}   frame {a}{}", mv.m.path, if mv.playing { "" } else { " (paused)" }));
            let skins = mv.skin.and_then(|k| mv.sets.get(k)).map(|s| s.source.clone()).unwrap_or_else(|| "the mesh's own".into());
            let slots: Vec<String> = mv.slot_names.iter().enumerate().map(|(i, n)| format!("{i}: {}", n.as_deref().unwrap_or("-"))).collect();
            lines.push(format!("skins of {skins} (K)   {}", slots.join("  ")));
            lines.push(format!(
                "{}{}   FOV {:.0}   Space play  Left/Right frame  Up/Down sequence  F in hand  M mod  B back faces  O open edges  L wire  X export",
                if mv.with_mod { "with mod   " } else { "" },
                mv.held.as_ref().map(|h| format!("in hand ({})", h.2)).unwrap_or_else(|| "free camera: drag, WASD/QE, wheel, R".into()),
                self.cam.fov
            ));
        }
        lines.push(self.status.clone());
        let y0 = h as i32 - lines.len() as i32 * lh - 8;
        c.fill_rect(PANEL, y0 - 4, vw as i32, h as i32 - y0 + 4, [0, 0, 0, 150]);
        for (k, l) in lines.iter().enumerate() {
            small.draw(&mut c, PANEL + 10, y0 + k as i32 * lh, l, [220, 220, 220, 255]);
        }
        c
    }
}

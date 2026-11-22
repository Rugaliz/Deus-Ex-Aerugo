//! The viewer window: a search box, filters and the result list on the left; the model (animated) or texture in the
//! middle with a toolbar above it; the model's sequences, skins and textures on the right.
//!
//! Everything has a button; the keys are shortcuts. Search: type to filter (every word must appear in the object
//! path), click a row or use Up/Down/PageUp/PageDown (the selection opens as you go), Enter or Tab to go to the view.
//! View: left mouse drag looks around, right drag turns around the model, wheel moves forward and back,
//! W/A/S/D/Q/E fly (Shift faster), R resets the camera; Space plays or pauses, Left/Right step frames, Up/Down change
//! the sequence, K the skin set, F shows a weapon in hand as the game places it, M the mods folder's replacement,
//! B back faces, O open edges, L wireframe, +/- field of view, X exports; Tab or / goes back to the search.
//! Ctrl and +/- (or Ctrl and the wheel) make the text and buttons bigger or smaller.

use crate::catalog::{self, Catalog, Kind, LoadedMesh, Picture, SkinSet};
use crate::font::Text;
use crate::render::{self, Camera, Canvas, DrawOptions, V3};
use sdl3::keyboard::Keycode as K;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

enum Shown {
    None,
    Mesh(Box<MeshView>),
    Texture { path: String, pic: Picture, zoom: f32, from: Option<Box<MeshView>> },
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
    /// First-person placement (offset from the eye, scale, class) when shown in hand.
    held: Option<(V3, f32, String)>,
    centre: V3,
    radius: f32,
    cam: Camera,
}

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Meshes,
    Textures,
}

/// What a click on a part of the window does.
#[derive(Clone, PartialEq)]
enum Act {
    SearchBox,
    ClearSearch,
    Filter(Filter),
    Row(usize),
    Divider,
    Key(K),
    ExportObj,
    OpenFolder,
    Seq(usize),
    Skin(Option<usize>),
    Slot(usize),
    BackToModel,
    Zoom(f32),
    Scale(i32),
}

struct Hit {
    r: [i32; 4],
    act: Act,
}

struct Viewer {
    cat: Catalog,
    out: PathBuf,
    query: String,
    filter: Filter,
    hits: Vec<usize>,
    sel: usize,
    top: usize,
    searching: bool,
    shown: Shown,
    cam: Camera,
    opt: DrawOptions,
    status: String,
    text: Option<Text>,
    /// Text and button size: every font texel drawn as `scale` x `scale` pixels.
    scale: i32,
    panel: i32,
    mouse: (i32, i32),
    hitmap: Vec<Hit>,
    title: String,
    /// The list scrolls to the selection (after keys moved it, not after the wheel).
    follow: bool,
    /// The side panel: its first row shown and where it is.
    side_top: usize,
    side_rect: [i32; 4],
}

pub fn run(game: &Path, args: &mut Vec<String>) -> Result<(), String> {
    let shot = crate::take_value(args, "--shot");
    let frames: u32 = crate::take_value(args, "--frames").and_then(|v| v.parse().ok()).unwrap_or(if shot.is_some() { 3 } else { u32::MAX });
    let size = crate::take_value(args, "--size").and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))).unwrap_or((1600, 960));
    let out = crate::take_value(args, "--out").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("ModKit"));
    let scale: Option<i32> = crate::take_value(args, "--scale").and_then(|v| v.parse().ok());
    let fp = crate::take_flag(args, "--fp");
    let with_mod = crate::take_flag(args, "--mod");
    let cat = Catalog::open(game)?;
    let text = Text::load(&cat, &["FontMenuSmall", "TechMedium", "SmallFont"]);
    let mut v = Viewer {
        cat,
        out,
        query: args.join(" "),
        filter: Filter::All,
        hits: Vec::new(),
        sel: 0,
        top: 0,
        searching: true,
        shown: Shown::None,
        cam: Camera { pos: [-100.0, 0.0, 0.0], yaw: 0.0, pitch: 0.0, fov: 60.0 },
        opt: DrawOptions::default(),
        status: "Type to search the game's models and textures, then click one (or Up/Down, Enter)".into(),
        text,
        scale: scale.unwrap_or(if size.1 >= 700 { 2 } else { 1 }).clamp(1, 4),
        panel: 0,
        mouse: (0, 0),
        hitmap: Vec::new(),
        title: String::new(),
        follow: true,
        side_top: 0,
        side_rect: [0; 4],
    };
    v.panel = 250 * v.scale;
    v.refilter();
    // a name given on the command line that names one entry opens it
    if !v.query.is_empty() && !v.hits.is_empty() {
        let exact = v.hits.iter().position(|&i| v.cat.entries[i].path.to_ascii_lowercase().ends_with(&format!(".{}", v.query.to_ascii_lowercase())));
        if v.hits.len() == 1 || exact.is_some() {
            v.sel = exact.unwrap_or(0);
            v.open();
            v.searching = false;
            if with_mod {
                if let Shown::Mesh(mv) = &mut v.shown {
                    mv.with_mod = true;
                }
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
    let mut dividing = false;
    let mut frame = 0u32;
    'run: loop {
        // window coordinates -> pixels (high-density screens)
        let (ww, _) = canvas.window().size();
        let (pw, _) = canvas.window().size_in_pixels();
        let density = pw as f32 / ww.max(1) as f32;
        let px = |x: f32| (x * density) as i32;
        for e in events.poll_iter() {
            use sdl3::event::Event;
            match e {
                Event::Quit { .. } => break 'run,
                Event::TextInput { text, .. } if v.searching => {
                    v.query.push_str(&text);
                    v.refilter();
                }
                Event::KeyDown { keycode: Some(k), keymod, .. } => {
                    let ctrl = keymod.intersects(sdl3::keyboard::Mod::LCTRLMOD | sdl3::keyboard::Mod::RCTRLMOD);
                    if ctrl && matches!(k, K::Equals | K::Plus | K::KpPlus) {
                        v.set_scale(v.scale + 1);
                    } else if ctrl && matches!(k, K::Minus | K::KpMinus) {
                        v.set_scale(v.scale - 1);
                    } else if v.searching {
                        v.search_key(k);
                    } else {
                        v.key(k);
                    }
                }
                Event::MouseButtonDown { mouse_btn, x, y, .. } => {
                    let (x, y) = (px(x), px(y));
                    match v.hit(x, y) {
                        Some(Act::Divider) => dividing = true,
                        Some(a) if mouse_btn == sdl3::mouse::MouseButton::Left => v.act(a),
                        Some(_) => {}
                        None if x > v.panel => {
                            v.searching = false;
                            drag = Some(mouse_btn);
                        }
                        None => {}
                    }
                }
                Event::MouseButtonUp { .. } => {
                    drag = None;
                    dividing = false;
                }
                Event::MouseMotion { x, y, xrel, yrel, .. } => {
                    v.mouse = (px(x), px(y));
                    if dividing {
                        v.panel = v.mouse.0.clamp(120 * v.scale, (pw as i32 / 2).max(200));
                    }
                    match drag {
                        Some(sdl3::mouse::MouseButton::Left) => v.look(xrel * 0.25, -yrel * 0.25),
                        Some(sdl3::mouse::MouseButton::Right) => v.orbit(xrel * 0.4, -yrel * 0.4),
                        _ => {}
                    }
                }
                Event::MouseWheel { y, mouse_x, .. } => {
                    let ctrl = sdl.keyboard().mod_state().intersects(sdl3::keyboard::Mod::LCTRLMOD | sdl3::keyboard::Mod::RCTRLMOD);
                    if ctrl {
                        v.set_scale(v.scale + y.signum() as i32);
                    } else if px(mouse_x) < v.panel {
                        v.scroll(-(y * 3.0) as i64);
                    } else if px(mouse_x) >= v.side_rect[0] && v.side_rect[2] > 0 {
                        v.side_top = (v.side_top as i64 - (y * 3.0) as i64).max(0) as usize;
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
        let (w, h) = (w.max(320), h.max(240));
        let img = v.draw(w as usize, h as usize);
        if texture.is_none() || (tw, th) != (w, h) {
            texture = Some(creator.create_texture_streaming(sdl3::pixels::PixelFormat::RGBA32, w, h).map_err(|e| e.to_string())?);
            (tw, th) = (w, h);
        }
        let title = v.window_title();
        if title != v.title {
            let _ = canvas.window_mut().set_title(&title);
            v.title = title;
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
    fn window_title(&self) -> String {
        match &self.shown {
            Shown::Mesh(mv) => format!("{} - Deus Ex Aerugo mod kit", mv.m.path),
            Shown::Texture { path, .. } => format!("{path} - Deus Ex Aerugo mod kit"),
            Shown::None => "Deus Ex Aerugo mod kit".into(),
        }
    }

    fn set_scale(&mut self, s: i32) {
        let s = s.clamp(1, 4);
        if s != self.scale {
            self.panel = self.panel * s / self.scale;
            self.scale = s;
        }
    }

    fn line_h(&self) -> i32 {
        self.text.as_ref().map_or(12, |t| t.height) * self.scale + 4 * self.scale
    }

    fn refilter(&mut self) {
        let q: Vec<String> = self.query.split_whitespace().map(str::to_ascii_lowercase).collect();
        let filter = self.filter;
        self.hits = (0..self.cat.entries.len())
            .filter(|&i| {
                let e = &self.cat.entries[i];
                let kind_ok = match filter {
                    Filter::All => true,
                    Filter::Meshes => e.kind == Kind::Mesh,
                    Filter::Textures => e.kind == Kind::Texture,
                };
                let p = e.path.to_ascii_lowercase();
                kind_ok && q.iter().all(|w| p.contains(w.as_str()))
            })
            .collect();
        self.sel = self.sel.min(self.hits.len().saturating_sub(1));
        self.top = self.top.min(self.sel);
    }

    fn scroll(&mut self, rows: i64) {
        self.follow = false;
        self.top = (self.top as i64 + rows).clamp(0, self.hits.len().saturating_sub(1) as i64) as usize;
    }

    /// Moves the selection and opens what it lands on.
    fn select(&mut self, sel: usize) {
        let sel = sel.min(self.hits.len().saturating_sub(1));
        let changed = sel != self.sel || matches!(self.shown, Shown::None);
        self.sel = sel;
        if changed && !self.hits.is_empty() {
            self.open();
        }
    }

    fn search_key(&mut self, k: K) {
        match k {
            K::Backspace => {
                self.query.pop();
                self.refilter();
            }
            K::Up | K::Down | K::PageUp | K::PageDown | K::Home | K::End if !self.hits.is_empty() => {
                self.follow = true;
                let sel = match k {
                    K::Up => self.sel.saturating_sub(1),
                    K::Down => self.sel + 1,
                    K::PageUp => self.sel.saturating_sub(20),
                    K::PageDown => self.sel + 20,
                    K::Home => 0,
                    _ => usize::MAX,
                };
                self.select(sel);
            }
            K::Return | K::KpEnter => {
                self.open();
                self.searching = false;
            }
            K::Tab | K::Escape => self.searching = false,
            _ => {}
        }
    }

    fn hit(&self, x: i32, y: i32) -> Option<Act> {
        self.hitmap.iter().rev().find(|h| x >= h.r[0] && y >= h.r[1] && x < h.r[0] + h.r[2] && y < h.r[1] + h.r[3]).map(|h| h.act.clone())
    }

    fn act(&mut self, a: Act) {
        match a {
            Act::SearchBox => self.searching = true,
            Act::ClearSearch => {
                self.query.clear();
                self.searching = true;
                self.follow = true;
                self.refilter();
            }
            Act::Filter(f) => {
                self.filter = f;
                self.searching = true;
                self.refilter();
            }
            Act::Row(r) => {
                self.searching = true;
                self.select(r);
            }
            Act::Divider => {}
            Act::Key(k) => self.key(k),
            Act::ExportObj => self.export_obj(),
            Act::OpenFolder => self.open_folder(),
            Act::Seq(i) => {
                if let Shown::Mesh(mv) = &mut self.shown {
                    mv.seq = i;
                    mv.t = 0.0;
                    mv.playing = true;
                }
            }
            Act::Skin(s) => self.set_skin(s),
            Act::Slot(i) => self.open_slot(i),
            Act::BackToModel => {
                if let Shown::Texture { from: Some(_), .. } = &self.shown {
                    let Shown::Texture { from: Some(mv), .. } = std::mem::replace(&mut self.shown, Shown::None) else { unreachable!() };
                    self.cam = mv.cam;
                    self.shown = Shown::Mesh(mv);
                }
            }
            Act::Zoom(z) => {
                if let Shown::Texture { zoom, .. } = &mut self.shown {
                    *zoom = z;
                }
            }
            Act::Scale(d) => self.set_scale(self.scale + d),
        }
    }

    fn open(&mut self) {
        let Some(&i) = self.hits.get(self.sel) else { return };
        let e = self.cat.entries[i].clone();
        // keep the camera of the model shown before for the same model
        match e.kind {
            Kind::Texture => match self.cat.picture(&e.path) {
                Ok(pic) => {
                    self.status = format!("{} ({}x{})", e.path, pic.w, pic.h);
                    self.shown = Shown::Texture { path: e.path, pic, zoom: 0.0, from: None };
                }
                Err(err) => self.status = err,
            },
            Kind::Mesh => match self.cat.mesh(&e.path, false) {
                Ok(m) => {
                    let sets = self.cat.skins_for(&m.path);
                    let skin = (!sets.is_empty()).then_some(0);
                    let (pics, slot_names, _) = crate::slot_pictures(&mut self.cat, &m, skin);
                    let mut mv = MeshView { m, pics, slot_names, sets, skin, seq: 0, t: 0.0, playing: true, with_mod: false, held: None, centre: [0.0; 3], radius: 1.0, cam: self.cam };
                    // start on an idle sequence, else one that moves
                    let idle = mv.m.seqs.iter().position(|s| s.2 > 1 && s.0.to_ascii_lowercase().starts_with("idle"));
                    if let Some(k) = idle.or_else(|| mv.m.seqs.iter().position(|s| s.2 > 1 && !s.0.eq_ignore_ascii_case("All"))) {
                        mv.seq = k;
                    }
                    self.status = format!("{} - {} frames, {} faces, {} sequences", e.path, mv.m.lod.mesh.anim_frames, mv.m.lod.faces.len(), mv.m.seqs.len());
                    self.shown = Shown::Mesh(Box::new(mv));
                    self.side_top = 0;
                    self.frame_camera();
                }
                Err(err) => self.status = err,
            },
        }
    }

    fn open_slot(&mut self, i: usize) {
        let Shown::Mesh(mv) = &self.shown else { return };
        let (Some(Some(path)), Some(Some(pic))) = (mv.slot_names.get(i).cloned(), mv.pics.get(i).cloned()) else { return };
        let Shown::Mesh(mut mv) = std::mem::replace(&mut self.shown, Shown::None) else { unreachable!() };
        // the camera comes back with the model
        mv.cam = self.cam;
        self.status = format!("{path} ({}x{}), texture slot {i} of {}", pic.w, pic.h, mv.m.path);
        self.shown = Shown::Texture { path, pic: (*pic).clone(), zoom: 0.0, from: Some(mv) };
    }

    fn set_skin(&mut self, skin: Option<usize>) {
        let Shown::Mesh(mv) = &mut self.shown else { return };
        mv.skin = skin;
        let m = std::mem::replace(&mut mv.m, LoadedMesh { path: String::new(), lod: ue_assets::mesh::LodMesh::from_plain(Default::default()), plain: true, slots: vec![], seqs: vec![] });
        let (pics, names, src) = crate::slot_pictures(&mut self.cat, &m, skin);
        let Shown::Mesh(mv) = &mut self.shown else { return };
        mv.m = m;
        mv.pics = pics;
        mv.slot_names = names;
        self.status = format!("skins: {}", src.unwrap_or_else(|| "the mesh's own textures".into()));
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

    fn look(&mut self, dyaw: f32, dpitch: f32) {
        if matches!(self.shown, Shown::Mesh(_)) {
            self.cam.yaw += dyaw;
            self.cam.pitch = (self.cam.pitch + dpitch).clamp(-89.0, 89.0);
        }
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

    fn texture_zoom(zoom: f32, fit: f32) -> f32 {
        if zoom > 0.0 {
            zoom
        } else {
            fit
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
            // the class holding this mesh in first person
            let class = mv.sets.iter().find(|s| s.source.ends_with(".PlayerViewMesh")).and_then(|s| s.source.split('.').nth(1).map(str::to_string));
            match class.map(|c| (catalog::first_person(&game, &c), c)) {
                Some((Ok((_, o, s)), c)) => {
                    self.status = format!("in hand as {c} holds it, Hor+ like the game: resize the window to try screen shapes");
                    mv.held = Some((o, s, c));
                }
                Some((Err(e), _)) => self.status = e,
                None => self.status = "no class holds this model in first person".into(),
            }
        }
        self.frame_camera();
    }

    fn key(&mut self, k: K) {
        match k {
            K::Tab | K::Slash => self.searching = true,
            K::R => self.frame_camera(),
            K::B => self.opt.back_faces = !self.opt.back_faces,
            K::O => self.opt.open_edges = !self.opt.open_edges,
            K::L => self.opt.wireframe = !self.opt.wireframe,
            K::Equals | K::Plus | K::KpPlus => self.cam.fov = (self.cam.fov + 5.0).min(170.0),
            K::Minus | K::KpMinus => self.cam.fov = (self.cam.fov - 5.0).max(10.0),
            K::F => self.toggle_held(),
            K::X => self.export(),
            K::Backspace => self.act(Act::BackToModel),
            K::M => {
                if let Shown::Mesh(mv) = &mut self.shown {
                    mv.with_mod = !mv.with_mod;
                    let on = mv.with_mod;
                    let has = ue_package::mods::mesh(&mv.m.path).is_some();
                    self.reload_mesh();
                    self.status = match (on, has) {
                        (true, true) => "with the mods folder's replacement".into(),
                        (true, false) => "no mod replaces this model".into(),
                        _ => "the game's own model".into(),
                    };
                }
            }
            K::K => {
                if let Shown::Mesh(mv) = &self.shown {
                    if !mv.sets.is_empty() {
                        let next = match mv.skin {
                            None => Some(0),
                            Some(i) if i + 1 < mv.sets.len() => Some(i + 1),
                            _ => None,
                        };
                        self.set_skin(next);
                    }
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
            Ok(f) => format!("exported {}", abs(&f).display()),
            Err(e) => e,
        };
    }

    fn export_obj(&mut self) {
        let Shown::Mesh(mv) = &self.shown else { return };
        let (a, _, _) = Self::mesh_frames(mv);
        let (pkg, name) = mv.m.path.split_once('.').unwrap_or(("", &mv.m.path));
        let file = self.out.join(pkg).join(name).join(format!("{name}_frame{a:03}.obj"));
        let r = std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string()).and_then(|_| crate::write_obj(&mv.m, a, &Self::pose(mv), &mv.pics, &file));
        self.status = match r {
            Ok(()) => format!("exported frame {a}: {}", abs(&file).display()),
            Err(e) => e,
        };
    }

    fn open_folder(&mut self) {
        let _ = std::fs::create_dir_all(&self.out);
        let p = abs(&self.out);
        let s = p.to_string_lossy().replace('\\', "/");
        let url = if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") };
        self.status = match sdl3::url::open_url(&url) {
            Ok(()) => format!("opened {}", p.display()),
            Err(e) => format!("cannot open {}: {e}", p.display()),
        };
    }

    // drawing

    fn button(&mut self, c: &mut Canvas, x: i32, y: i32, label: &str, on: bool, act: Act) -> i32 {
        let Some(text) = &self.text else { return x };
        let s = self.scale;
        let (w, h) = (text.width(label, s) + 10 * s, self.line_h() + 2 * s);
        let hover = self.mouse.0 >= x && self.mouse.1 >= y && self.mouse.0 < x + w && self.mouse.1 < y + h;
        let bg = match (on, hover) {
            (true, true) => [90, 120, 170, 255],
            (true, false) => [70, 96, 140, 255],
            (false, true) => [64, 68, 82, 255],
            (false, false) => [44, 47, 58, 255],
        };
        c.fill_rect(x, y, w, h, bg);
        text.draw(c, x + 5 * s, y + 3 * s, label, [235, 235, 235, 255], s);
        self.hitmap.push(Hit { r: [x, y, w, h], act });
        x + w + 4 * s
    }

    /// Buttons left to right, wrapping at `right`; returns the y below them.
    fn toolbar(&mut self, c: &mut Canvas, x0: i32, y0: i32, right: i32, items: Vec<(String, bool, Act)>) -> i32 {
        if self.text.is_none() {
            return y0;
        }
        let s = self.scale;
        let bh = self.line_h() + 2 * s;
        let (mut x, mut y) = (x0, y0);
        for (label, on, act) in items {
            let w = self.text.as_ref().unwrap().width(&label, s) + 10 * s;
            if x + w > right && x > x0 {
                x = x0;
                y += bh + 4 * s;
            }
            x = self.button(c, x, y, &label, on, act);
        }
        y + bh + 4 * s
    }

    fn draw(&mut self, w: usize, h: usize) -> Canvas {
        self.hitmap.clear();
        let mut c = Canvas::new(w, h);
        c.clear([24, 27, 36]);
        let s = self.scale;
        let lh = self.line_h();
        self.panel = self.panel.clamp(120 * s, (w as i32 / 2).max(200));
        let panel = self.panel;
        let view_x = panel + 4;
        let view_w = w as i32 - view_x;

        // the toolbar over the view
        let items = self.toolbar_items();
        let view_y = self.toolbar(&mut c, view_x + 6 * s, 6 * s, w as i32 - 6 * s, items);
        // the side panel of a model: sequences, skins, textures
        let side_w = if matches!(self.shown, Shown::Mesh(_)) && view_w >= 400 * s { (170 * s).min(view_w / 3) } else { 0 };
        let status_lines = 3;
        let status_h = status_lines * lh + 8 * s;
        let (vx, vy, vw, vh) = (view_x, view_y, (view_w - side_w).max(16), (h as i32 - view_y - status_h).max(16));

        // the picture
        let mut view = Canvas::new(vw as usize, vh as usize);
        view.clear([28, 32, 44]);
        match &self.shown {
            Shown::Mesh(mv) => {
                let pos = Self::pose(mv);
                let mut cam = self.cam;
                if mv.held.is_some() {
                    // Hor+ as the game: the field of view is the 4:3 one, widened with the view's shape
                    let ht = (cam.fov.to_radians() * 0.5).tan() * ((vw as f32 / vh as f32) / (4.0 / 3.0)).max(1.0);
                    cam.fov = 2.0 * ht.atan().to_degrees();
                }
                render::draw_mesh(&mut view, &cam, &mv.m, &pos, &mv.pics, self.opt);
            }
            Shown::Texture { pic, zoom, .. } => {
                let fit = ((vw as f32 - 40.0) / pic.w as f32).min((vh as f32 - 40.0) / pic.h as f32).clamp(0.05, 8.0);
                let z = Self::texture_zoom(*zoom, fit);
                let (x, y) = ((vw as f32 - pic.w as f32 * z) / 2.0, (vh as f32 - pic.h as f32 * z) / 2.0);
                view.picture(pic, x as i32, y as i32, z);
            }
            Shown::None => {
                if let Some(t) = &self.text {
                    let lines = [
                        "Search on the left: type a name (GEPGun, Trench, Skins ...),",
                        "then click a model (M) or a texture (T) in the list.",
                        "",
                        "Models: drag to look, right-drag to turn around, wheel to move,",
                        "W A S D Q E to fly. Buttons above do the rest.",
                    ];
                    for (k, l) in lines.iter().enumerate() {
                        t.draw(&mut view, 24 * s, 24 * s + k as i32 * lh, l, [170, 180, 200, 255], s);
                    }
                }
            }
        }
        for y in 0..vh as usize {
            let src = y * vw as usize * 4;
            let dst = ((vy as usize + y) * w + vx as usize) * 4;
            if vy as usize + y < h {
                c.rgba[dst..dst + vw as usize * 4].copy_from_slice(&view.rgba[src..src + vw as usize * 4]);
            }
        }
        self.side_rect = [0; 4];
        if side_w > 0 {
            self.side_panel(&mut c, vx + vw, vy, side_w, vh);
        }
        self.status_bar(&mut c, view_x, h as i32 - status_h, view_w, status_h);
        self.search_panel(&mut c, panel, h as i32);
        // the divider
        c.fill_rect(panel, 0, 4, h as i32, [50, 54, 66, 255]);
        self.hitmap.push(Hit { r: [panel - 3, 0, 10, h as i32], act: Act::Divider });
        c
    }

    fn toolbar_items(&self) -> Vec<(String, bool, Act)> {
        let mut items: Vec<(String, bool, Act)> = Vec::new();
        match &self.shown {
            Shown::Mesh(mv) => {
                items.push(("Export glTF (X)".into(), false, Act::Key(K::X)));
                items.push(("Export frame as OBJ".into(), false, Act::ExportObj));
                items.push((if mv.playing { "Pause (Space)" } else { "Play (Space)" }.into(), mv.playing, Act::Key(K::Space)));
                items.push(("< Frame".into(), false, Act::Key(K::Left)));
                items.push(("Frame >".into(), false, Act::Key(K::Right)));
                items.push(("In hand (F)".into(), mv.held.is_some(), Act::Key(K::F)));
                items.push(("Mod (M)".into(), mv.with_mod, Act::Key(K::M)));
                items.push(("Back faces (B)".into(), self.opt.back_faces, Act::Key(K::B)));
                items.push(("Open edges (O)".into(), self.opt.open_edges, Act::Key(K::O)));
                items.push(("Wire (L)".into(), self.opt.wireframe, Act::Key(K::L)));
                items.push(("Reset view (R)".into(), false, Act::Key(K::R)));
                items.push((format!("FOV {:.0} -", self.cam.fov), false, Act::Key(K::Minus)));
                items.push(("FOV +".into(), false, Act::Key(K::Plus)));
            }
            Shown::Texture { from, zoom, .. } => {
                if let Some(mv) = from {
                    items.push((format!("< Back to {}", mv.m.path), false, Act::BackToModel));
                }
                items.push(("Export PNG (X)".into(), false, Act::Key(K::X)));
                for (label, z) in [("Fit", 0.0), ("1:1", 1.0), ("2x", 2.0), ("4x", 4.0)] {
                    items.push((label.into(), *zoom == z, Act::Zoom(z)));
                }
            }
            Shown::None => {}
        }
        if !matches!(self.shown, Shown::None) {
            items.push(("Open export folder".into(), false, Act::OpenFolder));
        }
        items.push(("Text -".into(), false, Act::Scale(-1)));
        items.push(("Text +".into(), false, Act::Scale(1)));
        items
    }

    fn side_panel(&mut self, c: &mut Canvas, x: i32, y: i32, w: i32, h: i32) {
        let Some(text) = self.text.take() else { return };
        let s = self.scale;
        let lh = self.line_h();
        c.fill_rect(x, y, w, h, [20, 22, 30, 255]);
        let Shown::Mesh(mv) = &self.shown else {
            self.text = Some(text);
            return;
        };
        let mut rows: Vec<(String, bool, Option<Act>)> = Vec::new();
        rows.push(("Skins".into(), false, None));
        rows.push(("  the mesh's own".into(), mv.skin.is_none(), Some(Act::Skin(None))));
        for (i, set) in mv.sets.iter().enumerate() {
            let class = set.source.split('.').nth(1).unwrap_or(&set.source);
            rows.push((format!("  {class}"), mv.skin == Some(i), Some(Act::Skin(Some(i)))));
        }
        rows.push((String::new(), false, None));
        rows.push(("Textures (click to open)".into(), false, None));
        for (i, n) in mv.slot_names.iter().enumerate() {
            let short = n.as_deref().map(|p| p.rsplit('.').next().unwrap_or(p)).unwrap_or("-");
            rows.push((format!("  {i}: {short}"), false, n.as_ref().map(|_| Act::Slot(i))));
        }
        rows.push((String::new(), false, None));
        rows.push((format!("Sequences ({})", mv.m.seqs.len()), false, None));
        for (i, (name, _, n, _)) in mv.m.seqs.iter().enumerate() {
            rows.push((format!("  {name} ({n})"), i == mv.seq, Some(Act::Seq(i))));
        }
        // scrolled by the wheel over the panel
        let fits = ((h - 8 * s) / lh).max(1) as usize;
        self.side_top = self.side_top.min(rows.len().saturating_sub(fits));
        self.side_rect = [x, y, w, h];
        let total = rows.len();
        if total > fits {
            let bar_h = ((fits as f32 / total as f32) * h as f32).max(8.0 * s as f32) as i32;
            let bar_y = y + ((self.side_top as f32 / (total - fits) as f32) * (h - bar_h) as f32) as i32;
            c.fill_rect(x + w - 4 * s, bar_y, 3 * s, bar_h, [90, 96, 116, 255]);
        }
        let rows: Vec<_> = rows.into_iter().skip(self.side_top).collect();
        let mut yy = y + 4 * s;
        for (label, on, act) in rows {
            if yy + lh > y + h {
                break;
            }
            let hover = act.is_some() && self.mouse.0 >= x && self.mouse.0 < x + w && self.mouse.1 >= yy && self.mouse.1 < yy + lh;
            if on || hover {
                c.fill_rect(x + 2, yy, w - 4, lh, if on { [60, 80, 120, 255] } else { [44, 48, 60, 255] });
            }
            let col = if act.is_none() { [150, 170, 210, 255] } else { [225, 225, 225, 255] };
            text.draw(c, x + 6 * s, yy + 2 * s, &text.fit(&label, w - 10 * s, s), col, s);
            if let Some(a) = act {
                self.hitmap.push(Hit { r: [x, yy, w, lh], act: a });
            }
            yy += lh;
        }
        self.text = Some(text);
    }

    fn status_bar(&mut self, c: &mut Canvas, x: i32, y: i32, w: i32, h: i32) {
        let Some(text) = &self.text else { return };
        let s = self.scale;
        let lh = self.line_h();
        c.fill_rect(x, y, w, h, [14, 15, 20, 255]);
        let mut lines: Vec<String> = Vec::new();
        if let Shown::Mesh(mv) = &self.shown {
            let (a, _, _) = Self::mesh_frames(mv);
            let seq = mv.m.seqs.get(mv.seq).map(|q| format!("{} ({} frames at {}/s)", q.0, q.2, q.3)).unwrap_or_default();
            lines.push(format!("{}   {seq}   frame {a}{}", mv.m.path, if mv.playing { "" } else { "  (paused)" }));
            lines.push(match &mv.held {
                Some(h) => format!("in hand as {} holds it", h.2),
                None => "drag: look   right-drag: turn around   wheel: move   W A S D Q E: fly (Shift: faster)".into(),
            });
        }
        lines.push(self.status.clone());
        let y0 = y + h - lines.len() as i32 * lh - 4 * s;
        for (k, l) in lines.iter().enumerate() {
            let col = if k + 1 == lines.len() { [240, 220, 150, 255] } else { [210, 210, 210, 255] };
            text.draw(c, x + 8 * s, y0 + k as i32 * lh, &text.fit(l, w - 16 * s, s), col, s);
        }
    }

    fn search_panel(&mut self, c: &mut Canvas, panel: i32, h: i32) {
        let Some(text) = self.text.take() else { return };
        let s = self.scale;
        let lh = self.line_h();
        c.fill_rect(0, 0, panel, h, [16, 18, 24, 255]);
        // the search box with its clear button
        let bh = lh + 6 * s;
        c.fill_rect(6 * s, 6 * s, panel - 12 * s, bh, if self.searching { [40, 64, 44, 255] } else { [34, 36, 44, 255] });
        self.hitmap.push(Hit { r: [6 * s, 6 * s, panel - 12 * s, bh], act: Act::SearchBox });
        let cursor = if self.searching { "_" } else { "" };
        let shown = if self.query.is_empty() && !self.searching { "Search (click or Tab)".to_string() } else { format!("{}{cursor}", self.query) };
        let room = panel - 30 * s - text.width("x", s);
        text.draw(c, 11 * s, 9 * s, &text.fit(&shown, room, s), [225, 235, 225, 255], s);
        if !self.query.is_empty() {
            let bx = panel - 6 * s - lh - 2 * s;
            let hover = self.mouse.0 >= bx && self.mouse.0 < bx + lh && self.mouse.1 >= 6 * s && self.mouse.1 < 6 * s + bh;
            c.fill_rect(bx, 8 * s, lh, bh - 4 * s, if hover { [120, 70, 70, 255] } else { [70, 50, 50, 255] });
            text.draw(c, bx + lh / 2 - text.width("x", s) / 2, 9 * s, "x", [240, 240, 240, 255], s);
            self.hitmap.push(Hit { r: [bx, 6 * s, lh, bh], act: Act::ClearSearch });
        }
        // filters
        let q: Vec<String> = self.query.split_whitespace().map(str::to_ascii_lowercase).collect();
        let (mut nm, mut nt) = (0, 0);
        for e in &self.cat.entries {
            let p = e.path.to_ascii_lowercase();
            if q.iter().all(|w| p.contains(w.as_str())) {
                if e.kind == Kind::Mesh {
                    nm += 1
                } else {
                    nt += 1
                }
            }
        }
        self.text = Some(text);
        let chips = vec![
            (format!("All {}", nm + nt), self.filter == Filter::All, Act::Filter(Filter::All)),
            (format!("Models {nm}"), self.filter == Filter::Meshes, Act::Filter(Filter::Meshes)),
            (format!("Textures {nt}"), self.filter == Filter::Textures, Act::Filter(Filter::Textures)),
        ];
        let list_y = self.toolbar(c, 6 * s, 6 * s + bh + 6 * s, panel - 6 * s, chips) + 2 * s;
        let Some(text) = self.text.take() else { return };
        // the list
        let rows = ((h - list_y - 4 * s) / lh).max(1) as usize;
        if self.follow && (self.sel < self.top || self.sel >= self.top + rows) {
            self.top = if self.sel < self.top { self.sel } else { self.sel + 1 - rows };
        }
        let n = self.hits.len();
        self.top = self.top.min(n.saturating_sub(rows));
        for r in 0..rows.min(n.saturating_sub(self.top)) {
            let i = self.top + r;
            let y = list_y + r as i32 * lh;
            let hover = self.mouse.0 < panel && self.mouse.1 >= y && self.mouse.1 < y + lh;
            if i == self.sel {
                c.fill_rect(2 * s, y, panel - 4 * s - 4 * s, lh, [60, 74, 110, 255]);
            } else if hover {
                c.fill_rect(2 * s, y, panel - 4 * s - 4 * s, lh, [36, 40, 52, 255]);
            }
            let e = &self.cat.entries[self.hits[i]];
            let (tag, col) = if e.kind == Kind::Mesh { ("M", [140, 200, 255, 255]) } else { ("T", [230, 200, 120, 255]) };
            text.draw(c, 6 * s, y + 2 * s, tag, col, s);
            let tx = 6 * s + text.width("M", s) + 6 * s;
            text.draw(c, tx, y + 2 * s, &text.fit(&e.path, panel - tx - 10 * s, s), [215, 215, 215, 255], s);
            self.hitmap.push(Hit { r: [0, y, panel - 4 * s, lh], act: Act::Row(i) });
        }
        // the scroll bar
        if n > rows {
            let track = h - list_y - 4 * s;
            let bar_h = ((rows as f32 / n as f32) * track as f32).max(8.0 * s as f32) as i32;
            let bar_y = list_y + ((self.top as f32 / (n - rows) as f32) * (track - bar_h) as f32) as i32;
            c.fill_rect(panel - 4 * s, bar_y, 3 * s, bar_h, [90, 96, 116, 255]);
        }
        self.text = Some(text);
    }
}

fn abs(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

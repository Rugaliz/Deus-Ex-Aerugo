//! What the original renderer would draw from a camera (`dx-vis`, Render.dll's visibility pass): which actors count
//! as rendered (`LastRenderTime`) and which zones were seen. The actor filter and sprite boxes follow
//! `URender::SetupDynamics` and `FDynamicSprite::Setup` (see `docs/render-visibility.md`).

use crate::{LoadIssue, Session};
use dx_soft::Camera;
use dx_vis::walk::{occlude, DynLight, FrameInput, FrameVis, SkyZone, Sprite, VolLight, World};
use dx_vis::Frame;
use ue_vm::Value;

/// `EDrawType`: sprite, mesh, animated sprite.
const DT_SPRITE: u8 = 1;
const DT_MESH: u8 = 2;
const DT_SPRITE_ANIM_ONCE: u8 = 7;

/// `Session::vis_candidates` kinds.
const CAND_LIGHT: u8 = 1;
const CAND_SPRITE: u8 = 2;
const CAND_ITERATOR: u8 = 4;

impl Session {
    /// The moving brushes the tracker registers (1037ac40): actors with a `Brush` that are brushes and not `bStatic`
    /// (in the game only movers), in the level's actor order.
    fn tracker_movers(&mut self) -> Vec<u32> {
        let Some(brush) = self.vm.reg.class_by_name("Brush") else { return Vec::new() };
        self.vm
            .live_actors()
            .into_iter()
            .filter(|&a| self.vm.op(a, "Brush") != 0 && !self.vm.bp(a, "bStatic") && self.vm.class_of(a).map(|c| self.vm.is_a(c, brush)).unwrap_or(false))
            .collect()
    }

    /// What the tracker needs of mover `a` now: placement, `ABrush::BuildCoords`' point frame and its brush polygons.
    fn tracker_input(&mut self, a: u32) -> Option<(dx_vis::dynbsp::Mover, Vec<dx_vis::dynbsp::Poly>)> {
        let brush = self.vm.op(a, "Brush");
        let path = self.vm.reg.obj_path(ue_registry::ObjId(brush)).to_string();
        let (pi, export) = self.vm.reg.export_by_path(&path)?;
        if pi != self.map_pkg {
            return None;
        }
        if !self.brush_polys.contains_key(&export) {
            let polys = self.view.brush_polys(export).unwrap_or_default();
            let conv = polys
                .into_iter()
                .map(|p| dx_vis::dynbsp::Poly { base: p.base, normal: p.normal, texture_u: p.texture_u, texture_v: p.texture_v, vertices: p.vertices, poly_flags: p.poly_flags, texture: p.texture, pan_u: p.pan_u, pan_v: p.pan_v, i_brush_poly: p.i_brush_poly })
                .collect();
            self.brush_polys.insert(export, conv);
        }
        let pre_pivot = match self.vm.get_prop(a, "PrePivot", 0) {
            Ok(Value::Vector(v)) => v,
            _ => [0.0; 3],
        };
        let (point, vector, orientation) = self.vm.brush_coords(a);
        let m = dx_vis::dynbsp::Mover { id: a, location: self.vm.location(a), rotation: self.vm.rotation(a), pre_pivot, point, vector, orientation, special_lit: self.vm.bp(a, "bSpecialLit") };
        Some((m, self.brush_polys[&export].clone()))
    }

    /// The level's moving-brush tracker brought up to date before a visibility pass: made once per level
    /// (`GNewBrushTracker`, after `LoadMap` logged the player in), then `Update(Actor)` for every brush actor that
    /// `SetupDynamics` lets through (Render.dll 10b22f29: after the hidden and owner-see tests, a brush that is not
    /// static gets the tracker update instead of a sprite), in the actor order.
    fn update_brush_tracker(&mut self) {
        self.ensure_brush_tracker();
        let player = self.player;
        let behind_view = self.vm.bp(player, "bBehindView");
        let high_detail = self.vm.high_detail_actors;
        for a in self.tracker_movers() {
            if (!high_detail && self.vm.bp(a, "bHighDetail")) || self.vm.bp(a, "bHidden") {
                continue;
            }
            let owned = self.owned_by(a, player);
            if (self.vm.bp(a, "bOnlyOwnerSee") && !owned) || (self.vm.bp(a, "bOwnerNoSee") && owned && !behind_view) {
                continue;
            }
            let (loc, rot) = (self.vm.location(a), self.vm.rotation(a));
            if !self.brush_tracker.as_ref().map(|t| t.1.moved(a, loc, rot)).unwrap_or(false) {
                continue;
            }
            let Some(input) = self.tracker_input(a) else { continue };
            let Some((key, mut tracker, flags)) = self.brush_tracker.take() else { return };
            let Some(mut rc) = self.vm.world_model.take() else {
                self.brush_tracker = Some((key, tracker, flags));
                return;
            };
            {
                let mut others = |id: u32| self.tracker_input(id);
                let others_ref: &mut dyn FnMut(u32) -> _ = &mut others;
                let cell = std::cell::RefCell::new(others_ref);
                tracker.update(std::rc::Rc::make_mut(&mut rc), &input, &|id| (cell.borrow_mut())(id));
            }
            let key = (key.0, key.1, rc.surfs.len());
            self.vm.world_model = Some(rc);
            self.vm.world_model_gen += 1;
            self.brush_tracker = Some((key, tracker, flags));
        }
    }

    /// The level's model as the visibility walk and the level's collision see it (movers filtered in) and the dynamic
    /// nodes of mover `a` (tests and tools).
    pub fn tracked_model(&self, a: u32) -> Option<(&ue_assets::model::Model, Vec<usize>, usize)> {
        let (_, t, _) = self.brush_tracker.as_ref()?;
        Some((self.vm.world_model.as_deref()?, t.nodes_of(a), t.first_dynamic_surf))
    }

    /// The movers as the main frame's draw lists hold them (`OccludeBsp` draws a dynamic node like any other, whoever
    /// owns it, hidden or not): per mover, its brush export and the fragments the walk drew (the dynamic nodes' polygons
    /// with their surface's world frame, in the walk's order), and each node's place among them; the main frame's and its
    /// child frames' (sky, mirrors, at any depth). `None` without a walk
    /// that saw the movers.
    pub fn listed_movers(&mut self) -> Option<ListedMovers> {
        if !self.vis_movers {
            return None;
        }
        let mut found: Vec<(u32, u32, dx_vk::world::Fragment)> = Vec::new();
        let mut main: Vec<u32> = Vec::new();
        {
            let frames = &self.vis_cache.as_ref().filter(|(pkg, _)| *pkg == self.map_pkg)?.1;
            frames.first()?;
            let (_, tracker, _) = self.brush_tracker.as_ref()?;
            let m = self.vm.world_model.as_deref()?;
            let mut seen = std::collections::HashSet::new();
            // the main frame's and every child frame's (sky, mirrors: `child_views`), then the viewport windows' passes
            // (same engine frame, same tracker nodes)
            let views = self.subview_frames.iter().flatten().flatten();
            let drawn = frames.iter().enumerate().flat_map(|(fi, fv)| fv.vis.surfaces.iter().map(move |&(node, _, _)| (fi, node))).chain(views.flat_map(|fv| fv.vis.surfaces.iter().map(|&(node, _, _)| (usize::MAX, node))));
            for (fi, node) in drawn {
                if fi == 0 && !main.contains(&node) && m.nodes.get(node as usize).map(|nd| nd.i_surf >= 0 && tracker.surf_is_dynamic(nd.i_surf as usize)).unwrap_or(false) {
                    main.push(node);
                }
                let Some(nd) = m.nodes.get(node as usize) else { continue };
                if nd.i_surf < 0 || !tracker.surf_is_dynamic(nd.i_surf as usize) || !seen.insert(node) {
                    continue;
                }
                let Some((a, k)) = tracker.surf_owner(nd.i_surf as usize) else { continue };
                let sf = &m.surfs[nd.i_surf as usize];
                let vec = |i: i32| m.vectors.get(i as usize).copied().unwrap_or([0.0; 3]);
                let verts = (0..nd.num_vertices as usize).filter_map(|j| m.verts.get(nd.i_vert_pool as usize + j).and_then(|v| m.points.get(v.p_vertex as usize)).copied()).collect();
                let base = m.points.get(sf.p_base as usize).copied().unwrap_or([0.0; 3]);
                found.push((node, a, dx_vk::world::Fragment { surf: k, verts, base, u: vec(sf.v_texture_u), v: vec(sf.v_texture_v), normal: vec(sf.v_normal), zones: nd.i_zone }));
            }
        }
        let mut out = ListedMovers { main, ..Default::default() };
        let mut exports: std::collections::HashMap<u32, Option<usize>> = Default::default();
        for (node, a, f) in found {
            let export = match exports.get(&a) {
                Some(e) => *e,
                None => {
                    let e = self.mover_brush_export(a);
                    exports.insert(a, e);
                    if let Some(e) = e {
                        out.owners.push((a, e));
                    }
                    e
                }
            };
            let Some(export) = export else { continue };
            let list = out.fragments.entry(export).or_default();
            out.nodes.insert(node, (export, list.len()));
            list.push(f);
        }
        Some(out)
    }

    /// The level's model as stored in the map, without the movers (tests and tools).
    pub fn static_world_model(&self) -> &ue_assets::model::Model {
        &self.view.model
    }

    /// The level's tracker, made when the level is new (`LoadMap` makes it right after logging the player in, before
    /// the first tick: the session calls this at the start of each step). It works on the VM's level model
    /// (`Vm::world_model`, what the decal code reads too), and the level's collision gets its nodes.
    pub(crate) fn ensure_brush_tracker(&mut self) {
        let surfs = self.vm.world_model.as_ref().map(|m| m.surfs.len()).unwrap_or(0);
        let key = (self.vm.level, self.map_pkg);
        if self.brush_tracker.as_ref().map(|t| (t.0 .0, t.0 .1) == key && t.0 .2 == surfs).unwrap_or(false) {
            return;
        }
        if self.vm.world_model.is_none() {
            return;
        }
        let movers: Vec<_> = self.tracker_movers().into_iter().filter_map(|a| self.tracker_input(a)).collect();
        let mut rc = self.vm.world_model.take().unwrap();
        let tracker = dx_vis::dynbsp::Tracker::new(std::rc::Rc::make_mut(&mut rc), &movers);
        let flags = self.texture_merged_flags(&rc.surfs[tracker.first_dynamic_surf..].to_vec());
        let key = (key.0, key.1, rc.surfs.len());
        self.vm.world_model = Some(rc);
        self.vm.world_model_gen += 1;
        self.vm.sync_world_nodes();
        self.brush_tracker_synced = tracker.generation;
        self.brush_tracker = Some((key, tracker, flags));
    }

    /// Gives the level's collision the tracker's nodes when an update changed them.
    fn sync_world_nodes(&mut self) {
        let Some((_, t, _)) = &self.brush_tracker else { return };
        if t.generation != self.brush_tracker_synced {
            self.brush_tracker_synced = t.generation;
            self.vm.sync_world_nodes();
        }
    }

    /// Whether `a` is owned (through its `Owner` chain) by `viewer`.
    fn owned_by(&mut self, a: u32, viewer: u32) -> bool {
        let mut o = a;
        for _ in 0..64 {
            if o == 0 {
                return false;
            }
            if o == viewer {
                return true;
            }
            o = self.vm.op(o, "Owner");
        }
        false
    }

    /// The walk's flags of the movers' surfaces (the tracker's, after the level's): each surface's own with its
    /// texture's `PolyFlags` merged in, as for the level's surfaces (`vis_surf_flags`; Render.dll's walk ORs
    /// `Surf.Texture+0x80` into every surface's flags, 10b181b2, whoever owns it). This used to look up a `PolyFlags`
    /// property, which does not exist (0): a grate, vent cover or cell door that is a mover was drawn with holes
    /// but hid what lay behind it, so the holes showed nothing.
    fn texture_merged_flags(&mut self, surfs: &[ue_assets::model::BspSurf]) -> Vec<u32> {
        let mut cache: std::collections::HashMap<i32, u32> = Default::default();
        surfs.iter().map(|s| s.poly_flags | *cache.entry(s.texture).or_insert_with(|| self.view.texture_poly_flags(s.texture))).collect()
    }

    /// The poly flags of every world surface with its texture's `PolyFlags` merged in (the walk ORs them before
    /// deciding whether a surface hides what is behind it): `Texture`'s surface bools packed in flag order (there is
    /// no `PolyFlags` property to read; looking one up found nothing, so masked textures hid what was behind them).
    /// Cached per map.
    fn vis_surf_flags(&mut self) -> &Vec<u32> {
        if self.vis_flags.as_ref().map(|(m, _)| *m != self.map_pkg).unwrap_or(true) {
            let mut cache: std::collections::HashMap<i32, u32> = Default::default();
            let mut out = Vec::with_capacity(self.view.model.surfs.len());
            for i in 0..self.view.model.surfs.len() {
                let s = &self.view.model.surfs[i];
                let (flags, tex) = (s.poly_flags, s.texture);
                let tf = *cache.entry(tex).or_insert_with(|| self.view.texture_poly_flags(tex));
                out.push(flags | tf);
            }
            self.vis_flags = Some((self.map_pkg, out));
        }
        &self.vis_flags.as_ref().unwrap().1
    }

    /// The visibility frame of a camera on a `w` x `h` screen (`CreateMasterFrame`). The field of view spans a 4:3 width
    /// and widens on wider screens (Hor+, as the port draws); on a 4:3 screen it is the original's.
    pub fn vis_frame(cam: &Camera, w: i32, h: i32) -> Frame {
        let fov = ue_bsp::scene::hor_plus_fov(cam.fov, w as f32 / h.max(1) as f32);
        Frame::master(cam.pos, [cam.pitch, cam.yaw, cam.roll], fov, w, h)
    }

    /// Runs the original's visibility pass from `cam` (`URender::OccludeFrame`: the main view, then the sky zone and
    /// mirror frames it opens): the actors drawn in any frame (each once) and every frame's result.
    pub fn compute_visibility(&mut self, cam: &Camera, w: i32, h: i32) -> (Vec<u32>, Vec<FrameVis>) {
        self.vis_boxes.clear();
        self.vis_cands = None;
        let prof = std::env::var_os("DX_STEP_PROFILE").is_some();
        let mut t = std::time::Instant::now();
        let lap = |name: &'static str, t: &mut std::time::Instant| {
            if prof {
                let e = t.elapsed().as_secs_f64();
                crate::STEP_PROFILE.with(|p| *p.borrow_mut().entry(name).or_insert(0.0) += e);
                *t = std::time::Instant::now();
            }
        };
        let mut f = Self::vis_frame(cam, w, h);
        f.zone = self.vm.point_region(cam.pos).zone_number;
        let player = self.player;
        let player_ctrl = self.vm.ip(player, "ShowFlags") & 0x800 != 0;
        let mut flags = self.vis_surf_flags().clone();
        lap("    flags", &mut t);
        if self.vis_movers {
            self.update_brush_tracker();
            lap("    brush tracker", &mut t);
            self.sync_world_nodes();
            lap("    world nodes", &mut t);
        }
        // every zone's SkyZone actor (`ZoneInfo.SkyZone`; a zone without a ZoneInfo is the LevelInfo's), read before
        // the walk
        let mut skies: Vec<Option<SkyZone>> = vec![None; 256];
        for z in 0..self.view.model.zones.len().min(256) {
            let zi = self.vm.zone_actors.get(z).copied().filter(|&a| a != 0).unwrap_or(self.vm.level);
            if zi == 0 {
                continue;
            }
            let sky = self.vm.op(zi, "SkyZone");
            if sky != 0 && self.vm.is_live(sky) {
                skies[z] = Some(SkyZone { zone: self.vm.zone_number(sky), location: self.vm.location(sky), rotation: self.vm.rotation(sky) });
            }
        }
        // volumetric lighting: the device's `VolumetricLighting` (on in the shipped D3D settings) and the viewing
        // player in a fog zone
        let pz = self.vm.zone_of_actor(player);
        let volumetrics = pz != 0 && self.vm.bp(pz, "bFogZone");
        // (read by the walk only with volumetric lighting on)
        let vol_count = if volumetrics { self.vm.model_light_actors.len() } else { 0 };
        let vol_lights: Vec<Option<VolLight>> = (0..vol_count)
            .map(|i| {
                let a = self.vm.model_light_actors[i];
                (a != 0 && self.vm.is_live(a)).then(|| VolLight { id: a, pos: self.vm.location(a), volume_radius: self.vm.u8p(a, "VolumeRadius") })
            })
            .collect();
        // `ShinySurfaces` of the configured render device's ini section (D3D: True): translucent mirrors open a mirror
        // frame, otherwise the walk draws them solid
        let shiny_surfaces = self.render_device_flag("ShinySurfaces").unwrap_or(true);
        // the walk sees the level's model with the movers filtered in
        let tracked = if self.vis_movers { self.brush_tracker.as_ref().and_then(|(_, t, f)| Some((self.vm.world_model.clone()?, t.first_dynamic_surf, f.clone()))) } else { None };
        let first_dynamic_surf = match &tracked {
            Some((_, first, dyn_flags)) => {
                flags.truncate(*first);
                flags.extend_from_slice(dyn_flags);
                *first
            }
            None => usize::MAX,
        };
        let model = if tracked.is_none() { Some(std::mem::take(&mut self.view.model)) } else { None };
        // the stored render bounds of the shipped maps miss parts of some subtrees (rooms and doors skipped from most
        // views, in the original too): the walk tests repaired ones, made again when the movers change the model
        let bounds_key = (tracked.is_some(), self.vm.world_model_gen, self.map_pkg);
        let mut render_bounds = self.render_bounds.take().filter(|(k, _)| *k == bounds_key);
        if render_bounds.is_none() {
            if self.static_subtree_bounds.as_ref().map(|(pkg, _)| *pkg) != Some(self.map_pkg) {
                let stored = model.as_ref().unwrap_or(&self.view.model);
                self.static_subtree_bounds = Some((self.map_pkg, dx_vis::bound::static_subtree_bounds(stored)));
            }
            let m: &ue_assets::model::Model = match (&model, &tracked) {
                (Some(m), _) => m,
                (None, Some((m, _, _))) => m,
                _ => unreachable!(),
            };
            let static_sub = self.static_subtree_bounds.as_ref().map(|(_, b)| b.as_slice()).unwrap_or(&[]);
            render_bounds = Some((bounds_key, dx_vis::bound::repaired_render_bounds(m, first_dynamic_surf, static_sub)));
        }
        lap("    setup", &mut t);
        let frames = {
            let model_ref: &ue_assets::model::Model = match (&model, &tracked) {
                (Some(m), _) => m,
                (None, Some((m, _, _))) => m,
                _ => unreachable!(),
            };
            let world = World {
                model: model_ref,
                first_dynamic_surf,
                surf_flags: &|i| flags.get(i).copied().unwrap_or(0),
                sky_zone: &|z| skies[z as usize],
                player_ctrl,
                // the configured device is a hardware one (D3D): not span based; `ShinySurfaces` of its ini section
                span_based: false,
                shiny_surfaces,
                volumetrics,
                vol_light: &|i| vol_lights.get(i).copied().flatten(),
                render_bounds: render_bounds.as_ref().map(|(_, b)| b.as_slice()),
            };
            occlude(&world, f, &mut |fr| self.vis_sprites(fr))
        };
        if let Some(m) = model {
            self.view.model = m;
        }
        self.render_bounds = render_bounds;
        drop(tracked);
        let mut drawn = Vec::new();
        for fv in &frames {
            for (s, &v) in fv.sprites.iter().zip(&fv.vis.sprites) {
                if v && !drawn.contains(&s.id) {
                    drawn.push(s.id);
                }
            }
        }
        (drawn, frames)
    }

    /// `SetupDynamics`' actor filter and `FDynamicSprite::Setup`'s screen boxes for one frame.
    fn vis_sprites(&mut self, f: &Frame) -> FrameInput {
        let t0 = std::time::Instant::now();
        let r = self.vis_sprites_inner(f);
        if std::env::var_os("DX_STEP_PROFILE").is_some() {
            let e = t0.elapsed().as_secs_f64();
            crate::STEP_PROFILE.with(|p| *p.borrow_mut().entry("    sprite callback").or_insert(0.0) += e);
        }
        r
    }

    fn vis_sprites_inner(&mut self, f: &Frame) -> FrameInput {
        let player = self.player;
        let behind_view = self.vm.bp(player, "bBehindView");
        let view_target = self.vm.op(player, "ViewTarget");
        // the main view leaves out the actor it looks out of (unless in behind view); child frames show everyone
        let view_actor = if !behind_view && f.recursion == 0 { if view_target != 0 { view_target } else { player } } else { 0 };
        let mut sprites = Vec::new();
        let mut lights = Vec::new();
        let planes = f.view_planes();
        // the tests that do not depend on the frame, once per pass
        let cands = match &self.vis_cands {
            Some(c) => c.clone(),
            None => {
                let c = std::rc::Rc::new(self.vis_candidates(player, behind_view));
                self.vis_cands = Some(c.clone());
                c
            }
        };
        for &(a, kind) in cands.iter() {
            if f.recursion == 0 && !behind_view && view_target != 0 && a == view_target {
                continue;
            }
            // a dynamic light item for a light that is not static (or `bDynamicLight`) and is near the view
            // (hidden actors too: lights are hidden)
            if kind & CAND_LIGHT != 0 {
                if let Some(l) = self.vis_light(a, &planes) {
                    lights.push(l);
                }
            }
            if kind & CAND_SPRITE == 0 || (view_actor != 0 && a == view_actor) {
                continue;
            }
            // an actor with a render iterator gets one sprite per item it yields (particles: the proxy moved to each
            // particle), one of itself when it has no render interface
            if kind & CAND_ITERATOR != 0 {
                match self.vm.render_iterator_begin(a, player) {
                    Ok(Some(it)) => {
                        while !self.vm.render_iterator_done(it) {
                            match self.vm.render_iterator_item(it) {
                                Ok(item) => {
                                    let index = self.vm.ip(it, "Index") as u32;
                                    sprites.extend(self.vis_sprite_of(f, item).map(|s| Sprite { item: index, ..s }));
                                }
                                Err(e) => self.issues.push(LoadIssue { actor: a, class: "RenderIterator".into(), event: "CurrentItem".into(), error: e.to_string() }),
                            }
                            self.vm.render_iterator_next(it);
                        }
                        continue;
                    }
                    Ok(None) => {}
                    Err(e) => self.issues.push(LoadIssue { actor: a, class: "RenderIterator".into(), event: "Init".into(), error: e.to_string() }),
                }
            }
            sprites.extend(self.vis_sprite_of(f, a));
        }
        FrameInput { sprites, lights }
    }

    /// `SetupDynamics`' actor filter without what depends on the frame: per live actor (in order) whether it may be a
    /// light item (`vis_light`'s fixed tests), a sprite (high detail, hidden and owner-see rules) and has a render
    /// iterator class.
    fn vis_candidates(&mut self, player: u32, behind_view: bool) -> Vec<(u32, u8)> {
        let high_detail = self.vm.high_detail_actors;
        // all `Actor` properties: one offset (and bit) for every actor, read straight from its memory
        let vm = &mut self.vm;
        let mut off = |name: &'static str| vm.actor_prop_offset(name).unwrap_or((usize::MAX, 0));
        let (hd, lt, st, nd, dl, lb, lr) = (off("bHighDetail"), off("LightType"), off("bStatic"), off("bNoDelete"), off("bDynamicLight"), off("LightBrightness"), off("LightRadius"));
        let (hidden, owner, only, nosee, iter) = (off("bHidden"), off("Owner"), off("bOnlyOwnerSee"), off("bOwnerNoSee"), off("RenderIteratorClass"));
        let vm = &self.vm;
        let b = |a: u32, (o, m): (usize, u32)| o != usize::MAX && vm.flag(a, o, m);
        let byte = |a: u32, (o, _): (usize, u32)| if o == usize::MAX { 0 } else { vm.u8_at(a, o) };
        let word = |a: u32, (o, _): (usize, u32)| if o == usize::MAX { 0 } else { vm.u32_at(a, o) };
        let mut out = Vec::new();
        for a in vm.live_actors() {
            // high-detail actors only on a device with `HighDetailActors` (neither sprite nor light item otherwise)
            if !high_detail && b(a, hd) {
                continue;
            }
            let mut kind = 0u8;
            if byte(a, lt) != 0 {
                let fixed = b(a, st) || b(a, nd);
                if !(fixed && !b(a, dl)) && byte(a, lb) != 0 && byte(a, lr) != 0 {
                    kind |= CAND_LIGHT;
                }
            }
            let sprite = !b(a, hidden) && {
                // owner-see rules against the viewing pawn
                let mut o = a;
                let mut owned_by_viewer = false;
                let mut guard = 0;
                while o != 0 && guard < 64 {
                    if o == player {
                        owned_by_viewer = true;
                        break;
                    }
                    o = if vm.is_live(o) { word(o, owner) } else { 0 };
                    guard += 1;
                }
                !(b(a, only) && !owned_by_viewer) && !(b(a, nosee) && owned_by_viewer && !behind_view)
            };
            if sprite {
                kind |= CAND_SPRITE;
                if word(a, iter) != 0 {
                    kind |= CAND_ITERATOR;
                }
            }
            if kind != 0 {
                out.push((a, kind));
            }
        }
        out
    }

    /// `SetupDynamics`' light items: `LightType`, not static nor no-delete unless `bDynamicLight`, a brightness and a
    /// radius; inside every view plane grown by `max(WorldLightRadius, 25 * (VolumeRadius + 1))`.
    fn vis_light(&mut self, a: u32, planes: &[[f32; 4]; 4]) -> Option<DynLight> {
        let vm = &mut self.vm;
        if vm.u8p(a, "LightType") == 0 {
            return None;
        }
        let fixed = vm.bp(a, "bStatic") || vm.bp(a, "bNoDelete");
        if (fixed && !vm.bp(a, "bDynamicLight")) || vm.u8p(a, "LightBrightness") == 0 || vm.u8p(a, "LightRadius") == 0 {
            return None;
        }
        let radius = 25.0 * (vm.u8p(a, "LightRadius") as f32 + 1.0);
        let reach = radius.max((vm.u8p(a, "VolumeRadius") as f32 + 1.0) * 25.0);
        let p = vm.location(a);
        if !p.iter().all(|x| x.is_finite()) {
            return None;
        }
        if planes.iter().any(|pl| (pl[0] * p[0] + pl[1] * p[1] + pl[2] * p[2]) - pl[3] < -reach) {
            return None;
        }
        // volumetric: the light's zone is a fog zone, it has a volume, and the volume touches every view plane
        let (vr, vb) = (vm.u8p(a, "VolumeRadius"), vm.u8p(a, "VolumeBrightness"));
        let zone = vm.zone_of_actor(a);
        let lim = (vr as i32 + 1) as f32 * -25.0;
        let volumetric = zone != 0 && vm.bp(zone, "bFogZone") && vr != 0 && vb != 0 && planes.iter().all(|pl| (pl[0] * p[0] + pl[1] * p[1] + pl[2] * p[2]) - pl[3] >= lim);
        Some(DynLight { id: a, pos: p, radius, special_lit: vm.bp(a, "bSpecialLit"), volume_radius: vr, volumetric })
    }

    /// `FDynamicSprite::Setup`'s screen box and depth of actor `a` in frame `f` (`None`: no sprite).
    fn vis_sprite_of(&mut self, f: &Frame, a: u32) -> Option<Sprite> {
        let dt = self.vm.u8p(a, "DrawType");
        let loc = self.vm.location(a);
        let v = f.to_view(loc);
        if v[2] < 0.0 {
            return None;
        }
        let rect = match dt {
            DT_MESH => {
                // the mesh's world box does not change during a pass: worked out once for all its frames
                let cached = self.vis_boxes.get(&a).copied();
                let bx = match cached {
                    Some(b) => b,
                    None => {
                        let b = self.vm.mesh_draw_lite(a).and_then(|d| self.vis_mesh_box(a, &d));
                        self.vis_boxes.insert(a, b);
                        b
                    }
                };
                let (mn, mx) = bx?;
                dx_vis::bound::bound_visible(f, mn, mx, None).map(|b| (b.min_x as i32, b.min_y as i32, b.max_x as i32, b.max_y as i32))
            }
            DT_SPRITE | DT_SPRITE_ANIM_ONCE => {
                let tex = self.vm.op(a, "Texture");
                if tex == 0 {
                    return None;
                }
                let (u, vv) = self.vis_texture_size(tex);
                let scale = self.vm.fp(a, "DrawScale");
                let r = f.proj_z / v[2];
                let (sx, sy) = (v[0] * r + f.fx15, v[1] * r + f.fy15);
                let (hw, hh) = (u * scale * r * 0.5, vv * scale * r * 0.5);
                let x1 = ((sx - hw).ceil() as i32).clamp(0, f.x);
                let x2 = ((sx + hw).ceil() as i32).clamp(0, f.x);
                let y1 = ((sy - hh).ceil() as i32).clamp(0, f.y);
                let y2 = ((sy + hh).ceil() as i32).clamp(0, f.y);
                Some((x1, y1, x2, y2))
            }
            _ => None,
        };
        let Some((x1, y1, x2, y2)) = rect else { return None };
        if x2 <= x1 || y2 <= y1 {
            return None;
        }
        Some(Sprite { id: a, item: u32::MAX, x1, y1, x2, y2, z: v[2] })
    }

    /// `UMesh::GetRenderBoundingBox`: the mesh's stored boxes of the drawn frames (or of the whole mesh), offset by its
    /// origin, scaled by the mesh scale and draw scale, grown by 1, turned by `RotOrigin` and the actor's rotation and
    /// placed at `Location + PrePivot`; the world box around it.
    fn vis_mesh_box(&mut self, a: u32, d: &ue_vm::animation::MeshDraw) -> Option<([f32; 3], [f32; 3])> {
        let md = self.view.mesh_data(&d.mesh)?;
        let m = &md.lod.mesh;
        let pick = |i: usize| m.bounding_boxes.get(i).filter(|b| b.is_valid).map(|b| (b.min, b.max));
        let (mut mn, mut mx) = match (pick(d.frame_a), pick(d.frame_b)) {
            (Some(x), Some(y)) => ([x.0[0].min(y.0[0]), x.0[1].min(y.0[1]), x.0[2].min(y.0[2])], [x.1[0].max(y.1[0]), x.1[1].max(y.1[1]), x.1[2].max(y.1[2])]),
            (Some(x), None) | (None, Some(x)) => x,
            (None, None) => (m.bounding_box.min, m.bounding_box.max),
        };
        for k in 0..3 {
            let s = d.scale * m.scale[k];
            let (a0, b0) = ((mn[k] - m.origin[k]) * s, (mx[k] - m.origin[k]) * s);
            mn[k] = a0.min(b0) - 1.0;
            mx[k] = a0.max(b0) + 1.0;
        }
        let ro = dx_vk::actors::rotation_axes(m.rot_origin);
        let ra = dx_vk::actors::rotation_axes(d.rotation);
        let pp = match self.vm.get_prop(a, "PrePivot", 0) {
            Ok(Value::Vector(v)) => v,
            _ => [0.0; 3],
        };
        let base = [d.location[0] + pp[0], d.location[1] + pp[1], d.location[2] + pp[2]];
        let (mut wmn, mut wmx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for i in 0..8 {
            let c = [if i & 1 != 0 { mx[0] } else { mn[0] }, if i & 2 != 0 { mx[1] } else { mn[1] }, if i & 4 != 0 { mx[2] } else { mn[2] }];
            let t: [f32; 3] = std::array::from_fn(|k| c[0] * ro[0][k] + c[1] * ro[1][k] + c[2] * ro[2][k]);
            let w: [f32; 3] = std::array::from_fn(|k| base[k] + t[0] * ra[0][k] + t[1] * ra[1][k] + t[2] * ra[2][k]);
            for k in 0..3 {
                wmn[k] = wmn[k].min(w[k]);
                wmx[k] = wmx[k].max(w[k]);
            }
        }
        Some((wmn, wmx))
    }

    /// A texture's `USize`/`VSize` (textures are resources, not VM instances: read from the package).
    fn vis_texture_size(&mut self, tex: u32) -> (f32, f32) {
        self.vm.texture_size(tex)
    }
}

/// The movers drawn through the main frame's lists (`Session::listed_movers`).
#[derive(Default)]
pub struct ListedMovers {
    /// Per brush export: the fragments drawn, in the walk's order.
    pub fragments: std::collections::HashMap<usize, Vec<dx_vk::world::Fragment>>,
    /// Per dynamic node drawn: brush export and index into its fragments.
    pub nodes: std::collections::HashMap<u32, (usize, usize)>,
    /// The movers that own them, with their brush export.
    pub owners: Vec<(u32, usize)>,
    /// The dynamic nodes the main frame drew (the others show only in its child frames).
    pub main: Vec<u32>,
}

//! Loading a map's saved actors into the VM.

use crate::{inst_ref, ref_instance, Mem, Object, Value, Vm, VmError};
use std::collections::{HashMap, HashSet};
use ue_package::ObjectRef;
use ue_package::object::PropertyKind;
use ue_registry::{ClassId, PropDef, StructId, Target};

/// One script error met while bringing a map to life (loading continues past them).
#[derive(Debug, Clone)]
pub struct LoadIssue {
    pub actor: u32,
    pub class: String,
    pub event: String,
    pub error: String,
}

/// What `Vm::load_level` created.
#[derive(Debug, Default)]
pub struct LevelLoad {
    /// VM references of the map's actors in level order (empty slots of the original list are skipped).
    pub actors: Vec<u32>,
    /// Properties that could not be applied while instantiating saved actors.
    pub property_warnings: Vec<String>,
    /// Script errors raised by the start-up events.
    pub issues: Vec<LoadIssue>,
}

impl Vm {
    /// Drops every object and resets the world, keeping the registry and the compiled-code cache (for loading another map).
    pub fn clear_world(&mut self) {
        self.objects.clear();
        self.actor_slots.clear();
        self.actors_sorted = false;
        self.level_brush = 0;
        self.level = 0;
        self.time = 0.0;
        self.ai.reset();
        self.log.clear();
        self.bsp = None;
        self.zone_actors.clear();
        self.actor_light_cache.clear();
        self.reach_specs.clear();
        self.surf_info.clear();
        self.drop_collision_hash();
        self.decals.clear();
        self.surf_decals.clear();
        self.con_objects.clear();
        self.con_paths.clear();
        self.con_labels.clear();
        self.level_paths.clear();
    }

    /// Instantiates every saved actor of the map loaded as package `pi` of the registry (see `Registry::add_package`),
    /// links references between them, makes the map's `LevelInfo` the world's level, and runs the start-up events.
    ///
    /// Start-up as `UGameEngine::LoadMap` (see `begin_play`). Errors raised by scripts are recorded in the returned
    /// report instead of aborting the load.
    pub fn load_level(&mut self, pi: usize) -> Result<LevelLoad, VmError> {
        self.load_level_opts(pi, true)
    }

    /// `load_level`; without `run_events` the actors are instantiated and linked but nothing runs (a saved game then fills them in).
    pub fn load_level_opts(&mut self, pi: usize, run_events: bool) -> Result<LevelLoad, VmError> {
        self.load_level_with(pi, run_events, false)
    }

    /// `load_level_opts`; `keep_players` keeps the `PlayerPawn` actors of the package (a saved game's player) instead of
    /// dropping them as editor cameras.
    pub fn load_level_with(&mut self, pi: usize, run_events: bool, keep_players: bool) -> Result<LevelLoad, VmError> {
        let rt = |e: String| VmError::Runtime(e);
        let (actor_refs, bsp, surfs, zone_refs, light_refs, specs, level_time, level_path) = {
            let pkg = &self.reg.packages[pi].pkg;
            let export = (0..pkg.exports.len()).find(|&i| pkg.export_class_name(i) == "Level").ok_or_else(|| rt("map has no Level object".into()))?;
            let l = ue_assets::level::Level::load(pkg, export).map_err(|e| rt(e.to_string()))?;
            let level_path = format!("{}.{}", self.reg.packages[pi].name, pkg.object_path(ObjectRef::Export(export)).unwrap_or_default());
            let ObjectRef::Export(me) = ObjectRef::from_index(l.model) else { return Err(rt("level model is not an export".into())) };
            let model = ue_assets::model::Model::load(pkg, me).map_err(|e| rt(e.to_string()))?;
            let surfs: Vec<(Option<String>, u32)> = model.surfs.iter().map(|s| (self.reg.ref_path(pi, s.texture), s.poly_flags)).collect();
            let zone_refs: Vec<i32> = model.zones.iter().map(|z| z.zone_actor).collect();
            let bsp = ue_bsp::Bsp::from_model(&model);
            let lights = model.lights.clone();
            self.world_model = Some(std::rc::Rc::new(model));
            self.world_model_gen += 1;
            (l.actors, bsp, surfs, zone_refs, lights, l.reach_specs, l.time_seconds, level_path)
        };
        // `ULevel`: every actor's `XLevel` (`ULevel::PostLoad`, `SpawnActor`)
        self.xlevel = self.reg.obj_id(&level_path).0;
        // the clock stored with the map (`ULevel::TimeSeconds`, the editor session's); `begin_play` sets it to 0 for a
        // level that has not begun play, a saved game sets its own
        self.time = level_time;
        self.bsp = Some(bsp);
        self.zone_render_times = vec![0.0; 64];
        self.blend_cache.clear();
        // texture name and group (the texture's outer) of every surface, for `TraceTexture`
        self.surf_info = surfs
            .into_iter()
            .map(|(path, flags)| {
                let parts: Vec<&str> = path.as_deref().map(|p| p.split('.').collect()).unwrap_or_default();
                let name = parts.last().map(|n| self.reg.names.intern(n)).unwrap_or(0);
                let group = if parts.len() >= 3 { self.reg.names.intern(parts[parts.len() - 2]) } else { 0 };
                crate::collision::SurfInfo { texture_name: name, texture_group: group, flags, texture_path: path.as_deref().map(std::rc::Rc::from) }
            })
            .collect();
        let mut load = LevelLoad::default();
        self.level_paths.clear();
        // a new map package: its names are the map's own
        self.used_names[0].clear();

        // 1. instantiate
        let mut by_path: HashMap<u32, u32> = HashMap::new();
        for &a in actor_refs.iter().filter(|&&a| a != 0) {
            let ObjectRef::Export(ei) = ObjectRef::from_index(a) else { continue };
            // the game's linker loads only exports flagged `RF_LoadForClient` (`ULinkerLoad::CreateExport` on a
            // client): the editor-only brushes (the builder brush, CSG brushes saved `RF_NotForClient |
            // RF_NotForServer`) are null slots of the actor list in the original
            const RF_LOAD_FOR_CLIENT: u32 = 0x0001_0000;
            if self.reg.packages[pi].pkg.exports[ei].object_flags & RF_LOAD_FOR_CLIENT == 0 {
                continue;
            }
            // an actor whose class is not in the scripts (saved games of mods) is left out
            let Some(class) = self.reg.class_of_export(pi, ei) else {
                if load.actors.is_empty() {
                    return Err(rt(format!("actor class of export {ei} not found")));
                }
                load.issues.push(LoadIssue { actor: 0, class: format!("export {ei}"), event: "Load".into(), error: "actor class not found".into() });
                continue;
            };
            let (inst, warnings) = self.instantiate_export(class, pi, ei).map_err(|e| rt(e.to_string()))?;
            load.property_warnings.extend(warnings);
            let r = self.adopt(class, inst.data, inst.strings);
            let path = self.reg.ref_path(pi, a).ok_or_else(|| rt("actor without a path".into()))?;
            // the actor's name as the map saved it
            let name = self.reg.names.intern(path.rsplit('.').next().unwrap_or(&path));
            self.used_names[0].insert(name);
            self.set_object_name(r, name);
            by_path.insert(self.reg.obj_id(&path).0, r);
            self.level_paths.insert(r, path);
            load.actors.push(r);
        }

        // the list stays in creation order until the start-up is over; the map's second actor is its default brush
        self.actors_sorted = false;
        self.level_brush = load.actors.get(1).copied().unwrap_or(0);

        // where actors made from now on start (the level's list goes on with them: the game rules object first)
        let first_new = self.objects.len();

        // 2. link object references between the saved actors
        for &r in &load.actors {
            self.rewrite_refs(r, &by_path);
        }

        self.zone_actors = zone_refs
            .iter()
            .map(|&r| self.reg.ref_path(pi, r).and_then(|p| by_path.get(&self.reg.obj_id(&p).0).copied()).unwrap_or(0))
            .collect();
        self.model_light_actors = light_refs
            .iter()
            .map(|&r| if r == 0 { 0 } else { self.reg.ref_path(pi, r).and_then(|p| by_path.get(&self.reg.obj_id(&p).0).copied()).unwrap_or(0) })
            .collect();

        // path network links between navigation points (references resolved to the instantiated actors)
        let mut rts = Vec::with_capacity(specs.len());
        for s in &specs {
            let mut to_actor = |r: i32| self.reg.ref_path(pi, r).and_then(|p| by_path.get(&self.reg.obj_id(&p).0).copied()).unwrap_or(0);
            let (start, end) = (to_actor(s.start), to_actor(s.end));
            rts.push(crate::navigation::ReachSpecRt { distance: s.distance, start, end, radius: s.collision_radius, height: s.collision_height, flags: s.reach_flags, pruned: s.pruned });
        }
        self.reach_specs = rts;

        // 3. the map's LevelInfo is the world's level. A saved game keeps the running game rules object (the port's
        // snapshot does not carry it); a fresh level gets its own below (`spawn_level_game`), and the previous level's
        // rules object and what it owns end with that level (the world made by `init_world` included)
        let lvl = *load.actors.first().ok_or_else(|| rt("map has no actors".into()))?;
        let old_level = self.level;
        let game = if old_level != 0 { self.get_prop(old_level, "Game", 0)? } else { Value::Obj(0) };
        let old_game_objs = if run_events { self.game_closure() } else { Vec::new() };
        self.level = lvl;
        if run_events {
            for g in old_game_objs.into_iter().chain([old_level]) {
                if g != 0 && g != lvl {
                    self.set_alive_quiet(g);
                }
            }
        } else {
            self.set_prop(lvl, "Game", 0, game.clone())?;
            if let Value::Obj(g) = game {
                if g != 0 {
                    self.set_prop(g, "Level", 0, Value::Obj(lvl))?;
                }
            }
        }
        for &r in &load.actors {
            self.set_prop(r, "XLevel", 0, Value::Obj(self.xlevel))?;
            self.set_prop(r, "Level", 0, Value::Obj(lvl))?;
            self.set_prop(r, "Role", 0, Value::Byte(4))?;
        }

        // the saved list links are rebuilt from scratch (stale links would form a cycle)
        self.set_u32_prop(self.level, "NavigationPointList", 0);
        for &r in &load.actors {
            self.set_u32_prop(r, "nextNavigationPoint", 0);
        }
        for &r in &load.actors {
            self.register_nav_point(r);
        }
        // `SetActorCollision(1)` in `LoadMap` (1038d1fb): the hash is built from the level's actor list before start-up
        self.rebuild_actor_slots();
        self.rebuild_collision_set();
        // a fresh map's actors keep their saved regions until the zone pass after `BeginPlay`; a saved game's are
        // refreshed here (the port's snapshot does not carry them)
        for &r in load.actors.iter().filter(|_| !run_events) {
            if let Err(e) = self.set_actor_zone(r, true, true) {
                let class = self.class_of(r).map(|c| self.reg.class(c).path.clone()).unwrap_or_default();
                load.issues.push(LoadIssue { actor: r, class, event: "SetActorZone".into(), error: e.to_string() });
            }
        }

        // maps carry the editor's viewport cameras (`Camera` actors, PlayerPawns without a viewport) and their
        // replication infos; they are not gameplay actors, and `GameInfo.Login` would adopt one as "an existing
        // unoccupied player". They are dropped silently: `Destroyed` would tell every pawn that a player was killed.
        // (Inferred, UNVERIFIED against the original loader.)
        if let Some(pp) = self.reg.class_by_name("PlayerPawn").filter(|_| !keep_players) {
            for &r in &load.actors {
                if !self.class_of(r).map(|c| self.is_a(c, pp)).unwrap_or(false) {
                    continue;
                }
                let pri = self.get_prop(r, "PlayerReplicationInfo", 0).ok();
                for dead in [Some(Value::Obj(r)), pri].into_iter().flatten() {
                    if let Value::Obj(d) = dead {
                        if let Some(o) = ref_instance(d).and_then(|i| self.objects.get_mut(i)) {
                            o.alive = false;
                        }
                    }
                }
            }
        }

        // 4. the level's game rules object (fresh levels), then the detail mode from the render device
        // (`UGameEngine::LoadMap`, for fresh maps and saved games alike)
        if run_events {
            if let Err(e) = self.spawn_level_game() {
                load.issues.push(LoadIssue { actor: lvl, class: "GameInfo".into(), event: "Spawn".into(), error: e.to_string() });
            }
        }
        self.high_detail_actors = self.device_high_detail_actors();
        let hd = self.high_detail_actors;
        self.set_bp(lvl, "bHighDetailMode", hd);

        // 5. start-up events
        if !run_events {
            return Ok(load);
        }
        self.begin_play(&mut load, first_new);
        let issues = self.finish_level_start(false);
        load.issues.extend(issues);
        Ok(load)
    }

    /// `LoadMap`'s game rules object for a level without one: the class named by the URL's `Game=` option, else the
    /// level's `DefaultGameType`, else `[Engine.Engine] DefaultGame`; spawned like any actor (its spawn events run now,
    /// the start-up passes give it theirs again) and made `Level.Game`.
    fn spawn_level_game(&mut self) -> Result<(), VmError> {
        let lvl = self.level;
        if self.op(lvl, "Game") != 0 {
            return Ok(());
        }
        let url_game = self.url_options.iter().find(|(k, _)| k.eq_ignore_ascii_case("Game")).map(|(_, v)| v.clone());
        let mut class = url_game.and_then(|p| self.reg.class_by_path(&p));
        if class.is_none() {
            let t = self.op(lvl, "DefaultGameType");
            class = if t != 0 { self.class_of_ref(t) } else { None };
        }
        if class.is_none() {
            class = self.reg.config_value("System", "Engine.Engine", "DefaultGame").map(|c| ue_config::unquote(&c)).and_then(|p| self.reg.class_by_path(&p));
        }
        let class = class.or_else(|| self.reg.class_by_name("DeusExGameInfo")).ok_or_else(|| VmError::Runtime("no game class".into()))?;
        if let Some(g) = self.spawn_actor(class, 0, 0, Some([0.0; 3]), Some([0; 3]), false)? {
            self.set_prop(lvl, "Game", 0, Value::Obj(g))?;
        }
        Ok(())
    }

    /// The configured render device's `HighDetailActors`: `[Engine.Engine] GameRenderDevice` names the device's ini
    /// section (the device class's config). Without a configuration: the shipped D3D setting (on).
    fn device_high_detail_actors(&mut self) -> bool {
        self.render_device_flag("HighDetailActors")
    }

    /// A boolean setting of the configured render device (`[<GameRenderDevice>] key`, e.g. `HighDetailActors`,
    /// `Coronas`). Without a configuration: the shipped D3D settings (on).
    pub fn render_device_flag(&mut self, key: &str) -> bool {
        if self.reg.config.is_none() {
            return true;
        }
        let Some(dev) = self.reg.config_value("System", "Engine.Engine", "GameRenderDevice") else { return false };
        // `UBoolProperty::ImportText`: "1" or "True" (any case) is true; the D3D device never sets these itself
        self.reg
            .config_value("System", dev.trim(), key)
            .map(|v| v.trim() == "1" || v.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    /// `UGameEngine::LoadMap`'s start-up of a level that has not begun play (1038d7ea..1038dcd5): clocks to 0, touching
    /// lists cleared, `bBegunPlay` and `bStartup` set, then for every actor in turn `PreBeginPlay`, `BeginPlay`, a forced
    /// zone test (`SetActorZone(A, 1, 1)`), `LastRenderTime = -10`, `PostBeginPlay`, `SetInitialState`; bases found,
    /// `bStartup` cleared, and `PostPostBeginPlay` (Deus Ex) for every actor.
    ///
    /// Each pass walks the level's actor list as it is at that moment: actors spawned meanwhile are appended and get the
    /// pass's event too (on top of the full chain `SpawnActor` gave them, as in the engine); destroyed ones are skipped.
    /// The list goes on after the map's actors with everything made since `first_new` (the level's game rules object,
    /// spawned just before, and what its spawn events made). `InitGame` runs once `bBegunPlay` is set.
    fn begin_play(&mut self, load: &mut LevelLoad, first_new: usize) {
        let lvl = self.level;
        let mut order = load.actors.clone();
        let mut scanned = first_new;
        if self.bp(lvl, "bBegunPlay") {
            // a level saved in play: the clock goes on from its `TimeSeconds`
            self.time = self.fp(lvl, "TimeSeconds");
        } else {
            self.time = 0.0;
            self.set_fp(lvl, "TimeSeconds", 0.0);
            // `Touching[0..4]` (Actor+0xe8) of every actor
            let touching = self.reg.class_by_name("Actor").and_then(|c| self.reg.find_prop(c, "Touching")).map(|p| (p.offset as usize, p.dim.max(1) as usize));
            if let Some((off, n)) = touching {
                for &r in &order {
                    for k in 0..n {
                        self.set_u32_at(r, off + 4 * k, 0);
                    }
                }
            }
            self.set_bp(lvl, "bBegunPlay", true);
            self.set_bp(lvl, "bStartup", true);
            // `Info->Game->eventInitGame(Options, Error)`
            let game = self.op(lvl, "Game");
            if game != 0 && self.is_live(game) {
                let options = self.url_options_string();
                if let Err(e) = self.send_event(game, "InitGame", vec![Value::Str(options), Value::Str(String::new())]) {
                    load.issues.push(LoadIssue { actor: game, class: "GameInfo".into(), event: "InitGame".into(), error: e.to_string() });
                }
            }
            for ev in ["PreBeginPlay", "BeginPlay"] {
                self.startup_pass(&mut order, &mut scanned, load, ev, |vm, r| vm.send_event(r, ev, vec![]).map(|_| ()));
            }
            self.startup_pass(&mut order, &mut scanned, load, "SetActorZone", |vm, r| vm.set_actor_zone(r, true, true));
            // every actor counts as not drawn for 10 seconds (`LastRendered()` reads 10 at the start)
            self.startup_pass(&mut order, &mut scanned, load, "LastRenderTime", |vm, r| {
                vm.set_fp(r, "LastRenderTime", -10.0);
                Ok(())
            });
            for ev in ["PostBeginPlay", "SetInitialState"] {
                self.startup_pass(&mut order, &mut scanned, load, ev, |vm, r| vm.send_event(r, ev, vec![]).map(|_| ()));
            }
            self.startup_pass(&mut order, &mut scanned, load, "FindBase", |vm, r| vm.startup_base(r, lvl));
            self.set_bp(lvl, "bStartup", false);
        }
        self.startup_pass(&mut order, &mut scanned, load, "PostPostBeginPlay", |vm, r| vm.send_event(r, "PostPostBeginPlay", vec![]).map(|_| ()));
    }

    /// One start-up pass over the level's actors (`for i < Actors.Num()`, re-read every step): actors created since the
    /// last look are appended to `order` first.
    pub(crate) fn startup_pass(&mut self, order: &mut Vec<u32>, scanned: &mut usize, load: &mut LevelLoad, what: &str, mut f: impl FnMut(&mut Vm, u32) -> Result<(), VmError>) {
        let mut i = 0;
        loop {
            while *scanned < self.objects.len() {
                if self.objects[*scanned].actor && self.objects[*scanned].alive {
                    order.push(inst_ref(*scanned));
                }
                *scanned += 1;
            }
            let Some(&r) = order.get(i) else { break };
            i += 1;
            if !self.is_live(r) {
                continue;
            }
            if let Err(e) = f(self, r) {
                let class = self.class_of(r).map(|c| self.reg.class(c).path.clone()).unwrap_or_default();
                load.issues.push(LoadIssue { actor: r, class, event: what.to_string(), error: e.to_string() });
            }
        }
    }

    /// `LoadMap`'s base pass for one actor: with an `AttachTag`, based (silently) on the first actor of the level whose
    /// `Tag` matches; otherwise an unbased decoration, inventory item or pawn that collides with the world and has
    /// physics none or rotating looks for a base below (`FindBase`), and a base on the level itself is dropped.
    fn startup_base(&mut self, a: u32, lvl: u32) -> Result<(), VmError> {
        let o = self.offsets();
        // the engine's own members (Actor+0xd4, +0xb0): a subclass may declare a script variable of the same name
        // (`ParticleGenerator.attachTag`), which the engine never reads
        let (attach, tag) = {
            let cls = self.reg.class_by_name("Actor").ok_or_else(|| VmError::Runtime("no Actor class".into()))?;
            let off = |vm: &Vm, n: &str| vm.reg.find_prop(cls, n).map(|p| p.offset as usize);
            (off(self, "AttachTag"), off(self, "Tag"))
        };
        let (Some(attach_off), Some(tag_off)) = (attach, tag) else { return Ok(()) };
        let attach = self.u32_at(a, attach_off);
        if attach != 0 {
            let all: Vec<u32> = self.actor_slots.iter().filter(|&&i| self.objects[i].alive).map(|&i| inst_ref(i)).collect();
            if let Some(&target) = all.iter().find(|&&t| self.u32_at(t, tag_off) == attach) {
                self.set_base(a, target, false)?;
            }
            return Ok(());
        }
        if self.u32_at(a, o.base) != 0 || !self.bp(a, "bCollideWorld") {
            return Ok(());
        }
        let kinds: Vec<ClassId> = ["Decoration", "Inventory", "Pawn"].iter().filter_map(|n| self.reg.class_by_name(n)).collect();
        if !self.class_of(a).map(|c| kinds.iter().any(|&k| self.is_a(c, k))).unwrap_or(false) {
            return Ok(());
        }
        let physics = self.u8_at(a, o.physics);
        if physics != 0 && physics != 5 {
            return Ok(());
        }
        self.find_base(a)?;
        if self.u32_at(a, o.base) == lvl {
            self.set_base(a, 0, false)?;
        }
        Ok(())
    }

    /// Adds an object built from a saved instance and returns its reference.
    pub(crate) fn adopt(&mut self, class: ClassId, data: Vec<u8>, strings: Vec<String>) -> u32 {
        let mut mem = Mem { data, strings };
        if let Some(p) = self.reg.find_prop(class, "Class").cloned() {
            let path = self.reg.class(class).path.clone();
            let id = self.reg.obj_id(&path).0;
            if let Some(slot) = mem.data.get_mut(p.offset as usize..p.offset as usize + 4) {
                slot.copy_from_slice(&id.to_le_bytes());
            }
        }
        let actor = self.is_actor_class(class);
        if actor {
            self.actor_slots.push(self.objects.len());
        }
        self.objects.push(Object { class, mem, state: None, exec: None, latent: None, disabled: HashSet::new(), alive: true, goto_serial: 0, actor });
        inst_ref(self.objects.len() - 1)
    }

    /// Replaces object-reference values that designate saved actors (interned path ids) with instance references.
    pub(crate) fn rewrite_refs(&mut self, obj: u32, by_path: &HashMap<u32, u32>) {
        let Some(i) = ref_instance(obj) else { return };
        let class = self.objects[i].class;
        let props: Vec<PropDef> = self.reg.all_props(class).into_iter().cloned().collect();
        for p in &props {
            self.rewrite_prop(i, p, p.offset as usize, by_path);
        }
    }

    /// The raw object-reference values of an instance (interned path ids before linking), including struct members.
    pub(crate) fn object_ref_values(&mut self, obj: u32) -> Vec<u32> {
        let Some(i) = ref_instance(obj) else { return Vec::new() };
        let class = self.objects[i].class;
        let props: Vec<PropDef> = self.reg.all_props(class).into_iter().cloned().collect();
        let mut out = Vec::new();
        for p in &props {
            self.collect_prop_refs(i, p, p.offset as usize, &mut out);
        }
        out
    }

    fn collect_prop_refs(&self, obj: usize, p: &PropDef, base: usize, out: &mut Vec<u32>) {
        for e in 0..p.dim.max(1) as usize {
            let off = base + e * p.elem_size as usize;
            match (&p.kind, &p.target) {
                (PropertyKind::Object, _) => {
                    if let Some(slot) = self.objects[obj].mem.data.get(off..off + 4) {
                        let v = u32::from_le_bytes(slot.try_into().unwrap());
                        if v != 0 {
                            out.push(v);
                        }
                    }
                }
                (PropertyKind::Struct, Target::Struct(sid)) => {
                    for m in self.struct_members(*sid) {
                        self.collect_prop_refs(obj, &m, off + m.offset as usize, out);
                    }
                }
                _ => {}
            }
        }
    }

    fn rewrite_prop(&mut self, obj: usize, p: &PropDef, base: usize, by_path: &HashMap<u32, u32>) {
        for e in 0..p.dim.max(1) as usize {
            let off = base + e * p.elem_size as usize;
            match (&p.kind, &p.target) {
                (PropertyKind::Object, _) => {
                    let data = &mut self.objects[obj].mem.data;
                    let Some(slot) = data.get_mut(off..off + 4) else { continue };
                    let v = u32::from_le_bytes(slot.try_into().unwrap());
                    if let Some(&r) = by_path.get(&v) {
                        slot.copy_from_slice(&r.to_le_bytes());
                    }
                }
                (PropertyKind::Struct, Target::Struct(sid)) => {
                    let members = self.struct_members(*sid);
                    for m in &members {
                        self.rewrite_prop(obj, m, off + m.offset as usize, by_path);
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn struct_members(&self, sid: StructId) -> Vec<PropDef> {
        let mut chain = vec![sid];
        while let Some(s) = self.reg.struct_def(*chain.last().unwrap()).super_id {
            chain.push(s);
        }
        chain.iter().flat_map(|s| self.reg.struct_def(*s).props.clone()).collect()
    }
}

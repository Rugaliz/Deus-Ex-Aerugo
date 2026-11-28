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
        self.decal_pieces_cache.clear();
        self.surf_decals.clear();
        self.pending_deleted.clear();
        self.pending_deleted_set.clear();
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
            let mut model = ue_assets::model::Model::load(pkg, me).map_err(|e| rt(e.to_string()))?;
            // Aerugo repair: walls the shipped map left without a face get one (`ue_bsp::fill`; as the renderer's copy)
            let _ = ue_bsp::fill::fill_level(&mut model, pkg);
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
            // Aerugo repair of the map's data, before the pawns hand out their inventory (`PostBeginPlay`)
            self.repair_map_actors();
            let repaired = self.repair_initial_inventories(&order);
            if !repaired.is_empty() {
                self.log.push(format!("Aerugo repair: {} pawns get their class's own gun back ({})", repaired.len(), repaired.join(", ")));
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

impl Vm {
    /// Aerugo repairs of the shipped (GOTY) maps' actors, at a level's start and after a saved game is restored (a save
    /// keeps the actors' own settings). Movers that lost their movement: Maggie Chow's flat (06_HongKong_WanChai_Street): the
    /// display case with the Buddha statue (`Dispalycase`) is the secret way between her rooms and the room behind; a
    /// light switch and a trigger before it fire it, but both its keyframes are at rest, so it never moved, for JC or
    /// for the people walking through (the GOTY maps are older than the retail release's). Its leftover copy in the same
    /// map (`old_Dispalycase`, the same settings) sinks 140 units: the case gets that keyframe.
    ///
    /// Also pawns the story needs alive that the map left mortal (`INVINCIBLE_REPAIRS`: map, class): Bob Page in the MJ12
    /// lab under VersaLife (06_HongKong_MJ12lab) could be killed there, though he goes on to speak to JC at Vandenberg,
    /// the Ocean Lab and Area 51; he is made invincible, as the maps make Paul in the 'Ton.
    pub fn repair_map_actors(&mut self) {
        const MOVER_REPAIRS: &[(&str, &str, u32, [f32; 3])] = &[("06_HongKong_WanChai_Street", "Dispalycase", 1, [0.0, 0.0, -140.0])];
        const INVINCIBLE_REPAIRS: &[(&str, &str)] = &[("06_HongKong_MJ12lab", "BobPage")];
        // Levels the GOTY maps left without music (`LevelInfo.Song` empty), with the song the game ships for them:
        // the last Hell's Kitchen visit (08_NYC_Street; `NYCStreets2_Music`, a track no map refers to, was made for it)
        const MUSIC_REPAIRS: &[(&str, &str)] = &[("08_NYC_Street", "NYCStreets2_Music.NYCStreets2_Music")];
        // Datacubes the GOTY maps give the wrong picture (map, the picture's class, the right one): the Paris metro
        // guard shack's gives the catacombs picture (already Chad's) in place of the metro map, which no map gives
        const IMAGE_REPAIRS: &[(&str, &str, &str)] = &[("10_Paris_Metro", "DeusEx.Image10_Paris_Catacombs", "DeusEx.Image10_Paris_Metro")];
        let map = self.reg.obj_path(ue_registry::ObjId(self.xlevel)).split('.').next().unwrap_or("").to_string();
        for (m, from, to) in IMAGE_REPAIRS {
            let Some(dev) = self.reg.class_by_name("InformationDevices").filter(|_| m.eq_ignore_ascii_case(&map)) else { continue };
            let (from_id, to_id) = (self.reg.obj_id(from).0, self.reg.obj_id(to).0);
            for a in self.live_actors() {
                if self.class_of(a).is_some_and(|c| self.is_a(c, dev)) && self.get_prop(a, "imageClass", 0).ok() == Some(Value::Obj(from_id)) {
                    let _ = self.set_prop(a, "imageClass", 0, Value::Obj(to_id));
                    let line = format!("Aerugo repair: {} gives {to}", self.name_of(a));
                    self.log.push(line);
                }
            }
        }
        for (m, song) in MUSIC_REPAIRS {
            let level = self.level;
            if m.eq_ignore_ascii_case(&map) && level != 0 && self.op(level, "Song") == 0 {
                let id = self.reg.obj_id(song).0;
                if id != 0 && self.set_prop(level, "Song", 0, Value::Obj(id)).is_ok() {
                    self.log.push(format!("Aerugo repair: the level plays {song}"));
                }
            }
        }
        for (m, class) in INVINCIBLE_REPAIRS {
            let Some(c) = self.reg.class_by_name(class).filter(|_| m.eq_ignore_ascii_case(&map)) else { continue };
            for a in self.live_actors() {
                if self.class_of(a) == Some(c) && self.get_prop(a, "bInvincible", 0).ok() == Some(Value::Bool(false)) {
                    let _ = self.set_prop(a, "bInvincible", 0, Value::Bool(true));
                    let line = format!("Aerugo repair: {} cannot be killed here", self.name_of(a));
                    self.log.push(line);
                }
            }
        }
        let Some(mover) = self.reg.class_by_name("Mover") else { return };
        for (m, tag, key, pos) in MOVER_REPAIRS {
            if !m.eq_ignore_ascii_case(&map) {
                continue;
            }
            let tag = self.reg.names.intern(tag);
            for a in self.live_actors() {
                if !self.class_of(a).is_some_and(|c| self.is_a(c, mover)) || self.get_prop(a, "Tag", 0).ok() != Some(Value::Name(tag)) {
                    continue;
                }
                if self.get_prop(a, "KeyPos", *key).ok() == Some(Value::Vector([0.0; 3])) {
                    let _ = self.set_prop(a, "KeyPos", *key, Value::Vector(*pos));
                    let line = format!("Aerugo repair: {} moves again", self.name_of(a));
                    self.log.push(line);
                }
            }
        }
    }

    /// Aerugo repair of the shipped maps' slips in what pawns carry (`INVENTORY_REPAIRS`: map and pawn class): a pawn
    /// the map gave a gun whose ammunition it does not carry, together with ammunition no gun of its takes, while its
    /// class's own gun takes that ammunition, gets its class's gun back in place of the other. Battery Park's UNATCO
    /// troopers: a pistol and boxes of rifle ammunition, so after the pistol's six rounds the knife (the original does
    /// that: probe `ArmsGame`, WORKLOG 3.245); the class's assault rifle uses that ammunition. Only listed cases: the same
    /// pattern elsewhere can be deliberate (mission 4's military robot carries LAWs in place of its rockets and fights
    /// with its machine gun). Returns the pawns changed, by name.
    pub fn repair_initial_inventories(&mut self, actors: &[u32]) -> Vec<String> {
        const INVENTORY_REPAIRS: &[(&str, &str)] = &[("02_NYC_BatteryPark", "UNATCOTroop")];
        let map = self.reg.obj_path(ue_registry::ObjId(self.xlevel)).split('.').next().unwrap_or("").to_string();
        let classes: Vec<&str> = INVENTORY_REPAIRS.iter().filter(|(m, _)| m.eq_ignore_ascii_case(&map)).map(|(_, c)| *c).collect();
        if classes.is_empty() {
            return Vec::new();
        }
        let (Some(pawn_c), Some(weapon_c), Some(ammo_c)) = (self.reg.class_by_name("ScriptedPawn"), self.reg.class_by_name("Weapon"), self.reg.class_by_name("Ammo")) else { return Vec::new() };
        let class_of_id = |vm: &Vm, id: u32| if id == 0 { None } else { vm.reg.class_by_path(vm.reg.obj_path(ue_registry::ObjId(id))) };
        let mut changed = Vec::new();
        for &a in actors {
            let Some(c) = self.class_of(a) else { continue };
            if !self.is_a(c, pawn_c) || !classes.iter().any(|n| self.reg.names.str(self.reg.class(c).name).eq_ignore_ascii_case(n)) {
                continue;
            }
            let dim = self.reg.find_prop(c, "InitialInventory").map(|p| p.dim).unwrap_or(0);
            let slot = |vm: &mut Vm, k: u32| match vm.get_prop(a, "InitialInventory", k) {
                Ok(Value::Struct(sid, b)) if b.len() >= 8 => {
                    let (id, count) = (u32::from_le_bytes(b[0..4].try_into().unwrap()), i32::from_le_bytes(b[4..8].try_into().unwrap()));
                    Some((sid, b, id, count))
                }
                _ => None,
            };
            let ammo_of = |vm: &mut Vm, weapon: u32| -> u32 {
                let Some(wc) = class_of_id(vm, weapon) else { return 0 };
                let name = vm.reg.names.str(vm.reg.class(wc).name).to_string();
                match vm.default_value_of(&name, "AmmoName", 0) { Some(Value::Obj(o)) => o, _ => 0 }
            };
            let mut weapons = Vec::new();
            let mut ammo = Vec::new();
            for k in 0..dim {
                let Some((_, _, id, count)) = slot(self, k) else { continue };
                let Some(ic) = class_of_id(self, id) else { continue };
                if count <= 0 {
                    continue;
                }
                if self.is_a(ic, weapon_c) {
                    weapons.push((k, id));
                } else if self.is_a(ic, ammo_c) {
                    ammo.push(id);
                }
            }
            let used: Vec<u32> = weapons.iter().map(|&(_, w)| ammo_of(self, w)).collect();
            let stray: Vec<u32> = ammo.iter().copied().filter(|x| !used.contains(x)).collect();
            if stray.is_empty() {
                continue;
            }
            // the class's own guns (its default inventory)
            let cname = self.reg.names.str(self.reg.class(c).name).to_string();
            let defaults: Vec<u32> = (0..dim)
                .filter_map(|k| match self.default_value_of(&cname, "InitialInventory", k) {
                    Some(Value::Struct(_, b)) if b.len() >= 8 => Some(u32::from_le_bytes(b[0..4].try_into().unwrap())),
                    _ => None,
                })
                .collect();
            let own: Vec<u32> = defaults.into_iter().filter(|&id| class_of_id(self, id).is_some_and(|ic| self.is_a(ic, weapon_c))).collect();
            for (i, &(k, w)) in weapons.iter().enumerate() {
                let fits = ammo.contains(&used[i]) || used[i] == 0;
                if fits {
                    continue;
                }
                let none = self.reg.known_obj("Engine.AmmoNone").map(|o| o.0).unwrap_or(0);
                if used[i] == none {
                    continue;
                }
                let Some(&gun) = own.iter().find(|&&g| stray.contains(&ammo_of(self, g))) else { continue };
                let Some((sid, mut b, _, _)) = slot(self, k) else { continue };
                b[0..4].copy_from_slice(&gun.to_le_bytes());
                let _ = self.set_prop(a, "InitialInventory", k, Value::Struct(sid, b));
                let name = self.name_of(a);
                changed.push(format!("{name}: {} -> {}", self.reg.obj_path(ue_registry::ObjId(w)), self.reg.obj_path(ue_registry::ObjId(gun))));
                break;
            }
        }
        changed
    }
}

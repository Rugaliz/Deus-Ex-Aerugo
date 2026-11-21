//! A minimal world: the level object, spawning and destroying actors, timers and the per-frame tick.

use crate::{inst_ref, ref_instance, Value, Vm, VmError};
use ue_registry::ClassId;

/// `ROLE_Authority` (the enum is None, DumbProxy, SimulatedProxy, AutonomousProxy, Authority).
const ROLE_AUTHORITY: u8 = 4;

impl Vm {
    /// Creates the level object (`LevelInfo`) every actor refers to through its `Level` property, together with the
    /// game rules object (`Level.Game`, `DeusExGameInfo` when present) and the base mutator the game consults when
    /// deciding whether an actor is relevant.
    pub fn init_world(&mut self) -> Result<u32, VmError> {
        let class = self.reg.class_by_name("LevelInfo").ok_or_else(|| VmError::Runtime("no LevelInfo class".into()))?;
        let lvl = self.new_object(class)?;
        self.set_prop(lvl, "Level", 0, Value::Obj(lvl))?;
        // a bootstrap world for tools and tests is in play from the start (actors' events need `bBegunPlay`)
        self.set_prop(lvl, "bBegunPlay", 0, Value::Bool(true))?;
        self.level = lvl;
        let game_class = self
            .reg
            .class_by_name("DeusExGameInfo")
            .or_else(|| self.reg.class_by_name("GameInfo"))
            .ok_or_else(|| VmError::Runtime("no GameInfo class".into()))?;
        let game = self.new_object(game_class)?;
        self.set_prop(game, "Level", 0, Value::Obj(lvl))?;
        self.set_prop(lvl, "Game", 0, Value::Obj(game))?;
        if let Some(m) = self.reg.class_by_name("Mutator") {
            let base = self.new_object(m)?;
            self.set_prop(base, "Level", 0, Value::Obj(lvl))?;
            self.set_prop(game, "BaseMutator", 0, Value::Obj(base))?;
        }
        Ok(lvl)
    }

    /// References of all live actors (in creation order).
    pub fn live_actors(&self) -> Vec<u32> {
        self.actor_slots.iter().filter(|&&i| self.objects[i].alive).map(|&i| inst_ref(i)).collect()
    }

    /// Rebuilds the list of actor slots from the objects (after a level load, travel or a restored game: objects may
    /// have come back to life).
    ///
    /// Before and during a level's start-up the order is creation order (the map's list, then what was spawned). After it
    /// `LoadMap` (1038dcd5) rebuilds `GLevel->Actors` as the level and its default brush, then the static actors, then
    /// the others (`iFirstDynamicActor` is where they start), each group in the old order; empty slots are dropped.
    /// Later spawns are appended, so the same rule describes the list from then on.
    pub fn rebuild_actor_slots(&mut self) {
        let live: Vec<usize> = (0..self.objects.len()).filter(|&i| self.objects[i].actor && self.objects[i].alive).collect();
        if !self.actors_sorted {
            self.actor_slots = live;
            return;
        }
        let o = self.offsets();
        let front: Vec<usize> = [self.level, self.level_brush].into_iter().filter(|&r| r != 0 && self.is_live(r)).filter_map(ref_instance).collect();
        let rest: Vec<usize> = live.into_iter().filter(|i| !front.contains(i)).collect();
        let (statics, dynamics): (Vec<usize>, Vec<usize>) = rest.into_iter().partition(|&i| self.flag(inst_ref(i), o.b_static.0, o.b_static.1));
        self.actor_slots = front.into_iter().chain(statics).chain(dynamics).collect();
    }

    /// The end of `LoadMap` for a level: with `post_post_begin_play` (a restored saved game; a fresh level had it in
    /// `begin_play`) every actor gets `PostPostBeginPlay` in the list's current order (actors spawned meanwhile too);
    /// then the list is sorted (`rebuild_actor_slots`). Script errors are returned.
    pub(crate) fn finish_level_start(&mut self, post_post_begin_play: bool) -> Vec<crate::level::LoadIssue> {
        let mut load = crate::level::LevelLoad::default();
        if post_post_begin_play {
            let mut order = self.live_actors();
            let mut scanned = self.objects.len();
            self.startup_pass(&mut order, &mut scanned, &mut load, "PostPostBeginPlay", |vm, r| vm.send_event(r, "PostPostBeginPlay", vec![]).map(|_| ()));
        }
        self.actors_sorted = true;
        self.rebuild_actor_slots();
        load.issues
    }

    pub(crate) fn is_actor_class(&self, class: ClassId) -> bool {
        let actor = match self.actor_class.get() {
            Some(a) => Some(a),
            None => {
                let a = self.reg.class_by_name("Actor");
                self.actor_class.set(a);
                a
            }
        };
        actor.map(|a| self.is_a(class, a)).unwrap_or(false)
    }

    /// An engine-side spawn (`SpawnActor` with `bNoCollisionFail`: no placement, no encroachment test); `tag`, when
    /// given, is written afterwards. See `spawn_actor`. Unlike the engine it also makes abstract classes (tests and
    /// tools place bare `Pawn`s); script `Spawn` refuses them.
    pub fn spawn(
        &mut self,
        class: ClassId,
        owner: u32,
        tag: Option<u32>,
        location: Option<[f32; 3]>,
        rotation: Option<[i32; 3]>,
    ) -> Result<Option<u32>, VmError> {
        if !self.is_actor_class(class) {
            return Err(VmError::Runtime(format!("cannot spawn {}: not an actor class", self.reg.class(class).path)));
        }
        let made = self.spawn_actor_with(class, owner, 0, location, rotation, true, true)?;
        if let (Some(a), Some(t)) = (made, tag) {
            self.set_prop(a, "Tag", 0, Value::Name(t))?;
        }
        Ok(made)
    }

    /// `ULevel::SpawnActor` (10394f60). Refused (with a log line) for abstract, non-actor, static or no-delete classes.
    /// Unless `no_collision_fail`, a class that collides with the world (or `bCollideWhenPlacing`) is first moved to a
    /// spot where its default cylinder fits (`FindSpot`, world only), and not spawned when there is none. The new actor
    /// gets `Tag` = its class name, its owner (`SetOwner`: `LostChild`/`GainedChild`) and instigator, then `Spawned`,
    /// `PreBeginPlay`, `BeginPlay` (gone if it deleted itself), a zone test (`SetActorZone(A, 1, 1)`),
    /// `LastRenderTime = -10`, `PostBeginPlay`, the encroachment test (unless `no_collision_fail`: destroyed when it
    /// encroaches), `SetInitialState`, a base search below an unbased decoration, inventory item or pawn that collides
    /// with the world (physics none or rotating) and `PostPostBeginPlay`. (`SpawnNotify` actors: Deus Ex has none.)
    pub fn spawn_actor(
        &mut self,
        class: ClassId,
        owner: u32,
        instigator: u32,
        location: Option<[f32; 3]>,
        rotation: Option<[i32; 3]>,
        no_collision_fail: bool,
    ) -> Result<Option<u32>, VmError> {
        self.spawn_actor_with(class, owner, instigator, location, rotation, no_collision_fail, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_actor_with(
        &mut self,
        class: ClassId,
        owner: u32,
        instigator: u32,
        location: Option<[f32; 3]>,
        rotation: Option<[i32; 3]>,
        no_collision_fail: bool,
        allow_abstract: bool,
    ) -> Result<Option<u32>, VmError> {
        let path = self.reg.class(class).path.clone();
        // `CLASS_Abstract` (UClass::ClassFlags bit 0)
        if (self.reg.class(class).flags & 1 != 0 && !allow_abstract) || !self.is_actor_class(class) {
            self.log.push(format!("SpawnActor failed: {path} is abstract or not an actor class"));
            return Ok(None);
        }
        let obj = self.new_object(class)?;
        let o = self.offsets();
        if self.flag(obj, o.b_static.0, o.b_static.1) || self.bp(obj, "bNoDelete") {
            self.set_alive(obj, false);
            self.log.push(format!("SpawnActor failed: {path} is static or no-delete"));
            return Ok(None);
        }
        // placement with the class defaults' cylinder: `FindSpot(FVector(R, R, H), Location, 0, 1)`
        let mut location = location;
        let placing = self.bp(obj, "bCollideWorld") || (self.bp(obj, "bCollideWhenPlacing") && (self.level == 0 || self.u8p(self.level, "NetMode") != 3));
        if placing && !no_collision_fail {
            let (r, h) = (self.f32_at(obj, o.collision_radius), self.f32_at(obj, o.collision_height));
            let mut at = location.unwrap_or_else(|| self.location(obj));
            if !self.find_spot([r, r, h], &mut at, false, true) {
                self.set_alive(obj, false);
                return Ok(None);
            }
            location = Some(at);
        }
        // the native constructor of `PlayerPawnExt` gives every player pawn its own flag store
        if let (Some(ppe), Some(fb)) = (self.reg.class_by_name("PlayerPawnExt"), self.reg.class_by_name("FlagBase")) {
            if self.is_a(class, ppe) {
                let f = self.new_object(fb)?;
                self.set_prop(obj, "flagBase", 0, Value::Obj(f))?;
            }
        }
        if self.level != 0 {
            self.set_prop(obj, "Level", 0, Value::Obj(self.level))?;
        }
        self.set_prop(obj, "XLevel", 0, Value::Obj(self.xlevel))?;
        self.set_prop(obj, "Role", 0, Value::Byte(ROLE_AUTHORITY))?;
        // a spawned actor has no brush, starts in the level's region (pawns: head and foot too) and is named by its tag
        self.set_u32_at(obj, o.brush, 0);
        let name = self.reg.class(class).name;
        self.set_prop(obj, "Tag", 0, Value::Name(name))?;
        if let Some(l) = location {
            self.set_prop(obj, "Location", 0, Value::Vector(l))?;
            self.set_prop(obj, "OldLocation", 0, Value::Vector(l))?;
        }
        if let Some(r) = rotation {
            self.set_prop(obj, "Rotation", 0, Value::Rotator(r))?;
        }
        {
            let ro = self.region_offsets();
            let none = crate::movement::Region { zone: self.level, leaf: -1, zone_number: 0 };
            self.write_region(obj, ro.region, none);
            if self.is_instance_of(obj, o.class_pawn) {
                self.write_region(obj, ro.head_region, none);
                self.write_region(obj, ro.foot_region, none);
            }
        }
        // `SpawnActor` (10394f60): into the hash once placed, before any event
        self.hash_add_colliding(obj);
        self.register_nav_point(obj);
        self.set_owner(obj, owner)?;
        self.set_prop(obj, "Instigator", 0, Value::Obj(instigator))?;
        for ev in ["Spawned", "PreBeginPlay", "BeginPlay"] {
            self.send_event(obj, ev, vec![])?;
        }
        if self.get_prop(obj, "bDeleteMe", 0)? == Value::Bool(true) {
            return Ok(None);
        }
        self.set_actor_zone(obj, true, true)?;
        self.set_fp(obj, "LastRenderTime", -10.0);
        self.send_event(obj, "PostBeginPlay", vec![])?;
        if !no_collision_fail {
            let (loc, rot) = (self.location(obj), self.rotation(obj));
            if self.check_encroachment(obj, loc, rot, false)? {
                self.destroy(obj)?;
                return Ok(None);
            }
        }
        self.send_event(obj, "SetInitialState", vec![])?;
        if self.u32_at(obj, o.base) == 0 && self.bp(obj, "bCollideWorld") {
            let kinds: Vec<ClassId> = ["Decoration", "Inventory", "Pawn"].iter().filter_map(|n| self.reg.class_by_name(n)).collect();
            let physics = self.u8_at(obj, o.physics);
            if kinds.iter().any(|&k| self.is_a(class, k)) && (physics == 0 || physics == 5) {
                self.find_base(obj)?;
            }
        }
        self.send_event(obj, "PostPostBeginPlay", vec![])?;
        Ok(Some(obj))
    }

    /// `AActor::SetOwner`: the old owner gets `LostChild`, the new one `GainedChild` (no test for the same owner).
    pub fn set_owner(&mut self, a: u32, owner: u32) -> Result<(), VmError> {
        let o = self.offsets();
        let old = self.u32_at(a, o.owner);
        if old != 0 {
            self.send_event(old, "LostChild", vec![Value::Obj(a)])?;
        }
        self.set_u32_at(a, o.owner, owner);
        if owner != 0 {
            self.send_event(owner, "GainedChild", vec![Value::Obj(a)])?;
        }
        Ok(())
    }

    /// `ULevel::DestroyActor` (10395ba0). Static and `bNoDelete` actors are refused (false), an actor already deleted
    /// answers true, a non-authority one false. Then: `EndState` if it is probed, off its base and its riders off it
    /// (stops if that deleted it), out of the collision hash, `Destroyed`; unless that deleted it, every actor it owns
    /// loses it as owner (`SetOwner(None)`: `LostChild` here) and every actor touching it ends that touch (only the
    /// other gets `UnTouch`), walking the actor list in order and stopping if it got deleted, the owner gets
    /// `LostChild`, and the actor is gone.
    pub fn destroy(&mut self, obj: u32) -> Result<bool, VmError> {
        let Some(i) = ref_instance(obj) else { return Ok(false) };
        let Some(ob) = self.objects.get(i) else { return Ok(false) };
        if !ob.alive {
            // a deleted actor
            return Ok(ob.actor);
        }
        if self.bp(obj, "bStatic") || self.bp(obj, "bNoDelete") {
            return Ok(false);
        }
        let deleted = |vm: &mut Vm| !vm.objects[i].alive || vm.bp(obj, "bDeleteMe");
        if deleted(self) {
            return Ok(true);
        }
        if self.u8p(obj, "Role") != 4 && !self.bp(obj, "bNetTemporary") {
            return Ok(false);
        }
        if self.objects[i].state.is_some() && self.is_probing(obj, "EndState") {
            self.send_event(obj, "EndState", vec![])?;
            if deleted(self) {
                return Ok(true);
            }
        }
        let o = self.offsets();
        if self.u32_at(obj, o.base) != 0 {
            self.set_base(obj, 0, true)?;
            if deleted(self) {
                return Ok(true);
            }
        }
        if self.u8_at(obj, o.standing_count) != 0 {
            for b in self.live_actors() {
                if self.u32_at(b, o.base) == obj {
                    self.set_base(b, 0, true)?;
                }
            }
        }
        self.hash_remove_colliding(obj);
        self.send_event(obj, "Destroyed", vec![])?;
        if deleted(self) {
            return Ok(true);
        }
        for x in self.live_actors() {
            if self.u32_at(x, o.owner) == obj {
                self.set_owner(x, 0)?;
                if deleted(self) {
                    return Ok(true);
                }
            } else {
                for k in 0..4 {
                    if self.touching(x, k) == obj {
                        self.end_touch(obj, x, true)?;
                        if deleted(self) {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        let owner = self.u32_at(obj, o.owner);
        if owner != 0 {
            self.send_event(owner, "LostChild", vec![Value::Obj(obj)])?;
            if deleted(self) {
                return Ok(true);
            }
        }
        // `Destroyed` may have turned collision back on: the engine's hash keeps such links until the level goes;
        // ours drops them with the actor
        self.hash_remove(obj);
        self.set_prop(obj, "bDeleteMe", 0, Value::Bool(true))?;
        // `UObject::ConditionalDestroy` -> `Destroy` -> `ExitProperties`: the actor's strings are freed (read as empty);
        // everything else stays readable until the cleanup
        self.clear_strings(obj);
        let o = &mut self.objects[i];
        o.alive = false;
        o.exec = None;
        o.latent = None;
        // `Actor->Deleted = FirstDeleted; FirstDeleted = Actor`: the AI event manager forgets it at the cleanup
        // (`ULevel::CleanupDestroyed`, `UEventManager::DestroyActor`)
        self.pending_deleted.insert(0, obj);
        self.pending_deleted_set.insert(obj);
        self.ai.con_bound.remove(&obj);
        Ok(true)
    }

    /// A destroyed actor not yet cleaned up (`bDeleteMe`, on `ULevel::FirstDeleted`): scripts holding it still read and
    /// call through it (`UObject::execContext` only rejects a null pointer).
    pub fn is_pending_deleted(&self, r: u32) -> bool {
        self.pending_deleted_set.contains(&r)
    }

    /// `ULevel::CleanupDestroyed` (Engine 103965a0), at the end of every level tick (`force` false) and forced when a
    /// game is saved (`UGameEngine::SaveGame`) or the level is left. Nothing happens until more than 127 actors wait
    /// (unless forced); then every object property of every actor of the level (arrays and structs included) that
    /// points to one of them is cleared (`UStruct::CleanupDestroyed`, Core: `IsPendingKill`), the AI event manager forgets
    /// each (`UEventManager::DestroyActor`, newest first) and they are freed. (`CompactActors` first: the actor list keeps
    /// its order without the empty slots, which the port's list never has.)
    pub fn cleanup_destroyed(&mut self, force: bool) {
        if self.pending_deleted.is_empty() || (self.pending_deleted.len() < 128 && !force) {
            return;
        }
        let gone = std::mem::take(&mut self.pending_deleted_set);
        for i in 0..self.objects.len() {
            let r = inst_ref(i);
            if !self.objects[i].alive || !self.objects[i].actor {
                continue;
            }
            let Some(class) = self.class_of(r) else { continue };
            let mut data = std::mem::take(&mut self.objects[i].mem.data);
            self.map_slots(class, &mut data, &mut |kind, v| if kind == crate::save::Val::Obj && gone.contains(&v) { 0 } else { v });
            self.objects[i].mem.data = data;
        }
        for a in std::mem::take(&mut self.pending_deleted) {
            self.ai.events.destroy_actor(a);
        }
    }

    /// Frees the string properties of `obj` (structs included): they read as empty.
    fn clear_strings(&mut self, obj: u32) {
        let (Some(i), Some(class)) = (ref_instance(obj), self.class_of(obj)) else { return };
        let mut data = std::mem::take(&mut self.objects[i].mem.data);
        let props: Vec<ue_registry::PropDef> = self.reg.all_props(class).into_iter().cloned().collect();
        for p in &props {
            self.clear_string_slots(p, p.offset as usize, &mut data);
        }
        self.objects[i].mem.data = data;
    }

    fn clear_string_slots(&self, p: &ue_registry::PropDef, base: usize, data: &mut [u8]) {
        use ue_package::object::PropertyKind;
        for e in 0..p.dim.max(1) as usize {
            let off = base + e * p.elem_size as usize;
            match (&p.kind, &p.target) {
                (PropertyKind::Str, _) => {
                    if let Some(slot) = data.get_mut(off..off + 4) {
                        slot.copy_from_slice(&0u32.to_le_bytes());
                    }
                }
                (PropertyKind::Struct, ue_registry::Target::Struct(sid)) => {
                    for m in self.struct_members(*sid) {
                        self.clear_string_slots(&m, off + m.offset as usize, data);
                    }
                }
                _ => {}
            }
        }
    }

    /// Advances the world by `dt` seconds: level time, then for every actor that existed at the start of the frame
    /// the `Tick` event, its timer, its state code and its physics. Stops at the first script error.
    pub fn tick(&mut self, dt: f32) -> Result<(), VmError> {
        let mut issues = Vec::new();
        self.tick_report(dt, &mut issues);
        match issues.into_iter().next() {
            Some(i) => Err(VmError::Runtime(format!("{} {}: {}", i.class, i.event, i.error))),
            None => Ok(()),
        }
    }

    /// Like `tick`, but a script error only ends that actor's phase: it is recorded in `issues` and the world goes on.
    pub fn tick_report(&mut self, dt: f32, issues: &mut Vec<crate::level::LoadIssue>) {
        // `UGameEngine::Tick` (1038fe90), with a client, just before the level ticks: Deus Ex clears `bDropDetail` and
        // `bAggressiveLOD` every frame (no frame-rate test in this build), so both are always off for the scripts
        if self.level != 0 {
            let lvl = self.level;
            self.set_bp(lvl, "bDropDetail", false);
            self.set_bp(lvl, "bAggressiveLOD", false);
        }
        // `ULevel::Tick` (103a51a0): `LevelInfo.TimeDilation` scales the frame, the level clock advances by the
        // scaled time, then the time the actors see is clamped to 0.005..0.4 s
        let dilation = if self.level != 0 { self.get_prop(self.level, "TimeDilation", 0).ok().and_then(|v| v.as_f32()).unwrap_or(1.0) } else { 1.0 };
        let dt = if dilation.is_finite() { dt * dilation } else { dt };
        self.time += dt;
        if self.level != 0 {
            let t = self.get_prop(self.level, "TimeSeconds", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0);
            let _ = self.set_prop(self.level, "TimeSeconds", 0, Value::Float(t + dt));
        }
        let dt = if dt < 0.005 { 0.005 } else if dt >= 0.4 { 0.4 } else { dt };
        // a paused game (`Level.Pauser` set: the menus and screens pause through `SetPause`) ticks only the actors
        // marked `bAlwaysTick`; the player's input is read by the host (`player_frame`)
        if self.is_paused() {
            let o = self.offsets();
            let actor = self.reg.class_by_name("Actor");
            let scripted = self.reg.class_by_name("ScriptedPawn");
            for i in 0..self.objects.len() {
                let ob = &self.objects[i];
                let r = inst_ref(i);
                if ob.alive && actor.map(|a| self.is_a(ob.class, a)).unwrap_or(false) && r != self.level && !self.flag(r, o.b_static.0, o.b_static.1) && self.bp(r, "bAlwaysTick") {
                    self.tick_actor(i, dt, scripted, issues);
                }
            }
            return;
        }
        // `Level.bPlayersOnly`: the "viewports only" tick (only the local player runs; the others just animate)
        let players_only = self.level != 0 && self.bp(self.level, "bPlayersOnly");
        let actor = self.reg.class_by_name("Actor");
        let scripted = self.reg.class_by_name("ScriptedPawn");
        let o = self.offsets();
        // `ULevel::Tick` ticks the dynamic actors only: the level, its default brush and the static actors are sorted
        // in front of `iFirstDynamicActor` when the map loads and never tick
        let dynamic = |vm: &Vm, i: usize| -> bool {
            let ob = &vm.objects[i];
            // `actor`: the object's class is an `Actor` (decided once)
            ob.alive && ob.actor && actor.is_some() && inst_ref(i) != vm.level && !vm.flag(inst_ref(i), o.b_static.0, o.b_static.1)
        };
        self.update_distance_from_player(&dynamic);
        // the actor list is read live: what spawns during the frame ticks in the same frame. An actor whose owner has
        // not ticked yet (`bTicked` differs from the level's toggle) is put on a list that is gone through again,
        // newest first, as long as a pass ticks something; the toggle never matches a static owner on odd frames.
        let mut pending: Vec<usize> = Vec::new();
        let mut progress = false;
        let mut i = 0;
        while i < self.objects.len() {
            if dynamic(self, i) {
                match self.actor_tick(i, dt, players_only, scripted, issues) {
                    Ticked::Deferred => pending.push(i),
                    Ticked::Done => progress = true,
                }
            }
            i += 1;
        }
        while !pending.is_empty() && progress {
            progress = false;
            let list = std::mem::take(&mut pending);
            for &i in list.iter().rev() {
                if !dynamic(self, i) || self.bp(inst_ref(i), "bTicked") == self.tick_toggle {
                    continue;
                }
                match self.actor_tick(i, dt, players_only, scripted, issues) {
                    Ticked::Deferred => pending.push(i),
                    Ticked::Done => progress = true,
                }
            }
        }
        // after the actors: the AI event manager lets listeners notice what the senders emitted (`UEventManager::Tick`;
        // not in a players-only tick)
        if self.level != 0 && !players_only {
            if let Err(e) = self.ai_event_tick() {
                issues.push(crate::level::LoadIssue { actor: self.level, class: "EventManager".into(), event: "Tick".into(), error: e.to_string() });
            }
        }
        self.tick_toggle = !self.tick_toggle;
        // the end of `ULevel::Tick`
        self.cleanup_destroyed(false);
    }

    /// `AActor::InStasis` (1036bfe0): an actor with `bStasis` that is not moving on its own (`bForceStasis`, or physics
    /// none or rotating), not drawn for 5 s, whose zone has not been drawn for 5 s or that is more than 1200 units from
    /// the player, in a standalone game. Such an actor does not tick.
    pub fn in_stasis(&mut self, a: u32) -> bool {
        let o = self.offsets();
        if self.level == 0 || !self.is_live(a) {
            return false;
        }
        let now = self.fp(self.level, "TimeSeconds");
        if now - self.fp(a, "LastRenderTime") <= 5.0 || !self.bp(a, "bStasis") {
            return false;
        }
        let phys = self.u8_at(a, o.physics);
        if !self.bp(a, "bForceStasis") && phys != 0 && phys != crate::physics::PHYS_ROTATING {
            return false;
        }
        let zn = self.zone_number(a) as usize;
        let zone_seen = self.zone_render_times.get(zn).copied().unwrap_or(0.0);
        if now - zone_seen <= 5.0 && self.fp(a, "DistanceFromPlayer") <= 1200.0 {
            return false;
        }
        self.u8p(self.level, "NetMode") == 0
    }

    /// Is the game paused (`Level.Pauser` not empty)?
    pub fn is_paused(&mut self) -> bool {
        self.level != 0 && matches!(self.get_prop(self.level, "Pauser", 0), Ok(Value::Str(ref p)) if !p.is_empty())
    }

    /// One actor's tick (object index `i`): `AScriptedPawn::Tick` first for NPCs (not in a players-only tick), then
    /// `AActor::Tick` (Engine.dll 103a1aa0): in stasis nothing more (a transient actor is removed); an owner that has
    /// not ticked this frame defers it; otherwise animation, then for the local player its input, `PlayerInput` and
    /// `PlayerTick` (no `Tick` event), for any other actor the `Tick` event (in a players-only tick it stops here),
    /// then state code, timer, life span, physics, eye height and the pawn checks.
    fn actor_tick(&mut self, i: usize, dt: f32, players_only: bool, scripted: Option<ue_registry::ClassId>, issues: &mut Vec<crate::level::LoadIssue>) -> Ticked {
        let class = self.objects[i].class;
        let obj = inst_ref(i);
        let mut note = |vm: &Vm, event: &str, r: Result<(), VmError>| {
            if let Err(e) = r {
                issues.push(crate::level::LoadIssue { actor: obj, class: vm.reg.class(class).path.clone(), event: event.into(), error: e.to_string() });
            }
        };
        if scripted.map(|c| self.is_a(class, c)).unwrap_or(false) {
            let own_part = (self.u8p(obj, "RemoteRole") != 3 && self.u8p(obj, "Role") > 1 && !players_only) || self.bp(obj, "bSimulatedPawn");
            if own_part {
                match self.scripted_pawn_tick(obj, dt) {
                    Ok(true) => {}
                    Ok(false) => return Ticked::Done,
                    Err(e) => note(self, "ScriptedPawnTick", Err(e)),
                }
                if !self.objects[i].alive {
                    return Ticked::Done;
                }
            }
        }
        if self.in_stasis(obj) {
            if self.bp(obj, "bTransient") && self.u8p(obj, "Role") >= 2 {
                let _ = self.destroy(obj);
            }
            return Ticked::Done;
        }
        let o = self.offsets();
        let owner = self.u32_at(obj, o.owner);
        if owner != 0 && self.is_live(owner) && self.bp(owner, "bTicked") != self.tick_toggle {
            return Ticked::Deferred;
        }
        let toggle = self.tick_toggle;
        self.set_bp(obj, "bTicked", toggle);
        let r = self.tick_animation(obj, dt);
        note(self, "Animation", r);
        if self.objects[i].alive && self.u8p(obj, "RemoteRole") != 3 && self.u8p(obj, "Role") > 1 {
            if self.is_viewport_player(obj) {
                let r = self.player_slot(obj, dt);
                note(self, "PlayerTick", r);
            } else {
                if players_only {
                    return Ticked::Done;
                }
                let r = self.send_event(obj, "Tick", vec![Value::Float(dt)]).map(|_| ());
                note(self, "Tick", r);
            }
        }
        if self.objects[i].alive {
            let r = self.tick_state(obj, dt);
            note(self, "StateCode", r);
        }
        if self.objects[i].alive {
            let r = self.tick_timer(obj, dt);
            note(self, "Timer", r);
        }
        if self.objects[i].alive {
            let r = self.tick_life_span(obj, dt);
            note(self, "LifeSpan", r);
        }
        // `AActor::Tick` (Engine 103a26xx): physics only for an actor that has some (`Physics != PHYS_None`) and is not a
        // simulated proxy (`Role != ROLE_SimulatedProxy`); an actor at rest keeps its `RotationRate` without turning
        let o = self.offsets();
        if self.objects[i].alive && dt > 0.0 && self.u8_at(obj, o.physics) != crate::physics::PHYS_NONE && self.u8_at(obj, o.role) != 3 {
            let r = self.perform_physics(obj, dt);
            note(self, "Physics", r);
        }
        if self.objects[i].alive {
            let r = self.tick_view(obj, dt);
            note(self, "UpdateEyeHeight", r);
        }
        if self.objects[i].alive {
            let r = self.tick_pawn_extras(obj, dt, !players_only);
            note(self, "PawnTick", r);
        }
        Ticked::Done
    }

    /// The paused tick of a `bAlwaysTick` actor: its whole actor tick.
    fn tick_actor(&mut self, i: usize, dt: f32, scripted: Option<ue_registry::ClassId>, issues: &mut Vec<crate::level::LoadIssue>) {
        let _ = self.actor_tick(i, dt, false, scripted, issues);
    }

    /// `ULevel::Tick` (standalone games): every dynamic actor's distance to the local player.
    fn update_distance_from_player(&mut self, dynamic: &dyn Fn(&Vm, usize) -> bool) {
        let o = self.offsets();
        // the first live player pawn with a player, in object order (the actor slots hold every live actor)
        let slots = self.actor_slots.clone();
        let mut player = None;
        for &i in &slots {
            let r = crate::inst_ref(i);
            if self.objects[i].alive && self.is_instance_of(r, o.class_player_pawn) && self.op(r, "Player") != 0 && player.is_none_or(|p| r < p) {
                player = Some(r);
            }
        }
        let Some(player) = player else { return };
        let p = self.vec_at(player, o.location);
        // an `Actor` property: one offset for every actor
        let Some((off, _)) = self.actor_prop_offset("DistanceFromPlayer") else { return };
        for i in slots {
            if !dynamic(self, i) {
                continue;
            }
            let r = inst_ref(i);
            let l = self.vec_at(r, o.location);
            let d = ((l[0] - p[0]).powi(2) + (l[1] - p[1]).powi(2) + (l[2] - p[2]).powi(2)).sqrt();
            self.set_u32_at(r, off, d.to_bits());
        }
    }

    /// The end of `AActor::Tick` for pawns played by someone (`bIsPlayer`): a view target (the local player, whose
    /// viewport looks through it, or a pawn made one by `BecomeViewTarget`) eases its eye height (`UpdateEyeHeight`:
    /// crouching, landing, view bob); any other such pawn looks where it faces.
    fn tick_view(&mut self, obj: u32, dt: f32) -> Result<(), VmError> {
        let o = self.offsets();
        if !self.is_instance_of(obj, o.class_pawn) || !self.bp(obj, "bIsPlayer") {
            return Ok(());
        }
        let viewed = self.bp(obj, "bViewTarget") || (self.is_instance_of(obj, o.class_player_pawn) && self.op(obj, "Player") != 0);
        if viewed {
            self.send_event(obj, "UpdateEyeHeight", vec![Value::Float(dt)])?;
        } else {
            let r = self.get_prop(obj, "Rotation", 0)?;
            self.set_prop(obj, "ViewRotation", 0, r)?;
        }
        Ok(())
    }

    /// The end of `AActor::Tick` for pawns (Engine.dll 103a288d): the weapon is kept at the pawn, `SightCounter` paces
    /// the senses (a player that is not hidden shows itself to every pawn whose counter ran out and that has it in
    /// sight: `SeePlayer`; a pawn whose counter ran out checks its enemy: `EnemyNotVisible` or `LastSeenTime`),
    /// `PainTime` and `SpeechTime` count down to `PainTimer` (drowning, pain zones) and `SpeechTimer`, and pawns with
    /// `bAdvancedTactics` get `UpdateTactics`.
    fn tick_pawn_extras(&mut self, obj: u32, dt: f32, all: bool) -> Result<(), VmError> {
        let o = self.offsets();
        if !self.is_instance_of(obj, o.class_pawn) {
            return Ok(());
        }
        let weapon = self.op(obj, "Weapon");
        if weapon != 0 && self.is_live(weapon) {
            let l = self.vec_at(obj, o.location);
            self.far_move_actor(weapon, l, false, false)?;
        }
        // the sight counter, `ShowSelf` and the pawn timers: authority, full tick only
        if !all || self.u8p(obj, "Role") != 4 {
            return Ok(());
        }
        let mut sight = self.fp(obj, "SightCounter");
        if sight < 0.0 {
            sight += 0.2;
        }
        sight -= dt;
        self.set_fp(obj, "SightCounter", sight);
        if self.bp(obj, "bIsPlayer") && !self.bp(obj, "bHidden") {
            self.show_self(obj)?;
        }
        if !self.is_live(obj) {
            return Ok(());
        }
        if self.fp(obj, "SightCounter") < 0.0 && self.is_probing(obj, "EnemyNotVisible") {
            let enemy = self.op(obj, "Enemy");
            if enemy != 0 && self.is_live(enemy) {
                if !self.line_of_sight_to(obj, enemy, false, false) {
                    self.send_event(obj, "EnemyNotVisible", vec![])?;
                } else {
                    let now = self.get_prop(self.level, "TimeSeconds", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0);
                    self.set_fp(obj, "LastSeenTime", now);
                }
            }
            self.set_fp(obj, "SightCounter", 0.1);
        }
        for (timer, event) in [("PainTime", "PainTimer"), ("SpeechTime", "SpeechTimer")] {
            if !self.is_live(obj) {
                return Ok(());
            }
            let t = self.fp(obj, timer);
            if t > 0.0 {
                let left = t - dt;
                self.set_fp(obj, timer, left);
                if left < 0.001 {
                    self.set_fp(obj, timer, 0.0);
                    self.send_event(obj, event, vec![])?;
                }
            }
        }
        if self.is_live(obj) && self.bp(obj, "bAdvancedTactics") {
            self.send_event(obj, "UpdateTactics", vec![Value::Float(dt)])?;
        }
        Ok(())
    }

    /// `APawn::ShowSelf`: every other pawn in `Level.PawnList` whose sight counter has run out and that can see `obj`
    /// (`LineOfSightTo` with peripheral vision) hears `SeePlayer(obj)`.
    fn show_self(&mut self, obj: u32) -> Result<(), VmError> {
        if self.level == 0 {
            return Ok(());
        }
        let mut p = self.op(self.level, "PawnList");
        let mut guard = 0;
        while p != 0 && guard < 4096 {
            guard += 1;
            if p != obj && self.is_live(p) && self.fp(p, "SightCounter") < 0.0 && self.is_probing(p, "SeePlayer") && self.line_of_sight_to(p, obj, true, false) {
                self.send_event(p, "SeePlayer", vec![Value::Obj(obj)])?;
            }
            // the link is read after the event, as the engine's loop does
            p = self.op(p, "nextPawn");
        }
        Ok(())
    }

    /// `LifeSpan` counts down; at (nearly) zero the actor gets `Expired` and is destroyed.
    fn tick_life_span(&mut self, obj: u32, dt: f32) -> Result<(), VmError> {
        let life = self.get_prop(obj, "LifeSpan", 0)?.as_f32().unwrap_or(0.0);
        if life == 0.0 {
            return Ok(());
        }
        let left = life - dt;
        self.set_prop(obj, "LifeSpan", 0, Value::Float(left))?;
        if left <= 0.0001 {
            self.send_event(obj, "Expired", vec![])?;
            self.destroy(obj)?;
        }
        Ok(())
    }

    fn tick_timer(&mut self, obj: u32, dt: f32) -> Result<(), VmError> {
        let rate = self.get_prop_static(obj, "TimerRate", 0)?.as_f32().unwrap_or(0.0);
        if rate <= 0.0 {
            return Ok(());
        }
        let counter = self.get_prop_static(obj, "TimerCounter", 0)?.as_f32().unwrap_or(0.0) + dt;
        if counter < rate {
            self.set_prop_static(obj, "TimerCounter", 0, Value::Float(counter))?;
            return Ok(());
        }
        // Whole periods elapsed are subtracted; a one-shot timer clears its rate; `Timer` runs once.
        let periods = (counter / rate).floor();
        self.set_prop_static(obj, "TimerCounter", 0, Value::Float(counter - periods * rate))?;
        if periods != 0.0 && self.get_prop_static(obj, "bTimerLoop", 0)? != Value::Bool(true) {
            self.set_prop_static(obj, "TimerRate", 0, Value::Float(0.0))?;
        }
        self.send_event(obj, "Timer", vec![])?;
        Ok(())
    }
}

/// What one actor's tick did: ran (or was skipped for good: stasis, destroyed), or waits for its owner.
enum Ticked {
    Done,
    Deferred,
}


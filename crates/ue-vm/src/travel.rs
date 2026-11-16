//! Level travel: leaving the current map for another one and carrying the player along.
//!
//! The original engine exports the player pawn's `travel` properties (and, through them, the actors with `bTravel` and
//! the objects they point to) to text, loads the next map, logs a fresh pawn in and imports the text into it. Objects
//! that are not actors are kept alive here (the new pawn simply refers to them); actors are re-spawned in the new level
//! and receive the old ones' travel properties, with references to travelling actors pointing at the new copies.

use std::collections::{HashSet, VecDeque};

use crate::property_text::Sel;
use crate::{ref_instance, Value, Vm, VmError};
use ue_package::object::PropertyKind;
use ue_registry::PropDef;

const CPF_TRAVEL: u32 = 0x10000;

/// A request to change the level (`ClientTravel`, `ServerTravel`, the `open` console command).
#[derive(Debug, Clone)]
pub struct TravelRequest {
    pub url: String,
    /// Carry the player's items (`bItems` of `ClientTravel`).
    pub items: bool,
}

impl TravelRequest {
    /// The map name and the portal (`Map#Portal`) of the URL; options after `?` are dropped. `None` for `?restart` style URLs.
    pub fn map_and_portal(&self) -> Option<(String, String)> {
        let u = self.url.trim();
        let (head, _) = u.split_once('?').unwrap_or((u, ""));
        let (map, portal) = head.split_once('#').unwrap_or((head, ""));
        let map = map.trim().trim_end_matches(".dx").trim_end_matches(".DX");
        if map.is_empty() {
            return None;
        }
        Some((map.to_string(), portal.to_string()))
    }
}

/// One block of the travel text: an object that travels and its `travel` values that differ from the defaults.
#[derive(Debug, Clone)]
pub struct TravelRecord {
    pub old: u32,
    pub class: crate::ClassId,
    pub values: Vec<(PropDef, u32, TravelValue)>,
}

#[derive(Debug, Clone)]
pub enum TravelValue {
    /// `ExportText` of the value.
    Text(String),
    /// An object reference: the old object (0: None).
    Ptr(u32),
}

impl Vm {
    /// The `travel` properties of a class: (name, element count).
    fn travel_props(&self, class: crate::ClassId) -> Vec<(String, usize)> {
        self.reg.all_props(class).into_iter().filter(|p| p.flags & CPF_TRAVEL != 0).map(|p| (self.reg.names.str(p.name).to_string(), p.dim.max(1) as usize)).collect()
    }

    pub(crate) fn is_actor_instance(&self, r: u32) -> bool {
        self.class_of(r).map(|c| self.is_actor_class(c)).unwrap_or(false)
    }

    /// What travels with `pawn`: its travelling actors (the pawn first) and the other objects they refer to through
    /// `travel` properties.
    pub fn travel_closure(&mut self, pawn: u32) -> (Vec<u32>, Vec<u32>) {
        let (mut actors, mut objs) = (vec![pawn], Vec::new());
        let mut seen: HashSet<u32> = HashSet::from([pawn]);
        let mut queue = VecDeque::from([pawn]);
        while let Some(o) = queue.pop_front() {
            let Some(class) = self.class_of(o) else { continue };
            for (name, dim) in self.travel_props(class) {
                for i in 0..dim {
                    let Ok(Value::Obj(r)) = self.get_prop(o, &name, i as u32) else { continue };
                    if r == 0 || ref_instance(r).is_none() || !self.is_live(r) || !seen.insert(r) {
                        continue;
                    }
                    if self.is_actor_instance(r) {
                        if self.get_prop(r, "bTravel", 0).ok() == Some(Value::Bool(true)) {
                            actors.push(r);
                            queue.push_back(r);
                        }
                    } else {
                        objs.push(r);
                        queue.push_back(r);
                    }
                }
            }
        }
        (actors, objs)
    }

    pub(crate) fn set_alive_pub(&mut self, r: u32, alive: bool) {
        self.set_alive(r, alive)
    }

    pub(crate) fn set_alive(&mut self, r: u32, alive: bool) {
        if let Some(o) = ref_instance(r).and_then(|i| self.objects.get_mut(i)) {
            o.alive = alive;
        }
    }

    /// Ends an object with the level it belongs to: dead, no state code running.
    pub(crate) fn set_alive_quiet(&mut self, r: u32) {
        if let Some(o) = ref_instance(r).and_then(|i| self.objects.get_mut(i)) {
            o.alive = false;
            o.exec = None;
            o.latent = None;
        }
    }

    /// The game rules object and what it owns (mutators, replication infos...). Each fresh level makes its own (as
    /// the engine does); a restored saved game keeps the running one. Levels, pawns, navigation points and players'
    /// replication infos (the pawns', listed by the `GameReplicationInfo`) reachable from it are not followed.
    pub(crate) fn game_closure(&mut self) -> Vec<u32> {
        let game = match (self.level != 0).then(|| self.get_prop(self.level, "Game", 0).ok()).flatten() {
            Some(Value::Obj(g)) if g != 0 => g,
            _ => return Vec::new(),
        };
        let skip: Vec<crate::ClassId> = ["LevelInfo", "Pawn", "NavigationPoint", "Inventory", "Decoration", "PlayerReplicationInfo"].iter().filter_map(|n| self.reg.class_by_name(n)).collect();
        let (mut out, mut seen) = (vec![game], HashSet::from([game]));
        let mut queue = VecDeque::from([game]);
        while let Some(o) = queue.pop_front() {
            let Some(class) = self.class_of(o) else { continue };
            let props: Vec<(String, usize)> = self.reg.all_props(class).into_iter().filter(|p| matches!(p.kind, ue_package::object::PropertyKind::Object)).map(|p| (self.reg.names.str(p.name).to_string(), p.dim.max(1) as usize)).collect();
            for (name, dim) in props {
                for i in 0..dim {
                    let Ok(Value::Obj(r)) = self.get_prop(o, &name, i as u32) else { continue };
                    if r == 0 || ref_instance(r).is_none() || !self.is_live(r) || !seen.insert(r) {
                        continue;
                    }
                    if self.class_of(r).map(|c| skip.iter().any(|&s| self.is_a(c, s))).unwrap_or(true) {
                        continue;
                    }
                    out.push(r);
                    queue.push_back(r);
                }
            }
        }
        out
    }

    /// Leaves the current level. Everything is dropped except the objects in `keep`; the level object itself stays
    /// until the next level is loaded (a restored saved game takes the game rules object from it). Returns the old
    /// level.
    pub fn unload_level_keeping(&mut self, keep: &HashSet<u32>) -> u32 {
        let old_level = self.level;
        for i in 0..self.objects.len() {
            let r = crate::inst_ref(i);
            if keep.contains(&r) || r == old_level {
                continue;
            }
            let o = &mut self.objects[i];
            o.alive = false;
            o.exec = None;
            o.latent = None;
        }
        self.time = 0.0;
        self.ai.reset();
        // the level's destroyed actors go with it (`CleanupDestroyed(1)` when the level is left)
        self.pending_deleted.clear();
        self.pending_deleted_set.clear();
        // the conversation data objects were just parked with everything else: they are loaded again on demand
        self.con_objects.clear();
        self.con_paths.clear();
        self.con_labels.clear();
        self.bsp = None;
        self.zone_actors.clear();
        self.reach_specs.clear();
        self.surf_info.clear();
        self.drop_collision_hash();
        self.rebuild_actor_slots();
        old_level
    }

    /// `UGameEngine::Init` (103891c0) with a client loads `entry.dx` first (`LoadMap`: its actors, the default game
    /// info, start-up events; no viewport yet, so no player), keeps it as `GEntry` and then browses to the start map.
    /// The original ticks the entry level every frame next to the current one (`UGameEngine::Tick` 1038fe90); this
    /// port keeps one level, so the entry level's objects stop once the start map is loaded: what stays is what
    /// loading it did (object names drawn from the classes' counters, the log). No-op without `Maps/Entry.dx`.
    pub fn load_entry_level(&mut self, maps: &std::path::Path) -> Result<(), VmError> {
        let path = maps.join("Entry.dx");
        if !path.exists() {
            return Ok(());
        }
        let pi = self.reg.add_package(&path, "Entry").map_err(|e| VmError::Runtime(e.to_string()))?;
        self.load_level(pi)?;
        let level = self.unload_level_keeping(&HashSet::new());
        self.set_alive(level, false);
        // the entry level's `LevelInfo` stays the package `Entry`'s (`GetEntryLevel`; the original keeps the level)
        let entry = self.reg.obj_id("Entry").0;
        let _ = self.set_prop(level, "Outer", 0, crate::Value::Obj(entry));
        self.entry_level_info = level;
        Ok(())
    }

    /// Changes the level to package `pi`: the player `old` (if any) is carried into it. Returns the new pawn and the
    /// level's load report. With `old == 0` this is a plain (re)load that logs a fresh player in.
    pub fn travel_level(&mut self, old: u32, pi: usize, portal: &str, class_name: &str, items: bool) -> Result<(u32, crate::level::LevelLoad), VmError> {
        let records = if old != 0 && items { self.travel_export(old) } else { Vec::new() };
        let old_level = self.unload_level_keeping(&HashSet::new());
        let load = self.load_level(pi)?;
        let new_pawn = self.login_player_at(class_name, portal, (old != 0 && items).then_some(&records[..]))?;
        self.set_alive(old_level, false);
        Ok((new_pawn, load))
    }

    /// `DDeusExGameEngine::PruneTravelActors` (DeusEx.dll 1000f9b0), run on the level being left for another map of
    /// the same mission before it is saved: the player's augmentations (`AugmentationSystem.FirstAug`, `Next`) and
    /// their manager, its skills (`SkillSystem.FirstSkill`, `Next`) and their manager, its flag base and its carried
    /// decoration are destroyed and the references cleared. They travel with the player, so the saved level must not
    /// keep them. Here the level is only captured (`make_snapshot_excluding`) and then dropped, and the player's own
    /// objects are still needed for the travel, so this returns what the capture leaves out instead.
    pub fn prune_travel_actors(&mut self, player: u32) -> HashSet<u32> {
        let mut out = HashSet::new();
        let obj = |vm: &mut Vm, r: u32, name: &str| match vm.get_prop(r, name, 0) {
            Ok(Value::Obj(o)) if o != 0 && vm.is_live(o) => Some(o),
            _ => None,
        };
        for (manager, first, next) in [("AugmentationSystem", "FirstAug", "Next"), ("SkillSystem", "FirstSkill", "Next")] {
            if let Some(m) = obj(self, player, manager) {
                let mut item = obj(self, m, first);
                while let Some(i) = item {
                    if !out.insert(i) {
                        break;
                    }
                    item = obj(self, i, next);
                }
                out.insert(m);
            }
        }
        for field in ["FlagBase", "carriedDecoration"] {
            if let Some(o) = obj(self, player, field) {
                out.insert(o);
            }
        }
        out
    }

    /// Travel to a map of the same mission visited before: `DDeusExGameEngine::Browse` (DeusEx.dll 10010b90) finds
    /// `<Save>\Current\<map>.dxs` and browses to it instead of the map, so the level comes back as it was left
    /// (`snap`) and `UGameEngine::LoadMap` starts it as a saved level (`restore_snapshot`). The player then logs in
    /// as after any travel: `GameInfo.Login` takes the unoccupied pawn the level kept, `DeusExGameInfo.Login` resets it
    /// and puts it at the portal, and what the player carries comes along (`travel_level`).
    pub fn revisit_level(&mut self, old: u32, pi: usize, snap: &crate::save::Snapshot, portal: &str, class_name: &str, items: bool) -> Result<(u32, crate::level::LevelLoad), VmError> {
        let records = if items { self.travel_export(old) } else { Vec::new() };
        let old_level = self.unload_level_keeping(&HashSet::new());
        let load = self.load_level_opts(pi, false)?;
        // the random numbers are the C runtime's `rand()`, global to the process and not in a saved level: they go on
        // (a loaded game restores them, by the port's choice, to continue like the session that saved it)
        self.restore_snapshot_opts(snap, false)?;
        let new_pawn = self.login_player_at(class_name, portal, items.then_some(&records[..]))?;
        if old_level != self.level {
            self.set_alive(old_level, false);
        }
        Ok((new_pawn, load))
    }


    /// The `travel` properties of `class` in the order the engine's `TFieldIterator` gives them: the class's own
    /// fields first, then each super class's.
    fn travel_defs(&self, class: crate::ClassId) -> Vec<PropDef> {
        self.reg.chain(class).into_iter().rev().flat_map(|c| self.reg.class(c).props.iter()).filter(|p| p.flags & CPF_TRAVEL != 0).cloned().collect()
    }

    /// The travel export of `UGameEngine` (Engine 10391740), made for a player pawn with a `Player` when it travels
    /// with its items (`ClientTravel`'s `bItems`; nothing travels otherwise): a worklist from the pawn; each object
    /// on it that is not an actor, or an actor with `bTravel`, becomes a block (`Class=.. Name=.. Ptr=..`) of its
    /// `travel` properties whose value differs from the class defaults (`ExportText` against them), as text (newlines
    /// written as `\`), object references as the old pointer, whose object joins the worklist.
    pub fn travel_export(&mut self, pawn: u32) -> Vec<TravelRecord> {
        let mut list = vec![pawn];
        let mut out = Vec::new();
        let mut k = 0;
        while k < list.len() {
            let o = list[k];
            k += 1;
            let (Some(i), Some(class)) = (ref_instance(o), self.class_of(o)) else { continue };
            if !self.is_live(o) || (self.is_actor_class(class) && self.get_prop(o, "bTravel", 0).ok() != Some(Value::Bool(true))) {
                continue;
            }
            if self.class_default_mem(class).is_err() {
                continue;
            }
            let mut values = Vec::new();
            for def in self.travel_defs(class) {
                for e in 0..def.dim.max(1) {
                    let off = (def.offset + e * def.elem_size) as usize;
                    let now = self.read_prop(&self.objects[i].mem, &def, off);
                    let dflt = self.read_prop(&self.defaults[&class], &def, off);
                    if let (Ok(a), Ok(b)) = (&now, &dflt) {
                        if a == b {
                            continue;
                        }
                    }
                    if matches!(def.kind, PropertyKind::Object | PropertyKind::Class) {
                        let r = match now {
                            Ok(Value::Obj(r)) => r,
                            _ => 0,
                        };
                        values.push((def.clone(), e, TravelValue::Ptr(r)));
                        if r != 0 && ref_instance(r).is_some() && !list.contains(&r) {
                            list.push(r);
                        }
                    } else {
                        let t = self.export_item(Sel::Obj(i), &def, off, 0).replace('\n', "\\");
                        values.push((def.clone(), e, TravelValue::Text(t)));
                    }
                }
            }
            out.push(TravelRecord { old: o, class, values });
        }
        out
    }

    /// The travel import of `ULevel::SpawnPlayActor` (Engine 10396bd0), after `Login` gave `new_pawn`: each block
    /// makes its object (a player pawn's is `new_pawn`; another actor is spawned with the pawn as owner, at its
    /// location and rotation, without a collision test; anything else is constructed), its lines are imported in
    /// order (`ImportText`, `\\` back to newlines), object references wait for the end and then take the new object of
    /// the block with that pointer (none: the slot keeps what the object made). The pawn's own subsystems made by
    /// `Login` stay in the level when the carried ones replace them. Returns the objects made, in block order.
    pub(crate) fn import_travel(&mut self, new_pawn: u32, records: &[TravelRecord]) -> Result<Vec<u32>, VmError> {
        let player_pawn = self.reg.class_by_name("PlayerPawn");
        let loc = self.location(new_pawn);
        let rot = self.rotation(new_pawn);
        let mut made: Vec<(u32, u32)> = Vec::new();
        for r in records {
            let new = if player_pawn.map(|c| self.is_a(r.class, c)).unwrap_or(false) {
                new_pawn
            } else if self.is_actor_class(r.class) {
                match self.spawn(r.class, new_pawn, None, Some(loc), Some(rot))? {
                    Some(n) => n,
                    None => {
                        self.log.push(format!("Failed to import travelling actor of class {}", self.reg.class(r.class).path));
                        continue;
                    }
                }
            } else {
                self.new_object(r.class)?
            };
            made.push((r.old, new));
        }
        let new_of = |old: u32, made: &[(u32, u32)]| made.iter().find(|m| m.0 == old).map(|m| m.1);
        let mut fixups: Vec<(usize, PropDef, usize, u32)> = Vec::new();
        for r in records {
            let Some(new) = new_of(r.old, &made) else { continue };
            let Some(i) = ref_instance(new) else { continue };
            for (def, e, v) in &r.values {
                let off = (def.offset + e * def.elem_size) as usize;
                match v {
                    TravelValue::Text(t) => {
                        let t = t.replace('\\', "\n");
                        let _ = self.import_item(Sel::Obj(i), def, off, &t, 0);
                    }
                    TravelValue::Ptr(p) => fixups.push((i, def.clone(), off, *p)),
                }
            }
            // the flag base keeps its flags natively here; the original's flag objects travel as text (`%f` floats)
            if let Some(store) = self.flag_stores.remove(&r.old) {
                self.flag_stores.insert(new, crate::native_ext::travel_flags(store));
            }
        }
        for (i, def, off, p) in fixups {
            if let Some(n) = new_of(p, &made) {
                self.sel_write(Sel::Obj(i), &def, off, Value::Obj(n));
            }
        }
        Ok(made.into_iter().map(|m| m.1).collect())
    }
}

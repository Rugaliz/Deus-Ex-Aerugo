//! Level travel: leaving the current map for another one and carrying the player along.
//!
//! The original engine exports the player pawn's `travel` properties (and, through them, the actors with `bTravel` and
//! the objects they point to) to text, loads the next map, logs a fresh pawn in and imports the text into it. Objects
//! that are not actors are kept alive here (the new pawn simply refers to them); actors are re-spawned in the new level
//! and receive the old ones' travel properties, with references to travelling actors pointing at the new copies.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::{ref_instance, Value, Vm, VmError};

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
        let (mut actors, objs) = if old != 0 { self.travel_closure(old) } else { (Vec::new(), Vec::new()) };
        if !items {
            // only the pawn and what it refers to without being an inventory item
            actors.retain(|&a| a == old || !self.class_of(a).map(|c| self.reg.class_by_name("Inventory").map(|i| self.is_a(c, i)).unwrap_or(false)).unwrap_or(false));
        }
        // keep the non-actor objects alive; the travelling actors are parked (dead, memory intact) until their copies exist
        let keep: HashSet<u32> = objs.iter().copied().collect();
        let old_level = self.unload_level_keeping(&keep);
        let load = self.load_level(pi)?;
        let new_pawn = self.login_player_at(class_name, portal, (old != 0).then_some((old, &actors[..])))?;
        for r in &actors {
            self.set_alive(*r, false);
        }
        self.set_alive(old_level, false);
        Ok((new_pawn, load))
    }

    /// A property of a class's default object.
    fn class_default_value(&mut self, class: crate::ClassId, name: &str, elem: u32) -> Option<Value> {
        let def = self.reg.find_prop(class, name)?.clone();
        if elem >= def.dim {
            return None;
        }
        self.class_default_mem(class).ok()?;
        let mem = self.defaults.get(&class)?;
        self.read_prop(mem, &def, (def.offset + elem * def.elem_size) as usize).ok()
    }

    /// Copies the travel properties of the old actors into fresh ones: the pawn is `new_pawn`, the others are spawned.
    pub(crate) fn import_travel(&mut self, new_pawn: u32, actors: &[u32]) -> Result<Vec<u32>, VmError> {
        let mut map: HashMap<u32, u32> = HashMap::new();
        map.insert(actors[0], new_pawn);
        let loc = self.location(new_pawn);
        for &old in &actors[1..] {
            let Some(class) = self.class_of(old) else { continue };
            if let Some(n) = self.spawn(class, 0, None, Some(loc), None)? {
                map.insert(old, n);
            }
        }
        // the new pawn's own travel actors (made by its `PostBeginPlay`) are replaced by the carried ones
        let mut replaced = Vec::new();
        if let Some(class) = self.class_of(new_pawn) {
            for (name, dim) in self.travel_props(class) {
                for i in 0..dim {
                    if let Ok(Value::Obj(r)) = self.get_prop(new_pawn, &name, i as u32) {
                        if r != 0 && ref_instance(r).is_some() && self.is_live(r) && self.is_actor_instance(r) && !map.values().any(|&v| v == r) && self.get_prop(r, "Owner", 0).ok() == Some(Value::Obj(new_pawn)) {
                            replaced.push(r);
                        }
                    }
                }
            }
        }
        let pairs: Vec<(u32, u32)> = map.iter().map(|(&o, &n)| (o, n)).collect();
        for (old, new) in pairs {
            self.set_alive(old, true);
            let Some(class) = self.class_of(old) else { continue };
            for (name, dim) in self.travel_props(class) {
                for i in 0..dim {
                    let Ok(mut v) = self.get_prop(old, &name, i as u32) else { continue };
                    // the engine exports only what differs from the class defaults: a `None` reference leaves what the new
                    // pawn made for itself (the player's augmentation system after `ResetPlayer`), a value at its default
                    // leaves what the fresh actor's `PreBeginPlay` set (the pistol skill's level 1 after `ResetSkills`)
                    if v == Value::Obj(0) || self.class_default_value(class, &name, i as u32).as_ref() == Some(&v) {
                        continue;
                    }
                    if let Value::Obj(r) = v {
                        if r != 0 && ref_instance(r).is_some() && self.is_actor_instance(r) {
                            v = Value::Obj(map.get(&r).copied().unwrap_or(0));
                        }
                    }
                    let _ = self.set_prop(new, &name, i as u32, v);
                }
            }
            if new != new_pawn {
                let owner = match self.get_prop(old, "Owner", 0) {
                    Ok(Value::Obj(o)) => map.get(&o).copied().unwrap_or(0),
                    _ => 0,
                };
                let _ = self.set_prop(new, "Owner", 0, Value::Obj(owner));
            }
            self.set_alive(old, false);
        }
        // what the new pawn still refers to (a `None` was not carried over) stays
        let mut kept: Vec<u32> = Vec::new();
        if let Some(class) = self.class_of(new_pawn) {
            for (name, dim) in self.travel_props(class) {
                for i in 0..dim {
                    if let Ok(Value::Obj(r)) = self.get_prop(new_pawn, &name, i as u32) {
                        kept.push(r);
                    }
                }
            }
        }
        for r in replaced {
            if !kept.contains(&r) {
                let _ = self.destroy(r);
            }
        }
        let mut order = vec![new_pawn];
        order.extend(actors[1..].iter().filter_map(|a| map.get(a).copied()));
        Ok(order)
    }
}

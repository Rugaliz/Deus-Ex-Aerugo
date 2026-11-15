//! Reading a saved game made by the ORIGINAL game (`Save/<slot>/<map>.dxs`, format notes in `docs/formats/save-format.md`).
//! The file is a package holding the whole level: its actors (the player, its inventory, augmentations and skills, the game
//! rules objects, every pawn and decoration with the properties that differ from the defaults and the state they were in),
//! plus the non-actor objects the game keeps (`FlagBase` and its flags). The import loads it as the current level, links the
//! saved objects together, rebuilds the flag store, puts every actor back in its state (its code starts again from the top of
//! that state; the exact position in the code is not restored) and lets the player re-enter through a viewport and `Possess`.
//! Writing such files is not supported.

use std::collections::{HashMap, HashSet};

use crate::native_ext::{Flag, FlagStore};
use crate::{Value, Vm, VmError};
use ue_package::ObjectRef;

type Res<T> = Result<T, VmError>;

/// What the import found and could not carry over.
#[derive(Debug, Default)]
pub struct ImportReport {
    /// Flags put into the flag store.
    pub flags: usize,
    /// Notes, goals, logs, conversation history and key records brought back.
    pub list_objects: usize,
    /// Actors put back into the state they were saved in.
    pub states: usize,
    /// States that could not be entered (name, error).
    pub state_problems: Vec<String>,
    /// Saved objects whose class is not in the scripts (native-only classes: AI event records, the event manager).
    pub unknown_classes: Vec<String>,
}

impl Vm {
    /// Makes the saved game in package `pi` the current world. Returns the player pawn, the level's load report and what the
    /// import did.
    pub fn import_original_save(&mut self, pi: usize) -> Res<(u32, crate::level::LevelLoad, ImportReport)> {
        let rt = |s: String| VmError::Runtime(s);
        let mut report = ImportReport::default();
        let old_game = if self.level != 0 { self.get_prop(self.level, "Game", 0).ok() } else { None };
        let old_level = self.unload_level_keeping(&HashSet::new());
        let load = self.load_level_with(pi, false, true)?;
        let lvl = self.level;
        if old_level != lvl {
            self.set_alive_pub(old_level, false);
        }

        // path id -> instance, for the actors and the non-actor objects below
        let mut by_path: HashMap<u32, u32> = HashMap::new();
        for (&r, p) in &self.level_paths {
            by_path.insert(self.reg.obj_id(p).0, r);
        }

        // 1. the non-actor objects: the flag base and its flags
        let (flag_exports, base_exports) = {
            let pkg = &self.reg.packages[pi].pkg;
            let mut flags = Vec::new();
            let mut bases = Vec::new();
            for i in 0..pkg.exports.len() {
                match pkg.export_class_name(i).as_str() {
                    c @ ("FlagBool" | "FlagByte" | "FlagInt" | "FlagFloat" | "FlagName" | "FlagVector" | "FlagRotator") => flags.push((i, c.to_string())),
                    "FlagBase" => bases.push(i),
                    _ => {}
                }
            }
            (flags, bases)
        };
        let export_path = |vm: &mut Vm, i: usize| -> Option<u32> {
            let p = vm.reg.ref_path(pi, i as i32 + 1)?;
            Some(vm.reg.obj_id(&p).0)
        };
        let mut stores: HashMap<u32, u32> = HashMap::new(); // FlagBase path id -> instance
        for &i in &base_exports {
            let class = self.reg.class_of_export(pi, i).ok_or_else(|| rt("FlagBase class not found".into()))?;
            let (inst, _) = self.instantiate_export(class, pi, i).map_err(|e| rt(e.to_string()))?;
            let r = self.adopt(class, inst.data, inst.strings);
            if let Some(id) = export_path(self, i) {
                by_path.insert(id, r);
                stores.insert(id, r);
            }
            let mut store = FlagStore::default();
            store.default_expiration = self.get_prop(r, "defaultFlagExpiration", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0);
            self.flag_stores.insert(r, store);
        }
        for (i, kind) in flag_exports {
            let class = self.reg.class_of_export(pi, i).ok_or_else(|| rt("flag class not found".into()))?;
            let (inst, _) = self.instantiate_export(class, pi, i).map_err(|e| rt(e.to_string()))?;
            let r = self.adopt(class, inst.data, inst.strings);
            let (ty, prop) = match kind.as_str() {
                "FlagBool" => (0u8, "bValue"),
                "FlagByte" => (1, "byteValue"),
                "FlagInt" => (2, "intValue"),
                "FlagFloat" => (3, "floatValue"),
                "FlagName" => (4, "nameValue"),
                "FlagVector" => (5, "vectorValue"),
                _ => (6, "rotatorValue"),
            };
            let value = self.get_prop(r, prop, 0).unwrap_or(Value::Void);
            let name = match self.get_prop(r, "flagName", 0) {
                Ok(Value::Name(n)) => n,
                _ => 0,
            };
            let expiration = self.get_prop(r, "expiration", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0);
            // the flag knows its base by reference; with a single base every flag belongs to it
            let base_path = match self.get_prop(r, "flagBase", 0) {
                Ok(Value::Obj(b)) => b,
                _ => 0,
            };
            self.set_alive_pub(r, false);
            let Some(&store) = stores.get(&base_path).or_else(|| stores.values().next()) else { continue };
            let key = self.reg.names.str(name).to_ascii_lowercase();
            self.flag_stores.entry(store).or_default().add(Flag { name, key, ty, value, expiration });
            report.flags += 1;
        }

        // the lists the player keeps (notes, goals, logs, conversation history, key info) are objects outside the actor table
        let list_exports: Vec<usize> = {
            let pkg = &self.reg.packages[pi].pkg;
            (0..pkg.exports.len()).filter(|&i| ["ConHistory", "ConHistoryEvent", "DataVaultImageNote", "DeusExGoal", "DeusExLog", "DeusExNote", "NanoKeyInfo"].contains(&pkg.export_class_name(i).as_str())).collect()
        };
        let mut list_objects = Vec::new();
        for i in list_exports {
            let Some(class) = self.reg.class_of_export(pi, i) else { continue };
            let Ok((inst, _)) = self.instantiate_export(class, pi, i) else { continue };
            let r = self.adopt(class, inst.data, inst.strings);
            if let Some(id) = export_path(self, i) {
                by_path.insert(id, r);
            }
            list_objects.push(r);
            report.list_objects += 1;
        }

        // 2. references to the non-actor objects become instance references
        for &r in load.actors.iter().chain(list_objects.iter()) {
            self.rewrite_refs(r, &by_path);
        }

        // 3. the saved game rules object replaces the running one
        let game_class = self.reg.class_by_name("GameInfo");
        let saved_game = load.actors.iter().copied().find(|&a| match (self.class_of(a), game_class) {
            (Some(c), Some(g)) => self.is_a(c, g),
            _ => false,
        });
        if let Some(g) = saved_game {
            self.set_prop(lvl, "Game", 0, Value::Obj(g))?;
            if let Some(Value::Obj(old)) = old_game {
                if old != g {
                    self.set_alive_pub(old, false);
                }
            }
        }

        // 4. the player
        let player_class = self.reg.class_by_name("PlayerPawn");
        let pawn = load
            .actors
            .iter()
            .copied()
            .find(|&a| match (self.class_of(a), player_class) {
                (Some(c), Some(p)) => self.is_a(c, p) && self.is_live(a),
                _ => false,
            })
            .ok_or_else(|| rt("the saved game has no player".into()))?;

        // 5. every actor goes back into the state it was saved in
        let state_of: Vec<(u32, u32)> = {
            let mut v = Vec::new();
            for &a in &load.actors {
                let Some(path) = self.level_paths.get(&a).cloned() else { continue };
                // the export of this actor: found again by path
                let pkg_name = self.reg.packages[pi].name.clone();
                let Some(rest) = path.strip_prefix(&format!("{pkg_name}.")) else { continue };
                let export = {
                    let pkg = &self.reg.packages[pi].pkg;
                    (0..pkg.exports.len()).find(|&i| pkg.object_path(ObjectRef::Export(i)).as_deref() == Some(rest))
                };
                let Some(export) = export else { continue };
                let node = self.reg.packages[pi].pkg.stack_header(export).map(|h| h.state_node).unwrap_or(0);
                if node != 0 {
                    v.push((a, node as u32));
                }
            }
            v
        };
        for (a, node) in state_of {
            let Some(path) = self.reg.ref_path(pi, node as i32) else { continue };
            let Some(name) = path.rsplit('.').next() else { continue };
            let id = self.reg.names.intern(name);
            match self.goto_state(a, Some(id), None) {
                Ok(()) => report.states += 1,
                Err(e) => report.state_problems.push(format!("{}: {e}", name)),
            }
        }

        // 6. what the engine rebuilds after loading
        self.player_cache = 0;
        let t = self.get_prop(lvl, "TimeSeconds", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0);
        self.time = t;
        // `LoadMap`'s last pass (`PostPostBeginPlay` for every actor: conversation bindings, mission timers...) and the
        // sorted actor list
        self.rebuild_actor_slots();
        let mut load = load;
        load.issues.extend(self.finish_level_start(true));

        // 7. the viewport's player, as `LoadMap` does it after its start-up passes (`SpawnPlayActor`; `Login` takes the
        // saved pawn)
        let player = self.login_saved_player()?;
        if player != pawn {
            self.log.push(format!("import: Login gave a new player {player}, not the saved one {pawn}"));
        }
        Ok((player, load, report))
    }
}

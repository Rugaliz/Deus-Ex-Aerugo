//! Saving and loading games for a session (the snapshot itself is `ue_vm::save`).
use crate::Session;
use ue_vm::save::Snapshot;
use ue_vm::save_dir::{now_civil, slot_name, SaveMeta};
use ue_vm::Value;

impl Session {
    /// Where saved games go: `Save/` inside the per-user configuration directory (never the game folder).
    pub fn default_save_dir() -> Option<std::path::PathBuf> {
        crate::config_dir().map(|d| d.join("Save"))
    }

    /// Writes the current game to slot `index`: 0 = a new slot, -1 = the quick save. Returns the slot used.
    pub fn save_game(&mut self, index: i32, description: &str) -> Result<i32, String> {
        let dir = self.vm.save_dir.clone().ok_or("no folder for saved games")?;
        let index = if index == 0 { self.vm.save_slots().last().copied().unwrap_or(0) + 1 } else { index };
        let slot = dir.join(slot_name(index));
        // the count of saves of this game goes into the file
        let count = self.vm.get_prop(self.player, "saveCount", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0) + 1;
        let _ = self.vm.set_prop(self.player, "saveCount", 0, Value::Int(count));
        // `DDeusExGameEngine::SaveCurrentLevel` (DeusEx 1000fac0) forces `CleanupDestroyed(1)` first: the running game
        // goes on without the references to destroyed actors
        self.vm.cleanup_destroyed(true);
        let snap = self.vm.make_snapshot(&self.map_name, self.player).map_err(|e| e.to_string())?;
        let location = self.level_info_text("MissionLocation");
        let meta = SaveMeta {
            description: description.to_string(),
            mission_location: location,
            map_name: self.map_name.clone(),
            time: now_civil(),
            save_count: count,
            save_time: self.vm.get_prop(self.player, "saveTime", 0).ok().and_then(|v| v.as_f32()).unwrap_or(0.0) as i32,
            cheats: matches!(self.vm.get_prop(self.player, "bCheatsEnabled", 0), Ok(Value::Bool(true))),
        };
        std::fs::create_dir_all(&slot).map_err(|e| e.to_string())?;
        // written to a temporary name first so that a failed save never destroys the old one
        let tmp = slot.join("save.dxs.tmp");
        std::fs::write(&tmp, snap.encode()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, slot.join("save.dxs")).map_err(|e| e.to_string())?;
        std::fs::write(slot.join("info.txt"), meta.to_text()).map_err(|e| e.to_string())?;
        // the mission's levels left before (`SaveGame` copies the `Current` folder into the slot)
        let current = slot.join("Current");
        let _ = std::fs::remove_dir_all(&current);
        if !self.current_levels.is_empty() {
            std::fs::create_dir_all(&current).map_err(|e| e.to_string())?;
            for (map, snap) in &self.current_levels {
                std::fs::write(current.join(format!("{map}.dxs")), snap.encode()).map_err(|e| e.to_string())?;
            }
        }
        // `DDeusExGameEngine::SaveGame`: the saved game's picture (`SaveInfo.Snapshot`) unless the render device is
        // OpenGL: the root's snapshot size set to 160 x 120, the greyscale picture of the frame on screen (written once
        // the host has taken it)
        let _ = std::fs::remove_file(slot.join(ue_vm::snapshot::SAVE_FILE));
        let device = self.vm.reg.config_value("System", "Engine.Engine", "GameRenderDevice").unwrap_or_default();
        let root = self.vm.ui_root(self.player);
        if root != 0 && !device.trim().eq_ignore_ascii_case("OpenGLDrv.OpenGLRenderDevice") {
            self.vm.set_snapshot_size(root, 160.0, 120.0);
            self.vm.request_snapshot(root, Some(slot.join(ue_vm::snapshot::SAVE_FILE)));
        }
        Ok(index)
    }

    fn level_info_text(&mut self, field: &str) -> String {
        let Some(c) = self.vm.reg.class_by_name("DeusExLevelInfo") else { return String::new() };
        let Some(info) = self.vm.live_actors().into_iter().find(|&a| self.vm.class_of(a).map(|k| self.vm.is_a(k, c)).unwrap_or(false)) else { return String::new() };
        match self.vm.get_prop(info, field, 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        }
    }

    /// Loads saved game `index` (-1 = the quick save).
    pub fn load_game(&mut self, index: i32) -> Result<(), String> {
        let slot = self.vm.save_slot_dir(index).ok_or("no folder for saved games")?;
        let bytes = std::fs::read(slot.join("save.dxs")).map_err(|e| format!("cannot read the saved game: {e}"))?;
        let snap = Snapshot::decode(&bytes).map_err(|e| e.to_string())?;
        // `Browse` on `?loadgame=N`: the `Current` folder is emptied and the slot's levels copied into it
        let mut current = std::collections::HashMap::new();
        if let Ok(dir) = std::fs::read_dir(slot.join("Current")) {
            for e in dir.flatten() {
                let path = e.path();
                let Some(map) = path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_lowercase()) else { continue };
                if path.extension().and_then(|x| x.to_str()) != Some("dxs") {
                    continue;
                }
                let bytes = std::fs::read(&path).map_err(|e| format!("cannot read the saved game: {e}"))?;
                current.insert(map, Snapshot::decode(&bytes).map_err(|e| e.to_string())?);
            }
        }
        self.load_snapshot(&snap)?;
        self.current_levels = current;
        Ok(())
    }

    pub fn load_snapshot(&mut self, snap: &Snapshot) -> Result<(), String> {
        let path = self.find_map_file(&snap.map).ok_or_else(|| format!("the saved game's map {} is missing", snap.map))?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(&snap.map).to_string();
        let pi = self.vm.reg.add_package(&path, &stem).map_err(|e| e.to_string())?;
        let view = ue_assets_view(&self.game, &stem)?;
        let (pawn, load) = self.vm.load_saved_game(pi, snap).map_err(|e| e.to_string())?;
        self.view = view;
        self.map_pkg = pi;
        self.forget_level();
        self.player = pawn;
        self.issues.extend(load.issues);
        self.map_name = stem;
        self.map_changed = true;
        self.vm.ui_after_browse(self.player);
        self.use_configured_fov();
        if let Ok(Value::Rotator(r)) = self.vm.get_prop(pawn, "ViewRotation", 0) {
            self.yaw = r[1] & 0xFFFF;
            self.pitch = if r[0] & 0xFFFF > 32768 { (r[0] & 0xFFFF) - 65536 } else { r[0] & 0xFFFF };
        }
        if let Some(mut a) = self.audio.take() {
            a.level_changed(&mut self.vm);
            self.audio = Some(a);
        }
        Ok(())
    }

    /// The field of view is a setting (Display screen, `DefaultFOV` in the player's ini), not part of a saved game: a
    /// loaded player gets the configured one. A saved `DefaultFOV` that differs from the class default would otherwise
    /// stay, and every reset of the view to `Default.DesiredFOV` (the end of the drug effect, a scope put away, loading)
    /// would leave `DesiredFOV != DefaultFOV`, which hides the weapon in hand for good (`Weapon.RenderOverlays`): the
    /// original's bug with a changed field of view. A view zoomed at the save (`DesiredFOV` not the saved default)
    /// stays zoomed.
    fn use_configured_fov(&mut self) {
        let p = self.player;
        let Some(class) = self.vm.class_of(p).map(|c| self.vm.reg.names.str(self.vm.reg.class(c).name).to_string()) else { return };
        let Some(want) = self.vm.default_value_of(&class, "DefaultFOV", 0).and_then(|v| v.as_f32()) else { return };
        let saved = self.vm.get_prop(p, "DefaultFOV", 0).ok().and_then(|v| v.as_f32());
        if saved == Some(want) {
            return;
        }
        for key in ["DesiredFOV", "FOVAngle"] {
            if self.vm.get_prop(p, key, 0).ok().and_then(|v| v.as_f32()) == saved {
                let _ = self.vm.set_prop(p, key, 0, Value::Float(want));
            }
        }
        let _ = self.vm.set_prop(p, "DefaultFOV", 0, Value::Float(want));
    }

    /// Continues a game saved by the ORIGINAL game (`<map>.dxs` of a save slot, see `docs/formats/save-format.md`). The map
    /// is found by the file's name. Returns what the import carried over.
    pub fn import_original_save(&mut self, file: &std::path::Path) -> Result<ue_vm::import_save::ImportReport, String> {
        let stem = file.file_stem().and_then(|s| s.to_str()).ok_or("bad file name")?.to_string();
        let map_file = self.find_map_file(&stem).ok_or_else(|| format!("the map {stem} of the saved game is missing"))?;
        let map_stem = map_file.file_stem().and_then(|s| s.to_str()).unwrap_or(&stem).to_string();
        let pi = self.vm.reg.add_package(file, &format!("{map_stem}_save")).map_err(|e| e.to_string())?;
        let view = ue_assets_view(&self.game, &map_stem)?;
        let (pawn, load, report) = self.vm.import_original_save(pi).map_err(|e| e.to_string())?;
        self.view = view;
        self.map_pkg = pi;
        self.forget_level();
        self.player = pawn;
        self.issues.extend(load.issues);
        self.map_name = map_stem;
        self.map_changed = true;
        self.vm.ui_after_browse(self.player);
        self.use_configured_fov();
        if let Ok(Value::Rotator(r)) = self.vm.get_prop(pawn, "ViewRotation", 0) {
            self.yaw = r[1] & 0xFFFF;
            self.pitch = if r[0] & 0xFFFF > 32768 { (r[0] & 0xFFFF) - 65536 } else { r[0] & 0xFFFF };
        }
        if let Some(mut a) = self.audio.take() {
            a.level_changed(&mut self.vm);
            self.audio = Some(a);
        }
        Ok(report)
    }

    /// Deletes saved game `index` (the `DeleteGame` console command of the load screen).
    pub fn delete_save(&mut self, index: i32) {
        if let Some(d) = self.vm.save_slot_dir(index) {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    pub(crate) fn find_map_file(&self, map: &str) -> Option<std::path::PathBuf> {
        let maps = self.game.join("Maps");
        // a map of the mods folder (it replaces or adds one): opened by name, wherever it is (`Registry::add_package`)
        if ue_package::mods::package(map).is_some_and(|m| m.ext == "dx") {
            return Some(maps.join(format!("{map}.dx")));
        }
        std::fs::read_dir(&maps)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case(map)).unwrap_or(false) && p.extension().map(|x| x.eq_ignore_ascii_case("dx")).unwrap_or(false))
    }

    /// What the scripts asked for between frames: a save (`SaveGame`).
    pub(crate) fn perform_save_requests(&mut self) {
        if let Some((index, desc)) = self.vm.save_request.take() {
            if let Err(e) = self.save_game(index, &desc) {
                self.issues.push(crate::LoadIssue { actor: self.player, class: "save".into(), event: "SaveGame".into(), error: e });
            }
        }
    }
}

fn ue_assets_view(game: &std::path::Path, stem: &str) -> Result<dx_soft::MapView, String> {
    dx_soft::MapView::open(game, stem)
}

//! The engine's player start-up flow (`ULevel::SpawnPlayActor`): `GameInfo.Login` spawns the pawn at a start spot, the
//! viewport (`Player`) is attached, the pawn is possessed and the game is told with `PostLogin`.

use crate::{Value, Vm, VmError};

/// `ShowFlags` that `ULevel::SpawnPlayActor` gives the local player; bit 0x800 (`SHOW_PlayerCtrl`) makes the actor tick
/// read the viewport's input and run `PlayerInput`/`PlayerTick`.
pub const PLAYER_SHOW_FLAGS: i32 = 0x480c;
const SHOW_PLAYER_CTRL: i32 = 0x800;

/// The local player's input waiting for its tick slot.
#[derive(Debug, Clone)]
pub enum SlotInput {
    /// Axis variables from the bindings (`aBaseY`, `aMouseX`, ...): `PlayerInput` then `PlayerTick`.
    Bound(Vec<(String, f32)>),
    /// Tools and tests: `aForward`/`aStrafe` set directly, `PlayerTick` only.
    Direct { forward: f32, strafe: f32 },
}

impl Vm {
    /// Logs a player of `class_name` (the game may substitute its own class, as `DeusExGameInfo` does) into the loaded
    /// level and returns the pawn. Needs a loaded level whose `Game` object exists.
    pub fn login_player(&mut self, class_name: &str) -> Result<u32, VmError> {
        self.login_player_at(class_name, "", None)
    }

    /// Gives a pawn its viewport (a `Player` object that points at it; the window/input layer lives in the host) and the
    /// console (`[Engine.Engine] Console=`): menus and typed commands go through it.
    pub fn attach_viewport(&mut self, pawn: u32) -> Result<(), VmError> {
        self.link_viewport(pawn)?;
        self.set_play_actor_flags(pawn)
    }

    /// What `ULevel::SpawnPlayActor` (10396bd0) sets after `SetPlayer`: `Role` authority, `RemoteRole` (a local
    /// viewport: simulated proxy), `ShowFlags` 0x480c, `RendMap` 5 (dynamic light), `bAdmin` (no net driver).
    fn set_play_actor_flags(&mut self, pawn: u32) -> Result<(), VmError> {
        self.set_prop(pawn, "Role", 0, Value::Byte(4))?;
        self.set_prop(pawn, "RemoteRole", 0, Value::Byte(2))?;
        self.set_prop(pawn, "ShowFlags", 0, Value::Int(PLAYER_SHOW_FLAGS))?;
        self.set_prop(pawn, "RendMap", 0, Value::Int(5))?;
        self.set_prop(pawn, "bAdmin", 0, Value::Bool(true))?;
        Ok(())
    }

    /// The viewport (`Player`) and console objects, linked to the pawn (the links of `APlayerPawn::SetPlayer`).
    fn link_viewport(&mut self, pawn: u32) -> Result<(), VmError> {
        let rt = |s: String| VmError::Runtime(s);
        let player_class = self.reg.class_by_name("Player").ok_or_else(|| rt("login: no Player class".into()))?;
        let viewport = self.new_object(player_class)?;
        self.set_prop(viewport, "Actor", 0, Value::Obj(pawn))?;
        self.set_prop(pawn, "Player", 0, Value::Obj(viewport))?;
        let console_class = self.reg.config_value("System", "Engine.Engine", "Console").map(|c| ue_config::unquote(&c)).and_then(|p| self.reg.class_by_path(&p));
        if let Some(cc) = console_class {
            let console = self.new_object(cc)?;
            self.set_prop(console, "Viewport", 0, Value::Obj(viewport))?;
            self.set_prop(viewport, "Console", 0, Value::Obj(console))?;
        }
        Ok(())
    }

    /// The options of the URL a level is opened with, as `LoadMap` and `SpawnPlayActor` join them for `InitGame` and
    /// `Login` (`?Key=Value` each): the `[DefaultPlayer]` section of `User.ini` (`FURL::LoadURLConfig`), then the
    /// options set by `UpdateURL` (`FURL::AddOption` replaces a key in place).
    pub fn url_options_string(&mut self) -> String {
        let mut ops: Vec<(String, String)> = self.reg.config_section("User", "DefaultPlayer");
        for (k, v) in self.url_options.clone() {
            match ops.iter_mut().find(|(ok, _)| ok.eq_ignore_ascii_case(&k)) {
                Some(e) => e.1 = v,
                None => ops.push((k, v)),
            }
        }
        ops.iter().map(|(k, v)| format!("?{k}={v}")).collect()
    }

    /// `login_player` with a portal (the `#Name` of a travel URL: which `PlayerStart` to use) and, when the player comes
    /// from another level, the old pawn and its travelling actors (`travel_closure`) whose properties the new pawn takes
    /// over. In `ULevel::SpawnPlayActor`'s order (10396bd0): `Login`; `SetPlayer` (viewport links, `Possess`); the
    /// role and show flags; the pawn's `TravelPreAccept`; the travelling actors made; their `TravelPreAccept` from the
    /// last to the first; `AcceptInventory`; their `TravelPostAccept` from the last to the first; the pawn's
    /// `TravelPostAccept`; `PostLogin`.
    pub fn login_player_at(&mut self, class_name: &str, portal: &str, travel: Option<&[crate::travel::TravelRecord]>) -> Result<u32, VmError> {
        self.spawn_play_actor(class_name, portal, travel, "")
    }

    /// The player of a level loaded from a saved game. `DDeusExGameEngine::Browse` (DeusEx.dll 10010b90) turns
    /// `?loadgame=N` into the URL `<Save>\Current\<map>.dxs?load?loadonly?loadgame`; `UGameEngine::LoadMap` then calls
    /// `SpawnPlayActor` for the viewport (1038bf10) as for any level, and `GameInfo.Login` takes the saved
    /// `PlayerPawn` that has no `Player` instead of spawning one (`DeusExGameInfo.Login` sees `Loadgame` and leaves it
    /// where it was). `?loadonly` keeps travelling actors out. The class is the URL's (`Class=`, else
    /// `[DefaultPlayer]`, else `[URL]`).
    pub fn login_saved_player(&mut self) -> Result<u32, VmError> {
        let class_path = self.url_class_option().unwrap_or_else(|| "DeusEx.JCDentonMale".into());
        let class_name = class_path.rsplit('.').next().unwrap_or(&class_path).to_string();
        self.spawn_play_actor(&class_name, "", None, "?load?loadonly?loadgame")
    }

    /// `Class=` of the URL options (`url_options_string`), else `[URL] Class` of the game's ini.
    fn url_class_option(&mut self) -> Option<String> {
        let ops = self.url_options_string();
        ops.split('?')
            .find_map(|o| o.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case("Class")).map(|(_, v)| v.to_string()))
            .filter(|v| !v.is_empty())
            .or_else(|| self.reg.config_value("System", "URL", "Class").map(|c| ue_config::unquote(&c)))
    }

    /// `ULevel::SpawnPlayActor` (10396bd0); `extra_options` are appended to the URL's options (`?key` each).
    fn spawn_play_actor(&mut self, class_name: &str, portal: &str, travel: Option<&[crate::travel::TravelRecord]>, extra_options: &str) -> Result<u32, VmError> {
        let rt = |s: String| VmError::Runtime(s);
        let game = match self.get_prop(self.level, "Game", 0)? {
            Value::Obj(g) if g != 0 => g,
            _ => return Err(rt("login: the level has no game object".into())),
        };
        let class = self.reg.class_by_name(class_name).ok_or_else(|| rt(format!("login: no class {class_name}")))?;
        let path = self.reg.class(class).path.clone();
        let cls = Value::Obj(self.reg.obj_id(&path).0);
        let options = Value::Str(self.url_options_string() + extra_options);
        let pawn = match self.call_by_name(game, "Login", vec![Value::Str(portal.to_string()), options, Value::Str(String::new()), cls])? {
            Value::Obj(p) if p != 0 => p,
            _ => return Err(rt("login: the game refused the player".into())),
        };
        self.link_viewport(pawn)?;
        self.send_event(pawn, "Possess", vec![])?;
        self.set_play_actor_flags(pawn)?;
        // a script error in the accept events is logged; the player is still logged in
        if let Err(e) = self.send_event(pawn, "TravelPreAccept", vec![]) {
            self.log.push(format!("TravelPreAccept: {e}"));
        }
        let carried = match travel {
            Some(records) if !records.is_empty() => self.import_travel(pawn, records)?,
            _ => vec![pawn],
        };
        let others: Vec<u32> = carried.iter().rev().copied().filter(|&a| a != pawn).collect();
        for &a in &others {
            if let Err(e) = self.send_event(a, "TravelPreAccept", vec![]) {
                self.log.push(format!("TravelPreAccept: {e}"));
            }
        }
        if let Err(e) = self.call_by_name(game, "AcceptInventory", vec![Value::Obj(pawn)]) {
            self.log.push(format!("AcceptInventory: {e}"));
        }
        for &a in others.iter().chain(std::iter::once(&pawn)) {
            if let Err(e) = self.send_event(a, "TravelPostAccept", vec![]) {
                self.log.push(format!("TravelPostAccept: {e}"));
            }
        }
        self.call_by_name(game, "PostLogin", vec![Value::Obj(pawn)])?;
        Ok(pawn)
    }

    /// Movement for the player's next tick slot: `aForward`/`aStrafe` are set there and `PlayerTick` runs (no
    /// `PlayerInput`). The scripts scale the axes in place (`aForward *= 0.4`), so they must be given every frame;
    /// full speed forward is 6400. (Shortcut for tools; the input layer uses `player_frame`.)
    pub fn player_tick(&mut self, pawn: u32, _dt: f32, forward: f32, strafe: f32) -> Result<(), VmError> {
        self.player_slot_input = Some((pawn, SlotInput::Direct { forward, strafe }));
        Ok(())
    }

    /// One frame of input for the local player. Buttons (byte variables such as `bFire`) and exec commands (`ActivateBelt
    /// 3`) act at once, as key events do in the engine; the axes (`aBaseY`, `aMouseX`, ...) wait for the player's
    /// slot in the actor tick, where the engine reads the viewport's input and runs `PlayerInput` and `PlayerTick`
    /// (`player_slot`). While paused, `ULevel::Tick` reads the input and runs `PlayerInput` only, here at once.
    /// Failing commands are returned, not fatal.
    pub fn player_frame(&mut self, pawn: u32, dt: f32, buttons: &[(String, u8)], axes: &[(String, f32)], execs: &[String]) -> Result<Vec<String>, VmError> {
        for (n, v) in buttons {
            self.set_prop(pawn, n, 0, Value::Byte(*v))?;
        }
        let mut problems = Vec::new();
        for line in execs {
            match self.exec_input_command(pawn, line) {
                Ok(true) => {}
                Ok(false) => problems.push(format!("unknown command: {line}")),
                Err(e) => problems.push(format!("{line}: {e}")),
            }
        }
        if self.is_paused() {
            self.player_slot_input = None;
            self.clear_input_floats(pawn)?;
            for (n, v) in axes {
                self.set_prop(pawn, n, 0, Value::Float(*v))?;
            }
            self.send_event(pawn, "PlayerInput", vec![Value::Float(dt)])?;
            self.clear_input_floats(pawn)?;
        } else {
            self.player_slot_input = Some((pawn, SlotInput::Bound(axes.to_vec())));
        }
        Ok(problems)
    }

    /// Is `a` the local player whose tick reads the viewport's input (`AActor::Tick`: a `PlayerPawn` with role above
    /// simulated proxy, not an autonomous proxy remotely, with a `Player` and `SHOW_PlayerCtrl`)?
    pub(crate) fn is_viewport_player(&mut self, a: u32) -> bool {
        let o = self.offsets();
        self.is_instance_of(a, o.class_player_pawn)
            && self.u8p(a, "Role") >= 2
            && self.u8p(a, "RemoteRole") != 3
            && self.op(a, "Player") != 0
            && self.ip(a, "ShowFlags") & SHOW_PLAYER_CTRL != 0
    }

    /// The local player's part of its actor tick (`AActor::Tick`, Engine.dll 103a25a8): the viewport's input is read
    /// (the axes of `player_frame`), `PlayerInput` and `PlayerTick` run with the actor tick's time, then the input
    /// floats are zeroed again (`ReadInput(-1)`). Without input this frame the axes are simply zero.
    pub(crate) fn player_slot(&mut self, pawn: u32, dt: f32) -> Result<(), VmError> {
        let input = match self.player_slot_input.take() {
            Some((p, i)) if p == pawn => i,
            _ => SlotInput::Bound(Vec::new()),
        };
        self.clear_input_floats(pawn)?;
        match input {
            SlotInput::Bound(axes) => {
                for (n, v) in &axes {
                    self.set_prop(pawn, n, 0, Value::Float(*v))?;
                }
                self.send_event(pawn, "PlayerInput", vec![Value::Float(dt)])?;
                if self.is_live(pawn) {
                    self.send_event(pawn, "PlayerTick", vec![Value::Float(dt)])?;
                }
            }
            SlotInput::Direct { forward, strafe } => {
                self.set_prop(pawn, "aForward", 0, Value::Float(forward))?;
                self.set_prop(pawn, "aStrafe", 0, Value::Float(strafe))?;
                self.send_event(pawn, "PlayerTick", vec![Value::Float(dt)])?;
            }
        }
        if self.is_live(pawn) {
            self.clear_input_floats(pawn)?;
        }
        Ok(())
    }

    /// Zeroes every float property flagged as input (`aBaseX`, `aMouseY`, `aLookUp`, ...).
    fn clear_input_floats(&mut self, pawn: u32) -> Result<(), VmError> {
        const CPF_INPUT: u32 = 0x4;
        let Some(class) = self.class_of(pawn) else { return Ok(()) };
        let names: Vec<String> = self
            .reg
            .all_props(class)
            .into_iter()
            .filter(|p| p.flags & CPF_INPUT != 0 && p.kind == ue_package::object::PropertyKind::Float)
            .map(|p| self.reg.names.str(p.name).to_string())
            .collect();
        for n in names {
            self.set_prop(pawn, &n, 0, Value::Float(0.0))?;
        }
        Ok(())
    }

    /// Runs a command line as the console or the input system hands it to the pawn's viewport (`UViewport::Exec`
    /// and the rest of the chain, `console.rs`): `ActivateBelt 3`, `SetSensitivity 4.5`, `RMODE 6`, `set ...`.
    /// Returns whether something took it.
    pub fn exec_command(&mut self, actor: u32, line: &str) -> Result<bool, VmError> {
        Ok(self.exec_viewport_line(actor, line)?.0)
    }

    /// A command from a key binding: the input layer has expanded its aliases already, so it runs as the parts of
    /// an alias do (`UInput::ExecInputCommands` with aliases off).
    pub fn exec_input_command(&mut self, actor: u32, line: &str) -> Result<bool, VmError> {
        let was = std::mem::replace(&mut self.alias_running, true);
        let r = self.exec_viewport_line(actor, line);
        self.alias_running = was;
        Ok(r?.0)
    }
}

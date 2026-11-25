//! UnrealScript virtual machine for Unreal Engine 1 bytecode.
//!
//! Layers: `ir` compiles serialized tokens into typed expression trees; `exec` interprets them over object
//! and frame memory images laid out by `ue-registry`; `natives` implements the engine's built-in functions.

pub mod ai;
pub mod event_manager;
mod parabolic;
mod particles;
pub mod analysis;
pub mod collision;
pub mod canvas;
pub mod snapshot;
pub mod console;
pub mod crt;
mod load_object;
mod new_object;
mod object_config;
mod property_text;
pub mod conversation;
pub mod decal;
mod exec;
mod iterators;
pub mod ir;
pub mod movement;
pub mod level;
mod natives;
pub use natives::rotate_vector;
mod native_ext;
mod natives_rare;
mod sounds;
pub use native_ext::{flag_hash, Flag, FlagStore};
pub mod text_parser;
mod config;
pub mod ui;
mod ui_natives;
mod ui_clip;
mod ui_scale;
mod ui_scale_window;
mod ui_edit;
mod ui_computer;
pub mod save;
pub mod save_dir;
mod ui_list;
mod ui_scroll;
mod ui_radio;
pub mod hash;
pub mod import_save;
mod ui_viewport;
pub mod travel;
mod ui_widgets;
mod ui_textlog;
pub mod ui_gc;
mod ui_gc_natives;
mod ui_paint;
pub mod ui_input;
pub mod player;
mod ai_move;
pub mod animation;
pub mod navigation;
mod noise;
mod perception;
pub mod physics;
mod route;
pub mod lighting;
mod world;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use ue_registry::{ClassId, FuncLoc, PropDef, Registry, StructId};

pub use exec::Frame;

/// Applies a primitive cast token to a value (exposed for tests and tools).
pub fn natives_cast(vm: &mut Vm, op: u8, v: Value) -> Result<Value, VmError> {
    natives::cast(vm, op, v)
}

#[derive(Debug)]
pub enum VmError {
    Compile(String),
    Runtime(String),
    /// A feature the VM does not implement yet (engine natives, iterators, states...).
    Unsupported(String),
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::Compile(s) => write!(f, "compile error: {s}"),
            VmError::Runtime(s) => write!(f, "runtime error: {s}"),
            VmError::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for VmError {}

/// Object reference as stored in memory: 0 = none, bit 31 set = live instance (index + 1), otherwise an interned
/// path id (class or asset that is not a live object).
pub fn inst_ref(index: usize) -> u32 {
    0x8000_0000 | (index as u32 + 1)
}

pub fn ref_instance(r: u32) -> Option<usize> {
    if r & 0x8000_0000 != 0 {
        Some((r & 0x7FFF_FFFF) as usize - 1)
    } else {
        None
    }
}

/// A texture's run-time animation state (`UTexture` transient members).
#[derive(Debug, Clone, Copy, Default)]
pub struct TextureAnim {
    pub current: u32,
    pub accumulator: f32,
    pub prime_current: u8,
    pub last_update: f64,
}

/// A texture's animation timing properties.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextureAnimProps {
    pub prime_count: u8,
    pub min_frame_rate: f32,
    pub max_frame_rate: f32,
    /// `bMasked` (bit 1 of the texture's `PolyFlags`): `UCanvas::DrawTile` adds it to the flags it draws with.
    pub masked: bool,
}

/// One `Canvas.DrawActor` call (see `Vm::canvas_draws`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanvasDraw {
    pub actor: u32,
    pub wireframe: bool,
    pub clear_z: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// No value (void results, omitted optional arguments, accessed-none reads).
    Void,
    Byte(u8),
    Int(i32),
    Bool(bool),
    Float(f32),
    Name(u32),
    Str(String),
    Obj(u32),
    Vector([f32; 3]),
    Rotator([i32; 3]),
    Struct(StructId, Vec<u8>),
}

impl Value {
    pub fn as_i32(&self) -> Option<i32> {
        Some(match self {
            Value::Byte(b) => *b as i32,
            Value::Int(i) => *i,
            Value::Bool(b) => *b as i32,
            Value::Void => 0,
            _ => return None,
        })
    }

    pub fn as_f32(&self) -> Option<f32> {
        Some(match self {
            Value::Float(f) => *f,
            Value::Void => 0.0,
            _ => return None,
        })
    }

    pub fn as_bool(&self) -> Option<bool> {
        Some(match self {
            Value::Bool(b) => *b,
            Value::Void => false,
            Value::Byte(b) => *b != 0,
            Value::Int(i) => *i != 0,
            _ => return None,
        })
    }
}

/// A memory image with the string storage that backs its string slots.
#[derive(Debug, Clone, Default)]
pub struct Mem {
    pub data: Vec<u8>,
    pub strings: Vec<String>,
}

/// The engine's probe names (Core's hard-coded names 300..363, read from Core.dll's name table): the only events
/// `Enable`/`Disable` act on (one bit each of `StateFrame.ProbeMask`).
pub const PROBE_NAMES: [&str; 64] = [
    "Spawned", "Destroyed", "GainedChild", "LostChild", "Probe4", "Probe5", "Trigger", "UnTrigger", "Timer", "HitWall", "Falling", "Landed",
    "ZoneChange", "Touch", "UnTouch", "Bump", "BeginState", "EndState", "BaseChange", "Attach", "Detach", "ActorEntered", "ActorLeaving",
    "KillCredit", "AnimEnd", "EndedRotation", "InterpolateEnd", "EncroachingOn", "EncroachedBy", "FootZoneChange", "HeadZoneChange",
    "PainTimer", "SpeechTimer", "MayFall", "Probe34", "Die", "Tick", "PlayerTick", "Expired", "Probe39", "SeePlayer", "EnemyNotVisible",
    "HearNoise", "UpdateEyeHeight", "SeeMonster", "SeeFriend", "SpecialHandling", "BotDesireability", "Probe48", "Probe49", "Probe50",
    "Probe51", "Probe52", "Probe53", "Probe54", "Probe55", "Probe56", "Probe57", "Probe58", "Probe59", "Probe60", "Probe61", "Probe62", "All",
];

/// What a state-code frame is waiting for.
#[derive(Debug, Clone, PartialEq)]
pub enum Latent {
    /// `Sleep(seconds)`: resumes when the time has elapsed.
    Sleep(f32),
    /// Other latent natives (animation, movement): resumed by the engine through `Vm::resume_latent`.
    External(String),
}

/// A suspended state-code execution.
pub struct StateExec {
    pub code: Rc<ir::Code>,
    pub frame: Frame,
    /// Statement to continue at.
    pub pc: usize,
    /// Which code this is (the state's export and declared size), for saving.
    pub key: ((usize, usize), u32),
}

pub struct Object {
    pub class: ClassId,
    pub mem: Mem,
    /// Current state: (class declaring it, index in that class).
    pub state: Option<(ClassId, usize)>,
    pub exec: Option<StateExec>,
    pub latent: Option<Latent>,
    /// Events turned off with `Disable`.
    pub disabled: HashSet<u32>,
    /// False once destroyed; references to it then behave like none.
    pub alive: bool,
    /// Bumped by every `GotoState` on this object: running state code stops when it changes under it.
    pub goto_serial: u64,
    /// The object is an `Actor` (decided once, from its class).
    pub actor: bool,
}

pub struct Vm {
    pub reg: Registry,
    pub objects: Vec<Object>,
    /// Text written by `Log`/`Warn` and VM diagnostics.
    pub log: Vec<String>,
    /// The level object (`LevelInfo`), once `init_world` ran; 0 before.
    pub level: u32,
    /// A level change asked for by the scripts (`ClientTravel`, `ServerTravel`); the host carries it out between frames.
    pub travel_request: Option<travel::TravelRequest>,
    /// The host's part of the console command chain (input system, window, client: `console::HostExec`).
    pub host_console: Option<console::HostConsole>,
    /// What console commands asked the engine to do after the frame (exit, screenshot, flush...).
    pub engine_requests: Vec<console::EngineRequest>,
    /// An input alias is being run (`UInput::Exec` never expands an alias inside another).
    pub(crate) alias_running: bool,
    /// Names of the script functions being run, innermost last (for error messages).
    pub(crate) func_stack: Vec<u32>,
    /// Natives the scripts called that the port does not have (each reported once).
    pub missing_natives: std::collections::BTreeSet<String>,
    /// Treat a missing native as an error that stops the script (tools that look for missing natives).
    pub strict_natives: bool,
    /// Game time in seconds.
    pub time: f32,
    /// When set, every delivered event is recorded as (object, event name).
    pub event_trace: Option<Vec<(u32, String)>>,
    pub(crate) defaults: HashMap<ClassId, Mem>,
    /// Bounding boxes of brush models by object reference (minimum, maximum in the brush's own space).
    pub(crate) brush_bounds: HashMap<u32, Option<([f32; 3], [f32; 3])>>,
    /// `Friction` of textures by object path (see `surface_friction`).
    pub(crate) texture_friction: HashMap<String, f32>,
    /// Model `BoundingBox` of brushes by brush reference (the collision hash files brush actors by it).
    pub(crate) brush_boxes: HashMap<u32, Option<([f32; 3], [f32; 3])>>,
    pub(crate) code: ue_registry::FxHashMap<(usize, usize), Rc<ir::Code>>,
    pub(crate) vector: Option<StructId>,
    pub(crate) rotator: Option<StructId>,
    pub(crate) rng: u32,
    pub(crate) steps: u64,
    pub(crate) depth: usize,
    /// Latent request made by the native that just ran (consumed by the state-code loop).
    pub(crate) pending_latent: Option<Latent>,
    /// Incremented by every `GotoState`; lets running state code notice it was replaced.
    /// Recorded AI event registrations and similar engine bookkeeping.
    pub ai: ai::AiState,
    /// Attached decals by actor (see `decal.rs`).
    pub decals: ue_registry::FxHashMap<u32, decal::DecalPlace>,
    /// `decal_pieces` per decal, with the model generation and `DrawScale` bits they were clipped for (dropped with
    /// the decal's entry whenever `decals` changes).
    pub decal_pieces_cache: ue_registry::FxHashMap<u32, (u64, u32, Vec<Vec<(ue_assets::model::Vec3, [f32; 2])>>)>,
    /// Per BSP surface: its decals in drawing order (`FBspSurf::Decals`).
    pub surf_decals: std::collections::HashMap<usize, Vec<u32>>,
    /// The level's BSP model (decals need its nodes, polygons and surfaces).
    pub world_model: Option<std::rc::Rc<ue_assets::model::Model>>,
    /// Bumped whenever `world_model` is replaced or changed (movers filtered in): indexes of it are remade.
    pub world_model_gen: u64,
    /// The map actors of the current level by reference -> object path in the map package (what a saved game uses to find them).
    pub level_paths: HashMap<u32, String>,
    /// `UClass::ClassUnique` per class: the next number `MakeUniqueObjectName` tries (0 when the class is loaded).
    pub(crate) class_unique: HashMap<ClassId, i32>,
    /// Object names in use per outer: [0] the map package (actors), [1] the transient package (`new` objects).
    pub(crate) used_names: [HashSet<u32>; 2],
    /// Object names in use inside other objects (`new (Outer) Class`), by outer.
    pub(crate) object_names: HashMap<u32, HashSet<u32>>,
    /// The current map's `Level` object (`<Map>.MyLevel`, a package object): every actor's `XLevel`.
    pub(crate) xlevel: u32,
    /// The start of the cycle counter `clock`/`unclock` read (the original reads the CPU's time stamp counter).
    pub(crate) clock_start: std::time::Instant,
    /// Object header members (`Name`, `Class`, `Outer`) of package objects (classes, textures...), made on demand.
    pub(crate) headers: HashMap<u32, Mem>,
    /// Objects inside their `EndState` (object flag 0x2000 in `UObject::GotoState`): no second `EndState` meanwhile.
    pub(crate) ending_state: HashSet<usize>,
    /// Where saved games live (set by the host) and the save the scripts asked for: (index, description).
    pub save_dir: Option<std::path::PathBuf>,
    pub save_request: Option<(i32, String)>,
    /// `DX_VM_PROFILE=1`: per function (calls, self seconds) and the stack of child times.
    pub profile_on: bool,
    /// The `Actor` class, found once.
    pub(crate) actor_class: std::cell::Cell<Option<ClassId>>,
    /// The player pawn found last by `GetPlayerPawn` (checked first).
    pub(crate) player_cache: u32,
    pub profile: std::collections::HashMap<String, (u64, f64)>,
    pub(crate) profile_stack: Vec<f64>,
    /// Conversation data objects loaded so far, by path (see `conversation.rs`).
    pub con_objects: std::collections::HashMap<String, u32>,
    pub(crate) con_paths: std::collections::HashMap<u32, String>,
    /// Labels of `ConEventRandomLabel` objects (their native payload).
    pub(crate) con_labels: std::collections::HashMap<u32, Vec<String>>,
    /// Per laser iterator: where its last walk put the proxy, item by item (index, location, rotation).
    pub(crate) laser_items: std::collections::HashMap<u32, Vec<(usize, [f32; 3], [i32; 3])>>,
    /// The entry level's `LevelInfo` (kept as an object after the entry level is unloaded; `GetEntryLevel`).
    pub entry_level_info: u32,
    /// The map of the URL the level was browsed with, as written (`DX.dx`, `01_NYC_UNATCOIsland`): `XLevel->URL.Map`.
    pub url_map: String,
    /// The port's own version line for the main menu (`Aerugo <version> (<tag or commit>)`), set by the game
    /// executable; the port adds it beside the original's version text (`ui_add_port_version`). `None` in tests and
    /// probes, which keep the original's window tree.
    pub port_version: Option<String>,
    /// The portal of that URL (`#Name`), if any.
    pub url_portal: String,
    /// `appClipboardCopy`/`appClipboardPaste`'s text.
    pub clipboard: String,
    /// `FlagBase` contents by object reference (see `native_ext`).
    pub flag_stores: HashMap<u32, native_ext::FlagStore>,
    /// Host state of the `DeusExTextParser` objects.
    pub(crate) text_parsers: HashMap<u32, text_parser::TextParser>,
    /// Collision data: property offsets, the world BSP and the actors that collide with other actors.
    pub(crate) offsets: Option<collision::Offsets>,
    pub bsp: Option<ue_bsp::Bsp>,
    /// Window system state (see `ui.rs`).
    pub ui: ui::UiState,
    /// Collision BSPs of mover brushes by brush reference.
    pub(crate) mover_bsps: HashMap<u32, Option<Rc<ue_bsp::Bsp>>>,
    /// Indices of `objects` that are actors (dead ones included until the next rebuild): actor scans skip everything else.
    pub(crate) actor_slots: Vec<usize>,
    /// The level's collision hash (`ULevel::Hash`, see `hash.rs`) and its shuffled index tables (made once).
    pub(crate) hash: Option<hash::CollisionHash>,
    pub(crate) hash_tables: Option<Rc<hash::HashTables>>,
    /// Texture and flags of every surface of the world model.
    pub surf_info: Vec<collision::SurfInfo>,
    /// VM references of the world model's zone actors, indexed by zone number.
    pub zone_actors: Vec<u32>,
    /// VM references of the actors in the world model's light lists (`UModel::Lights`, 0 for list ends and unknowns).
    pub model_light_actors: Vec<u32>,
    /// The renderer's per-frame random tables (`FGlobalRandoms`) and the shared strobe state of `GlobalLighting`.
    pub randoms: ue_assets::light::GlobalRandoms,
    pub strobe: ue_assets::light::Strobe,
    /// Per actor: the lights it was lit by and their fade bytes (`SetupForActor`'s cached light list, at most 16).
    pub actor_light_cache: std::collections::HashMap<u32, Vec<(u32, u8)>>,
    /// `SetupForActor`'s call counter, stamped into the lights' `ExtraTag` while gathering.
    pub light_stamp: i32,
    /// Texture slots (as object path ids) of meshes, by mesh path id.
    pub(crate) mesh_textures: HashMap<u32, Vec<u32>>,
    pub(crate) region_offsets: Option<movement::RegionOffsets>,
    pub(crate) mesh_anims: HashMap<u32, Option<Rc<Vec<animation::SeqInfo>>>>,
    pub reach_specs: Vec<navigation::ReachSpecRt>,
    pub(crate) alliance_layout: Option<ai_move::AllianceLayout>,
    /// Property offsets by class and name; keyed by the name's address (a `&'static str` literal), not its bytes.
    pub(crate) prop_cache: ue_registry::FxHashMap<(ClassId, usize, usize), Option<(usize, u32)>>,
    /// `get_prop_static`'s lookups: (object's class, name address, name length) -> (declaring class, property index).
    static_props: std::cell::RefCell<ue_registry::FxHashMap<(ClassId, usize, usize), Option<(ClassId, usize)>>>,
    /// The last hits of `static_props` in a small direct-mapped table: (name address, name length, class, result).
    static_props_recent: std::cell::RefCell<Vec<(usize, usize, Option<ClassId>, Option<(ClassId, usize)>)>>,
    /// A direct-mapped front of `prop_cache` (class, name address, name length, result): most lookups end here.
    pub(crate) prop_fast: Box<[(u32, usize, usize, Option<(usize, u32)>)]>,
    /// Per function, what a call needs from its definition (`call_plan`).
    pub(crate) call_plans: ue_registry::FxHashMap<FuncLoc, std::rc::Rc<exec::CallPlan>>,
    /// Spare frame memories and argument lists, reused by calls instead of allocated each time.
    pub(crate) frame_pool: Vec<Vec<u8>>,
    /// `is_class`: the class of a name literal, by the literal's address and length.
    pub(crate) class_name_cache: std::cell::RefCell<ue_registry::FxHashMap<(usize, usize), ClassId>>,
    pub(crate) arg_pool: Vec<Vec<Value>>,
    /// Per class, a bit set of its ancestors (itself included): `is_a` in constant time. Filled on first use.
    pub(crate) ancestry: std::cell::RefCell<Vec<Vec<u64>>>,
    /// Script functions known to do nothing (an empty body, no return value): events sent to them are skipped, like the
    /// engine skips events an actor does not probe (e.g. the empty `Actor.Tick` inherited by every brush and path node).
    empty_events: ue_registry::FxHashMap<FuncLoc, bool>,
    /// Set by a native that writes a whole static array argument passed by reference (the engine hands such natives a
    /// pointer, e.g. `Conversation.BindEvents`): the argument's index and the elements to store from element 0 on.
    pub(crate) native_array_out: Option<(usize, Vec<Value>)>,
    /// When each zone of the world model was last drawn (level time; `FZoneProperties.LastRenderTime`), stamped by the
    /// renderer; `InStasis` reads it.
    pub zone_render_times: Vec<f32>,
    /// Options added to the current URL by `UpdateURL` (`?Name=...`).
    pub url_options: Vec<(String, String)>,
    /// Blend channel poses per (actor, channel), for the tweens of `PlayBlendAnim` (renderer side state).
    pub(crate) blend_cache: HashMap<(u32, u8), animation::PoseCache>,
    /// `ULevel`'s tick toggle: an actor has ticked this frame when its `bTicked` equals it; flips after every frame.
    pub(crate) tick_toggle: bool,
    /// `ULevel::FirstDeleted`: actors destroyed and not yet cleaned up (newest first). They stay addressable by the
    /// scripts that still hold them until `cleanup_destroyed` clears every reference (`world.rs`).
    pub(crate) pending_deleted: Vec<u32>,
    /// Set by `DeleteSaveGameFiles` for the `Current` folder: the host forgets the levels it keeps for the mission.
    pub clear_current_levels: bool,
    pub(crate) pending_deleted_set: HashSet<u32>,
    /// The render device's `HighDetailActors` (`[<GameRenderDevice>] HighDetailActors` in the game's ini; on for the
    /// shipped D3D device). `LoadMap` copies it into `Level.bHighDetailMode`; `SetupDynamics` leaves out `bHighDetail`
    /// actors when it is off. Read when a map loads.
    pub high_detail_actors: bool,
    /// The level's actor list is in the order `LoadMap` sorts it to once the start-up is over (see
    /// `rebuild_actor_slots`), and the level's default brush (its second actor) that stays in front.
    pub(crate) actors_sorted: bool,
    /// The actors `Canvas.DrawActor` drew during the last `RenderOverlays` (the first-person weapon and item in hand).
    pub canvas_draws: Vec<CanvasDraw>,
    /// The scene frame the canvas draws into (`Canvas.FramePtr`), set by the host around the engine's drawing.
    pub canvas_frame: canvas::CanvasFrame,
    /// What the canvas text and tile natives drew this frame (the console's `PostRender`), in screen pixels, drawn
    /// over the window system.
    pub canvas_list: Vec<ui_gc::DrawCmd>,
    /// Textures made at run time (snapshots), by object id.
    pub runtime_textures: ue_registry::FxHashMap<u32, snapshot::RuntimeTexture>,
    /// Snapshots the host is to take from the last frame drawn.
    /// Window classes the port creates in place of the game's (`NewChild`), by class object: set by
    /// `apply_port_menus` (`CreditsWindow` -> `AerugoCreditsWindow`).
    pub window_substitutes: std::collections::HashMap<u32, u32>,
    pub snapshot_requests: Vec<snapshot::SnapshotRequest>,
    pub(crate) runtime_serial: u32,
    /// Textures' `AnimNext` by texture id (read once from their properties).
    pub(crate) anim_next_cache: ue_registry::FxHashMap<u32, u32>,
    /// Animation state of textures (`AnimCurrent`, `Accumulator`, `PrimeCurrent`, last update time) and their timing
    /// properties (see `texture_get`).
    pub(crate) texture_anim: ue_registry::FxHashMap<u32, TextureAnim>,
    pub(crate) texture_props_cache: ue_registry::FxHashMap<u32, TextureAnimProps>,
    pub(crate) level_brush: u32,
    /// The local player's input for its next tick slot (`player_frame`/`player_tick`).
    pub(crate) player_slot_input: Option<(u32, player::SlotInput)>,
}

impl Vm {
    pub fn new(reg: Registry) -> Vm {
        let vector = reg.struct_id_by_path("Core.Object.Vector");
        let rotator = reg.struct_id_by_path("Core.Object.Rotator");
        Vm {
            reg,
            objects: Vec::new(),
            log: Vec::new(),
            level: 0,
            travel_request: None,
            host_console: None,
            engine_requests: Vec::new(),
            alias_running: false,
            func_stack: Vec::new(),
            missing_natives: Default::default(),
            strict_natives: false,
            time: 0.0,
            event_trace: None,
            defaults: HashMap::new(),
            brush_bounds: HashMap::new(),
            brush_boxes: HashMap::new(),
            texture_friction: HashMap::new(),
            code: Default::default(),
            vector,
            rotator,
            rng: 0x2545_F491,
            steps: 0,
            depth: 0,
            pending_latent: None,
            decals: Default::default(),
            decal_pieces_cache: Default::default(),
            surf_decals: std::collections::HashMap::new(),
            world_model: None,
            world_model_gen: 0,
            level_paths: HashMap::new(),
            class_unique: HashMap::new(),
            used_names: [HashSet::new(), HashSet::new()],
            object_names: HashMap::new(),
            xlevel: 0,
            clock_start: std::time::Instant::now(),
            headers: HashMap::new(),
            ending_state: HashSet::new(),
            save_dir: None,
            save_request: None,
            actor_class: std::cell::Cell::new(None),
            player_cache: 0,
            profile_on: std::env::var_os("DX_VM_PROFILE").is_some(),
            profile: std::collections::HashMap::new(),
            profile_stack: Vec::new(),
            con_objects: std::collections::HashMap::new(),
            con_paths: std::collections::HashMap::new(),
            con_labels: std::collections::HashMap::new(),
            laser_items: std::collections::HashMap::new(),
            entry_level_info: 0,
            url_map: String::new(),
            port_version: None,
            url_portal: String::new(),
            clipboard: String::new(),
            ai: ai::AiState::default(),
            flag_stores: HashMap::new(),
            text_parsers: HashMap::new(),
            offsets: None,
            bsp: None,
            ui: ui::UiState::default(),
            mover_bsps: HashMap::new(),
            actor_slots: Vec::new(),
            hash: None,
            hash_tables: None,
            surf_info: Vec::new(),
            zone_actors: Vec::new(),
            model_light_actors: Vec::new(),
            randoms: Default::default(),
            strobe: Default::default(),
            actor_light_cache: Default::default(),
            light_stamp: 0,
            mesh_textures: HashMap::new(),
            region_offsets: None,
            mesh_anims: HashMap::new(),
            reach_specs: Vec::new(),
            alliance_layout: None,
            native_array_out: None,
            zone_render_times: vec![0.0; 64],
            url_options: Vec::new(),
            blend_cache: HashMap::new(),
            tick_toggle: false,
            pending_deleted: Vec::new(),
            clear_current_levels: false,
            pending_deleted_set: HashSet::new(),
            high_detail_actors: true,
            actors_sorted: false,
            canvas_draws: Vec::new(),
            canvas_frame: Default::default(),
            canvas_list: Vec::new(),
            runtime_textures: Default::default(),
            snapshot_requests: Vec::new(),
            window_substitutes: Default::default(),
            runtime_serial: 0,
            anim_next_cache: Default::default(),
            texture_anim: Default::default(),
            texture_props_cache: Default::default(),
            level_brush: 0,
            player_slot_input: None,
            prop_cache: Default::default(),
            static_props: Default::default(),
            static_props_recent: std::cell::RefCell::new(vec![(0, 0, None, None); 1024]),
            call_plans: Default::default(),
            frame_pool: Vec::new(),
            class_name_cache: Default::default(),
            arg_pool: Vec::new(),
            prop_fast: vec![(u32::MAX, 0, 0, None); crate::physics::PROP_FAST_SLOTS].into_boxed_slice(),
            ancestry: Default::default(),
            empty_events: Default::default(),
        }
    }

    /// Creates an object of `class` with default property values; returns its reference value.
    pub fn new_object(&mut self, class: ClassId) -> Result<u32, VmError> {
        // `InitProperties`: a copy of the class's defaults
        let mut mem = self.default_copy(class)?;
        // `Object.Class` holds the object's class.
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
        // `StaticAllocateObject` without a name: `MakeUniqueObjectName` in the outer (an actor's is the map package,
        // `new`'s default the transient package)
        let r = inst_ref(self.objects.len() - 1);
        let name = self.make_unique_name(class, if actor { 0 } else { 1 });
        self.set_object_name(r, name);
        Ok(r)
    }

    /// `UObject::MakeUniqueObjectName` (Core 101575b0): the class name with trailing digits removed, then the class's
    /// `ClassUnique` counter (bumped on every try), the base cut so the whole fits 63 characters, until no object of
    /// that name exists in the outer (`outer`: 0 the map package, 1 the transient package).
    pub(crate) fn make_unique_name(&mut self, class: ClassId, outer: usize) -> u32 {
        let full = self.reg.names.str(self.reg.class(class).name).to_string();
        let base = full.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
        loop {
            let n = self.class_unique.entry(class).or_insert(0);
            let num = n.to_string();
            *n += 1;
            let keep: String = base.chars().take(63usize.saturating_sub(num.len())).collect();
            let id = self.reg.names.intern(&format!("{keep}{num}"));
            if self.used_names[outer].insert(id) {
                return id;
            }
        }
    }

    /// The names in use per outer from the live objects (after a saved game brought its objects back with their names).
    pub(crate) fn rebuild_used_names(&mut self) {
        let Some(obj) = self.reg.class_by_name("Object") else { return };
        let Some(def) = self.reg.find_prop(obj, "Name").cloned() else { return };
        self.used_names = [HashSet::new(), HashSet::new()];
        self.object_names.clear();
        for i in 0..self.objects.len() {
            if !self.objects[i].alive {
                continue;
            }
            if let Ok(Value::Name(n)) = self.read_prop(&self.objects[i].mem, &def, def.offset as usize) {
                if n != 0 {
                    let key = self.outer_key_of(inst_ref(i));
                    self.names_in(key).insert(n);
                }
            }
        }
    }

    /// Writes an instance's `Object.Name`.
    pub(crate) fn set_object_name(&mut self, r: u32, name: u32) {
        let Some(i) = ref_instance(r) else { return };
        let Some(obj) = self.reg.class_by_name("Object") else { return };
        let Some(def) = self.reg.find_prop(obj, "Name").cloned() else { return };
        let _ = self.write_prop(i, &def, def.offset as usize, Value::Name(name));
    }

    pub fn class_of(&self, obj: u32) -> Option<ClassId> {
        ref_instance(obj).and_then(|i| self.objects.get(i)).map(|o| o.class)
    }

    /// Compiles (once) and returns the code of the function at `export`.
    pub fn code_for(&mut self, export: (usize, usize), mem_size: u32) -> Result<Rc<ir::Code>, VmError> {
        if let Some(c) = self.code.get(&export) {
            return Ok(c.clone());
        }
        let c = Rc::new(ir::compile(&mut self.reg, export, mem_size)?);
        self.code.insert(export, c.clone());
        Ok(c)
    }

    /// Calls a function by name on an object with already evaluated arguments (in parameter order).
    pub fn call_by_name(&mut self, obj: u32, name: &str, args: Vec<Value>) -> Result<Value, VmError> {
        let class = self.class_of(obj).ok_or_else(|| VmError::Runtime("call on a non-object".into()))?;
        let id = self.reg.names.get(name).ok_or_else(|| VmError::Runtime(format!("unknown name {name}")))?;
        let loc = self
            .reg
            .find_function(class, id)
            .ok_or_else(|| VmError::Runtime(format!("no function {name} on {}", self.reg.class(class).path)))?;
        let mut args = args;
        self.call_function(obj, loc, &mut args)
    }

    /// `call_by_name` that leaves the arguments in `args` as the function left them (out parameters).
    pub fn call_by_name_out(&mut self, obj: u32, name: &str, args: &mut Vec<Value>) -> Result<Value, VmError> {
        let class = self.class_of(obj).ok_or_else(|| VmError::Runtime("call on a non-object".into()))?;
        let id = self.reg.names.get(name).ok_or_else(|| VmError::Runtime(format!("unknown name {name}")))?;
        let loc = self.reg.find_function(class, id).ok_or_else(|| VmError::Runtime(format!("no function {name} on {}", self.reg.class(class).path)))?;
        self.call_function(obj, loc, args)
    }

    /// Calls a function of `class` (or an ancestor) without an object, as for static or object-independent functions.
    pub fn call_static(&mut self, class: &str, func: &str, args: Vec<Value>) -> Result<Value, VmError> {
        let cid = self.reg.class_by_name(class).ok_or_else(|| VmError::Runtime(format!("no class {class}")))?;
        let id = self.reg.names.get(func).ok_or_else(|| VmError::Runtime(format!("unknown name {func}")))?;
        let loc = self
            .reg
            .find_function(cid, id)
            .ok_or_else(|| VmError::Runtime(format!("no function {func} on {class}")))?;
        let mut args = args;
        let path = self.reg.class(cid).path.clone();
        let this = self.reg.obj_id(&path).0;
        self.call_function(this, loc, &mut args)
    }

    /// Reads a property of an object by name.
    pub fn get_prop(&self, obj: u32, name: &str, elem: u32) -> Result<Value, VmError> {
        let i = ref_instance(obj).ok_or_else(|| VmError::Runtime("not an object".into()))?;
        let o = self.objects.get(i).ok_or_else(|| VmError::Runtime("bad object".into()))?;
        let def = self.reg.find_prop(o.class, name).ok_or_else(|| VmError::Runtime(format!("no property {name}")))?;
        if elem >= def.dim {
            return Err(VmError::Runtime(format!("{name}[{elem}] out of range")));
        }
        self.read_prop(&o.mem, def, (def.offset + elem * def.elem_size) as usize)
    }

    /// The property `name` (a name fixed in the code) of an object's class, cached by the name's address: one small
    /// hash instead of interning the name and looking it up (the window system reads its fields this way every frame).
    fn static_prop_def(&self, class: ClassId, name: &'static str) -> Option<&PropDef> {
        let key = (class, name.as_ptr() as usize, name.len());
        let slot = ((key.1 >> 3) ^ (class.0 as usize).wrapping_mul(0x9e37)) & 1023;
        {
            let recent = self.static_props_recent.borrow();
            let r = &recent[slot];
            if r.0 == key.1 && r.1 == key.2 && r.2 == Some(class) {
                return r.3.map(|(c, i)| &self.reg.class(c).props[i]);
            }
        }
        let hit = self.static_props.borrow().get(&key).copied();
        let found = match hit {
            Some(found) => found,
            None => {
                let found = self.reg.find_prop(class, name).map(|d| {
                    let mut c = Some(class);
                    let mut at = None;
                    while let Some(k) = c {
                        if let Some(i) = self.reg.class(k).props.iter().position(|p| std::ptr::eq(p, d)) {
                            at = Some((k, i));
                            break;
                        }
                        c = self.reg.class(k).super_id;
                    }
                    at
                });
                let found = found.flatten();
                self.static_props.borrow_mut().insert(key, found);
                found
            }
        };
        self.static_props_recent.borrow_mut()[slot] = (key.1, key.2, Some(class), found);
        found.map(|(c, i)| &self.reg.class(c).props[i])
    }

    /// `get_prop` for a name fixed in the code (see `static_prop_def`).
    pub fn get_prop_static(&self, obj: u32, name: &'static str, elem: u32) -> Result<Value, VmError> {
        let i = ref_instance(obj).ok_or_else(|| VmError::Runtime("not an object".into()))?;
        let o = self.objects.get(i).ok_or_else(|| VmError::Runtime("bad object".into()))?;
        let def = self.static_prop_def(o.class, name).ok_or_else(|| VmError::Runtime(format!("no property {name}")))?;
        if elem >= def.dim {
            return Err(VmError::Runtime(format!("{name}[{elem}] out of range")));
        }
        self.read_prop(&o.mem, def, (def.offset + elem * def.elem_size) as usize)
    }

    /// `set_prop` for a name fixed in the code (see `static_prop_def`).
    pub fn set_prop_static(&mut self, obj: u32, name: &'static str, elem: u32, v: Value) -> Result<(), VmError> {
        let i = ref_instance(obj).ok_or_else(|| VmError::Runtime("not an object".into()))?;
        let class = self.objects.get(i).ok_or_else(|| VmError::Runtime("bad object".into()))?.class;
        let def = self.static_prop_def(class, name).cloned().ok_or_else(|| VmError::Runtime(format!("no property {name}")))?;
        if elem >= def.dim {
            return Err(VmError::Runtime(format!("{name}[{elem}] out of range")));
        }
        self.write_prop(i, &def, (def.offset + elem * def.elem_size) as usize, v)
    }

    /// Writes a property of an object by name.
    pub fn set_prop(&mut self, obj: u32, name: &str, elem: u32, v: Value) -> Result<(), VmError> {
        let i = ref_instance(obj).ok_or_else(|| VmError::Runtime("not an object".into()))?;
        let class = self.objects.get(i).ok_or_else(|| VmError::Runtime("bad object".into()))?.class;
        let def = self.reg.find_prop(class, name).cloned().ok_or_else(|| VmError::Runtime(format!("no property {name}")))?;
        if elem >= def.dim {
            return Err(VmError::Runtime(format!("{name}[{elem}] out of range")));
        }
        self.write_prop(i, &def, (def.offset + elem * def.elem_size) as usize, v)
    }

    /// Interns a name and returns it as a value.
    pub fn name_value(&mut self, s: &str) -> Value {
        Value::Name(self.reg.names.intern(s))
    }

    /// Name of the object's current state.
    /// Where an object's state code is suspended, for probes: the latent action it waits for, the statement index and
    /// that statement (debug form).
    pub fn debug_state_exec(&self, obj: u32) -> String {
        let Some(o) = ref_instance(obj).and_then(|i| self.objects.get(i)) else { return "no object".into() };
        let owner = o.state.map(|(c, i)| format!("{}#{i}", self.reg.class(c).path)).unwrap_or_default();
        let owner = format!("[{owner}] ");
        owner + &match &o.exec {
            None => format!("no state code running; latent {:?}", o.latent),
            Some(e) => {
                let stmt = e.code.stmts.get(e.pc).map(|s| format!("{s:?}")).unwrap_or_default();
                let stmt: String = stmt.chars().take(300).collect();
                let (pkg, ex) = e.key.0;
                let src = self.reg.packages.get(pkg).and_then(|p| p.pkg.object_path(ue_package::ObjectRef::Export(ex))).unwrap_or_default();
                format!("latent {:?} pc {} of {} (code of {src}, size {}): {stmt}", o.latent, e.pc, e.code.stmts.len(), e.key.1)
            }
        }
    }

    pub fn state_name(&self, obj: u32) -> Option<&str> {
        let st = ref_instance(obj).and_then(|i| self.objects.get(i)).and_then(|o| o.state)?;
        Some(self.reg.names.str(self.reg.state_def(st).name))
    }

    /// Function that a call of `name` on `this` resolves to (current state first, then the class).
    pub fn resolve_function(&self, this: u32, name: u32) -> Option<FuncLoc> {
        let class = self.class_of_ref(this)?;
        let state = ref_instance(this).and_then(|i| self.objects.get(i)).and_then(|o| o.state);
        self.reg.find_function_in_state(class, state, name)
    }

    /// `AActor::ProcessEvent` (Engine 10369100): an actor's events are dropped until its level has begun play
    /// (`Level->bBegunPlay`); a level's game info spawned before the start-up passes gets none at spawn.
    pub(crate) fn actor_events_off(&mut self, obj: u32) -> bool {
        let Some(i) = ref_instance(obj) else { return false };
        if !self.objects.get(i).is_some_and(|o| o.actor) {
            return false;
        }
        let o = self.offsets();
        let lvl = self.u32_at(obj, o.level);
        !(lvl != 0 && self.flag(lvl, o.b_begun_play.0, o.b_begun_play.1))
    }

    /// Delivers an engine event (Tick, Touch, Timer, ...) unless it was disabled. Returns `None` if skipped
    /// or the object has no such function.
    pub fn send_event(&mut self, obj: u32, name: &str, args: Vec<Value>) -> Result<Option<Value>, VmError> {
        let Some(id) = self.reg.names.get(name) else { return Ok(None) };
        if ref_instance(obj).and_then(|i| self.objects.get(i)).map(|o| o.disabled.contains(&id) || !o.alive).unwrap_or(true) {
            return Ok(None);
        }
        if self.actor_events_off(obj) {
            return Ok(None);
        }
        let Some(loc) = self.resolve_function(obj, id) else { return Ok(None) };
        if let Some(t) = self.event_trace.as_mut() {
            t.push((obj, name.to_string()));
        }
        if self.is_empty_function(loc) {
            return Ok(Some(Value::Void));
        }
        let mut args = args;
        self.call_function(obj, loc, &mut args).map(Some)
    }

    /// Runs an event and returns its arguments afterwards (for `out` parameters); `None` when the object has no
    /// such function or the event is disabled.
    pub fn call_event_out(&mut self, obj: u32, name: &str, args: Vec<Value>) -> Result<Option<Vec<Value>>, VmError> {
        let Some(id) = self.reg.names.get(name) else { return Ok(None) };
        if ref_instance(obj).and_then(|i| self.objects.get(i)).map(|o| o.disabled.contains(&id) || !o.alive).unwrap_or(true) {
            return Ok(None);
        }
        if self.actor_events_off(obj) {
            return Ok(None);
        }
        let Some(loc) = self.resolve_function(obj, id) else { return Ok(None) };
        let mut args = args;
        self.call_function(obj, loc, &mut args)?;
        Ok(Some(args))
    }

    /// Does the function do nothing: not native, no return value, a body that only returns?
    fn is_empty_function(&mut self, loc: FuncLoc) -> bool {
        if let Some(&e) = self.empty_events.get(&loc) {
            return e;
        }
        const FUNC_NATIVE: u32 = 0x400;
        const CPF_RETURN: u32 = 0x400;
        let (flags, has_return, export, size) = {
            let f = self.reg.function(loc);
            (f.flags, f.props.iter().any(|p| p.flags & CPF_RETURN != 0), f.export, f.script_mem_size)
        };
        let empty = flags & FUNC_NATIVE == 0
            && !has_return
            && self.code_for(export, size).map(|c| c.stmts.len() == 1 && matches!(c.stmts[0], ir::Stmt::Return(ir::Expr::Nothing))).unwrap_or(false);
        self.empty_events.insert(loc, empty);
        empty
    }

    /// Switches the object to `state` (none = no state) and arranges for its code to start at `label`
    /// (default `Begin`) the next time `tick_state` runs. Calls `EndState` on the old and `BeginState` on the new state.
    pub fn goto_state(&mut self, obj: u32, state: Option<u32>, label: Option<u32>) -> Result<(), VmError> {
        let idx = ref_instance(obj).ok_or_else(|| VmError::Runtime("GotoState on a non-object".into()))?;
        let class = self.objects.get(idx).ok_or_else(|| VmError::Runtime("bad object".into()))?.class;
        // Like the original: `Auto` selects the class's auto state; an unknown name falls back to no state.
        let auto = self.reg.names.get("Auto");
        let new_state = match state {
            Some(n) if Some(n) == auto => self.reg.find_auto_state(class),
            Some(n) if n != 0 => {
                let found = self.reg.find_state(class, n);
                if found.is_none() {
                    self.log.push(format!("State {} not found", self.reg.names.str(n)));
                }
                found
            }
            _ => None,
        };
        self.objects[idx].goto_serial += 1;
        let serial = self.objects[idx].goto_serial;
        let old = self.objects[idx].state;
        if new_state != old {
            // `UObject::GotoState` (1012e8f0): `EndState` only when leaving a state, and not while the object is
            // already in its `EndState` (flag 0x2000)
            if old.is_some() && self.ending_state.insert(idx) {
                let r = self.send_event(obj, "EndState", vec![]);
                self.ending_state.remove(&idx);
                r?;
            }
            // a state change made by EndState or BeginState wins: this one is preempted (the engine's
            // GOTOSTATE_Preempted), so it must not install its own state code afterwards
            if self.objects[idx].goto_serial != serial {
                return Ok(());
            }
            let o = &mut self.objects[idx];
            o.state = new_state;
            o.latent = None;
            // the probe mask is made again for the new state: what `Disable` turned off is on again
            o.disabled.clear();
            // `BeginState` only into a state
            if new_state.is_some() {
                let _ = self.send_event(obj, "BeginState", vec![])?;
            }
            if self.objects[idx].goto_serial != serial {
                return Ok(());
            }
        }
        self.objects[idx].exec = None;
        self.objects[idx].latent = None;
        let Some(st) = new_state else { return Ok(()) };
        let label_name = match label {
            Some(l) if l != 0 => l,
            _ => self.reg.names.intern("Begin"),
        };
        // The state code (labels) of a state that only overrides functions lives in the state it continues: the same-named state
        // of the superclass, or the state it extends. Take the first one up the chain that has the label.
        let mut cur = Some(st);
        while let Some(c) = cur {
            let (export, size, next) = {
                let d = self.reg.state_def(c);
                (d.export, d.script_mem_size, d.super_state)
            };
            if size > 0 {
                let code = self.code_for(export, size)?;
                if let Some(pc) = code.label_index(label_name) {
                    let frame = Frame::new(self.zero_mem(0), obj);
                    self.objects[idx].exec = Some(StateExec { code, frame, pc, key: (export, size) });
                    return Ok(());
                }
            }
            cur = next;
        }
        Ok(())
    }

    /// Advances an object's state code by `dt` seconds: finishes a sleep, then runs until the code suspends or ends.
    pub fn tick_state(&mut self, obj: u32, dt: f32) -> Result<(), VmError> {
        let Some(idx) = ref_instance(obj) else { return Ok(()) };
        match self.objects[idx].latent.clone() {
            // the engine's poll: subtract the frame time, done when less than half a frame is left
            Some(Latent::Sleep(t)) => {
                if t - dt >= dt * 0.5 {
                    self.objects[idx].latent = Some(Latent::Sleep(t - dt));
                    return Ok(());
                }
                self.objects[idx].latent = None;
            }
            Some(Latent::External(name)) => {
                if self.poll_latent(obj, &name, dt)? {
                    self.objects[idx].latent = None;
                } else {
                    return Ok(()); // waiting for an external condition
                }
            }
            None => {}
        }
        // `AActor::ProcessState` (103e5b60): the code runs on after a `GotoState`/`Goto` in the same call (the new
        // state's code from its label) until it waits on a latent action, ends, or the actor is deleted; a change of
        // state counts, and the fifth stops it (the code stays installed for the next tick)
        let mut changes = 0;
        loop {
            let Some(mut ex) = self.objects[idx].exec.take() else { return Ok(()) };
            let state_before = self.objects[idx].state;
            self.depth += 1;
            let serial = self.objects[idx].goto_serial;
            let r = self.run_state(&mut ex);
            self.depth -= 1;
            let end = r?;
            if self.objects[idx].goto_serial == serial {
                // Keep the execution only if the code suspended and was not replaced meanwhile.
                if let exec::RunEnd::Suspended(pc) = end {
                    ex.pc = pc;
                    self.objects[idx].exec = Some(ex);
                }
                return Ok(());
            }
            if !self.objects[idx].alive || self.objects[idx].latent.is_some() {
                return Ok(());
            }
            if self.objects[idx].state != state_before {
                changes += 1;
                if changes >= 5 {
                    return Ok(());
                }
            }
        }
    }

    /// Polls an external latent native; true when the state code may continue.
    fn poll_latent(&mut self, obj: u32, name: &str, dt: f32) -> Result<bool, VmError> {
        match name {
            // done once the pawn is no longer falling; after the timeout `LongFall` is sent every frame
            "WaitForLanding" => {
                if self.u8p(obj, "Physics") != physics::PHYS_FALLING {
                    return Ok(true);
                }
                let left = self.fp(obj, "LatentFloat") - dt;
                self.set_fp(obj, "LatentFloat", left);
                if left < 0.0 {
                    self.send_event(obj, "LongFall", vec![])?;
                }
                Ok(false)
            }
            "FinishAnim" => Ok(self.bp(obj, "bAnimFinished")),
            // a mover is done when it stops interpolating (`physMovingBrush` clears the flag at alpha 1)
            "FinishInterpolation" => Ok(!self.bp(obj, "bInterpolating")),
            _ => Ok(self.poll_ai_latent(obj, name)?.unwrap_or(false)),
        }
    }

    /// Audio commands (`PlaySound`, `StopSound`) issued since the last call, oldest first.
    pub fn take_sound_events(&mut self) -> Vec<ai::SoundEvent> {
        std::mem::take(&mut self.ai.sound_events)
    }

    /// The latent request made by the native that just ran, if any (state code normally consumes it).
    pub fn take_pending_latent(&mut self) -> Option<Latent> {
        self.pending_latent.take()
    }

    /// Clears an external latent wait (for example when an animation finished).
    pub fn resume_latent(&mut self, obj: u32) {
        if let Some(i) = ref_instance(obj) {
            if let Some(o) = self.objects.get_mut(i) {
                o.latent = None;
            }
        }
    }

    pub(crate) fn zero_mem(&self, size: u32) -> Mem {
        Mem { data: vec![0; size as usize], strings: Vec::new() }
    }


}

pub(crate) fn _unused(_: &PropDef, _: FuncLoc) {}

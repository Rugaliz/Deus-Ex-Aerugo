//! Saved games: a snapshot of the live actors of the level (and the non-actor objects they refer to), written with portable
//! ids so that any later session can restore it onto a freshly loaded level.
//!
//! Format idea: every object is stored as its raw memory image plus its state, state-code position and latent action. Inside
//! the memory image, every Name and every object reference is rewritten to an index into the snapshot's own tables (names,
//! object paths, snapshot objects, "game" objects, conversation objects); `restore_snapshot` maps them back to the ids of the
//! running session. Map actors are found again by their path in the map package; everything else is created anew.
//! Not saved: windows and the graphics context (the root window is rebuilt by `Possess`), conversation data objects (found
//! by path), the viewport and console, transient properties.
use crate::ai::AiCallback;
use crate::native_ext::{Flag, FlagStore};
use crate::{ref_instance, ClassId, Frame, Latent, StateExec, Value, Vm, VmError};
use std::collections::{HashMap, HashSet};
use ue_package::object::PropertyKind;
use ue_registry::{PropDef, Target};

type Res<T> = Result<T, VmError>;

const MAGIC: &[u8; 8] = b"DXSHSAVE";
/// The format written. Every version from 1 up stays readable (the compatibility promise of 1.0): a change appends an
/// optional section at the end (older saves end before it, see `decode`) or raises the version, with `decode` keeping
/// the reading of the older ones. Reference saves of each version: `crates/dx-play/tests/save_compat.rs`.
const VERSION: u32 = 1;
const OLDEST_VERSION: u32 = 1;

/// Portable object reference: kind in the top three bits, index in the rest.
const K_OBJECT: u32 = 1 << 29;
const K_PATH: u32 = 2 << 29;
const K_GAME: u32 = 3 << 29;
const K_CON: u32 = 4 << 29;
const K_MASK: u32 = 7 << 29;

#[derive(Debug, Clone, PartialEq)]
pub struct SavedExec {
    pub package: String,
    pub export: usize,
    pub size: u32,
    pub pc: usize,
}

#[derive(Debug, Clone)]
pub struct SavedObject {
    pub class: String,
    /// Path of the map actor this object is, if it came from the map.
    pub level_path: Option<String>,
    pub data: Vec<u8>,
    pub strings: Vec<String>,
    pub state: Option<(String, usize)>,
    pub exec: Option<SavedExec>,
    pub latent: Option<Latent>,
    pub disabled: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SavedFlag {
    pub name: String,
    pub key: String,
    pub ty: u8,
    pub value: Value,
    pub expiration: i32,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Map name (file stem).
    pub map: String,
    pub time: f32,
    pub rng: u32,
    pub player: u32,
    pub objects: Vec<SavedObject>,
    pub names: Vec<String>,
    pub paths: Vec<String>,
    /// (class path, occurrence among the game objects of that class).
    pub game: Vec<(String, u32)>,
    pub flag_stores: Vec<(u32, i32, Vec<SavedFlag>)>,
    pub decals: Vec<(u32, [f32; 3], [f32; 3])>,
    /// Per saved decal (same order as `decals`): its surface entries (surface, corners, touched nodes); empty in
    /// older saves.
    pub decal_surfs: Vec<Vec<crate::decal::DecalSurf>>,
    /// (actor object, event name, callback name, score callback name, 4 flags)
    pub callbacks: Vec<(u32, String, String, String, [bool; 4])>,
    /// Render time of each world zone (`InStasis`); empty in older saves.
    pub zone_times: Vec<f32>,
    /// The level's tick toggle (which `bTicked` value means "ticked this frame"); false in older saves.
    pub tick_toggle: bool,
}

// ---- binary encoding ------------------------------------------------------------------------------------------------

struct W(Vec<u8>);
impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    fn strs(&mut self, v: &[String]) {
        self.u32(v.len() as u32);
        for s in v {
            self.str(s);
        }
    }
    fn vec3(&mut self, v: [f32; 3]) {
        for c in v {
            self.f32(c);
        }
    }
}

struct R<'a>(&'a [u8], usize);
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Res<&'a [u8]> {
        let end = self.1.checked_add(n).filter(|&e| e <= self.0.len()).ok_or_else(|| VmError::Runtime("saved game is truncated".into()))?;
        let s = &self.0[self.1..end];
        self.1 = end;
        Ok(s)
    }
    fn u8(&mut self) -> Res<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Res<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Res<i32> {
        Ok(self.u32()? as i32)
    }
    fn f32(&mut self) -> Res<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn bytes(&mut self) -> Res<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> Res<String> {
        String::from_utf8(self.bytes()?).map_err(|_| VmError::Runtime("saved game has bad text".into()))
    }
    fn strs(&mut self) -> Res<Vec<String>> {
        let n = self.u32()? as usize;
        (0..n).map(|_| self.str()).collect()
    }
    fn vec3(&mut self) -> Res<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
}

fn put_value(w: &mut W, names: &dyn Fn(u32) -> String, v: &Value) {
    match v {
        Value::Void => w.u8(0),
        Value::Byte(b) => {
            w.u8(1);
            w.u8(*b)
        }
        Value::Int(i) => {
            w.u8(2);
            w.i32(*i)
        }
        Value::Bool(b) => {
            w.u8(3);
            w.u8(*b as u8)
        }
        Value::Float(f) => {
            w.u8(4);
            w.f32(*f)
        }
        Value::Name(n) => {
            w.u8(5);
            w.str(&names(*n))
        }
        Value::Str(s) => {
            w.u8(6);
            w.str(s)
        }
        Value::Vector(v) => {
            w.u8(7);
            w.vec3(*v)
        }
        Value::Rotator(r) => {
            w.u8(8);
            for c in r {
                w.i32(*c);
            }
        }
        // objects and structs are not stored in flags
        _ => w.u8(0),
    }
}

fn get_value(r: &mut R, intern: &mut dyn FnMut(&str) -> u32) -> Res<Value> {
    Ok(match r.u8()? {
        1 => Value::Byte(r.u8()?),
        2 => Value::Int(r.i32()?),
        3 => Value::Bool(r.u8()? != 0),
        4 => Value::Float(r.f32()?),
        5 => Value::Name(intern(&r.str()?)),
        6 => Value::Str(r.str()?),
        7 => Value::Vector(r.vec3()?),
        8 => Value::Rotator([r.i32()?, r.i32()?, r.i32()?]),
        _ => Value::Void,
    })
}

impl Snapshot {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::new());
        w.0.extend_from_slice(MAGIC);
        w.u32(VERSION);
        w.str(&self.map);
        w.f32(self.time);
        w.u32(self.rng);
        w.u32(self.player);
        w.strs(&self.names);
        w.strs(&self.paths);
        w.u32(self.game.len() as u32);
        for (c, n) in &self.game {
            w.str(c);
            w.u32(*n);
        }
        w.u32(self.objects.len() as u32);
        for o in &self.objects {
            w.str(&o.class);
            match &o.level_path {
                Some(p) => {
                    w.u8(1);
                    w.str(p)
                }
                None => w.u8(0),
            }
            w.bytes(&o.data);
            w.strs(&o.strings);
            match &o.state {
                Some((c, i)) => {
                    w.u8(1);
                    w.str(c);
                    w.u32(*i as u32)
                }
                None => w.u8(0),
            }
            match &o.exec {
                Some(e) => {
                    w.u8(1);
                    w.str(&e.package);
                    w.u32(e.export as u32);
                    w.u32(e.size);
                    w.u32(e.pc as u32)
                }
                None => w.u8(0),
            }
            match &o.latent {
                Some(Latent::Sleep(t)) => {
                    w.u8(1);
                    w.f32(*t)
                }
                Some(Latent::External(n)) => {
                    w.u8(2);
                    w.str(n)
                }
                None => w.u8(0),
            }
            w.strs(&o.disabled);
        }
        w.u32(self.flag_stores.len() as u32);
        for (obj, default_exp, flags) in &self.flag_stores {
            w.u32(*obj);
            w.i32(*default_exp);
            w.u32(flags.len() as u32);
            for f in flags {
                w.str(&f.name);
                w.str(&f.key);
                w.u8(f.ty);
                w.i32(f.expiration);
                // names in flag values were turned into text by `make_snapshot`
                put_value(&mut w, &|_| String::new(), &f.value);
            }
        }
        w.u32(self.decals.len() as u32);
        for (o, c, n) in &self.decals {
            w.u32(*o);
            w.vec3(*c);
            w.vec3(*n);
        }
        w.u32(self.callbacks.len() as u32);
        for (o, e, c, s, b) in &self.callbacks {
            w.u32(*o);
            w.str(e);
            w.str(c);
            w.str(s);
            for x in b {
                w.u8(*x as u8);
            }
        }
        w.u32(self.zone_times.len() as u32);
        for t in &self.zone_times {
            w.f32(*t);
        }
        w.u8(self.tick_toggle as u8);
        // decal surface entries (added later)
        w.u32(self.decal_surfs.len() as u32);
        for entries in &self.decal_surfs {
            w.u32(entries.len() as u32);
            for e in entries {
                w.u32(e.surf as u32);
                for v in &e.verts {
                    w.vec3(*v);
                }
                w.u32(e.nodes.len() as u32);
                for &n in &e.nodes {
                    w.u32(n as u32);
                }
            }
        }
        w.0
    }

    pub fn decode(bytes: &[u8]) -> Res<Snapshot> {
        let mut r = R(bytes, 0);
        if r.take(8)? != MAGIC {
            return Err(VmError::Runtime("not a saved game".into()));
        }
        let version = r.u32()?;
        if !(OLDEST_VERSION..=VERSION).contains(&version) {
            return Err(VmError::Runtime(if version > VERSION {
                format!("saved game version {version} is from a newer Aerugo (this one reads versions {OLDEST_VERSION} to {VERSION})")
            } else {
                format!("saved game version {version} is not supported")
            }));
        }
        let mut s = Snapshot { map: r.str()?, time: r.f32()?, rng: r.u32()?, player: r.u32()?, ..Default::default() };
        s.names = r.strs()?;
        s.paths = r.strs()?;
        for _ in 0..r.u32()? {
            s.game.push((r.str()?, r.u32()?));
        }
        for _ in 0..r.u32()? {
            let class = r.str()?;
            let level_path = if r.u8()? == 1 { Some(r.str()?) } else { None };
            let data = r.bytes()?;
            let strings = r.strs()?;
            let state = if r.u8()? == 1 { Some((r.str()?, r.u32()? as usize)) } else { None };
            let exec = if r.u8()? == 1 { Some(SavedExec { package: r.str()?, export: r.u32()? as usize, size: r.u32()?, pc: r.u32()? as usize }) } else { None };
            let latent = match r.u8()? {
                1 => Some(Latent::Sleep(r.f32()?)),
                2 => Some(Latent::External(r.str()?)),
                _ => None,
            };
            let disabled = r.strs()?;
            s.objects.push(SavedObject { class, level_path, data, strings, state, exec, latent, disabled });
        }
        for _ in 0..r.u32()? {
            let obj = r.u32()?;
            let default_exp = r.i32()?;
            let mut flags = Vec::new();
            for _ in 0..r.u32()? {
                let name = r.str()?;
                let key = r.str()?;
                let ty = r.u8()?;
                let expiration = r.i32()?;
                // names in flag values are kept as ids only while a session runs; here they are text-free (see make_snapshot)
                let value = get_value(&mut r, &mut |_| 0)?;
                flags.push(SavedFlag { name, key, ty, value, expiration });
            }
            s.flag_stores.push((obj, default_exp, flags));
        }
        for _ in 0..r.u32()? {
            s.decals.push((r.u32()?, r.vec3()?, r.vec3()?));
        }
        for _ in 0..r.u32()? {
            let o = r.u32()?;
            let (e, c, sc) = (r.str()?, r.str()?, r.str()?);
            let b = [r.u8()? != 0, r.u8()? != 0, r.u8()? != 0, r.u8()? != 0];
            s.callbacks.push((o, e, c, sc, b));
        }
        // added later: older saves end here
        if r.1 < r.0.len() {
            for _ in 0..r.u32()? {
                s.zone_times.push(r.f32()?);
            }
        }
        if r.1 < r.0.len() {
            s.tick_toggle = r.u8()? != 0;
        }
        if r.1 < r.0.len() {
            for _ in 0..r.u32()? {
                let mut entries = Vec::new();
                for _ in 0..r.u32()? {
                    let surf = r.u32()? as usize;
                    let verts = [r.vec3()?, r.vec3()?, r.vec3()?, r.vec3()?];
                    let mut nodes = Vec::new();
                    for _ in 0..r.u32()? {
                        nodes.push(r.u32()? as usize);
                    }
                    entries.push(crate::decal::DecalSurf { surf, verts, nodes });
                }
                s.decal_surfs.push(entries);
            }
        }
        Ok(s)
    }
}

// ---- walking the memory image ------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Val {
    Name,
    Obj,
}

impl Vm {
    /// Calls `f` for every Name and object-reference slot of an object's memory image and stores the result back.
    pub(crate) fn map_slots(&self, class: ClassId, data: &mut [u8], f: &mut dyn FnMut(Val, u32) -> u32) {
        let props: Vec<PropDef> = self.reg.all_props(class).into_iter().cloned().collect();
        for p in &props {
            self.map_prop_slots(p, p.offset as usize, data, f);
        }
    }

    fn map_prop_slots(&self, p: &PropDef, base: usize, data: &mut [u8], f: &mut dyn FnMut(Val, u32) -> u32) {
        for e in 0..p.dim.max(1) as usize {
            let off = base + e * p.elem_size as usize;
            match (&p.kind, &p.target) {
                (PropertyKind::Object | PropertyKind::Class, _) | (PropertyKind::Name, _) => {
                    let kind = if p.kind == PropertyKind::Name { Val::Name } else { Val::Obj };
                    if let Some(slot) = data.get_mut(off..off + 4) {
                        let v = u32::from_le_bytes(slot.try_into().unwrap());
                        slot.copy_from_slice(&f(kind, v).to_le_bytes());
                    }
                }
                (PropertyKind::Struct, Target::Struct(sid)) => {
                    for m in self.struct_members(*sid) {
                        self.map_prop_slots(&m, off + m.offset as usize, data, f);
                    }
                }
                _ => {}
            }
        }
    }

    fn excluded_class(&self, class: ClassId) -> bool {
        ["Window", "GC", "Player", "Console", "Canvas"].iter().any(|n| self.reg.class_by_name(n).map(|c| self.is_a(class, c)).unwrap_or(false))
    }

    /// Identifies the game-rules objects that saves older than 3.86 refer to: (class path, occurrence among those of the
    /// class).
    fn game_keys(&mut self) -> Vec<(u32, String, u32)> {
        let objs = self.game_closure();
        let mut counts: HashMap<String, u32> = HashMap::new();
        objs.into_iter()
            .filter_map(|r| {
                let c = self.class_of(r)?;
                let path = self.reg.class(c).path.clone();
                let n = counts.entry(path.clone()).or_insert(0);
                let k = *n;
                *n += 1;
                Some((r, path, k))
            })
            .collect()
    }

    /// Captures the level as it is now.
    pub fn make_snapshot(&mut self, map: &str, player: u32) -> Res<Snapshot> {
        self.make_snapshot_excluding(map, player, &HashSet::new())
    }

    /// `make_snapshot` without the objects `exclude` (references to them are saved as `None`): a level left for
    /// another of the same mission, as `DDeusExGameEngine::SaveCurrentLevel` writes it after `PruneTravelActors`.
    pub fn make_snapshot_excluding(&mut self, map: &str, player: u32, exclude: &HashSet<u32>) -> Res<Snapshot> {
        // the game rules object and what it owns are saved like the other actors (the original's saves hold the
        // level's `GameInfo`); `game` stays empty (older saves referred to the running one through it)
        let game: Vec<(u32, String, u32)> = Vec::new();
        let game_index: HashMap<u32, u32> = HashMap::new();
        // 1. what is saved: the live actors, then the non-actor objects they refer to
        let mut order: Vec<u32> = Vec::new();
        let mut index: HashMap<u32, u32> = HashMap::new();
        for a in self.live_actors() {
            if game_index.contains_key(&a) || exclude.contains(&a) || self.class_of(a).map(|c| self.excluded_class(c)).unwrap_or(true) {
                continue;
            }
            index.insert(a, order.len() as u32);
            order.push(a);
        }
        let mut i = 0;
        while i < order.len() {
            for v in self.object_ref_values(order[i]) {
                if ref_instance(v).is_none() || !self.is_live(v) || index.contains_key(&v) || exclude.contains(&v) || game_index.contains_key(&v) || self.con_paths.contains_key(&v) {
                    continue;
                }
                if self.class_of(v).map(|c| self.excluded_class(c)).unwrap_or(true) {
                    continue;
                }
                index.insert(v, order.len() as u32);
                order.push(v);
            }
            i += 1;
        }
        // 2. portable ids
        let mut names: Vec<String> = Vec::new();
        let mut name_idx: HashMap<u32, u32> = HashMap::new();
        let mut paths: Vec<String> = Vec::new();
        let mut path_idx: HashMap<String, u32> = HashMap::new();
        let mut snap = Snapshot { map: map.to_string(), time: self.time, rng: self.rng, zone_times: self.zone_render_times.clone(), tick_toggle: self.tick_toggle, ..Default::default() };
        for &r in &order {
            let oi = ref_instance(r).unwrap();
            let class = self.objects[oi].class;
            let mut data = self.objects[oi].mem.data.clone();
            {
                let reg_names = &self.reg;
                let con_paths = &self.con_paths;
                let mut enc = |kind: Val, v: u32| -> u32 {
                    match kind {
                        Val::Name => *name_idx.entry(v).or_insert_with(|| {
                            names.push(reg_names.names.str(v).to_string());
                            names.len() as u32 - 1
                        }),
                        Val::Obj => {
                            if v == 0 {
                                0
                            } else if ref_instance(v).is_some() {
                                if let Some(&k) = index.get(&v) {
                                    K_OBJECT | (k + 1)
                                } else if let Some(&k) = game_index.get(&v) {
                                    K_GAME | (k + 1)
                                } else if let Some(p) = con_paths.get(&v) {
                                    let n = *path_idx.entry(p.clone()).or_insert_with(|| {
                                        paths.push(p.clone());
                                        paths.len() as u32 - 1
                                    });
                                    K_CON | (n + 1)
                                } else {
                                    0
                                }
                            } else {
                                let p = reg_names.obj_path(ue_registry::ObjId(v)).to_string();
                                let n = *path_idx.entry(p.clone()).or_insert_with(|| {
                                    paths.push(p);
                                    paths.len() as u32 - 1
                                });
                                K_PATH | (n + 1)
                            }
                        }
                    }
                };
                self.map_slots(class, &mut data, &mut enc);
            }
            let o = &self.objects[oi];
            let state = o.state.map(|(c, i)| (self.reg.class(c).path.clone(), i));
            let exec = o.exec.as_ref().map(|e| SavedExec { package: self.reg.packages[(e.key.0).0].name.clone(), export: (e.key.0).1, size: e.key.1, pc: e.pc });
            let disabled = o.disabled.iter().map(|&n| self.reg.names.str(n).to_string()).collect();
            snap.objects.push(SavedObject {
                class: self.reg.class(class).path.clone(),
                level_path: self.level_paths.get(&r).cloned(),
                data,
                strings: o.mem.strings.clone(),
                state,
                exec,
                latent: o.latent.clone(),
                disabled,
            });
        }
        snap.names = names;
        snap.paths = paths;
        snap.game = game.iter().map(|(_, c, k)| (c.clone(), *k)).collect();
        snap.player = index.get(&player).copied().ok_or_else(|| VmError::Runtime("the player is not part of the level".into()))?;
        // 3. host-side state attached to objects
        for (&r, store) in &self.flag_stores {
            if let Some(&k) = index.get(&r) {
                let flags = store
                    .flags
                    .iter()
                    .map(|f| SavedFlag {
                        name: self.reg.names.str(f.name).to_string(),
                        key: f.key.clone(),
                        ty: f.ty,
                        // a Name value is turned into text here and back into an id on restore
                        value: match &f.value {
                            Value::Name(n) => Value::Str(format!("\u{1}{}", self.reg.names.str(*n))),
                            v => v.clone(),
                        },
                        expiration: f.expiration,
                    })
                    .collect();
                snap.flag_stores.push((k, store.default_expiration, flags));
            }
        }
        for (&r, d) in &self.decals {
            if let Some(&k) = index.get(&r) {
                snap.decals.push((k, d.center, d.normal));
                snap.decal_surfs.push(d.surfs.clone());
            }
        }
        for (actor, event, cb) in self.ai.events.callbacks() {
            if let Some(&k) = index.get(&actor) {
                snap.callbacks.push((
                    k,
                    self.reg.names.str(event).to_string(),
                    self.reg.names.str(cb.callback).to_string(),
                    self.reg.names.str(cb.score_callback).to_string(),
                    [cb.check_visibility, cb.check_dir, cb.check_cylinder, cb.check_los],
                ));
            }
        }
        Ok(snap)
    }

    /// Puts a snapshot into the level that `load_level_opts(pi, false)` just made: map actors are overwritten (or removed
    /// when they were destroyed), everything else is created. Returns the player pawn.
    pub fn restore_snapshot(&mut self, snap: &Snapshot) -> Res<u32> {
        self.restore_snapshot_opts(snap, true)
    }

    /// `restore_snapshot`; `with_rng`: also the random number state saved with it (a loaded game), else the running
    /// one goes on (a level of the mission entered again).
    pub fn restore_snapshot_opts(&mut self, snap: &Snapshot, with_rng: bool) -> Res<u32> {
        let rt = |s: String| VmError::Runtime(s);
        // ids of this session
        let names: Vec<u32> = snap.names.iter().map(|n| self.reg.names.intern(n)).collect();
        let game_now = self.game_keys();
        let game_refs: Vec<u32> = snap
            .game
            .iter()
            .map(|(c, k)| game_now.iter().find(|(_, gc, gk)| gc == c && gk == k).map(|(r, _, _)| *r).unwrap_or(0))
            .collect();
        let con_refs: Vec<u32> = snap.paths.iter().map(|p| if p.contains("Text.") { self.load_con_object(p) } else { 0 }).collect();
        let path_ids: Vec<u32> = snap.paths.iter().map(|p| self.reg.obj_id(p).0).collect();
        // 1. which object each snapshot object is
        let by_path: HashMap<String, u32> = self.level_paths.iter().map(|(r, p)| (p.clone(), *r)).collect();
        let mut refs: Vec<u32> = Vec::with_capacity(snap.objects.len());
        let mut used: HashSet<u32> = HashSet::new();
        for o in &snap.objects {
            let class = self.reg.class_by_path(&o.class).ok_or_else(|| rt(format!("saved game: no class {}", o.class)))?;
            let r = match o.level_path.as_ref().and_then(|p| by_path.get(p)).copied() {
                Some(r) => r,
                None => self.new_object(class)?,
            };
            used.insert(r);
            refs.push(r);
        }
        // map actors that were destroyed (or never saved) are gone
        let dead: Vec<u32> = self.level_paths.keys().copied().filter(|r| !used.contains(r)).collect();
        for r in dead {
            if let Some(o) = ref_instance(r).and_then(|i| self.objects.get_mut(i)) {
                o.alive = false;
            }
        }
        // 2. contents
        for (k, o) in snap.objects.iter().enumerate() {
            let r = refs[k];
            let oi = ref_instance(r).unwrap();
            let class = self.objects[oi].class;
            let mut data = o.data.clone();
            let mut dec = |kind: Val, v: u32| -> u32 {
                match kind {
                    Val::Name => names.get(v as usize).copied().unwrap_or(0),
                    Val::Obj => {
                        let idx = (v & !K_MASK) as usize;
                        match v & K_MASK {
                            K_OBJECT => refs.get(idx.wrapping_sub(1)).copied().unwrap_or(0),
                            K_PATH => path_ids.get(idx.wrapping_sub(1)).copied().unwrap_or(0),
                            K_GAME => game_refs.get(idx.wrapping_sub(1)).copied().unwrap_or(0),
                            K_CON => con_refs.get(idx.wrapping_sub(1)).copied().unwrap_or(0),
                            _ => 0,
                        }
                    }
                }
            };
            self.map_slots(class, &mut data, &mut dec);
            let state = o.state.as_ref().and_then(|(c, i)| self.reg.class_by_path(c).map(|c| (c, *i)));
            let exec = match (&o.exec, state) {
                (Some(e), Some(_)) => {
                    let pi = self.reg.packages.iter().position(|p| p.name == e.package);
                    match pi {
                        Some(pi) => {
                            let code = self.code_for((pi, e.export), e.size)?;
                            Some(StateExec { code, frame: Frame::new(self.zero_mem(0), r), pc: e.pc, key: ((pi, e.export), e.size) })
                        }
                        None => None,
                    }
                }
                _ => None,
            };
            let disabled: HashSet<u32> = o.disabled.iter().map(|n| self.reg.names.intern(n)).collect();
            let obj = &mut self.objects[oi];
            obj.mem.data = data;
            obj.mem.strings = o.strings.clone();
            obj.state = state;
            obj.exec = exec;
            obj.latent = o.latent.clone();
            obj.disabled = disabled;
            obj.alive = true;
            obj.goto_serial = 0;
        }
        // 3. the transient things a loaded actor does not have. The render iterators (`RenderInterface`, particles)
        // are kept although transient: the engine would make new ones, but they draw random numbers when they start,
        // and a loaded game is to continue exactly like the session that saved it.
        for &r in &refs {
            for f in ["ConListItems", "RootWindow"] {
                if self.prop_at_name(r, f) {
                    let _ = self.set_prop(r, f, 0, Value::Obj(0));
                }
            }
            // the loaded level is every actor's `XLevel` again
            if self.is_actor_instance(r) {
                let _ = self.set_prop(r, "XLevel", 0, Value::Obj(self.xlevel));
            }
        }
        // 4. host-side state
        self.time = snap.time;
        if with_rng {
            self.rng = snap.rng;
        }
        self.tick_toggle = snap.tick_toggle;
        if !snap.zone_times.is_empty() {
            self.zone_render_times = snap.zone_times.clone();
        }
        self.decals.clear();
        self.decal_pieces_cache.clear();
        self.surf_decals.clear();
        // a mover's surfaces exist only while the level's moving-brush tracker does: its teardown (1037b0b0, when the
        // level is saved or left) removes the dynamic surfaces, nodes and points, and with them the decals on them; the
        // tracker made for the restored level has fresh ones. The decal actor stays, on no surface (never drawn).
        let level_surfs = self.surf_info.len();
        for (i, (k, c, n)) in snap.decals.iter().enumerate() {
            let saved = snap.decal_surfs.get(i).cloned().unwrap_or_default();
            let surfs: Vec<crate::decal::DecalSurf> = saved.iter().filter(|e| e.surf < level_surfs).cloned().collect();
            if !saved.is_empty() && surfs.is_empty() {
                continue;
            }
            let a = refs[*k as usize];
            for e in &surfs {
                self.surf_decals.entry(e.surf).or_default().push(a);
            }
            self.decals.insert(a, crate::decal::DecalPlace { center: *c, normal: *n, surfs });
        }
        self.flag_stores.retain(|r, _| !used.contains(r));
        for (k, default_expiration, flags) in &snap.flag_stores {
            let mut store = FlagStore::default();
            store.default_expiration = *default_expiration;
            for f in flags {
                let name = self.reg.names.intern(&f.name);
                let value = match &f.value {
                    Value::Str(s) if s.starts_with('\u{1}') => Value::Name(self.reg.names.intern(&s[1..])),
                    v => v.clone(),
                };
                store.add(Flag { name, key: f.key.clone(), ty: f.ty, value, expiration: f.expiration });
            }
            self.flag_stores.insert(refs[*k as usize], store);
        }
        self.ai.events.reset();
        for (k, event, cb, score, b) in &snap.callbacks {
            let (event, callback, score_callback) = (self.reg.names.intern(event), self.reg.names.intern(cb), if score.is_empty() { 0 } else { self.reg.names.intern(score) });
            self.ai.events.set_callback(refs[*k as usize], event, AiCallback { callback, score_callback, check_visibility: b[0], check_dir: b[1], check_cylinder: b[2], check_los: b[3] });
        }
        self.player_cache = 0;
        self.rebuild_actor_slots();
        self.rebuild_collision_set();
        self.rebuild_used_names();
        // `LoadMap` on a level saved in play: `PostPostBeginPlay` for every actor (pawns, decorations and the player bind
        // their conversations again, mission scripts restart their timer, ...), then the actor list is sorted
        self.rebuild_actor_slots();
        self.repair_map_actors();
        for i in self.finish_level_start(true) {
            self.log.push(format!("{} {}: {}", i.class, i.event, i.error));
        }
        refs.get(snap.player as usize).copied().ok_or_else(|| rt("saved game has no player".into()))
    }

    /// Loads a saved game: the current level is left (the game rules object stays), the map `pi` is instantiated without running
    /// its start-up events, the snapshot is put into it, and the player logs in again through `SpawnPlayActor` (`login_saved_player`:
    /// `Login` finds the saved pawn, `Possess` rebuilds the root window and the HUD, the accept events, `PostLogin`). Returns the player pawn and the level's load report.
    pub fn load_saved_game(&mut self, pi: usize, snap: &Snapshot) -> Res<(u32, crate::level::LevelLoad)> {
        let game_objs = self.game_closure();
        let keep: HashSet<u32> = game_objs.iter().copied().collect();
        let old_level = self.unload_level_keeping(&keep);
        let load = self.load_level_opts(pi, false)?;
        let lvl = self.level;
        for &g in &game_objs {
            if self.is_actor_instance(g) {
                let _ = self.set_prop(g, "Level", 0, Value::Obj(lvl));
            }
        }
        let pawn = self.restore_snapshot(snap)?;
        // a save that holds its own game rules object (`Level.Game` now one of the restored objects) replaces the
        // running one; older saves keep it
        let game_now = self.op(lvl, "Game");
        if !game_objs.contains(&game_now) {
            for &g in &game_objs {
                self.set_alive_quiet(g);
            }
            self.rebuild_actor_slots();
        }
        if old_level != lvl {
            self.set_alive_pub(old_level, false);
        }
        let player = self.login_saved_player()?;
        if player != pawn {
            self.log.push(format!("load: Login gave a new player {player}, not the saved one {pawn}"));
        }
        Ok((player, load))
    }

    fn prop_at_name(&mut self, r: u32, name: &str) -> bool {
        let Some(class) = self.class_of(r) else { return false };
        self.reg.find_prop(class, name).is_some()
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn every_version_up_to_this_one_reads_and_a_newer_one_says_so() {
        let bytes = Snapshot { map: "Test".into(), ..Default::default() }.encode();
        assert_eq!(Snapshot::decode(&bytes).unwrap().map, "Test");
        let mut newer = bytes.clone();
        newer[8..12].copy_from_slice(&(VERSION + 1).to_le_bytes());
        let e = Snapshot::decode(&newer).unwrap_err().to_string();
        assert!(e.contains("newer Aerugo"), "{e}");
        let mut zero = bytes;
        zero[8..12].copy_from_slice(&0u32.to_le_bytes());
        assert!(Snapshot::decode(&zero).is_err());
    }
}

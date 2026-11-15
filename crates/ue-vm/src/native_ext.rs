//! Natives of the Extension and DeusEx packages that are plain services (no window system): the flag store
//! (`FlagBase`), factories for helper objects, the dump-location stand-in and the version string.
//!
//! `FlagBase` keeps its flags in the host (the script-side `hashTable` property is unused). A flag is identified by
//! its name (case-insensitive) and type; setting with `bAdd` (default true) creates it. Reads of a missing flag give
//! the zero value. An expiration of 0 never expires; `DeleteExpiredFlags(n)` removes flags with `0 < expiration <= n`.
//! The flags are kept in the order of the original's table (`XFlagBase::FindName`/`GetNextFlag`, Extension.dll
//! 10024e60/10024c50): 64 buckets by the low 6 bits of the name's hash (the engine's case-insensitive CRC), each
//! chain sorted by the whole hash compared as a signed number, then by type. Iterators walk that order live: they
//! remember the flag they stopped at, so deleting the flag before it (as `ResetFlags` does) is safe.

use crate::{ref_instance, Value, Vm, VmError};
use std::collections::HashMap;

type Res<T> = Result<T, VmError>;

/// Number of `EFlagType` values; an iterator type outside `0..7` means "all types".
const FLAG_TYPES: u8 = 7;

#[derive(Clone, Debug)]
pub struct Flag {
    pub name: u32,
    pub key: String,
    pub ty: u8,
    pub value: Value,
    pub expiration: i32,
}

/// Position of a flag in the original's table: bucket, hash as a signed number, then name and type.
type FlagOrder = (u32, i32, String, u8);

#[derive(Default, Debug)]
pub struct FlagStore {
    /// In table order (see the module notes); add flags with `add`.
    pub flags: Vec<Flag>,
    pub default_expiration: i32,
    /// iterator id -> (flag type filter, the flag returned last)
    iters: HashMap<i32, (u8, Option<FlagOrder>)>,
    next_iter: i32,
}

/// `XFlagBase::FindName`'s hash of a flag name: the engine's CRC table over each UTF-16 unit (low byte, then high
/// byte) with `a`..`z` upper-cased, starting from 0.
pub fn flag_hash(name: &str) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = (i as u32) << 24;
            for _ in 0..8 {
                c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04c1_1db7 } else { c << 1 };
            }
            *e = c;
        }
        t
    });
    let mut h = 0u32;
    for u in name.encode_utf16() {
        let c = if (0x61..=0x7a).contains(&u) { u - 0x20 } else { u } as u32;
        h = t[((h ^ c) & 0xff) as usize] ^ (h >> 8);
        h = t[(((c >> 8) ^ h) & 0xff) as usize] ^ (h >> 8);
    }
    h
}

fn order_of(f: &Flag) -> FlagOrder {
    let h = flag_hash(&f.key);
    (h & 0x3f, h as i32, f.key.clone(), f.ty)
}

impl FlagStore {
    fn find(&self, key: &str, ty: u8) -> Option<usize> {
        self.flags.iter().position(|f| f.ty == ty && f.key == key)
    }

    /// Adds a flag at its place in the table order (replacing one with the same name and type).
    pub fn add(&mut self, f: Flag) -> usize {
        if let Some(i) = self.find(&f.key, f.ty) {
            self.flags[i] = f;
            return i;
        }
        let k = order_of(&f);
        let i = self.flags.partition_point(|g| order_of(g) < k);
        self.flags.insert(i, f);
        i
    }
}

fn rt<T: Into<String>>(s: T) -> VmError {
    VmError::Runtime(s.into())
}

/// Zero value of a flag type.
fn zero(ty: u8) -> Value {
    match ty {
        0 => Value::Bool(false),
        1 => Value::Byte(0),
        2 => Value::Int(0),
        3 => Value::Float(0.0),
        4 => Value::Name(0),
        5 => Value::Vector([0.0; 3]),
        _ => Value::Rotator([0; 3]),
    }
}

impl Vm {
    fn flag_store(&mut self, obj: u32) -> &mut FlagStore {
        self.flag_stores.entry(obj).or_default()
    }

    pub(crate) fn is_class(&self, obj: u32, name: &'static str) -> bool {
        // the class of a name literal, cached by its address (only found ones: a package loaded later adds classes)
        let key = (name.as_ptr() as usize, name.len());
        let cached = self.class_name_cache.borrow().get(&key).copied();
        let target = cached.or_else(|| {
            let t = self.reg.class_by_name(name);
            if let Some(t) = t {
                self.class_name_cache.borrow_mut().insert(key, t);
            }
            t
        });
        match (self.class_of(obj), target) {
            (Some(c), Some(t)) => self.is_a(c, t),
            _ => false,
        }
    }

    /// `UObject::GetOuter`: the `Outer` an object was made with, else the map package for an actor (0 for none).
    pub(crate) fn outer_of(&mut self, obj: u32) -> u32 {
        let o = self.op(obj, "Outer");
        if o != 0 {
            return o;
        }
        let actor = ref_instance(obj).and_then(|i| self.objects.get(i)).is_some_and(|o| o.actor);
        match self.level_paths.get(&self.level).and_then(|p| p.split('.').next()).map(str::to_string) {
            Some(pkg) if actor && !pkg.is_empty() => self.reg.obj_id(&pkg).0,
            _ => 0,
        }
    }

    /// A script object of `class` in `outer` (0: the transient package), as `new (outer) class`.
    fn make_object_in(&mut self, class: &str, outer: u32) -> Res<Value> {
        let c = self.reg.class_by_name(class).ok_or_else(|| rt(format!("no class {class}")))?;
        Ok(Value::Obj(self.new_script_object(c, outer, 0)?))
    }

    fn flag_set(&mut self, obj: u32, ty: u8, a: &[Value]) -> Value {
        let Some(Value::Name(name)) = a.first().cloned() else { return Value::Bool(false) };
        let key = self.reg.names.str(name).to_ascii_lowercase();
        let add = match a.get(2) { None | Some(Value::Void) => true, Some(v) => v.as_bool().unwrap_or(true) };
        let st = self.flag_store(obj);
        // an omitted or -1 expiration means the base's default; an explicit 0 means "never"
        let exp = match a.get(3) {
            None | Some(Value::Void) => -1,
            Some(v) => v.as_i32().unwrap_or(-1),
        };
        let exp = if exp == -1 { st.default_expiration } else { exp };
        let value = a.get(1).cloned().unwrap_or_else(|| zero(ty));
        let i = match st.find(&key, ty) {
            Some(i) => {
                st.flags[i].value = value;
                i
            }
            None if add => {
                st.add(Flag { name, key, ty, value, expiration: 0 })
            }
            None => return Value::Bool(false),
        };
        st.flags[i].expiration = exp;
        Value::Bool(true)
    }

    fn flag_get(&mut self, obj: u32, ty: u8, a: &[Value]) -> Value {
        let Some(Value::Name(name)) = a.first().cloned() else { return zero(ty) };
        let key = self.reg.names.str(name).to_ascii_lowercase();
        let st = self.flag_store(obj);
        match st.find(&key, ty) {
            Some(i) => st.flags[i].value.clone(),
            None => zero(ty),
        }
    }

    fn flag_key_type(&mut self, a: &[Value]) -> Option<(String, u8)> {
        let Some(Value::Name(name)) = a.first().cloned() else { return None };
        let ty = a.get(1).and_then(Value::as_i32).unwrap_or(0) as u8;
        Some((self.reg.names.str(name).to_ascii_lowercase(), ty))
    }

    /// Advances an iterator; returns the next matching flag (the first after the one returned last, in table order).
    fn flag_next(&mut self, obj: u32, iter: i32) -> Option<(u32, u8)> {
        let st = self.flag_store(obj);
        let (filter, last) = st.iters.get(&iter)?.clone();
        let start = match &last {
            Some(k) => st.flags.partition_point(|g| order_of(g) <= *k),
            None => 0,
        };
        let found = st.flags[start..].iter().find(|f| filter >= FLAG_TYPES || f.ty == filter).map(|f| (f.name, f.ty, order_of(f)));
        match found {
            Some((n, ty, k)) => {
                st.iters.insert(iter, (filter, Some(k)));
                Some((n, ty))
            }
            None => {
                // the end: a further call stays at the end
                st.iters.insert(iter, (filter, Some((u32::MAX, i32::MAX, String::new(), u8::MAX))));
                None
            }
        }
    }

    /// Dispatches the natives of this module. `None` when `name` is not one of them for the class of `this`.
    pub(crate) fn native_ext(&mut self, this: u32, name: &str, a: &mut Vec<Value>) -> Option<Res<Value>> {
        use Value::*;
        if self.is_class(this, "ExtString") {
            if let Some(r) = self.native_ext_string(this, name, a) {
                return Some(Ok(r));
            }
        }
        if let Some(r) = self.native_canvas(this, name, a) {
            return Some(r);
        }
        if let Some(r) = self.native_ui(this, name, a) {
            return Some(r);
        }
        if let Some(r) = self.native_rare(this, name, a) {
            return Some(r);
        }
        if let Some(r) = self.native_text_parser(this, name, a) {
            return Some(r);
        }
        let v = match name {
            // `PlayerPawn.ResetKeyboard`: every key is up
            "ResetKeyboard" if self.is_class(this, "PlayerPawn") => {
                let root = self.wo_pub(this, "rootWindow");
                if root != 0 {
                    for k in 0..255u32 {
                        let _ = self.set_prop(root, "keyDownMap", k, Byte(0));
                    }
                }
                Void
            }
            // saved games do not exist yet (road item E2): deleting finds nothing, saving says so
            "DeleteSaveGameFiles" => Void,
            // saving happens between frames, in the host (`Vm::save_request`)
            "SaveGame" if self.is_class(this, "DeusExPlayer") => {
                let index = a.first().and_then(Value::as_i32).unwrap_or(0);
                let desc = match a.get(1) {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                self.save_request = Some((index, desc));
                Void
            }
            "StringToName" => {
                let s = match a.first() {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                Name(self.reg.names.intern(&s))
            }
            // ---- GameDirectory: the map list of the game folder and the list of saved games
            "SetDirType" | "SetDirFilter" | "GetGameDirectory" | "GetDirCount" | "GetDirFilename" | "GetNewSaveFileIndex" | "GenerateSaveFilename" | "GenerateNewSaveFilename" | "GetSaveInfo"
            | "GetSaveInfoFromDirectoryIndex" | "GetTempSaveInfo" | "DeleteSaveInfo" | "PurgeAllSaveInfo" | "GetSaveFreeSpace" | "GetSaveDirectorySize"
                if self.is_class(this, "GameDirectory") =>
            {
                if let Some(v) = self.save_directory(this, name, a) {
                    return Some(Ok(v));
                }
                return Some(self.game_directory(this, name, a));
            }
            // (DeusEx 10014df0) `appSystemTime`: the local date and time
            "UpdateTimeStamp" if self.is_class(this, "DeusExSaveInfo") => {
                let t = crate::save_dir::now_civil();
                for (n, v) in ["Year", "Month", "Day", "Hour", "Minute", "Second"].iter().zip(t) {
                    self.set_i_pub(this, n, v);
                }
                Void
            }
            // ---- FlagBase
            "SetBool" | "SetByte" | "SetInt" | "SetFloat" | "SetName" | "SetVector" | "SetRotator" if self.is_class(this, "FlagBase") => {
                let ty = ["SetBool", "SetByte", "SetInt", "SetFloat", "SetName", "SetVector", "SetRotator"].iter().position(|n| *n == name).unwrap() as u8;
                self.flag_set(this, ty, a)
            }
            "GetBool" | "GetByte" | "GetInt" | "GetFloat" | "GetName" | "GetVector" | "GetRotator" if self.is_class(this, "FlagBase") => {
                let ty = ["GetBool", "GetByte", "GetInt", "GetFloat", "GetName", "GetVector", "GetRotator"].iter().position(|n| *n == name).unwrap() as u8;
                self.flag_get(this, ty, a)
            }
            "CheckFlag" if self.is_class(this, "FlagBase") => {
                let Some((key, ty)) = self.flag_key_type(a) else { return Some(Ok(Bool(false))) };
                Bool(self.flag_store(this).find(&key, ty).is_some())
            }
            "DeleteFlag" if self.is_class(this, "FlagBase") => {
                let Some((key, ty)) = self.flag_key_type(a) else { return Some(Ok(Bool(false))) };
                let st = self.flag_store(this);
                match st.find(&key, ty) {
                    Some(i) => {
                        st.flags.remove(i);
                        Bool(true)
                    }
                    None => Bool(false),
                }
            }
            "SetExpiration" if self.is_class(this, "FlagBase") => {
                if let Some((key, ty)) = self.flag_key_type(a) {
                    let e = a.get(2).and_then(Value::as_i32).unwrap_or(0);
                    let st = self.flag_store(this);
                    if let Some(i) = st.find(&key, ty) {
                        st.flags[i].expiration = e;
                    }
                }
                Void
            }
            "GetExpiration" if self.is_class(this, "FlagBase") => {
                // (10024a20) -1 for a flag that does not exist
                let Some((key, ty)) = self.flag_key_type(a) else { return Some(Ok(Int(-1))) };
                let st = self.flag_store(this);
                Int(st.find(&key, ty).map(|i| st.flags[i].expiration).unwrap_or(-1))
            }
            "DeleteExpiredFlags" if self.is_class(this, "FlagBase") => {
                let limit = a.first().and_then(Value::as_i32).unwrap_or(0);
                self.flag_store(this).flags.retain(|f| !(f.expiration != 0 && f.expiration <= limit));
                Void
            }
            "SetDefaultExpiration" if self.is_class(this, "FlagBase") => {
                self.flag_store(this).default_expiration = a.first().and_then(Value::as_i32).unwrap_or(0);
                Void
            }
            "CreateIterator" if self.is_class(this, "FlagBase") => {
                let ty = a.first().and_then(Value::as_i32).map(|t| t as u8).unwrap_or(FLAG_TYPES);
                let st = self.flag_store(this);
                st.next_iter += 1;
                let id = st.next_iter;
                st.iters.insert(id, (ty, None));
                Int(id)
            }
            "GetNextFlagName" | "GetNextFlag" if self.is_class(this, "FlagBase") => {
                let iter = a.first().and_then(Value::as_i32).unwrap_or(0);
                match self.flag_next(this, iter) {
                    Some((n, ty)) => {
                        a[1] = Name(n);
                        if name == "GetNextFlag" {
                            a[2] = Byte(ty);
                        }
                        Bool(true)
                    }
                    None => {
                        a[1] = Name(0);
                        if name == "GetNextFlag" {
                            a[2] = Byte(0);
                        }
                        Bool(false)
                    }
                }
            }
            "DestroyIterator" if self.is_class(this, "FlagBase") => {
                let iter = a.first().and_then(Value::as_i32).unwrap_or(0);
                self.flag_store(this).iters.remove(&iter);
                Void
            }
            "DeleteAllFlags" if self.is_class(this, "FlagBase") => {
                self.flag_store(this).flags.clear();
                Void
            }

            // ---- DeusExPlayer: object factories (DeusEx 10014640..10014c30) and small services: made in the player's
            // outer (the level's package), the game directory in the transient package
            "CreateHistoryObject" | "CreateHistoryEvent" | "CreateLogObject" | "CreateDataVaultImageNoteObject" | "CreateDumpLocationObject"
            | "CreateGameDirectoryObject" => {
                let class = match name {
                    "CreateHistoryObject" => "ConHistory",
                    "CreateHistoryEvent" => "ConHistoryEvent",
                    "CreateLogObject" => "DeusExLog",
                    "CreateDataVaultImageNoteObject" => "DataVaultImageNote",
                    "CreateDumpLocationObject" => "DumpLocation",
                    _ => "GameDirectory",
                };
                let outer = if name == "CreateGameDirectoryObject" { 0 } else { self.outer_of(this) };
                return Some(self.make_object_in(class, outer));
            }
            // build stamp then version, from the original's `%s %s` format (separator UNVERIFIED)
            "GetDeusExVersion" => Str("Mon Mar 19 12:06:14 2001 v1.112fm".into()),
            "UnloadTexture" => Void,
            "GetEnum" => {
                let path = match a.first() {
                    Some(Obj(r)) => self.reg.obj_path(ue_registry::ObjId(*r)).to_string(),
                    _ => String::new(),
                };
                let i = match a.get(1) { None | Some(Value::Void) => -1, Some(v) => v.as_i32().unwrap_or(-1) };
                let n = self.reg.enum_value_names(&path).and_then(|v| usize::try_from(i).ok().and_then(|i| v.get(i).copied())).unwrap_or(0);
                Name(n)
            }
            // no window system yet: the player simply has no root window (scripts that need one check for None or fail visibly)
            "InitRootWindow" => {
                if let Err(e) = self.ui_construct_root_window(this) {
                    self.log.push(format!("InitRootWindow failed: {e}"));
                }
                Void
            }
            "PreRenderWindows" | "PostRenderWindows" => Void,

            // ---- DumpLocation: no saved dump locations (the feature belongs to the multi-level "dump" cheat/travel files)
            "HasLocationBeenSaved" | "OpenDumpFile" | "GetFirstDumpFileLocation" | "GetNextDumpFileLocation" | "SelectDumpFileLocation"
                if self.is_class(this, "DumpLocation") =>
            {
                Bool(false)
            }
            "GetFirstDumpFile" | "GetNextDumpFile" | "GetCurrentUser" if self.is_class(this, "DumpLocation") => Str(String::new()),
            "GetDumpFileIndex" | "GetDumpFileCount" | "GetDumpLocationIndex" | "GetDumpFileLocationCount" | "GetNextDumpFileLocationID"
                if self.is_class(this, "DumpLocation") =>
            {
                Int(0)
            }
            "CloseDumpFile" | "DeleteDumpFile" | "GetDumpFileLocationInfo" | "DeleteDumpFileLocation" | "AddDumpFileLocation" | "SetPlayer" | "SaveLocation"
            | "LoadLocation"
                if self.is_class(this, "DumpLocation") =>
            {
                Void
            }
            _ => return None,
        };
        Some(Ok(v))
    }
}

impl Vm {
    /// The `GameDirectory` natives (DeusEx 100171e0..10018290). Every one but `GetGameDirectory` works on the last
    /// listing whatever its type; the filter is stored and never used.
    fn game_directory(&mut self, this: u32, name: &str, a: &mut [Value]) -> Res<Value> {
        use Value::*;
        let int = |i: usize| a.get(i).and_then(Value::as_i32);
        self.ui.game_dirs.entry(this).or_default();
        Ok(match name {
            "SetDirType" => {
                self.game_dir(this).kind = int(0).unwrap_or(0);
                Void
            }
            "SetDirFilter" => {
                self.game_dir(this).filter = match a.first() {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                Void
            }
            "GetGameDirectory" => {
                match self.game_dir(this).kind {
                    0 => {
                        let list = self.list_maps();
                        self.game_dir(this).list = list;
                    }
                    1 => {
                        let list = self.list_saves();
                        self.game_dir(this).list = list;
                    }
                    _ => {}
                }
                Void
            }
            "GetDirCount" => Int(self.game_dir(this).list.len() as i32),
            // past the end: a null string (also for a negative index, where the original reads before its array)
            "GetDirFilename" => {
                let d = self.game_dir(this);
                Str(int(0).and_then(|i| usize::try_from(i).ok()).and_then(|i| d.list.get(i).cloned()).unwrap_or_default())
            }
            "GetNewSaveFileIndex" => Int(self.new_save_index(this)),
            "GenerateSaveFilename" => Str(self.save_filename(int(0).unwrap_or(0))),
            // the saved games listed again; an omitted index (-1) is the next free one
            "GenerateNewSaveFilename" => {
                let list = self.list_saves();
                self.game_dir(this).list = list;
                let i = match int(0) {
                    None | Some(-1) => self.new_save_index(this),
                    Some(i) => i,
                };
                Str(self.save_filename(i))
            }
            "GetSaveFreeSpace" => {
                // the save folder, or the folder it would be made in
                let dir = self.save_dir.clone();
                let dir = dir.as_deref().map(|d| d.ancestors().find(|a| a.exists()).unwrap_or(d).to_path_buf());
                Int(crate::save_dir::save_free_space(dir.as_deref()))
            }
            "GetSaveDirectorySize" => Int(0),
            // (DeusEx 10018120) made once per directory object, in the transient package
            "GetTempSaveInfo" => {
                let held = self.game_dir(this).temp_info;
                if held != 0 && self.is_live(held) {
                    Obj(held)
                } else {
                    let o = match self.make_object_in("DeusExSaveInfo", 0)? {
                        Obj(o) => o,
                        _ => 0,
                    };
                    self.game_dir(this).temp_info = o;
                    Obj(o)
                }
            }
            "GetSaveInfo" | "GetSaveInfoFromDirectoryIndex" => Obj(0),
            _ => Void,
        })
    }

    fn game_dir(&mut self, this: u32) -> &mut crate::save_dir::GameDir {
        self.ui.game_dirs.entry(this).or_default()
    }

    /// `GetMapsDirectory` (DeusEx 10017200): the files of every `Paths` folder whose pattern holds `*.<DefaultMapExt>`,
    /// names with their extension, each name once (compared ignoring case), in each folder's listing order.
    fn list_maps(&mut self) -> Vec<String> {
        let ext = self.reg.config_value("System", "URL", "MapExt").map(|e| ue_config::unquote(&e)).unwrap_or_else(|| "dx".into());
        let pattern = format!("*.{ext}");
        let paths: Vec<String> = match self.reg.config.as_mut() {
            Some(c) => c.values("System", "Core.System", "Paths"),
            None => vec![format!("..\\Maps\\{pattern}")],
        };
        let Some(system) = self.reg.game_dir().map(|g| g.join("System")) else { return Vec::new() };
        let mut list: Vec<String> = Vec::new();
        for p in paths {
            let Some(at) = p.find(&pattern) else { continue };
            let dir = system.join(p[..at].replace('\\', "/"));
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut found: Vec<String> = rd
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| n.rsplit_once('.').is_some_and(|(_, x)| x.eq_ignore_ascii_case(&ext)))
                .collect();
            crate::save_dir::listing_order(&mut found);
            for n in found {
                if !list.iter().any(|l| l.eq_ignore_ascii_case(&n)) {
                    list.push(n);
                }
            }
        }
        list
    }

    /// `GetSaveGamesDirectory` (DeusEx 10017520): the folders `Save*` of the save folder, in listing order.
    fn list_saves(&self) -> Vec<String> {
        let Some(dir) = &self.save_dir else { return Vec::new() };
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .filter(|n| n.len() >= 4 && n[..4].eq_ignore_ascii_case("save"))
                    .collect()
            })
            .unwrap_or_default();
        crate::save_dir::listing_order(&mut v);
        v
    }

    /// `GetNewSaveFileIndex` (DeusEx 10017cc0): one past the highest index among the listed names (1 for none).
    fn new_save_index(&mut self, this: u32) -> i32 {
        self.game_dir(this).list.iter().filter_map(|n| crate::save_dir::listed_save_index(n)).fold(0, i32::max) + 1
    }

    /// `GenerateSaveFilename` (DeusEx 10017d60): `Save<index, 4 digits>.<DefaultSaveExt>`.
    fn save_filename(&mut self, index: i32) -> String {
        let ext = self.reg.config_value("System", "URL", "SaveExt").map(|e| ue_config::unquote(&e)).unwrap_or_else(|| "dxs".into());
        format!("Save{index:04}.{ext}")
    }

    /// `XExtString` (Extension 10044170..10044ad0): a long text handed out in parts of 239 characters
    /// (`GetFirstTextPart`/`GetNextTextPart` with the page in `speechPage`) or any part up to 240 (`GetTextPart`).
    fn native_ext_string(&mut self, this: u32, name: &str, a: &mut [Value]) -> Option<Value> {
        const PAGE: i32 = 0xef;
        let text = || match self.get_prop(this, "text", 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        };
        let arg = |a: &[Value], i: usize| match a.get(i) {
            Some(Value::Str(s)) => s.clone(),
            Some(v) => self.value_text(v),
            None => String::new(),
        };
        let mid = |t: &str, start: i32, n: i32| -> String { t.chars().skip(start.max(0) as usize).take(n.max(0) as usize).collect() };
        Some(match name {
            "SetText" => {
                let s = arg(a, 0);
                let _ = self.set_prop(this, "text", 0, Value::Str(s));
                Value::Void
            }
            "AppendText" => {
                let s = text() + &arg(a, 0);
                let _ = self.set_prop(this, "text", 0, Value::Str(s));
                Value::Void
            }
            "GetText" => Value::Str(text()),
            "GetTextLength" => Value::Int(text().chars().count() as i32),
            "GetTextPart" => {
                let t = text();
                let len = t.chars().count() as i32;
                let start = a.first().and_then(Value::as_i32).unwrap_or(0);
                let mut n = if start < 0 { 0 } else { a.get(1).and_then(Value::as_i32).unwrap_or(0) };
                n = n.min(len - start).min(0xf0).max(0);
                if let Some(x) = a.get_mut(2) {
                    *x = Value::Str(if n == 0 { String::new() } else { mid(&t, start, n) });
                }
                Value::Int(n)
            }
            "GetFirstTextPart" => {
                let t = text();
                let n = (t.chars().count() as i32).min(PAGE);
                let _ = self.set_prop(this, "speechPage", 0, Value::Int(0));
                if let Some(x) = a.get_mut(0) {
                    *x = Value::Str(mid(&t, 0, n));
                }
                Value::Int(n)
            }
            "GetNextTextPart" => {
                let t = text();
                let len = t.chars().count() as i32;
                let page = self.get_prop(this, "speechPage", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0) + 1;
                let _ = self.set_prop(this, "speechPage", 0, Value::Int(page));
                if len / PAGE < page {
                    // the out text stays as it was
                    return Some(Value::Int(0));
                }
                let n = (len - page * PAGE).min(PAGE);
                if let Some(x) = a.get_mut(0) {
                    *x = Value::Str(mid(&t, page * PAGE, n));
                }
                Value::Int(n)
            }
            _ => return None,
        })
    }
}

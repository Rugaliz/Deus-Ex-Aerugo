//! Class registry built from Unreal Engine 1 script packages (`.u`).
//!
//! Loads packages in the order the game configures them, resolves inheritance across packages, computes the
//! in-memory layout of every class, struct and function frame, and can create instances filled with default values.

mod defaults;
mod config;
mod layout;

use std::collections::HashMap;

pub mod fxhash;
pub use fxhash::{FxHashMap, FxHashSet};

/// Script packages built into the port, loaded after the game's own (`aerugo/`: the port's menu choices, MIT).
pub const BUILTIN_PACKAGES: [(&str, &[u8]); 1] = [("Aerugo", include_bytes!("../../../aerugo/Aerugo.u"))];
use std::path::{Path, PathBuf};
use ue_package::object::{FunctionObject, Object, PropertyKind, PropertyObject, StateObject, Tag};
use ue_package::{ObjectRef, Package};

pub use defaults::DefaultError;

#[derive(Debug)]
pub enum Error {
    Package(PathBuf, ue_package::Error),
    Io(std::io::Error),
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Package(p, e) => write!(f, "{}: {e}", p.display()),
            Error::Io(e) => write!(f, "io error: {e}"),
            Error::Invalid(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<ue_package::Error> for Error {
    fn from(e: ue_package::Error) -> Self {
        Error::Package(PathBuf::new(), e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Case-insensitive string interner (Unreal names compare case-insensitively). Id 0 is always "None".
#[derive(Debug, Default)]
pub struct Interner {
    map: FxHashMap<String, u32>,
    /// Every spelling seen so far, as written: most lookups use a spelling the scripts or the host used before, which
    /// finds the id without making a lowercase copy.
    exact: FxHashMap<String, u32>,
    strings: Vec<String>,
}

impl Interner {
    fn new() -> Self {
        let mut i = Interner::default();
        i.intern("None");
        i
    }

    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.exact.get(s) {
            return id;
        }
        let key = s.to_ascii_lowercase();
        let id = match self.map.get(&key) {
            Some(&id) => id,
            None => {
                let id = self.strings.len() as u32;
                self.strings.push(s.to_string());
                self.map.insert(key, id);
                id
            }
        };
        self.exact.insert(s.to_string(), id);
        id
    }

    pub fn get(&self, s: &str) -> Option<u32> {
        if let Some(&id) = self.exact.get(s) {
            return Some(id);
        }
        self.map.get(&s.to_ascii_lowercase()).copied()
    }

    pub fn str(&self, id: u32) -> &str {
        self.strings.get(id as usize).map(String::as_str).unwrap_or("<bad>")
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);
    };
}
id_type!(ClassId);
id_type!(StructId);
// Identifier of an interned object path such as `Engine.Actor`; 0 means none.
id_type!(ObjId);

/// What a property points at, where relevant.
#[derive(Debug, Clone)]
pub enum Target {
    None,
    Enum(String),
    Object(ObjId),
    Class { class: ObjId, meta: ObjId },
    Struct(StructId),
    Inner(Box<PropDef>),
}

#[derive(Debug, Clone)]
pub struct PropDef {
    pub name: u32,
    pub kind: PropertyKind,
    /// Element count (1 unless a fixed array).
    pub dim: u32,
    pub flags: u32,
    pub offset: u32,
    /// Size of one element in the in-memory layout.
    pub elem_size: u32,
    /// For bool properties: the bit within the 32-bit word at `offset`.
    pub bit_mask: u32,
    pub target: Target,
    /// `RF_Public` of the property object: a `private` variable is not (`GetPropertyText`/`SetPropertyText` skip it).
    pub public: bool,
    /// The editor category (`var(Category)`, registry name id; 0 = None): `UProperty::Port` keeps native or transient
    /// members that have one.
    pub category: u32,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub path: String,
    pub name: u32,
    pub super_id: Option<StructId>,
    /// Own members in declaration order.
    pub props: Vec<PropDef>,
    pub size: u32,
    pub align: u32,
}

#[derive(Debug, Clone)]
pub struct FuncDef {
    pub name: u32,
    pub flags: u32,
    pub native_index: u16,
    pub oper_precedence: u8,
    /// Parameters, return value and locals in declaration order, laid out as a frame starting at offset 0.
    pub props: Vec<PropDef>,
    pub frame_size: u32,
    /// Where the compiled code lives: (package index, export index).
    pub export: (usize, usize),
    pub script_mem_size: u32,
}

#[derive(Debug, Clone)]
pub struct StateDef {
    pub name: u32,
    pub parent: Option<u32>,
    pub flags: u32,
    pub probe_mask: u64,
    pub ignore_mask: u64,
    pub label_table_offset: u16,
    pub functions: Vec<FuncDef>,
    pub export: (usize, usize),
    /// Declared size of the state's own code (labels and state statements) in memory.
    pub script_mem_size: u32,
    /// Parent state (`state B extends A`, or the same-named state of the superclass): (class, state index).
    pub super_state: Option<(ClassId, usize)>,
    super_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: u32,
    pub values: Vec<u32>,
}

/// What owns a property definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Class(ClassId),
    Struct(StructId),
    /// A function's parameter, return value or local (function export).
    Function((usize, usize)),
}

/// Where a property referenced from bytecode lives.
#[derive(Debug, Clone)]
pub struct PropLoc {
    pub owner: Owner,
    /// Index within the owner's property list (function frames: position among params/locals).
    pub index: usize,
    pub def: PropDef,
}

/// Location of a function definition: a class function, or a function inside one of its states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FuncLoc {
    pub class: ClassId,
    pub state: Option<usize>,
    pub index: usize,
}

#[derive(Debug)]
pub struct ClassDef {
    pub path: String,
    pub name: u32,
    pub package: usize,
    pub super_id: Option<ClassId>,
    pub flags: u32,
    pub within: ObjId,
    pub config_name: u32,
    /// Properties declared by this class (not inherited), with absolute offsets.
    pub props: Vec<PropDef>,
    /// Size in bytes of instances of this class, including all inherited properties.
    pub size: u32,
    pub consts: Vec<(u32, String)>,
    pub enums: Vec<EnumDef>,
    pub structs: Vec<StructId>,
    pub functions: Vec<FuncDef>,
    pub states: Vec<StateDef>,
    pub export: (usize, usize),
    default_tags: Vec<Tag>,
}

/// One loaded `.u` package.
pub struct LoadedPackage {
    pub name: String,
    pub pkg: Package,
    /// Local name-table index -> registry-wide name id.
    pub name_map: Vec<u32>,
}

#[derive(Debug)]
pub struct Instance {
    pub class: ClassId,
    /// Memory image laid out exactly like the original objects (strings and arrays hold handles).
    pub data: Vec<u8>,
    /// Backing storage for string properties; the property slot holds the index plus one (0 = empty).
    pub strings: Vec<String>,
}

pub struct Registry {
    pub names: Interner,
    pub packages: Vec<LoadedPackage>,
    pub classes: Vec<ClassDef>,
    pub structs: Vec<StructDef>,
    obj_paths: Interner,
    class_by_path: FxHashMap<String, ClassId>,
    /// Lower-case short class name -> the first class of that name in load order.
    class_by_short: FxHashMap<String, ClassId>,
    struct_by_path: FxHashMap<String, StructId>,
    /// lower-case "Pkg.Path" -> (package index, export index), for all exports of loaded packages.
    exports_by_path: FxHashMap<String, (usize, usize)>,
    prop_exports: FxHashMap<(usize, usize), PropLoc>,
    func_exports: FxHashMap<(usize, usize), FuncLoc>,
    natives_by_index: FxHashMap<u16, Vec<FuncLoc>>,
    state_exports: FxHashMap<(usize, usize), (ClassId, usize)>,
    /// Ini files and localised text applied to class defaults (see `config.rs`); none = package defaults only.
    pub config: Option<ue_config::Config>,
    /// The game folder, when loaded from one (resource packages are found under it on demand).
    game_dir: Option<std::path::PathBuf>,
    config_cache: HashMap<ClassId, Vec<(PropDef, u32, String)>>,
    /// Defaults changed at run time (`set Class Prop Value`), applied to the class and everything derived from it.
    runtime_defaults: HashMap<ClassId, Vec<(String, String)>>,
    /// `find_prop` answers by (class, name id): the class that declares it and the index in its list.
    prop_lookup: std::cell::RefCell<FxHashMap<(ClassId, u32), Option<(ClassId, usize)>>>,
    /// `find_function_in_state` answers by (class, state, name id).
    func_lookup: std::cell::RefCell<FxHashMap<(ClassId, Option<(ClassId, usize)>, u32), Option<FuncLoc>>>,
}

/// Package names listed in `[Editor.EditorEngine]` as `EditPackages=`, in order.
pub fn edit_packages(ini: &str) -> Vec<String> {
    ini.lines()
        .filter_map(|l| l.trim().strip_prefix("EditPackages="))
        .map(|s| s.trim().to_string())
        .collect()
}

impl Registry {
    /// Loads the script packages the game configures (in `System/DeusEx.ini`) from `<game>/System/`.
    /// The game folder this registry was loaded from.
    pub fn game_dir(&self) -> Option<&std::path::Path> {
        self.game_dir.as_deref()
    }

    pub fn load_game(game_dir: &Path) -> Result<Registry> {
        let sys = game_dir.join("System");
        let ini = std::fs::read(sys.join("DeusEx.ini")).map_err(Error::Io)?;
        let ini = String::from_utf8_lossy(&ini).into_owned();
        let mut names = edit_packages(&ini);
        if names.is_empty() {
            return Err(Error::Invalid("no EditPackages found in DeusEx.ini".into()));
        }
        // the port's own script package (menu choices of its settings), built in
        for (name, _) in BUILTIN_PACKAGES {
            if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                names.push(name.to_string());
            }
        }
        let mut reg = Registry::load_packages(&sys, &names)?;
        reg.game_dir = Some(game_dir.to_path_buf());
        Ok(reg)
    }

    pub fn load_packages(system_dir: &Path, names: &[String]) -> Result<Registry> {
        let mut reg = Registry {
            names: Interner::new(),
            packages: Vec::new(),
            classes: Vec::new(),
            structs: Vec::new(),
            obj_paths: Interner::default(),
            class_by_path: FxHashMap::default(),
            class_by_short: FxHashMap::default(),
            struct_by_path: FxHashMap::default(),
            exports_by_path: FxHashMap::default(),
            prop_exports: FxHashMap::default(),
            func_exports: FxHashMap::default(),
            natives_by_index: FxHashMap::default(),
            config: None,
            game_dir: None,
            config_cache: HashMap::new(),
            runtime_defaults: HashMap::new(),
            state_exports: FxHashMap::default(),
            prop_lookup: Default::default(),
            func_lookup: Default::default(),
        };
        reg.obj_paths.intern("<none>");
        for n in names {
            let path = system_dir.join(format!("{n}.u"));
            let builtin = BUILTIN_PACKAGES.iter().find(|(b, _)| b.eq_ignore_ascii_case(n)).map(|(_, bytes)| *bytes);
            let pkg = match builtin {
                // a built-in package unless the folder has its own (a development build of it)
                Some(bytes) if !path.exists() => Package::parse(bytes.to_vec()).map_err(|e| Error::Package(path.clone(), e))?,
                _ => Package::open(&path).map_err(|e| Error::Package(path.clone(), e))?,
            };
            let name_map = pkg.names.iter().map(|e| reg.names.intern(&e.name)).collect();
            reg.packages.push(LoadedPackage { name: n.clone(), pkg, name_map });
        }
        reg.index_exports();
        reg.build_classes()?;
        Ok(reg)
    }

    fn index_exports(&mut self) {
        for pi in 0..self.packages.len() {
            self.index_package(pi);
        }
    }

    fn index_package(&mut self, pi: usize) {
        let lp = &self.packages[pi];
        for ei in 0..lp.pkg.exports.len() {
            if let Some(p) = lp.pkg.object_path(ObjectRef::Export(ei)) {
                let key = format!("{}.{}", lp.name, p).to_ascii_lowercase();
                self.exports_by_path.entry(key).or_insert((pi, ei));
            }
        }
    }

    /// Opens another package (for example a map) so that its objects can be instantiated; returns its package index.
    /// Packages added this way contribute no classes.
    pub fn add_package(&mut self, path: &Path, name: &str) -> Result<usize> {
        // a package's name is a name: it is spelled as the name table first saw it (a map `DX` loaded after a package
        // with the name `dx` is `dx`, as in the original's paths)
        let id = self.names.intern(name);
        let name = &self.names.str(id).to_string();
        let pkg = Package::open(path).map_err(|e| Error::Package(path.to_path_buf(), e))?;
        let name_map = pkg.names.iter().map(|e| self.names.intern(&e.name)).collect();
        self.packages.push(LoadedPackage { name: name.to_string(), pkg, name_map });
        let pi = self.packages.len() - 1;
        self.index_package(pi);
        Ok(pi)
    }

    /// Makes sure the package named by the first component of an object path (`DXFonts.Foo`) is loaded, looking for a
    /// resource package (`.utx`, `.uax`, `.umx`, `.u`, `.dx`) in the game's `Textures`, `Sounds`, `Music`, `System` and `Maps`
    /// folders. Returns whether the package is loaded now.
    pub fn ensure_package_for_path(&mut self, path: &str) -> bool {
        let Some((pkg, _)) = path.split_once('.') else { return false };
        if self.packages.iter().any(|p| p.name.eq_ignore_ascii_case(pkg)) {
            return true;
        }
        let Some(game) = self.game_dir.clone() else { return false };
        for dir in ["Textures", "Sounds", "Music", "System", "Maps"] {
            let Ok(rd) = std::fs::read_dir(game.join(dir)) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let ok_ext = p.extension().map(|x| ["utx", "uax", "umx", "u", "dx"].iter().any(|k| x.eq_ignore_ascii_case(k))).unwrap_or(false);
                if ok_ext && p.file_stem().map(|s| s.to_string_lossy().eq_ignore_ascii_case(pkg)).unwrap_or(false) {
                    return self.add_package(&p, pkg).is_ok();
                }
            }
        }
        false
    }

    /// The class of export `ei` of package `pi` (an object's class reference), if it is a loaded class.
    pub fn class_of_export(&self, pi: usize, ei: usize) -> Option<ClassId> {
        let r = self.packages[pi].pkg.exports.get(ei)?.class;
        self.class_by_path(&self.ref_path(pi, r)?)
    }

    /// Full path (`Package.Group.Name`) of a reference inside package `pi`.
    pub fn ref_path(&self, pi: usize, r: i32) -> Option<String> {
        let lp = &self.packages[pi];
        match ObjectRef::from_index(r) {
            ObjectRef::None => None,
            ObjectRef::Import(i) => lp.pkg.object_path(ObjectRef::Import(i)),
            ObjectRef::Export(i) => lp.pkg.object_path(ObjectRef::Export(i)).map(|p| format!("{}.{}", lp.name, p)),
        }
    }

    pub fn obj_id(&mut self, path: &str) -> ObjId {
        ObjId(self.obj_paths.intern(path))
    }

    /// The id of an object path something has referenced already (no new id is made).
    pub fn known_obj(&self, path: &str) -> Option<ObjId> {
        self.obj_paths.get(path).map(ObjId)
    }

    pub fn obj_path(&self, id: ObjId) -> &str {
        self.obj_paths.str(id.0)
    }

    pub fn class_by_path(&self, path: &str) -> Option<ClassId> {
        self.class_by_path.get(&path.to_ascii_lowercase()).copied()
    }

    /// Looks a class up by short name (`Actor`) searching all packages in load order.
    pub fn class_by_name(&self, name: &str) -> Option<ClassId> {
        if name.bytes().any(|b| b.is_ascii_uppercase()) {
            // lowercased on the stack (this runs in hot loops)
            let mut buf = [0u8; 64];
            if name.len() <= buf.len() {
                let b = &mut buf[..name.len()];
                b.copy_from_slice(name.as_bytes());
                b.make_ascii_lowercase();
                return std::str::from_utf8(b).ok().and_then(|n| self.class_by_short.get(n).copied());
            }
            self.class_by_short.get(&name.to_ascii_lowercase()).copied()
        } else {
            self.class_by_short.get(name).copied()
        }
    }

    pub fn class(&self, id: ClassId) -> &ClassDef {
        &self.classes[id.0 as usize]
    }

    pub fn struct_def(&self, id: StructId) -> &StructDef {
        &self.structs[id.0 as usize]
    }

    /// The number of classes (ids `0..class_count()`).
    pub fn class_count(&self) -> usize {
        self.classes.len()
    }

    /// The class and its ancestors, root first.
    pub fn chain(&self, id: ClassId) -> Vec<ClassId> {
        let mut v = vec![id];
        let mut cur = self.class(id).super_id;
        while let Some(s) = cur {
            v.push(s);
            cur = self.class(s).super_id;
        }
        v.reverse();
        v
    }

    /// Finds a property by name in the class or any ancestor.
    pub fn find_prop(&self, class: ClassId, name: &str) -> Option<&PropDef> {
        let id = self.names.get(name)?;
        self.find_prop_id(class, id)
    }

    /// `find_prop` by name id: the most derived declaration (cached).
    pub fn find_prop_id(&self, class: ClassId, id: u32) -> Option<&PropDef> {
        if let Some(hit) = self.prop_lookup.borrow().get(&(class, id)) {
            return hit.map(|(c, i)| &self.class(c).props[i]);
        }
        let mut found = None;
        let mut cur = Some(class);
        while let Some(c) = cur {
            if let Some(i) = self.class(c).props.iter().position(|p| p.name == id) {
                found = Some((c, i));
                break;
            }
            cur = self.class(c).super_id;
        }
        self.prop_lookup.borrow_mut().insert((class, id), found);
        found.map(|(c, i)| &self.class(c).props[i])
    }

    /// All properties of a class including inherited ones, root class first, in declaration order.
    pub fn all_props(&self, class: ClassId) -> Vec<&PropDef> {
        self.chain(class).into_iter().flat_map(|c| self.class(c).props.iter()).collect()
    }

    fn build_classes(&mut self) -> Result<()> {
        // Collect class exports first so superclasses can be resolved across packages.
        let mut pending: Vec<(usize, usize)> = Vec::new();
        for (pi, lp) in self.packages.iter().enumerate() {
            for ei in 0..lp.pkg.exports.len() {
                if lp.pkg.export_class_name(ei) == "Class" {
                    pending.push((pi, ei));
                }
            }
        }
        let mut by_path: HashMap<String, (usize, usize)> = HashMap::new();
        for &(pi, ei) in &pending {
            if let Some(p) = self.ref_path(pi, (ei + 1) as i32) {
                by_path.insert(p.to_ascii_lowercase(), (pi, ei));
            }
        }
        let mut order: Vec<(usize, usize)> = Vec::new();
        let mut state: HashMap<(usize, usize), u8> = HashMap::new();
        for &c in &pending {
            self.topo(c, &by_path, &mut state, &mut order)?;
        }
        for (pi, ei) in order {
            self.build_class(pi, ei)?;
        }
        self.resolve_state_supers();
        Ok(())
    }

    fn topo(
        &self,
        c: (usize, usize),
        by_path: &HashMap<String, (usize, usize)>,
        state: &mut HashMap<(usize, usize), u8>,
        order: &mut Vec<(usize, usize)>,
    ) -> Result<()> {
        match state.get(&c) {
            Some(2) => return Ok(()),
            Some(1) => return Err(Error::Invalid("class inheritance cycle".into())),
            _ => {}
        }
        state.insert(c, 1);
        let Object::Class(co) = self.packages[c.0].pkg.load_object(c.1).map_err(Error::from)? else {
            return Err(Error::Invalid("class export is not a class".into()));
        };
        if let Some(sp) = self.ref_path(c.0, co.field.super_ref) {
            if let Some(&sup) = by_path.get(&sp.to_ascii_lowercase()) {
                self.topo(sup, by_path, state, order)?;
            }
        }
        state.insert(c, 2);
        order.push(c);
        Ok(())
    }

    fn build_class(&mut self, pi: usize, ei: usize) -> Result<()> {
        let pkg = &self.packages[pi].pkg;
        let Object::Class(co) = pkg.load_object(ei).map_err(Error::from)? else { unreachable!() };
        let path = self.ref_path(pi, (ei + 1) as i32).unwrap_or_default();
        let super_id = self
            .ref_path(pi, co.field.super_ref)
            .and_then(|sp| self.class_by_path.get(&sp.to_ascii_lowercase()).copied());
        let base = super_id.map(|s| self.class(s).size).unwrap_or(0);
        let name = self.packages[pi].name_map[self.packages[pi].pkg.exports[ei].object_name as usize];

        let mut consts = Vec::new();
        let mut enums = Vec::new();
        let mut structs = Vec::new();
        let mut functions = Vec::new();
        let mut states = Vec::new();
        let mut prop_objs: Vec<PropertyObject> = Vec::new();
        let mut prop_names: Vec<u32> = Vec::new();
        let mut prop_ex: Vec<usize> = Vec::new();
        let kids = self.packages[pi].pkg.children_of(co.body.children).map_err(Error::from)?;
        for k in kids {
            let kname = self.packages[pi].name_map[self.packages[pi].pkg.exports[k].object_name as usize];
            match self.packages[pi].pkg.load_object(k).map_err(Error::from)? {
                Object::Property(p) => {
                    prop_objs.push(p);
                    prop_names.push(kname);
                    prop_ex.push(k);
                }
                Object::Const(c) => consts.push((kname, c.value)),
                Object::Enum(e) => {
                    let values = e.names.iter().map(|&n| self.packages[pi].name_map[n as usize]).collect();
                    enums.push(EnumDef { name: kname, values });
                }
                Object::Struct(_) => {
                    let sid = self.struct_for(pi, k)?;
                    structs.push(sid);
                }
                Object::Function(f) => functions.push(self.build_function(pi, k, kname, &f)?),
                Object::State(s) => states.push(self.build_state(pi, k, kname, &s)?),
                _ => {}
            }
        }
        let (props, top, _) = self.layout_members(pi, &prop_objs, &prop_names, &prop_ex, base, 4)?;
        let id = ClassId(self.classes.len() as u32);
        let within = self.ref_path(pi, co.within).map(|p| self.obj_id(&p)).unwrap_or(ObjId(0));
        let cd = ClassDef {
            path: path.clone(),
            name,
            package: pi,
            super_id,
            flags: co.class_flags,
            within,
            config_name: self.packages[pi].name_map[co.config_name as usize],
            props,
            size: (top + 3) / 4 * 4,
            consts,
            enums,
            structs,
            functions,
            states,
            export: (pi, ei),
            default_tags: co.defaults.clone(),
        };
        for (i, &k) in prop_ex.iter().enumerate() {
            let def = cd.props[i].clone();
            self.prop_exports.insert((pi, k), PropLoc { owner: Owner::Class(id), index: i, def });
        }
        for (i, f) in cd.functions.iter().enumerate() {
            self.func_exports.insert(f.export, FuncLoc { class: id, state: None, index: i });
            if f.native_index != 0 {
                self.natives_by_index.entry(f.native_index).or_default().push(FuncLoc { class: id, state: None, index: i });
            }
        }
        for (si, st) in cd.states.iter().enumerate() {
            self.state_exports.insert(st.export, (id, si));
            for (i, f) in st.functions.iter().enumerate() {
                self.func_exports.insert(f.export, FuncLoc { class: id, state: Some(si), index: i });
            }
        }
        let lower = path.to_ascii_lowercase();
        if let Some((_, short)) = lower.rsplit_once('.') {
            self.class_by_short.entry(short.to_string()).or_insert(id);
        }
        self.class_by_path.insert(lower, id);
        self.classes.push(cd);
        Ok(())
    }

    fn build_function(&mut self, pi: usize, ei: usize, name: u32, f: &FunctionObject) -> Result<FuncDef> {
        let kids = self.packages[pi].pkg.children_of(f.body.children).map_err(Error::from)?;
        let mut objs = Vec::new();
        let mut names = Vec::new();
        let mut exs = Vec::new();
        for k in kids {
            if let Object::Property(p) = self.packages[pi].pkg.load_object(k).map_err(Error::from)? {
                names.push(self.packages[pi].name_map[self.packages[pi].pkg.exports[k].object_name as usize]);
                objs.push(p);
                exs.push(k);
            }
        }
        let (props, top, _) = self.layout_members(pi, &objs, &names, &exs, 0, 1)?;
        for (i, &k) in exs.iter().enumerate() {
            self.prop_exports.insert((pi, k), PropLoc { owner: Owner::Function((pi, ei)), index: i, def: props[i].clone() });
        }
        Ok(FuncDef {
            name,
            flags: f.flags,
            native_index: f.native_index,
            oper_precedence: f.oper_precedence,
            props,
            frame_size: (top + 3) / 4 * 4,
            export: (pi, ei),
            script_mem_size: f.body.script_mem_size as u32,
        })
    }

    fn build_state(&mut self, pi: usize, ei: usize, name: u32, s: &StateObject) -> Result<StateDef> {
        let kids = self.packages[pi].pkg.children_of(s.body.children).map_err(Error::from)?;
        let mut functions = Vec::new();
        for k in kids {
            let kname = self.packages[pi].name_map[self.packages[pi].pkg.exports[k].object_name as usize];
            if let Object::Function(f) = self.packages[pi].pkg.load_object(k).map_err(Error::from)? {
                functions.push(self.build_function(pi, k, kname, &f)?);
            }
        }
        let parent = self
            .ref_path(pi, s.field.super_ref)
            .and_then(|p| p.rsplit('.').next().map(|n| self.names.get(n)))
            .flatten();
        Ok(StateDef {
            name,
            parent,
            flags: s.state_flags,
            probe_mask: s.probe_mask,
            ignore_mask: s.ignore_mask,
            label_table_offset: s.label_table_offset,
            functions,
            export: (pi, ei),
            script_mem_size: s.body.script_mem_size as u32,
            super_state: None,
            super_path: self.ref_path(pi, s.field.super_ref),
        })
    }
}

/// Typed reads of properties from an instance's memory image.
impl Registry {
    fn slot(&self, inst: &Instance, name: &str, elem: u32) -> Option<(PropDef, usize)> {
        let p = self.find_prop(inst.class, name)?.clone();
        if elem >= p.dim {
            return None;
        }
        let off = (p.offset + elem * p.elem_size) as usize;
        Some((p, off))
    }

    pub fn get_i32(&self, inst: &Instance, name: &str, elem: u32) -> Option<i32> {
        let (_, o) = self.slot(inst, name, elem)?;
        Some(i32::from_le_bytes(inst.data.get(o..o + 4)?.try_into().ok()?))
    }

    pub fn get_f32(&self, inst: &Instance, name: &str, elem: u32) -> Option<f32> {
        self.get_i32(inst, name, elem).map(|v| f32::from_bits(v as u32))
    }

    pub fn get_u8(&self, inst: &Instance, name: &str, elem: u32) -> Option<u8> {
        let (_, o) = self.slot(inst, name, elem)?;
        inst.data.get(o).copied()
    }

    pub fn get_bool(&self, inst: &Instance, name: &str) -> Option<bool> {
        let (p, o) = self.slot(inst, name, 0)?;
        let w = u32::from_le_bytes(inst.data.get(o..o + 4)?.try_into().ok()?);
        Some(w & p.bit_mask != 0)
    }

    /// Value of a name property as text.
    pub fn get_name(&self, inst: &Instance, name: &str, elem: u32) -> Option<&str> {
        self.get_i32(inst, name, elem).map(|id| self.names.str(id as u32))
    }

    pub fn get_string<'a>(&self, inst: &'a Instance, name: &str, elem: u32) -> Option<&'a str> {
        let h = self.get_i32(inst, name, elem)? as usize;
        if h == 0 {
            Some("")
        } else {
            inst.strings.get(h - 1).map(String::as_str)
        }
    }
}

/// Lookups used by the script compiler and interpreter.
impl Registry {
    /// Property definition referenced by object reference `r` inside package `pi`.
    pub fn prop_loc(&self, pi: usize, r: i32) -> Option<&PropLoc> {
        match ObjectRef::from_index(r) {
            ObjectRef::Export(i) => self.prop_exports.get(&(pi, i)),
            ObjectRef::Import(_) => {
                let path = self.ref_path(pi, r)?;
                let key = self.exports_by_path.get(&path.to_ascii_lowercase())?;
                self.prop_exports.get(key)
            }
            ObjectRef::None => None,
        }
    }

    /// Function referenced by object reference `r` inside package `pi`.
    pub fn func_loc(&self, pi: usize, r: i32) -> Option<FuncLoc> {
        match ObjectRef::from_index(r) {
            ObjectRef::Export(i) => self.func_exports.get(&(pi, i)).copied(),
            ObjectRef::Import(_) => {
                let path = self.ref_path(pi, r)?;
                let key = self.exports_by_path.get(&path.to_ascii_lowercase())?;
                self.func_exports.get(key).copied()
            }
            ObjectRef::None => None,
        }
    }

    pub fn function(&self, l: FuncLoc) -> &FuncDef {
        let c = self.class(l.class);
        match l.state {
            None => &c.functions[l.index],
            Some(s) => &c.states[s].functions[l.index],
        }
    }

    /// Finds a function by name in the class or its ancestors (state-independent), nearest class first.
    pub fn find_function(&self, class: ClassId, name: u32) -> Option<FuncLoc> {
        let mut cur = Some(class);
        while let Some(c) = cur {
            let cd = self.class(c);
            if let Some(i) = cd.functions.iter().position(|f| f.name == name) {
                return Some(FuncLoc { class: c, state: None, index: i });
            }
            cur = cd.super_id;
        }
        None
    }

    /// Native functions registered under an index (usually one; a few indices are reused by different classes).
    pub fn natives_by_index(&self, index: u16) -> &[FuncLoc] {
        self.natives_by_index.get(&index).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Serialized code bytes of a function or state and the range of the code within them.
    pub fn function_code(&self, export: (usize, usize)) -> Result<(Vec<u8>, std::ops::Range<usize>)> {
        let pkg = &self.packages[export.0].pkg;
        let range = match pkg.load_object(export.1).map_err(Error::from)? {
            Object::Function(f) => f.body.script,
            Object::State(s) => s.body.script,
            _ => return Err(Error::Invalid("export is not a function or state".into())),
        };
        let data = pkg.export_bytes(export.1).map_err(Error::from)?.to_vec();
        Ok((data, range))
    }

    /// Registry-wide id of a name-table entry of package `pi`.
    pub fn global_name(&self, pi: usize, local: i32) -> u32 {
        self.packages[pi].name_map.get(local as usize).copied().unwrap_or(0)
    }

    /// Export position of a loaded object path (case-insensitive), e.g. `Core.Object.Vector`.
    pub fn export_by_path(&self, path: &str) -> Option<(usize, usize)> {
        self.exports_by_path.get(&path.to_ascii_lowercase()).copied()
    }
}

impl Registry {
    pub fn struct_id_by_path(&self, path: &str) -> Option<StructId> {
        self.struct_by_path.get(&path.to_ascii_lowercase()).copied()
    }
}

/// States: inheritance and state-aware function lookup.
impl Registry {
    fn resolve_state_supers(&mut self) {
        let mut updates: Vec<(usize, usize, (ClassId, usize))> = Vec::new();
        for (ci, c) in self.classes.iter().enumerate() {
            for (si, st) in c.states.iter().enumerate() {
                if let Some(p) = &st.super_path {
                    if let Some(key) = self.exports_by_path.get(&p.to_ascii_lowercase()) {
                        if let Some(&target) = self.state_exports.get(key) {
                            updates.push((ci, si, target));
                        }
                    }
                }
            }
        }
        for (ci, si, t) in updates {
            self.classes[ci].states[si].super_state = Some(t);
        }
    }

    pub fn state_def(&self, s: (ClassId, usize)) -> &StateDef {
        &self.class(s.0).states[s.1]
    }

    /// Finds a state by name in the class or its ancestors, nearest class first.
    pub fn find_state(&self, class: ClassId, name: u32) -> Option<(ClassId, usize)> {
        let mut cur = Some(class);
        while let Some(c) = cur {
            let cd = self.class(c);
            if let Some(i) = cd.states.iter().position(|s| s.name == name) {
                return Some((c, i));
            }
            cur = cd.super_id;
        }
        None
    }

    /// The state flagged `auto` (state flag 0x2) of the class, searching the class first, then its ancestors.
    pub fn find_auto_state(&self, class: ClassId) -> Option<(ClassId, usize)> {
        let mut cur = Some(class);
        while let Some(c) = cur {
            let cd = self.class(c);
            if let Some(i) = cd.states.iter().position(|s| s.flags & 0x2 != 0) {
                return Some((c, i));
            }
            cur = cd.super_id;
        }
        None
    }

    /// Function lookup for an object of `class` that is in `state`: the state and its parent states first,
    /// then the class and its ancestors.
    pub fn find_function_in_state(&self, class: ClassId, state: Option<(ClassId, usize)>, name: u32) -> Option<FuncLoc> {
        // classes and states do not change once loaded (later packages only add classes)
        let key = (class, state, name);
        if let Some(r) = self.func_lookup.borrow().get(&key) {
            return *r;
        }
        let r = self.find_function_in_state_uncached(class, state, name);
        self.func_lookup.borrow_mut().insert(key, r);
        r
    }

    fn find_function_in_state_uncached(&self, class: ClassId, state: Option<(ClassId, usize)>, name: u32) -> Option<FuncLoc> {
        let mut cur = state;
        let mut guard = 0;
        while let Some(s) = cur {
            let sd = self.state_def(s);
            if let Some(i) = sd.functions.iter().position(|f| f.name == name) {
                return Some(FuncLoc { class: s.0, state: Some(s.1), index: i });
            }
            cur = sd.super_state;
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        self.find_function(class, name)
    }
}

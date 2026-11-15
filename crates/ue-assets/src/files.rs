//! Locating game packages and resolving object references (imports) across them.

use crate::Payload;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use ue_package::{ObjectRef, Package};

/// Sub-folders of the game folder that hold packages.
const FOLDERS: &[&str] = &["Textures", "Maps", "System", "Sounds", "Music"];
/// File extensions of packages (the System folder also holds .dll, .ini, .int files with the same stems).
const PACKAGE_EXTENSIONS: &[&str] = &["u", "utx", "dx", "uax", "umx", "unr", "ut", "usa"];

/// A package together with a path index of its exports (`Group.Name` without the package name).
pub struct Loaded {
    pub name: String,
    pub pkg: Package,
    paths: HashMap<String, usize>,
}

impl Loaded {
    /// Export with the given dotted path inside this package.
    pub fn find(&self, path: &str) -> Option<usize> {
        self.paths.get(&path.to_ascii_lowercase()).copied()
    }
}

/// The game folder: finds packages by name and caches them.
pub struct GameFiles {
    files: HashMap<String, PathBuf>,
    cache: RefCell<HashMap<String, Rc<Loaded>>>,
}

impl GameFiles {
    pub fn new(root: &Path) -> GameFiles {
        let mut files = HashMap::new();
        for f in FOLDERS {
            let Ok(rd) = std::fs::read_dir(root.join(f)) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let ext = p.extension().and_then(|s| s.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
                if !PACKAGE_EXTENSIONS.contains(&ext.as_str()) {
                    continue;
                }
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    files.entry(stem.to_ascii_lowercase()).or_insert(p);
                }
            }
        }
        GameFiles { files, cache: RefCell::new(HashMap::new()) }
    }

    /// Opens (or returns the cached) package called `name` (file name without extension, any case).
    pub fn package(&self, name: &str) -> Option<Rc<Loaded>> {
        let key = name.to_ascii_lowercase();
        if let Some(l) = self.cache.borrow().get(&key) {
            return Some(l.clone());
        }
        let path = self.files.get(&key)?;
        let pkg = Package::open(path).ok()?;
        let mut paths = HashMap::new();
        for i in 0..pkg.exports.len() {
            if let Some(p) = pkg.object_path(ObjectRef::Export(i)) {
                // a texture's palette often has the texture's name: the path names the texture
                let key = p.to_ascii_lowercase();
                let keep_old = paths.get(&key).map(|&o| pkg.export_class_name(o) != "Palette").unwrap_or(false);
                if !keep_old {
                    paths.insert(key, i);
                }
            }
        }
        let l = Rc::new(Loaded { name: name.to_string(), pkg, paths });
        self.cache.borrow_mut().insert(key, l.clone());
        Some(l)
    }

    /// Resolves a raw object reference made inside `from` to the package and export it designates, following
    /// imports into other packages. `None` for a null reference or an unresolvable import.
    pub fn resolve(&self, from: &Rc<Loaded>, raw: i32) -> Option<(Rc<Loaded>, usize)> {
        match ObjectRef::from_index(raw) {
            ObjectRef::None => None,
            ObjectRef::Export(i) => (i < from.pkg.exports.len()).then(|| (from.clone(), i)),
            r @ ObjectRef::Import(_) => {
                let path = from.pkg.object_path(r)?;
                let (pkg_name, rest) = path.split_once('.')?;
                let other = self.package(pkg_name)?;
                let i = other.find(rest)?;
                Some((other, i))
            }
        }
    }
}

/// Reads a `Vector` or `Rotator` struct property of an object (rotator components are returned as f32 units).
pub fn vector_prop(pkg: &Package, export: usize, name: &str) -> Option<[f32; 3]> {
    let p = crate::payload(pkg, export).ok()?;
    p.vector(pkg, name)
}

/// Reads an object-reference property (compact index) of an object, e.g. a brush actor's `Brush`.
pub fn object_prop(pkg: &Package, export: usize, name: &str) -> Option<i32> {
    let p = crate::payload(pkg, export).ok()?;
    p.object(pkg, name)
}

impl Payload<'_> {
    pub fn vector(&self, pkg: &Package, name: &str) -> Option<[f32; 3]> {
        use ue_package::object::PropType;
        let t = self.tags.iter().find(|t| pkg.name(t.name) == name)?;
        let is_rot = matches!(t.ptype, PropType::Rotator) || t.struct_name.map(|s| pkg.name(s) == "Rotator").unwrap_or(false);
        let is_vec = matches!(t.ptype, PropType::Vector) || t.struct_name.map(|s| pkg.name(s) == "Vector").unwrap_or(false);
        if (!is_rot && !is_vec) || t.data.len() != 12 {
            return None;
        }
        let b = &self.data[t.data.clone()];
        let w = |i: usize| <[u8; 4]>::try_from(&b[i * 4..i * 4 + 4]).unwrap();
        Some(if is_rot {
            [i32::from_le_bytes(w(0)) as f32, i32::from_le_bytes(w(1)) as f32, i32::from_le_bytes(w(2)) as f32]
        } else {
            [f32::from_le_bytes(w(0)), f32::from_le_bytes(w(1)), f32::from_le_bytes(w(2))]
        })
    }
}

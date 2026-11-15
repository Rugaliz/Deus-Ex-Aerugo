//! `DynamicLoadObject` as Core runs it: `UObject::StaticLoadObject(Class, NULL, Name, NULL, LOAD_NoWarn | (MayFail ?
//! LOAD_Quiet : 0), NULL)` (exec 1013eb30, StaticLoadObject 10154d50).
//!
//! * `ResolveName` (10154ad0, create and throw): `ini:Section.Key` / `usr:Section.Key` (under 1024 characters, with a
//!   dot) is replaced by that value of the system / user configuration, split at the last dot; a missing key fails.
//!   Every dotted part before the last names a package (created when missing); the last part is the object's name.
//! * The outer is replaced by its outermost package. `GetPackageLinker` (10154530): no package fails
//!   (`PackageResolveFailed`), a package without a file fails (`PackageNotFound`).
//! * `ULinkerLoad::Create` (10161cc0) -> `FindExportIndex` (10161e00) with any outer: an export of that name whose
//!   class has exactly the asked class's name and package (no subclasses); a `Mesh` asked for is tried again as
//!   `LodMesh`. The export hash chains are built by inserting at the head in export order, so the last such export
//!   wins. The export's object is created, or the one already loaded returned (a map's actor: the live actor).
//! * Else `StaticFindObject(Class, Package, Name)` (10151de0): an object in memory directly inside the package (not in
//!   a group) of the class or a subclass.
//! * A failure throws; the handler (10154eb8) retries remapped packages (only `UnrealShare`/`UnrealI`, neither of which
//!   Deus Ex ships), then logs `FailedLoadObject` unless quiet and returns None.

use crate::{ref_instance, Vm};
use ue_package::ObjectRef;
use ue_registry::{ClassId, ObjId};

/// The class asked for: its path (`Core.Class`, `Fire.FireTexture`) and, unless it is one of Core's intrinsic classes
/// the registry has no script class for (`Class`), the script class.
struct Want {
    path: String,
    class: Option<ClassId>,
}

impl Want {
    fn package_and_name(&self) -> (&str, &str) {
        self.path.split_once('.').unwrap_or(("", &self.path))
    }
}

/// What the port counts as loaded for `StaticFindObject`: script classes, live objects and resources something has
/// referenced already (the original loads resources on demand; a reference made the port intern the path).
impl Vm {
    /// `DynamicLoadObject(Name, Class, MayFail)`: the object, or 0.
    pub fn dynamic_load_object(&mut self, name: &str, class: u32, may_fail: bool) -> u32 {
        // `check(ObjectClass)`: the engine stops here; None is the closest a script can see
        if class == 0 || ref_instance(class).is_some() {
            return 0;
        }
        let want = Want { path: self.reg.obj_path(ObjId(class)).to_string(), class: self.class_of_ref(class) };
        let class_path = want.path.clone();
        match self.static_load_object(&want, name) {
            Ok(r) => r,
            Err((outer, obj, err)) => {
                if !may_fail {
                    // `LOAD_NoWarn`: written to the log only (`SafeLoadError`)
                    let class_name = class_path.rsplit('.').next().unwrap_or(&class_path);
                    self.log.push(format!("Failed to load '{class_name} {outer}.{obj}': {err}"));
                }
                0
            }
        }
    }

    /// `StaticLoadObject` without a file name or remaps; `Err((outer, name, message))` as the handler reports it.
    fn static_load_object(&mut self, want: &Want, in_name: &str) -> Result<u32, (String, String, String)> {
        let fail = |outer: &str, name: &str, msg: String| (outer.to_string(), name.to_string(), msg);
        // `ResolveName`
        let mut name = in_name.to_string();
        let lower = name.get(..4).map(str::to_ascii_lowercase);
        let ini = lower.as_deref() == Some("ini:");
        if (ini || lower.as_deref() == Some("usr:")) && name.encode_utf16().count() < 0x400 && name.contains('.') {
            let rest = &name[4..];
            let dot = rest.rfind('.').expect("a dot after the prefix");
            let (section, key) = (&rest[..dot], &rest[dot + 1..]);
            match self.reg.config_value(if ini { "System" } else { "User" }, section, key) {
                // `GetString` into 1024 characters
                Some(v) => name = v.chars().take(0x3ff).collect(),
                None => return Err(fail("None", in_name, format!("Can't find '{in_name}' in configuration file"))),
            }
        }
        let mut parts: Vec<&str> = name.split('.').collect();
        let obj = parts.pop().unwrap_or("").to_string();
        let Some(&top) = parts.first() else { return Err(fail("None", &obj, "Can't resolve package name".into())) };
        let top = top.to_string();
        // `GetPackageLinker`: the package's file
        let pi = match self.reg.ensure_package_for_path(&format!("{top}.x")) {
            true => self.reg.packages.iter().position(|p| p.name.eq_ignore_ascii_case(&top)),
            false => None,
        };
        let Some(pi) = pi else { return Err(fail(&top, &obj, format!("Can't find file for package '{top}'"))) };
        if obj.is_empty() {
            return Err(fail(&top, &obj, format!("Failed to find object '{} {top}.{obj}'", want.package_and_name().1)));
        }
        let (class_pkg, class_name) = want.package_and_name();
        let mut found = self.find_export_index(pi, class_pkg, class_name, &obj);
        if found.is_none() && class_name.eq_ignore_ascii_case("Mesh") {
            found = self.find_export_index(pi, class_pkg, "LodMesh", &obj);
        }
        if let Some(ei) = found {
            if let Some(r) = self.export_object(pi, ei) {
                return Ok(r);
            }
        }
        if let Some(r) = self.find_in_package(want, pi, &top, &obj) {
            return Ok(r);
        }
        Err(fail(&top, &obj, format!("Failed to find object '{} {top}.{obj}'", want.package_and_name().1)))
    }

    /// `FindExportIndex(ClassName, ClassPackage, ObjectName, INDEX_NONE)`: the last export of package `pi` with that
    /// name whose class is exactly `ClassPackage.ClassName` (an import's package is its outer; class 0 is
    /// `Core.Class`; a class defined in the package belongs to it). Names compare without case.
    fn find_export_index(&self, pi: usize, class_pkg: &str, class_name: &str, obj: &str) -> Option<usize> {
        let lp = &self.reg.packages[pi];
        let p = &lp.pkg;
        (0..p.exports.len()).rev().find(|&i| {
            let e = &p.exports[i];
            if !p.name(e.object_name).eq_ignore_ascii_case(obj) {
                return false;
            }
            let (pkg, cname) = match ObjectRef::from_index(e.class) {
                ObjectRef::None => ("Core", "Class"),
                ObjectRef::Import(k) => {
                    let Some(im) = p.imports.get(k) else { return false };
                    let pkg = match ObjectRef::from_index(im.outer) {
                        ObjectRef::Import(o) => p.imports.get(o).map(|o| p.name(o.object_name)).unwrap_or(""),
                        _ => "",
                    };
                    (pkg, p.name(im.object_name))
                }
                ObjectRef::Export(k) => (lp.name.as_str(), p.exports.get(k).map(|x| p.name(x.object_name)).unwrap_or("")),
            };
            pkg.eq_ignore_ascii_case(class_pkg) && cname.eq_ignore_ascii_case(class_name)
        })
    }

    /// The object of export `ei` of package `pi` (`CreateExport`): a map actor is the live actor made from it (None once
    /// destroyed); everything else is referred to by its path.
    fn export_object(&mut self, pi: usize, ei: usize) -> Option<u32> {
        let path = self.reg.ref_path(pi, ei as i32 + 1)?;
        if let Some((&r, _)) = self.level_paths.iter().find(|(_, p)| p.eq_ignore_ascii_case(&path)) {
            return self.is_live(r).then_some(r);
        }
        // conversation data (`ConObject`s): the object graph as live objects, as the binding code loads it
        let con_object = self.reg.class_by_name("ConObject");
        if let (Some(c), Some(base)) = (self.reg.class_of_export(pi, ei), con_object) {
            if self.is_a(c, base) {
                let r = self.load_con_object(&path);
                if r != 0 {
                    return Some(r);
                }
            }
        }
        Some(self.reg.obj_id(&path).0)
    }

    /// `StaticFindObject(Class, Package, Name)`: an object directly inside the package, of the class or a subclass,
    /// already in memory. In the port: the package's top-level exports that are script classes, live map actors (a
    /// map file keeps its actors at the top level) or resources something has referenced, and the actors spawned into
    /// the current map's package.
    fn find_in_package(&mut self, want: &Want, pi: usize, top: &str, obj: &str) -> Option<u32> {
        let n = self.reg.packages[pi].pkg.exports.len();
        // export order is not the hash order of `GObjHash`; names are unique within one outer anyway
        for ei in 0..n {
            let e = &self.reg.packages[pi].pkg.exports[ei];
            if ObjectRef::from_index(e.outer) != ObjectRef::None || !self.reg.packages[pi].pkg.name(e.object_name).eq_ignore_ascii_case(obj) {
                continue;
            }
            let Some(path) = self.reg.ref_path(pi, ei as i32 + 1) else { continue };
            let is_class = ObjectRef::from_index(e.class) == ObjectRef::None;
            // a class object is a `Class` (an `Object`); other objects by their export's class
            let fits = if is_class {
                self.reg.class_by_path(&path).is_some()
                    && (want.path.eq_ignore_ascii_case("Core.Class") || want.path.eq_ignore_ascii_case("Core.Object"))
            } else {
                match (self.reg.class_of_export(pi, ei), want.class) {
                    (Some(have), Some(class)) => self.is_a(have, class),
                    _ => false,
                }
            };
            if !fits {
                continue;
            }
            // a map actor (top level in the map file): the live actor
            if self.level_paths.values().any(|p| p.eq_ignore_ascii_case(&path)) {
                match self.export_object(pi, ei) {
                    Some(r) => return Some(r),
                    None => continue,
                }
            }
            if is_class || self.reg.known_obj(&path).is_some() {
                return Some(self.reg.obj_id(&path).0);
            }
        }
        let map = self.level_paths.get(&self.level).and_then(|p| p.split('.').next()).map(str::to_string)?;
        if !map.eq_ignore_ascii_case(top) {
            return None;
        }
        let class = want.class?;
        let name = self.reg.names.get(obj)?;
        let actors: Vec<u32> = self.actor_slots.iter().map(|&i| crate::inst_ref(i)).collect();
        actors.into_iter().find(|&r| {
            ref_instance(r).is_some()
                && self.is_live(r)
                && self.class_of_ref(r).is_some_and(|c| self.is_a(c, class))
                && matches!(self.get_prop(r, "Name", 0), Ok(crate::Value::Name(n)) if n == name)
        })
    }
}

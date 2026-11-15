//! `new (Outer, Name, Flags) Class` as Core runs it (`execNew` 1013d9f0, `StaticConstructObject` 10157ff0,
//! `StaticAllocateObject` 101579c0), and `CriticalDelete` (1013db50).
//!
//! * The outer defaults to the object running the code (an omitted argument leaves `execNew`'s default, `this`);
//!   `None` is the transient package. The name defaults to `MakeUniqueObjectName(Outer, Class)`.
//! * Abstract classes are refused (an error, None); so is an outer that is not of the class's `within` class.
//! * A name already used in that outer by an object of the class (or a subclass) gives that object back, destroyed
//!   and built again from the class's defaults; otherwise a new object, its properties copied from the class's
//!   defaults. A per-object config class then loads its section (`LoadConfig(0)`).
//! * `CriticalDelete(obj)` deletes the object at once (its virtual destructor with delete): it is gone; the return
//!   value is never written.

use crate::property_text::Sel;
use crate::{inst_ref, ref_instance, Object, Value, Vm, VmError};
use std::collections::HashSet;
use ue_registry::ClassId;

const CLASS_ABSTRACT: u32 = 0x1;
const CLASS_PER_OBJECT_CONFIG: u32 = 0x400;

/// Which set of names an outer keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum OuterKey {
    /// The map package (actors).
    Map,
    /// The transient package.
    Transient,
    /// Another object.
    Obj(u32),
}

impl Vm {
    /// The transient package's object id.
    pub(crate) fn transient_package(&mut self) -> u32 {
        self.reg.obj_id("Transient").0
    }

    /// The `Outer` of an instance: its field, or for objects the engine made without one, the map package (actors)
    /// or the transient package (0 when neither can be named).
    pub(crate) fn outer_key_of(&self, r: u32) -> OuterKey {
        let Some(i) = ref_instance(r) else { return OuterKey::Transient };
        let outer = self.op_const(r, "Outer");
        if outer != 0 {
            return self.outer_key(outer);
        }
        if self.objects.get(i).is_some_and(|o| o.actor) {
            OuterKey::Map
        } else {
            OuterKey::Transient
        }
    }

    fn outer_key(&self, outer: u32) -> OuterKey {
        if ref_instance(outer).is_some() {
            return OuterKey::Obj(outer);
        }
        let path = self.reg.obj_path(ue_registry::ObjId(outer));
        if path.eq_ignore_ascii_case("Transient") {
            OuterKey::Transient
        } else if self.map_package_name().is_some_and(|m| m.eq_ignore_ascii_case(path)) {
            OuterKey::Map
        } else {
            OuterKey::Obj(outer)
        }
    }

    /// The current map's package name.
    pub(crate) fn map_package_name(&self) -> Option<&str> {
        self.level_paths.get(&self.level).and_then(|p| p.split('.').next())
    }

    /// The name of an object's outer, `None` for the transient package (per-object config files are named after it).
    pub(crate) fn outer_name(&self, r: u32) -> Option<String> {
        match self.outer_key_of(r) {
            OuterKey::Transient => None,
            OuterKey::Map => self.map_package_name().map(str::to_string),
            OuterKey::Obj(o) => match ref_instance(o) {
                Some(_) => match self.get_prop(o, "Name", 0) {
                    Ok(Value::Name(n)) => Some(self.reg.names.str(n).to_string()),
                    _ => None,
                },
                None => Some(self.reg.obj_path(ue_registry::ObjId(o)).rsplit('.').next().unwrap_or("").to_string()),
            },
        }
    }

    /// An object reference field read without needing `&mut`.
    fn op_const(&self, r: u32, name: &str) -> u32 {
        match self.get_prop(r, name, 0) {
            Ok(Value::Obj(o)) => o,
            _ => 0,
        }
    }

    /// The names in use in an outer.
    pub(crate) fn names_in(&mut self, key: OuterKey) -> &mut HashSet<u32> {
        match key {
            OuterKey::Map => &mut self.used_names[0],
            OuterKey::Transient => &mut self.used_names[1],
            OuterKey::Obj(o) => self.object_names.entry(o).or_default(),
        }
    }

    /// `MakeUniqueObjectName(Outer, Class)` for any outer (see `make_unique_name`).
    pub(crate) fn make_unique_name_in(&mut self, class: ClassId, key: OuterKey) -> u32 {
        let full = self.reg.names.str(self.reg.class(class).name).to_string();
        let base = full.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
        loop {
            let n = self.class_unique.entry(class).or_insert(0);
            let num = n.to_string();
            *n += 1;
            let keep: String = base.chars().take(63usize.saturating_sub(num.len())).collect();
            let id = self.reg.names.intern(&format!("{keep}{num}"));
            if self.names_in(key).insert(id) {
                return id;
            }
        }
    }

    /// `new (outer, name) class`: `outer` 0 is the transient package, `name` 0 none. Returns the object or 0.
    pub fn new_script_object(&mut self, class: ClassId, outer: u32, name: u32) -> Result<u32, VmError> {
        let cpath = self.reg.class(class).path.clone();
        if self.reg.class(class).flags & CLASS_ABSTRACT != 0 {
            // Core.int `Abstract`
            self.log.push(format!("Can't create object '{name}': class {cpath} is abstract", name = self.reg.names.str(name)));
            return Ok(0);
        }
        let outer = if outer == 0 { self.transient_package() } else { outer };
        // `ClassWithin`: the outer must be one
        let within = self.reg.obj_path(self.reg.class(class).within).to_string();
        if let Some(w) = self.reg.class_by_path(&within) {
            let object = self.reg.class_by_name("Object");
            if Some(w) != object && !(ref_instance(outer).is_some() && self.class_of_ref(outer).is_some_and(|c| self.is_a(c, w))) {
                self.log.push(format!("Object is not within {within}: {cpath}"));
                return Ok(0);
            }
        }
        let key = self.outer_key(outer);
        let name = if name == 0 {
            self.make_unique_name_in(class, key)
        } else {
            // an object of the class with that name in that outer is built again in place
            let existing = (0..self.objects.len()).map(inst_ref).find(|&r| {
                self.is_live(r)
                    && self.class_of_ref(r).is_some_and(|c| self.is_a(c, class))
                    && matches!(self.get_prop(r, "Name", 0), Ok(Value::Name(n)) if n == name)
                    && self.outer_key_of(r) == key
            });
            if let Some(r) = existing {
                let i = ref_instance(r).expect("instance");
                let c = self.objects[i].class;
                let mem = self.default_copy(c)?;
                let o = &mut self.objects[i];
                o.mem = mem;
                o.state = None;
                o.exec = None;
                o.latent = None;
                o.disabled.clear();
                self.init_header(r, c, outer, name);
                self.after_construct(i, c);
                return Ok(r);
            }
            self.names_in(key).insert(name);
            name
        };
        let mem = self.default_copy(class)?;
        let actor = self.is_actor_class(class);
        if actor {
            self.actor_slots.push(self.objects.len());
        }
        self.objects.push(Object { class, mem, state: None, exec: None, latent: None, disabled: HashSet::new(), alive: true, goto_serial: 0, actor });
        let i = self.objects.len() - 1;
        let r = inst_ref(i);
        self.init_header(r, class, outer, name);
        self.after_construct(i, class);
        Ok(r)
    }

    /// `Object.Class`, `Outer` and `Name` of a new object.
    fn init_header(&mut self, r: u32, class: ClassId, outer: u32, name: u32) {
        let path = self.reg.class(class).path.clone();
        let cid = self.reg.obj_id(&path).0;
        let _ = self.set_prop(r, "Class", 0, Value::Obj(cid));
        let _ = self.set_prop(r, "Outer", 0, Value::Obj(outer));
        self.set_object_name(r, name);
    }

    /// What `StaticAllocateObject` does after the properties: per-object config.
    fn after_construct(&mut self, i: usize, class: ClassId) {
        if self.reg.class(class).flags & CLASS_PER_OBJECT_CONFIG != 0 {
            self.load_config(Sel::Obj(i), class, false, None);
        }
    }

    /// `CriticalDelete(obj)`.
    pub(crate) fn critical_delete(&mut self, r: u32) {
        let Some(i) = ref_instance(r) else { return };
        if !self.is_live(r) {
            return;
        }
        if self.objects[i].actor {
            self.hash_remove(r);
        }
        let o = &mut self.objects[i];
        o.alive = false;
        o.exec = None;
        o.latent = None;
    }
}

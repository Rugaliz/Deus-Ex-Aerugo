//! `GetPropertyText` / `SetPropertyText` (Core 1013e0b0 / 1013e320) and the property text forms they use
//! (`UProperty::ExportText` / `ExportTextItem` / `ImportText` per type).
//!
//! * The property is found as `FindField` (10143fd0) does: the class's own fields, then each super class's, the first
//!   property whose name matches without case. It must be public (`RF_Public`): a `private` variable answers "" and
//!   cannot be set. `SetPropertyText` also refuses `const` properties (`CPF_Const`).
//! * Only element 0 is read or written. Get exports with `PPF_Localized` (data and delta the same object, so
//!   `ExportText` 10165440 always exports); set imports with `PPF_Localized`. Inside a struct, `PPF_Delimited` is
//!   added (strings in quotes) and the struct's members are written with `PPF_Delimited` alone.
//! * A failed import leaves what it wrote so far: `SetPropertyText` ignores the result.

use crate::console::{atof, atoi, read_token};
use crate::{ref_instance, Value, Vm};
use ue_package::object::PropertyKind as K;
use ue_registry::{PropDef, StructId, Target};

/// Where a property lives: an object's memory or a class's default object (`MemId::Def`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sel {
    Obj(usize),
    Def(ue_registry::ClassId),
}

pub(crate) const PPF_LOCALIZED: u32 = 1;
pub(crate) const PPF_DELIMITED: u32 = 2;
const CPF_CONST: u32 = 0x2;
const CPF_NATIVE: u32 = 0x1000;
const CPF_TRANSIENT: u32 = 0x2000;

impl Vm {
    pub(crate) fn sel_mem(&self, sel: Sel) -> &crate::Mem {
        match sel {
            Sel::Obj(i) => &self.objects[i].mem,
            Sel::Def(c) => &self.defaults[&c],
        }
    }

    pub(crate) fn sel_write(&mut self, sel: Sel, def: &PropDef, off: usize, v: Value) {
        let m = match sel {
            Sel::Obj(i) => &mut self.objects[i].mem,
            Sel::Def(c) => self.defaults.get_mut(&c).expect("class defaults loaded"),
        };
        let _ = Self::write_to(m, def, off, v);
    }

    /// `FindField` for a property of the object's class: public ones only (`RF_Public`).
    fn text_property(&self, obj: usize, name: &str) -> Option<PropDef> {
        let class = self.objects.get(obj).filter(|o| o.alive)?.class;
        self.reg.find_prop(class, name).filter(|d| d.public).cloned()
    }

    /// `Object.GetPropertyText`.
    pub fn get_property_text(&self, obj: u32, name: &str) -> String {
        let Some(i) = ref_instance(obj) else { return String::new() };
        let Some(def) = self.text_property(i, name) else { return String::new() };
        self.export_item(Sel::Obj(i), &def, def.offset as usize, PPF_LOCALIZED)
    }

    /// `Object.SetPropertyText`.
    pub fn set_property_text(&mut self, obj: u32, name: &str, text: &str) {
        let Some(i) = ref_instance(obj) else { return };
        let Some(def) = self.text_property(i, name) else { return };
        if def.flags & CPF_CONST != 0 {
            return;
        }
        let _ = self.import_item(Sel::Obj(i), &def, def.offset as usize, text, PPF_LOCALIZED);
    }

    /// `ExportTextItem` of one element at `off` in object `obj`'s memory.
    pub(crate) fn export_item(&self, sel: Sel, def: &PropDef, off: usize, flags: u32) -> String {
        if def.kind == K::Struct {
            let Target::Struct(sid) = def.target else { return String::new() };
            return self.export_struct(sel, sid, off);
        }
        let Ok(v) = self.read_from(self.sel_mem(sel), def, off) else { return String::new() };
        match (def.kind, v) {
            // 101659d0: an enum's value name, else `%i`
            (K::Byte, Value::Byte(b)) => match &def.target {
                Target::Enum(e) => self.reg.enum_value_names(e).and_then(|n| n.get(b as usize)).map(|&n| self.reg.names.str(n).to_string()).unwrap_or_default(),
                _ => b.to_string(),
            },
            // 10165e60 `%i`
            (K::Int, Value::Int(n)) => n.to_string(),
            // 101662b0: `GTrue`/`GFalse` (localised; English here) or "True"/"False"
            (K::Bool, Value::Bool(b)) => if b { "True" } else { "False" }.into(),
            // 10166890 `%f`
            (K::Float, Value::Float(f)) => crate::natives::float_str(f),
            // 10167420
            (K::Name, Value::Name(n)) => self.reg.names.str(n).to_string(),
            // 101678a0: `"%s"` when delimited (no escaping)
            (K::Str | K::String, Value::Str(s)) => if flags & PPF_DELIMITED != 0 { format!("\"{s}\"") } else { s },
            // 10166d40: `Class'Path'` or None
            (K::Object | K::Class, Value::Obj(r)) => {
                if r == 0 || !self.is_live(r) {
                    return "None".into();
                }
                format!("{}'{}'", self.object_class_name(r), self.obj_to_string(r))
            }
            _ => String::new(),
        }
    }

    /// The name of an object's class: an instance's class, a resource's export class, `Class` for a class.
    fn object_class_name(&self, r: u32) -> String {
        if ref_instance(r).is_some() {
            return self.class_of_ref(r).map(|c| self.reg.names.str(self.reg.class(c).name).to_string()).unwrap_or_default();
        }
        if let Some(p) = self.resource_class_path(r) {
            return p.rsplit('.').next().unwrap_or(&p).to_string();
        }
        let path = self.reg.obj_path(ue_registry::ObjId(r));
        if self.reg.class_by_path(path).is_some() {
            "Class".into()
        } else {
            "Object".into()
        }
    }

    // (struct members in `TFieldIterator` order, own then the super struct's: `Vm::struct_members`)

    /// `UProperty::Port` (101655c0): a size, a category or neither native nor transient, and not named `Class`.
    pub(crate) fn ports(&self, def: &PropDef) -> bool {
        def.elem_size * def.dim != 0
            && (def.category != 0 || def.flags & (CPF_NATIVE | CPF_TRANSIENT) == 0)
            && !self.reg.names.str(def.name).eq_ignore_ascii_case("Class")
    }

    /// `UStructProperty::ExportTextItem` (10169710): `(Name=value,Name[i]=value,...)` over the members that port,
    /// each element written with `PPF_Delimited`; "" when nothing was written.
    fn export_struct(&self, sel: Sel, sid: StructId, off: usize) -> String {
        let mut out = String::new();
        let mut n = 0;
        for m in self.struct_members(sid) {
            if !self.ports(&m) {
                continue;
            }
            for i in 0..m.dim {
                let v = self.export_item(sel, &m, off + (m.offset + i * m.elem_size) as usize, PPF_DELIMITED);
                n += 1;
                out.push(if n == 1 { '(' } else { ',' });
                let name = self.reg.names.str(m.name);
                if m.dim == 1 {
                    out.push_str(&format!("{name}={v}"));
                } else {
                    out.push_str(&format!("{name}[{i}]={v}"));
                }
            }
        }
        if n > 0 {
            out.push(')');
        }
        out
    }

    /// `ImportText` of one element at `off` in object `obj`'s memory: the rest of the text, or `None` on failure
    /// (what was written stays).
    pub(crate) fn import_item<'a>(&mut self, sel: Sel, def: &PropDef, off: usize, s: &'a str, flags: u32) -> Option<&'a str> {
        let digit = |s: &str| s.starts_with(|c: char| c.is_ascii_digit());
        let put = |vm: &mut Vm, v: Value| {
            vm.sel_write(sel, def, off, v);
        };
        match def.kind {
            // 10165f00: a number only when the text starts with a minus or a digit; the run of both is passed
            K::Int => {
                if s.starts_with(|c: char| c == '-' || c.is_ascii_digit()) {
                    put(self, Value::Int(atoi(s)));
                }
                Some(&s[s.find(|c: char| c != '-' && !c.is_ascii_digit()).unwrap_or(s.len())..])
            }
            // 10166940: always written; up to a comma, a parenthesis or the line's end
            K::Float => {
                put(self, Value::Float(atof(s)));
                Some(&s[s.find([',', ')', '\r', '\n']).unwrap_or(s.len())..])
            }
            // 10166380: 1/True/GTrue set, 0/False/GFalse clear, anything else fails
            K::Bool => {
                let (t, rest) = read_token(s, 0x400, false)?;
                let on = ["1", "True"].iter().any(|w| t.eq_ignore_ascii_case(w));
                let off_ = ["0", "False"].iter().any(|w| t.eq_ignore_ascii_case(w));
                if !on && !off_ {
                    return None;
                }
                put(self, Value::Bool(on));
                Some(rest)
            }
            // 10165aa0 (asm): an enum reads a token first; a value name is taken, else digits must follow the token
            K::Byte => {
                let mut at = s;
                if let Target::Enum(e) = &def.target {
                    let e = e.clone();
                    let (t, rest) = read_token(s, 0x400, false)?;
                    if let Some(id) = self.reg.names.get(&t) {
                        if let Some(i) = self.reg.enum_value_names(&e).and_then(|n| n.iter().position(|&n| n == id)) {
                            put(self, Value::Byte(i as u8));
                            return Some(rest);
                        }
                    }
                    at = rest;
                }
                if !digit(at) {
                    return None;
                }
                put(self, Value::Byte(atoi(at) as u8));
                Some(&at[at.find(|c: char| !c.is_ascii_digit()).unwrap_or(at.len())..])
            }
            // 101674d0: a token, added to the name table ("" is None)
            K::Name => {
                let (t, rest) = read_token(s, 0x400, false)?;
                let v = if t.is_empty() { Value::Name(0) } else { self.name_value(&t) };
                put(self, v);
                Some(rest)
            }
            // 10167980: the whole text, the cursor stays; delimited: a token (quoted)
            K::Str | K::String => {
                if flags & PPF_DELIMITED != 0 {
                    let (t, rest) = read_token(s, 0x1000, false)?;
                    put(self, Value::Str(t));
                    Some(rest)
                } else {
                    put(self, Value::Str(s.to_string()));
                    Some(s)
                }
            }
            K::Object | K::Class => self.import_object(sel, def, off, s),
            K::Struct => {
                let Target::Struct(sid) = def.target else { return None };
                self.import_struct(sel, sid, off, s, flags)
            }
            _ => None,
        }
    }

    /// `UObjectProperty::ImportText` (10166e20) and `UClassProperty::ImportText` (10167110).
    fn import_object<'a>(&mut self, sel: Sel, def: &PropDef, off: usize, s: &'a str) -> Option<&'a str> {
        let (t, rest) = read_token(s, 0x400, true)?;
        if t.eq_ignore_ascii_case("None") {
            self.sel_write(sel, def, off, Value::Obj(0));
            return Some(rest);
        }
        let rest = rest.trim_start_matches(' ');
        let mut chars = rest.chars();
        let first = chars.next();
        // the cursor steps over the character after the token
        let after = chars.as_str();
        let (found, rest) = if first == Some('\'') {
            // `Class'Name'`: the class named, then the object of that class (the property's class is not checked)
            let (name, r) = read_token(after, 0x400, true)?;
            let r = r.strip_prefix('\'')?;
            let class = self.reg.class_by_name(t.rsplit('.').next().unwrap_or(&t));
            let class_object = t.eq_ignore_ascii_case("Class");
            if class.is_none() && !class_object {
                return None;
            }
            (self.find_any_object(class, class_object, &name), r)
        } else {
            let (class, class_object) = match &def.target {
                Target::Class { .. } => (None, true),
                Target::Object(c) => {
                    let p = self.reg.obj_path(*c).to_string();
                    (self.reg.class_by_path(&p), p.eq_ignore_ascii_case("Core.Class"))
                }
                _ => (None, false),
            };
            (self.find_any_object(class, class_object, &t), after)
        };
        let mut v = found.unwrap_or(0);
        // a class property keeps only classes derived from its meta class
        if let (Target::Class { meta, .. }, true) = (&def.target, v != 0) {
            let meta = self.reg.class_by_path(&self.reg.obj_path(*meta).to_string());
            let is_class = ref_instance(v).is_none() && self.reg.class_by_path(self.reg.obj_path(ue_registry::ObjId(v))).is_some();
            let fits = is_class && match (self.class_of_ref(v), meta) {
                (Some(c), Some(m)) => self.is_a(c, m),
                _ => true,
            };
            if !fits {
                v = 0;
            }
        }
        self.sel_write(sel, def, off, Value::Obj(v));
        found.map(|_| rest)
    }

    /// `StaticFindObject(Class, ANY_PACKAGE, Name)`: a dotted name is looked up by its path, a plain one by name in
    /// any package; the object must be of the class (or a subclass). Classes when `class_object`.
    fn find_any_object(&mut self, class: Option<ue_registry::ClassId>, class_object: bool, name: &str) -> Option<u32> {
        let short = name.rsplit('.').next().unwrap_or(name);
        if class_object {
            let c = if name.contains('.') { self.reg.class_by_path(name) } else { self.reg.class_by_name(short) }?;
            let path = self.reg.class(c).path.clone();
            return Some(self.reg.obj_id(&path).0);
        }
        let id = self.reg.names.get(short)?;
        let outer = name.rsplit_once('.').map(|(o, _)| o.to_string());
        let found = (0..self.objects.len()).map(crate::inst_ref).find(|&r| {
            self.is_live(r)
                && class.is_none_or(|w| self.class_of_ref(r).is_some_and(|c| self.is_a(c, w)))
                && matches!(self.get_prop(r, "Name", 0), Ok(Value::Name(n)) if n == id)
                && outer.as_deref().is_none_or(|o| self.obj_to_string(r).rsplit_once('.').is_some_and(|(p, _)| p.eq_ignore_ascii_case(o)))
        });
        if found.is_some() {
            return found;
        }
        // a resource of a loaded package by its path
        let path = name.to_string();
        if outer.is_some() {
            if let Some((pi, ei)) = self.reg.export_by_path(&path) {
                let c = self.reg.class_of_export(pi, ei);
                if class.is_none_or(|w| c.is_some_and(|c| self.is_a(c, w))) {
                    return Some(self.reg.obj_id(&path).0);
                }
            }
        }
        None
    }

    /// `UStructProperty::ImportText` (101698b0).
    fn import_struct<'a>(&mut self, sel: Sel, sid: StructId, off: usize, s: &'a str, flags: u32) -> Option<&'a str> {
        let mut s = s.strip_prefix('(')?;
        let members = self.struct_members(sid);
        loop {
            if let Some(r) = s.strip_prefix(')') {
                return Some(r);
            }
            // the key: up to '=', '[' or the end, at most 63 characters
            let end = s.find(['=', '[']).unwrap_or(s.len());
            let key: String = s[..end].chars().take(63).collect();
            let key_end = s.char_indices().nth(key.chars().count()).map(|(i, _)| i).unwrap_or(s.len()).min(end);
            s = &s[key_end..];
            if s.starts_with('[') {
                // the digit scan starts on the '[' itself, so it never moves and the ']' test always fails
                return None;
            }
            s = s.strip_prefix('=')?;
            let mut parsed = false;
            if let Some(id) = self.reg.names.get(&key) {
                for m in &members {
                    // (element 0: an index never gets this far)
                    if m.name == id && m.dim > 0 && self.ports(m) {
                        let at = off + m.offset as usize;
                        s = self.import_item(sel, m, at, s, flags | PPF_DELIMITED)?;
                        parsed = true;
                    }
                }
            }
            if !parsed {
                // skip the value: up to the line's end, or a ',' / ')' outside parentheses; a quote is tested where it
                // stands, so the scan goes on through the quoted text
                let mut depth = 0i32;
                let b = s.as_bytes();
                let mut i = 0;
                while i < b.len() && b[i] != b'\n' && b[i] != b'\r' && (depth > 0 || (b[i] != b')' && b[i] != b',')) {
                    match b[i] {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth < 0 {
                                return None;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                if depth > 0 {
                    return None;
                }
                s = &s[i..];
            }
            if let Some(r) = s.strip_prefix(',') {
                s = r;
            } else if !s.starts_with(')') {
                return None;
            }
        }
    }
}

//! Configuration and localised text applied to class defaults, and the text import of property values they need.
//!
//! As in the engine, when a class is initialised its `config` / `globalconfig` properties are read from the section
//! named after the class (`[Engine.PlayerPawn]`) of the class's config file (`System` = `DeusEx.ini`, `User` = `User.ini`),
//! and its `localized` properties from the section named after the class (`[Binoculars]`) of `<Package>.int`.
//! A `globalconfig` property is read only from the section of the class that declares it.

use crate::*;
use ue_package::object::PropertyKind;

const CPF_CONFIG: u32 = 0x4000;
const CPF_LOCALIZED: u32 = 0x8000;
const CPF_GLOBAL_CONFIG: u32 = 0x40000;

fn put(inst: &mut Instance, off: u32, bytes: &[u8]) {
    let o = off as usize;
    inst.data[o..o + bytes.len()].copy_from_slice(bytes);
}

fn set_bit(inst: &mut Instance, off: u32, mask: u32, on: bool) {
    let o = off as usize;
    let mut w = u32::from_le_bytes(inst.data[o..o + 4].try_into().unwrap());
    if on {
        w |= mask;
    } else {
        w &= !mask;
    }
    put(inst, off, &w.to_le_bytes());
}

/// Splits `(A=1,B=(C=2,D="x,y"))` into its top-level `key=value` pairs.
fn split_struct(text: &str) -> Vec<(String, String)> {
    let t = text.trim();
    let t = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')).unwrap_or(t);
    let (mut out, mut depth, mut quoted, mut cur) = (Vec::new(), 0i32, false, String::new());
    for ch in t.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                cur.push(ch);
            }
            '(' if !quoted => {
                depth += 1;
                cur.push(ch);
            }
            ')' if !quoted => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if !quoted && depth == 0 => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out.into_iter().filter_map(|p| p.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))).collect()
}

/// The object path inside `Class'Package.Name'`, or the text itself.
fn object_path(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("none") {
        return None;
    }
    match (t.find('\''), t.rfind('\'')) {
        (Some(a), Some(b)) if b > a => Some(t[a + 1..b].to_string()),
        _ => Some(t.trim_matches('"').to_string()),
    }
}

impl Registry {
    /// Installs the configuration used by `instantiate` for class defaults. Call before any class is instantiated.
    pub fn set_config(&mut self, mut config: ue_config::Config) {
        config.load("System");
        config.load("User");
        self.config = Some(config);
        self.config_cache.clear();
    }

    /// Writes the changed user layers of the configuration to the per-user directory.
    pub fn flush_config(&mut self) {
        if let Some(c) = self.config.as_mut() {
            let _ = c.save();
        }
        self.config_cache.clear();
    }

    /// Every key and value of a configuration section (`file` as for `config_value`).
    pub fn config_section(&mut self, file: &str, section: &str) -> Vec<(String, String)> {
        self.config.as_mut().map(|c| c.section_entries(file, section)).unwrap_or_default()
    }

    /// Reads a configuration value (`file` is `System`, `User` or an ini name).
    pub fn config_value(&mut self, file: &str, section: &str, key: &str) -> Option<String> {
        let cfg = self.config.as_mut()?;
        cfg.load(file);
        cfg.value(file, section, key)
    }

    /// Applies the configuration and the localised text of class `c` (and the properties it inherits) to `inst`.
    /// The values found for a class are looked up once and cached.
    pub(crate) fn apply_config_and_text(&mut self, inst: &mut Instance, c: ClassId) {
        if let Some(rt) = self.runtime_defaults.get(&c).cloned() {
            // applied after the configuration below, so first make sure that has been applied
            self.apply_config_values(inst, c);
            for (name, text) in rt {
                self.apply_default_text(inst, c, &name, &text);
            }
            return;
        }
        self.apply_config_values(inst, c);
    }

    fn apply_config_values(&mut self, inst: &mut Instance, c: ClassId) {
        if self.config.is_none() {
            return;
        }
        if !self.config_cache.contains_key(&c) {
            let found = self.find_config_values(c);
            self.config_cache.insert(c, found);
        }
        let found = self.config_cache[&c].clone();
        for (p, off, v) in found {
            // a bad value in a user's ini must not stop the game: the default stays
            let _ = self.import_text(inst, &p, off, &v);
        }
    }

    /// Changes the default of `prop` (`Name` or `Name[i]`) for class `c` and the classes derived from it, from text.
    pub fn set_default_text(&mut self, c: ClassId, prop: &str, text: &str) -> std::result::Result<(), String> {
        // validate against a fresh instance first
        let mut probe = self.instantiate(c).map_err(|e| e.to_string())?;
        if !self.apply_default_text(&mut probe, c, prop, text) {
            return Err(format!("cannot set {prop} to {text}"));
        }
        let list = self.runtime_defaults.entry(c).or_default();
        list.retain(|(n, _)| !n.eq_ignore_ascii_case(prop));
        list.push((prop.to_string(), text.to_string()));
        Ok(())
    }

    /// Imports `text` into the property named `prop` (`Name` or `Name[i]`) of `inst`, seen from class `c`.
    pub fn apply_default_text(&mut self, inst: &mut Instance, c: ClassId, prop: &str, text: &str) -> bool {
        let (base, idx) = match prop.split_once('[') {
            Some((b, i)) => (b.to_string(), i.trim_end_matches(']').parse::<u32>().unwrap_or(0)),
            None => (prop.to_string(), 0),
        };
        let Some(p) = self.all_props(c).into_iter().find(|p| self.names.str(p.name).eq_ignore_ascii_case(&base)).cloned() else { return false };
        if idx >= p.dim.max(1) {
            return false;
        }
        self.import_text(inst, &p, p.offset + idx * p.elem_size, text).is_ok()
    }

    /// Applies the localised text of class `c` (its properties and those it inherits) to `inst` (`LoadLocalized`).
    pub fn apply_localized(&mut self, inst: &mut Instance, c: ClassId) {
        if self.config.is_none() {
            return;
        }
        for (p, off, v) in self.find_values(c, CPF_LOCALIZED) {
            let _ = self.import_text(inst, &p, off, &v);
        }
    }

    fn find_config_values(&mut self, c: ClassId) -> Vec<(PropDef, u32, String)> {
        self.find_values(c, CPF_CONFIG | CPF_LOCALIZED)
    }

    /// The configured (`CPF_CONFIG`) and localised (`CPF_LOCALIZED`) values of class `c`'s properties, as
    /// `LoadConfig(0)` / `LoadLocalized` read them when the class is loaded: the class's own section for every
    /// property, a `globalconfig` one from the section of the class that declares it.
    fn find_values(&mut self, c: ClassId, mask: u32) -> Vec<(PropDef, u32, String)> {
        let (path, cname, package, file) = {
            let cd = self.class(c);
            (cd.path.clone(), self.names.str(cd.name).to_string(), self.packages[cd.package].name.clone(), self.names.str(cd.config_name).to_string())
        };
        let file = if file.is_empty() || file.eq_ignore_ascii_case("none") { None } else { Some(file) };
        // (declaring class, property) in inheritance order
        let props: Vec<(ClassId, PropDef)> = self.chain(c).into_iter().flat_map(|o| self.class(o).props.iter().map(move |p| (o, p.clone())).collect::<Vec<_>>()).collect();
        let mut out = Vec::new();
        for (owner, p) in props {
            if p.flags & mask & (CPF_CONFIG | CPF_LOCALIZED) == 0 {
                continue;
            }
            let pname = self.names.str(p.name).to_string();
            for i in 0..p.dim.max(1) {
                let key = if p.dim > 1 { format!("{pname}[{i}]") } else { pname.clone() };
                let mut value = None;
                if p.flags & mask & CPF_LOCALIZED != 0 {
                    value = self.config.as_mut().unwrap().localized(&package, &cname, &key);
                }
                if p.flags & mask & CPF_CONFIG != 0 {
                    if let Some(f) = &file {
                        let section = if p.flags & CPF_GLOBAL_CONFIG != 0 { self.class(owner).path.clone() } else { path.clone() };
                        let cfg = self.config.as_mut().unwrap();
                        cfg.load(f);
                        if let Some(v) = cfg.value(f, &section, &key) {
                            value = Some(v);
                        }
                    }
                }
                if let Some(v) = value {
                    out.push((p.clone(), p.offset + i * p.elem_size, v));
                }
            }
        }
        out
    }

    /// Imports the text form of one property element (as in ini files and `SetPropertyText`) at byte offset `off`.
    pub fn import_text(&mut self, inst: &mut Instance, prop: &PropDef, off: u32, text: &str) -> std::result::Result<(), String> {
        let t = text.trim();
        match prop.kind {
            PropertyKind::Bool => {
                let on = matches!(t.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on");
                let off_ok = matches!(t.to_ascii_lowercase().as_str(), "false" | "0" | "no" | "off");
                if !on && !off_ok {
                    return Err(format!("not a bool: {t}"));
                }
                set_bit(inst, off, prop.bit_mask, on);
            }
            PropertyKind::Byte => {
                let v = match t.parse::<i32>() {
                    Ok(v) => v,
                    Err(_) => {
                        let Target::Enum(ename) = &prop.target else { return Err(format!("not a byte: {t}")) };
                        self.enum_value(ename, t).ok_or_else(|| format!("{t} is not a value of {ename}"))? as i32
                    }
                };
                put(inst, off, &[v as u8]);
            }
            PropertyKind::Int => {
                let v = t.parse::<i32>().or_else(|_| t.parse::<f32>().map(|f| f as i32)).map_err(|_| format!("not an int: {t}"))?;
                put(inst, off, &v.to_le_bytes());
            }
            PropertyKind::Float => {
                let v = t.parse::<f32>().map_err(|_| format!("not a float: {t}"))?;
                put(inst, off, &v.to_le_bytes());
            }
            PropertyKind::Name => {
                let id = self.names.intern(ue_config::unquote(t).as_str());
                put(inst, off, &id.to_le_bytes());
            }
            PropertyKind::Str | PropertyKind::String => {
                inst.strings.push(ue_config::unquote(t));
                put(inst, off, &(inst.strings.len() as u32).to_le_bytes());
            }
            PropertyKind::Object | PropertyKind::Class => {
                let id = match object_path(t) {
                    Some(p) => self.obj_id(&p).0,
                    None => 0,
                };
                put(inst, off, &id.to_le_bytes());
            }
            PropertyKind::Struct => {
                let Target::Struct(sid) = prop.target else { return Err("struct without definition".into()) };
                let mut chain = vec![sid];
                while let Some(s) = self.struct_def(*chain.last().unwrap()).super_id {
                    chain.push(s);
                }
                let members: Vec<PropDef> = chain.iter().flat_map(|s| self.struct_def(*s).props.clone()).collect();
                for (k, v) in split_struct(t) {
                    let (base, idx) = match k.split_once('[') {
                        Some((b, i)) => (b.to_string(), i.trim_end_matches(']').parse::<u32>().unwrap_or(0)),
                        None => (k.clone(), 0),
                    };
                    let Some(m) = members.iter().find(|m| self.names.str(m.name).eq_ignore_ascii_case(&base)) else { continue };
                    self.import_text(inst, m, off + m.offset + idx * m.elem_size, &v)?;
                }
            }
            other => return Err(format!("cannot import {other:?} values")),
        }
        Ok(())
    }

    /// The value names (name ids) of the enum called `ename` (a path or a plain name), declared by any class.
    pub fn enum_value_names(&self, ename: &str) -> Option<&Vec<u32>> {
        let short = ename.rsplit('.').next().unwrap_or(ename);
        self.classes.iter().flat_map(|c| c.enums.iter()).find(|e| self.names.str(e.name).eq_ignore_ascii_case(short)).map(|e| &e.values)
    }

    /// Index of `value` in the enum called `ename` (declared by any class).
    fn enum_value(&self, ename: &str, value: &str) -> Option<usize> {
        let short = ename.rsplit('.').next().unwrap_or(ename);
        self.classes.iter().flat_map(|c| c.enums.iter()).find(|e| self.names.str(e.name).eq_ignore_ascii_case(short)).and_then(|e| e.values.iter().position(|v| self.names.str(*v).eq_ignore_ascii_case(value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_and_object_text() {
        assert_eq!(split_struct("(X=1.0,Y=(A=2,B=\"p,q\"),Z=3)"), vec![("X".to_string(), "1.0".to_string()), ("Y".to_string(), "(A=2,B=\"p,q\")".to_string()), ("Z".to_string(), "3".to_string())]);
        assert_eq!(object_path("Texture'DeusExUI.Foo'").as_deref(), Some("DeusExUI.Foo"));
        assert_eq!(object_path("Pkg.Thing").as_deref(), Some("Pkg.Thing"));
        assert_eq!(object_path("None"), None);
    }
}

//! Class default objects, `LoadConfig` / `SaveConfig` and the `GET` / `SET` console commands, as Core does them.
//!
//! * A class's defaults are built as `UClass::Serialize` (101011f9) builds them: a copy of the super class's defaults
//!   (`InitClassDefaultObject`), the class's own default block, `LoadConfig(0)`, then `LoadLocalized`. Objects made by
//!   `new` and spawned actors copy them (`InitProperties`), so `default.X = ...` reaches later objects.
//! * `LoadConfig(Propagate, Class, Filename)` (10150d60): nothing unless the class is `config` (`CLASS_Config`); with
//!   `Propagate` the super class first (recursively); then every config property of the class (its own and inherited)
//!   from the section named after the class (a `globalconfig` one: after the class that declares it; a per-object one:
//!   after the object), `Name` or `Name[i]` per element, imported with no port flags.
//! * `SaveConfig(CPF_Config)` (10151540): every config property exported (no port flags) into that section of the
//!   class's config file (per-object outside the transient package: the file named after the outer), flushed, then
//!   every loaded class derived from the topmost config class reloads its defaults with `LoadConfig(1)`.
//! * `GET class property` / `SET class property value` (`UObject::StaticExec` 10153110): the class by name or path
//!   (`ini:`/`usr:` as `ResolveName`), the property by `FindField` (private ones too); "Unrecognized class %s" /
//!   "Unrecognized property %s". GET exports the class defaults (`PPF_Localized`); SET is `GlobalSetProperty`
//!   (101523a0): every live object of the class (or a subclass) and the class's defaults import the value
//!   (`PPF_Localized`), then the defaults are saved as above.

use crate::console::{parse_command, parse_token};
use crate::property_text::{Sel, PPF_LOCALIZED};
use crate::{ref_instance, Mem, Value, Vm, VmError};
use ue_registry::{ClassId, Instance, PropDef};

const CPF_CONFIG: u32 = 0x4000;
const CPF_GLOBAL_CONFIG: u32 = 0x40000;
const CLASS_CONFIG: u32 = 0x4;
const CLASS_PER_OBJECT_CONFIG: u32 = 0x400;

impl Vm {
    /// The class default object of `class` (built on first use).
    pub(crate) fn class_default_mem(&mut self, class: ClassId) -> Result<&Mem, VmError> {
        if !self.defaults.contains_key(&class) {
            self.build_class_defaults(class)?;
        }
        Ok(&self.defaults[&class])
    }

    fn build_class_defaults(&mut self, class: ClassId) -> Result<(), VmError> {
        let size = self.reg.class(class).size as usize;
        let mut inst = match self.reg.class(class).super_id {
            Some(s) => {
                let m = self.class_default_mem(s)?;
                Instance { class, data: m.data.clone(), strings: m.strings.clone() }
            }
            None => Instance { class, data: Vec::new(), strings: Vec::new() },
        };
        inst.data.resize(size, 0);
        self.reg.apply_class_tags(&mut inst, class).map_err(|e| VmError::Runtime(e.to_string()))?;
        self.defaults.insert(class, Mem { data: inst.data, strings: inst.strings });
        self.load_config(Sel::Def(class), class, false, None);
        let m = self.defaults.remove(&class).expect("just inserted");
        let mut inst = Instance { class, data: m.data, strings: m.strings };
        self.reg.apply_localized(&mut inst, class);
        self.defaults.insert(class, Mem { data: inst.data, strings: inst.strings });
        Ok(())
    }

    /// A copy of the class's defaults (`InitProperties`).
    pub(crate) fn default_copy(&mut self, class: ClassId) -> Result<Mem, VmError> {
        let m = self.class_default_mem(class)?;
        Ok(Mem { data: m.data.clone(), strings: m.strings.clone() })
    }

    /// An object saved in a package (a map's actor, a conversation, ...): a copy of its class's defaults, then the
    /// object's own property tags (with the tags that could not be applied).
    pub(crate) fn instantiate_export(&mut self, class: ClassId, pi: usize, export: usize) -> Result<(Instance, Vec<String>), String> {
        let m = self.default_copy(class).map_err(|e| e.to_string())?;
        let mut inst = Instance { class, data: m.data, strings: m.strings };
        let warnings = self.reg.apply_object_tags(&mut inst, class, pi, export).map_err(|e| e.to_string())?;
        Ok((inst, warnings))
    }

    fn sel_class(&self, sel: Sel) -> ClassId {
        match sel {
            Sel::Obj(i) => self.objects[i].class,
            Sel::Def(c) => c,
        }
    }

    /// The config properties of `class` in `TFieldIterator` order (its own, then each super class's), with the class
    /// that declares each.
    fn config_props(&self, class: ClassId, flags: u32) -> Vec<(ClassId, PropDef)> {
        self.reg.chain(class).into_iter().rev().flat_map(|c| self.reg.class(c).props.iter().filter(|p| p.flags & flags == flags).map(move |p| (c, p.clone()))).collect()
    }

    /// Whether the object is configured per object (`CLASS_PerObjectConfig` and a real object, not a class's
    /// defaults), and the config file: the class's, or for a per-object one outside the transient package the outer's
    /// name.
    fn config_target(&mut self, sel: Sel) -> (bool, String) {
        let class = self.sel_class(sel);
        let per_object = self.reg.class(class).flags & CLASS_PER_OBJECT_CONFIG != 0 && matches!(sel, Sel::Obj(_));
        if per_object {
            if let Sel::Obj(i) = sel {
                if let Some(outer) = self.outer_name(crate::inst_ref(i)) {
                    return (true, outer);
                }
            }
        }
        (per_object, self.reg.names.str(self.reg.class(class).config_name).to_string())
    }

    /// The section a config property is read from or written to.
    fn config_section(&self, sel: Sel, per_object: bool, class: ClassId, owner: ClassId, p: &PropDef) -> String {
        if per_object {
            if let Sel::Obj(i) = sel {
                if let Ok(crate::Value::Name(n)) = self.get_prop(crate::inst_ref(i), "Name", 0) {
                    return self.reg.names.str(n).to_string();
                }
            }
        }
        let c = if p.flags & CPF_GLOBAL_CONFIG != 0 { owner } else { class };
        self.reg.class(c).path.clone()
    }

    /// `UObject::LoadConfig`.
    pub(crate) fn load_config(&mut self, sel: Sel, class: ClassId, propagate: bool, filename: Option<&str>) {
        if self.reg.config.is_none() || self.reg.class(class).flags & CLASS_CONFIG == 0 {
            return;
        }
        if propagate {
            if let Some(s) = self.reg.class(class).super_id {
                self.load_config(sel, s, true, filename);
            }
        }
        let (per_object, file) = self.config_target(sel);
        let file = filename.map(str::to_string).unwrap_or(file);
        if file.is_empty() || file.eq_ignore_ascii_case("None") {
            return;
        }
        for (owner, p) in self.config_props(class, CPF_CONFIG) {
            let section = self.config_section(sel, per_object, class, owner, &p);
            let name = self.reg.names.str(p.name).to_string();
            for i in 0..p.dim.max(1) {
                let key = if p.dim == 1 { name.clone() } else { format!("{name}[{i}]") };
                if let Some(v) = self.reg.config_value(&file, &section, &key) {
                    // what the configuration holds: a value parsed from a file was trimmed and unquoted there, one set
                    // since then is returned as set (trailing spaces kept)
                    let v = match v.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
                        Some(inner) => inner.replace("\\\"", "\"").replace("\\\\", "\\"),
                        None => v,
                    };
                    let _ = self.import_item(sel, &p, (p.offset + i * p.elem_size) as usize, &v, 0);
                }
            }
        }
    }

    /// `UObject::SaveConfig(CPF_Config)` on an object or a class's defaults.
    pub(crate) fn save_config_of(&mut self, sel: Sel) -> Result<(), VmError> {
        if self.reg.config.is_none() {
            return Ok(());
        }
        let class = self.sel_class(sel);
        if let Sel::Def(c) = sel {
            self.class_default_mem(c)?;
        }
        let (per_object, file) = self.config_target(sel);
        if !(file.is_empty() || file.eq_ignore_ascii_case("None")) {
            for (owner, p) in self.config_props(class, CPF_CONFIG) {
                let section = self.config_section(sel, per_object, class, owner, &p);
                let name = self.reg.names.str(p.name).to_string();
                for i in 0..p.dim.max(1) {
                    let key = if p.dim == 1 { name.clone() } else { format!("{name}[{i}]") };
                    let value = self.export_item(sel, &p, (p.offset + i * p.elem_size) as usize, 0);
                    if let Some(c) = self.reg.config.as_mut() {
                        c.set(&file, &section, &key, &value);
                    }
                }
            }
        }
        self.reg.flush_config();
        // every class under the topmost config class reloads its defaults, the super classes' sections first
        let mut base = class;
        while let Some(s) = self.reg.class(base).super_id {
            if self.reg.class(s).flags & CLASS_CONFIG == 0 {
                break;
            }
            base = s;
        }
        let derived: Vec<ClassId> = (0..self.reg.class_count()).map(|i| ClassId(i as u32)).filter(|&c| self.is_a(c, base)).collect();
        for c in derived {
            if self.class_default_mem(c).is_ok() {
                self.load_config(Sel::Def(c), c, true, None);
            }
        }
        Ok(())
    }

    /// `StaticFindObject(UClass, ANY_PACKAGE, name)`: `ini:`/`usr:` names resolved through the configuration first
    /// (`ResolveName`), then a dotted path or a plain class name.
    fn find_class_any(&mut self, name: &str) -> Option<ClassId> {
        let mut name = name.to_string();
        let lower = name.get(..4).map(str::to_ascii_lowercase);
        let ini = lower.as_deref() == Some("ini:");
        if (ini || lower.as_deref() == Some("usr:")) && name.contains('.') {
            let rest = &name[4..];
            let (section, key) = rest.rsplit_once('.')?;
            name = ue_config::unquote(&self.reg.config_value(if ini { "System" } else { "User" }, section, key)?);
        }
        if name.contains('.') {
            self.reg.class_by_path(&name)
        } else {
            self.reg.class_by_name(&name)
        }
    }

    /// The `GET` / `SET` commands of `UObject::StaticExec`; `None` when `cmd` is neither.
    pub(crate) fn config_console(&mut self, cmd: &str) -> Option<String> {
        let mut s = cmd;
        let get = parse_command(&mut s, "GET");
        if !get && !parse_command(&mut s, "SET") {
            return None;
        }
        let class_token = parse_token(&mut s, 0x100, true).unwrap_or_default();
        let Some(class) = (!class_token.is_empty()).then(|| self.find_class_any(&class_token)).flatten() else {
            // the menus' `ini:Section.Key Property`: a class the port does not have (the audio subsystem's)
            if let Some(out) = self.ini_fallback(get, &class_token, s) {
                return Some(out);
            }
            return Some(format!("Unrecognized class {class_token}"));
        };
        let prop_token = parse_token(&mut s, 0x100, true).unwrap_or_default();
        // `FindField`: any property, private ones too
        let Some(p) = self.reg.find_prop(class, &prop_token).cloned().filter(|_| !prop_token.is_empty()) else {
            return Some(format!("Unrecognized property {prop_token}"));
        };
        if self.class_default_mem(class).is_err() {
            return Some(String::new());
        }
        if get {
            return Some(self.export_item(Sel::Def(class), &p, p.offset as usize, PPF_LOCALIZED));
        }
        let value = s.trim_start_matches(' ').to_string();
        // `GlobalSetProperty`: live objects of the class, then its defaults, then save
        let live: Vec<usize> = (0..self.objects.len()).filter(|&i| self.objects[i].alive && self.is_a(self.objects[i].class, class)).collect();
        for i in live {
            let _ = self.import_item(Sel::Obj(i), &p, p.offset as usize, &value, PPF_LOCALIZED);
        }
        let _ = self.import_item(Sel::Def(class), &p, p.offset as usize, &value, PPF_LOCALIZED);
        let _ = self.save_config_of(Sel::Def(class));
        Some(String::new())
    }

    /// `ini:Section.Key Property [Value]` for a class the port does not model: the value straight from / into the
    /// section the key names (the audio subsystem's settings).
    fn ini_fallback(&mut self, get: bool, class_token: &str, rest: &str) -> Option<String> {
        let section = class_token.strip_prefix("ini:").or_else(|| class_token.strip_prefix("INI:"))?;
        let section = self.resolve_ini_section(section);
        let mut r = rest;
        let key = parse_token(&mut r, 0x100, true)?;
        if get {
            return Some(self.get_config(&section, &key));
        }
        let value = r.trim_start_matches(' ').to_string();
        if let Some(c) = self.reg.config.as_mut() {
            c.set("System", &section, &key, &value);
            let _ = c.save();
        }
        Some(String::new())
    }

    /// `SaveConfig` (an object) / `StaticSaveConfig` (a class's defaults), called on `this`.
    pub(crate) fn save_config(&mut self, this: u32, static_: bool) -> Result<(), VmError> {
        let sel = match (ref_instance(this), static_) {
            (Some(i), false) => Sel::Obj(i),
            _ => match self.class_of_ref(this) {
                Some(c) => Sel::Def(c),
                None => return Ok(()),
            },
        };
        self.save_config_of(sel)
    }
}

impl Vm {
    /// Element `index` of class `class`'s default property `name` (the class default object), if the class has it.
    pub fn default_value_of(&mut self, class: &str, name: &str, index: u32) -> Option<Value> {
        let c = self.reg.class_by_name(class)?;
        let def = self.reg.find_prop(c, name)?.clone();
        if index >= def.dim.max(1) {
            return None;
        }
        self.class_default_mem(c).ok()?;
        let off = def.offset as usize + index as usize * def.elem_size as usize;
        self.read_from(&self.defaults[&c], &def, off).ok()
    }

    /// Sets element `index` of class `class`'s default property `name` (objects made afterwards start with it).
    pub fn set_class_default(&mut self, class: &str, name: &str, index: u32, v: Value) -> Result<(), VmError> {
        let c = self.reg.class_by_name(class).ok_or_else(|| VmError::Runtime(format!("no class {class}")))?;
        let def = self.reg.find_prop(c, name).ok_or_else(|| VmError::Runtime(format!("no {class}.{name}")))?.clone();
        if index >= def.dim.max(1) {
            return Err(VmError::Runtime(format!("{class}.{name}[{index}] out of range")));
        }
        self.class_default_mem(c)?;
        let off = def.offset as usize + index as usize * def.elem_size as usize;
        let m = self.defaults.get_mut(&c).expect("built above");
        Self::write_to(m, &def, off, v)
    }

    /// The port's menus (`Aerugo`): the display and sound screens without the choices of 2000's hardware (render device,
    /// texture depth and detail, detail textures, decals, effect channels, sample format, 3D hardware), with vertical
    /// sync, frame rate limit, mouse DPI, field of view, a button to the Advanced Graphics screen (`MenuScreenAerugoGraphics`)
    /// and output rate of the port's own; the controls screen's mouse sensitivity in
    /// half steps labelled with the travel of a full turn. Choices are listed in the screens' `choices` defaults (a
    /// `None` is skipped and the others close up; an empty row keeps the Sound screen's art lined up).
    pub fn apply_port_menus(&mut self) {
        let screens: [(&str, &[&str]); 2] = [
            ("MenuScreenDisplay", &["MenuChoice_Brightness", "MenuChoice_FullScreen", "MenuChoice_Resolution", "MenuChoice_AerugoVSync", "MenuChoice_AerugoFrameLimit", "MenuChoice_AerugoMouseDPI", "MenuChoice_AerugoFOV", "MenuChoice_AerugoGraphics"]),
            ("MenuScreenSound", &["MenuChoice_MusicVolume", "MenuChoice_SoundVolume", "MenuChoice_SpeechVolume", "MenuChoice_AerugoSpacer", "MenuChoice_AerugoOutputRate", "MenuChoice_ReverseStereo", "MenuChoice_SurroundSound"]),
        ];
        let class_ref = |vm: &mut Vm, name: &str| -> Option<Value> {
            let c = vm.reg.class_by_name(name)?;
            let path = vm.reg.class(c).path.clone();
            Some(Value::Obj(vm.reg.obj_id(&path).0))
        };
        if self.reg.class_by_name("MenuChoice_AerugoVSync").is_none() {
            return;
        }
        for (screen, list) in screens {
            let Some(dim) = self.reg.class_by_name(screen).and_then(|c| self.reg.find_prop(c, "choices")).map(|p| p.dim) else { continue };
            for i in 0..dim {
                let v = list.get(i as usize).and_then(|n| class_ref(self, n)).unwrap_or(Value::Obj(0));
                let _ = self.set_class_default(screen, "choices", i, v);
            }
        }
        // the controls screen: its sensitivity slider replaced in place
        if let (Some(dim), Some(old), Some(new)) = (
            self.reg.class_by_name("MenuScreenControls").and_then(|c| self.reg.find_prop(c, "choices")).map(|p| p.dim),
            class_ref(self, "MenuChoice_MouseSensitivity"),
            class_ref(self, "MenuChoice_AerugoMouseSpeed"),
        ) {
            for i in 0..dim {
                if self.default_value_of("MenuScreenControls", "choices", i) == Some(old.clone()) {
                    let _ = self.set_class_default("MenuScreenControls", "choices", i, new.clone());
                }
            }
        }
    }
}

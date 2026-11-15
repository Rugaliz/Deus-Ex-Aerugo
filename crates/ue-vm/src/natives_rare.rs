//! Natives no Deus Ex script calls (census 3.60), ported so that mods behave as in the original: colour operators,
//! `OrthoRotation`, `GetSoundDuration`, `GetEntryLevel`, `GetLocalURL`, `ParseKillMessage`, the clipboard and the
//! network address queries of a standalone game. `ZoneActors` is a walk in `iterators.rs`.
use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

/// `appAtan2(y, x) * 32768 / PI` truncated (`_ftol`), the constant being the double at Core 10178060.
fn angle(y: f64, x: f64) -> i32 {
    crate::crt::x87_atan2_units(y, x, 10430.378350470453)
}

fn color(v: Option<&Value>) -> [u8; 4] {
    match v {
        Some(Value::Struct(_, b)) if b.len() >= 4 => [b[0], b[1], b[2], b[3]],
        _ => [0; 4],
    }
}

/// `(BYTE)(channel * f)`: the product on the x87 stack, truncated, the low byte kept.
fn scale_byte(c: u8, f: f32) -> u8 {
    crate::route::ftol(c as f64 * f as f64) as u8
}

/// One `Object=(...)` line of an `.int` file's `[Public]` section (`FRegistryObjectInfo`).
#[derive(Debug, Clone, Default)]
pub struct RegistryObject {
    pub object: String,
    pub class: String,
    pub meta_class: String,
    pub description: String,
}

/// `Parse(Stream, Match, Value)` (Core): the first occurrence of `key` anywhere in the text, ignoring case (so
/// `Class=` also matches inside `MetaClass=` when that comes first); a quoted value runs to the next quote, otherwise
/// to the first space, tab, line break, comma or closing bracket.
pub(crate) fn parse_value(text: &str, key: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let at = lower.find(&key.to_ascii_lowercase())? + key.len();
    let rest = &text[at..];
    Some(if let Some(q) = rest.strip_prefix('"') {
        q.split('"').next().unwrap_or("").to_string()
    } else {
        rest.split([' ', '\r', '\n', '\t', ',', ')']).next().unwrap_or("").to_string()
    })
}

impl Vm {
    /// `UObject::CacheDrivers` (Core 10158900): for every `Paths` entry, the `.int` files of its folder (listing
    /// order), and in each the `[Public]` section's `Object=` lines, the brackets around the value removed.
    pub fn registry_objects(&mut self) -> Vec<RegistryObject> {
        let Some(system) = self.reg.game_dir().map(|g| g.join("System")) else { return Vec::new() };
        let paths: Vec<String> = match self.reg.config.as_mut() {
            Some(c) => c.values("System", "Core.System", "Paths"),
            None => Vec::new(),
        };
        let mut out = Vec::new();
        for p in paths {
            let Some(star) = p.find("*.") else { continue };
            let dir = system.join(p[..star].replace('\\', "/"));
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut files: Vec<String> = rd
                .flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| n.rsplit_once('.').is_some_and(|(_, x)| x.eq_ignore_ascii_case("int")))
                .collect();
            crate::save_dir::listing_order(&mut files);
            for f in files {
                let Ok(bytes) = std::fs::read(dir.join(&f)) else { continue };
                let text = ue_config::decode(&bytes);
                let mut in_public = false;
                for line in text.lines() {
                    let l = line.trim();
                    if l.starts_with('[') {
                        in_public = l.eq_ignore_ascii_case("[Public]");
                        continue;
                    }
                    if !in_public {
                        continue;
                    }
                    let Some((k, v)) = l.split_once('=') else { continue };
                    if !k.trim().eq_ignore_ascii_case("Object") {
                        continue;
                    }
                    let mut v = v.to_string();
                    if v.starts_with('(') {
                        v.remove(0);
                    }
                    if v.ends_with(')') {
                        v.pop();
                    }
                    out.push(RegistryObject {
                        object: parse_value(&v, "Name=").unwrap_or_default(),
                        class: parse_value(&v, "Class=").unwrap_or_default(),
                        meta_class: parse_value(&v, "MetaClass=").unwrap_or_default(),
                        description: parse_value(&v, "Description=").unwrap_or_default(),
                    });
                }
            }
        }
        out
    }

    /// `GetRegistryObjects(Class, MetaClass)`: the entries of that class, and of that meta class's path name.
    fn registry_of(&mut self, class: &str, meta: Option<&str>) -> Vec<RegistryObject> {
        self.registry_objects()
            .into_iter()
            .filter(|r| r.class.eq_ignore_ascii_case(class) && meta.is_none_or(|m| r.meta_class.eq_ignore_ascii_case(m)))
            .collect()
    }

    /// The meta class `GetNextInt`/`GetNextIntDesc` look up by name (`StaticFindObjectChecked(UClass, ANY_PACKAGE)`).
    /// A class with no script (`RenderDevice`, `AudioSubsystem`, `NetDriver`: C++ only) is found by the original too;
    /// the port then takes a dotted name as the path, or the registry's meta class of that last name.
    fn class_path_named(&mut self, name: &str) -> Option<String> {
        if let Some(c) = self.reg.class_by_path(name).or_else(|| self.reg.class_by_name(name.rsplit('.').next().unwrap_or(name))) {
            return Some(self.reg.class(c).path.clone());
        }
        if name.contains('.') {
            return Some(name.to_string());
        }
        let suffix = format!(".{}", name.to_ascii_lowercase());
        self.registry_objects().into_iter().map(|r| r.meta_class).find(|m| m.to_ascii_lowercase().ends_with(&suffix))
    }

    /// `GetMapName` (Engine 103b99f0): the map files `<folder><NameEnding>*.<MapExt>` of every `Paths` entry holding
    /// `*.<MapExt>` (each name once, ignoring case), then the one `Dir` places from `MapName` (exact case) with
    /// wrap-around; the first when `MapName` is not among them, "" when there are none.
    fn map_name_step(&mut self, ending: &str, map: &str, dir: i32) -> String {
        let ext = self.reg.config_value("System", "URL", "MapExt").map(|e| ue_config::unquote(&e)).unwrap_or_else(|| "dx".into());
        let pattern = format!("*.{ext}");
        let paths: Vec<String> = match self.reg.config.as_mut() {
            Some(c) => c.values("System", "Core.System", "Paths"),
            None => Vec::new(),
        };
        let Some(system) = self.reg.game_dir().map(|g| g.join("System")) else { return String::new() };
        let mut list: Vec<String> = Vec::new();
        for p in paths {
            let Some(at) = p.find(&pattern) else { continue };
            let folder = system.join(p[..at].replace('\\', "/"));
            let Ok(rd) = std::fs::read_dir(&folder) else { continue };
            let mut found: Vec<String> = rd
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| n.len() >= ending.len() && n[..ending.len()].eq_ignore_ascii_case(ending))
                .filter(|n| n.rsplit_once('.').is_some_and(|(_, x)| x.eq_ignore_ascii_case(&ext)))
                .collect();
            crate::save_dir::listing_order(&mut found);
            for n in found {
                if !list.iter().any(|l| l.eq_ignore_ascii_case(&n)) {
                    list.push(n);
                }
            }
        }
        if list.is_empty() {
            return String::new();
        }
        let Some(i) = list.iter().position(|l| l == map) else { return list[0].clone() };
        let mut j = i as i64 + dir as i64;
        if j < 0 {
            j = list.len() as i64 - 1;
        } else if j >= list.len() as i64 {
            j = 0;
        }
        list[j as usize].clone()
    }

    pub(crate) fn native_rare(&mut self, this: u32, name: &str, a: &mut [Value]) -> Option<Res<Value>> {
        use Value::*;
        let color_struct = |vm: &Vm, c: [u8; 4]| -> Value {
            let id = match a.first() {
                Some(Struct(id, _)) => *id,
                _ => match a.get(1) {
                    Some(Struct(id, _)) => *id,
                    _ => match vm.reg.struct_id_by_path("Core.Object.Color") {
                        Some(id) => id,
                        None => return Void,
                    },
                },
            };
            Struct(id, c.to_vec())
        };
        Some(Ok(match name {
            // Engine 103011e5 / 103023fb: channel by channel with byte wrap-around, alpha from A
            "Add_ColorColor" | "Subtract_ColorColor" => {
                let (x, y) = (color(a.first()), color(a.get(1)));
                let op = |p: u8, q: u8| if name == "Add_ColorColor" { p.wrapping_add(q) } else { p.wrapping_sub(q) };
                color_struct(self, [op(x[0], y[0]), op(x[1], y[1]), op(x[2], y[2]), x[3]])
            }
            // Engine 10301668 (Color * float) / 103036e8 (float * Color): `(BYTE)(channel * f)`, alpha kept
            "Multiply_ColorFloat" | "Multiply_FloatColor" => {
                let (c, f) = if name == "Multiply_ColorFloat" {
                    (color(a.first()), a.get(1).and_then(Value::as_f32).unwrap_or(0.0))
                } else {
                    (color(a.get(1)), a.first().and_then(Value::as_f32).unwrap_or(0.0))
                };
                color_struct(self, [scale_byte(c[0], f), scale_byte(c[1], f), scale_byte(c[2], f), c[3]])
            }
            // `FCoords::OrthoRotation` (Core 10144830): yaw and pitch of the X axis, then the roll of Z and Y against
            // the Y axis of the unit frame turned back by that pitch and yaw
            "OrthoRotation" => {
                let v = |i: usize| match a.get(i) {
                    Some(Vector(v)) => *v,
                    _ => [0.0; 3],
                };
                let (x, y, z) = (v(0), v(1), v(2));
                let yaw = angle(x[1] as f64, x[0] as f64);
                let pitch = angle(x[2] as f64, ((x[0] * x[0] + x[1] * x[1]) as f64).sqrt());
                let mut s = ue_bsp::Coords::UNIT;
                s.div_rotator([pitch, yaw, 0], crate::natives::sin_tab);
                let d = |p: [f32; 3], q: [f32; 3]| p[0] as f64 * q[0] as f64 + p[1] as f64 * q[1] as f64 + p[2] as f64 * q[2] as f64;
                let roll = angle(d(s.y, z), d(s.y, y));
                Rotator([pitch, yaw, roll])
            }
            // Engine 10302c11: `USound::Duration`, worked out once (`FSoundData::GetPeriod`) while it is below 0
            "GetSoundDuration" => {
                let s = match a.first() {
                    Some(Obj(s)) => *s,
                    _ => 0,
                };
                if s == 0 {
                    // the original reads through None here (a crash); a mod would not get this far
                    Float(0.0)
                } else {
                    Float(self.sound_length(s))
                }
            }
            // Engine 10303be8: the entry level's `LevelInfo` (`GEntry->Actors(0)`)
            "GetEntryLevel" => Obj(self.entry_level_info),
            // Engine 10302842: `XLevel->URL.String()`: the map as browsed, `?option` for each of the login options
            // (the user's `[DefaultPlayer]` and the URL's), `#portal`
            "GetLocalURL" => {
                let mut s = if self.url_map.is_empty() {
                    self.level_paths.get(&self.level).and_then(|p| p.split('.').next()).unwrap_or("").to_string()
                } else {
                    self.url_map.clone()
                };
                s.push_str(&self.url_options_string());
                if !self.url_portal.is_empty() {
                    s.push('#');
                    s.push_str(&self.url_portal);
                }
                Str(s)
            }
            // a standalone game has no network connection or driver: empty strings
            "GetPlayerNetworkAddress" | "GetNetworkNumber" => Str(String::new()),
            // Engine 103e6c70: the first `%k`, `%o`, `%w` replaced by killer, victim and weapon. When the message has
            // no `%k`, the result is empty (the original copies its still-empty output string back).
            "ParseKillMessage" => {
                let s = |i: usize| match a.get(i) {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                let (killer, victim, weapon, message) = (s(0), s(1), s(2), s(3));
                let replace = |text: &str, mark: &str, by: &str| -> String {
                    match text.find(mark) {
                        Some(p) => format!("{}{}{}", &text[..p], by, &text[p + mark.len()..]),
                        None => String::new(),
                    }
                };
                let mut out = replace(&message, "%k", &killer);
                if let Some(p) = out.find("%o") {
                    out = format!("{}{}{}", &out[..p], victim, &out[p + 2..]);
                }
                if let Some(p) = out.find("%w") {
                    out = format!("{}{}{}", &out[..p], weapon, &out[p + 2..]);
                }
                Str(out)
            }
            // `appClipboardCopy`/`appClipboardPaste`: the port keeps the text (the host may mirror it)
            "CopyToClipboard" => {
                if let Some(Str(s)) = a.first() {
                    self.clipboard = s.clone();
                }
                Void
            }
            "PasteFromClipboard" => Str(self.clipboard.clone()),
            "GetMapName" => {
                let st = |i: usize| match a.get(i) {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                let dir = a.get(2).and_then(Value::as_i32).unwrap_or(0);
                Str(self.map_name_step(&st(0), &st(1), dir))
            }
            // Engine 103b8010 / 103b8310: entry `Num` of the registry's classes of meta class `ClassName` ("" past
            // the end; the original reads before its array for a negative number)
            "GetNextInt" | "GetNextIntDesc" => {
                let class = match a.first() {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                let num = a.get(1).and_then(Value::as_i32).unwrap_or(0);
                let list = match self.class_path_named(&class) {
                    Some(meta) => self.registry_of("Class", Some(&meta)),
                    None => Vec::new(),
                };
                let e = usize::try_from(num).ok().and_then(|i| list.get(i)).cloned().unwrap_or_default();
                if name == "GetNextInt" {
                    Str(e.object)
                } else {
                    if a.len() > 3 {
                        a[2] = Str(e.object);
                        a[3] = Str(e.description);
                    }
                    Void
                }
            }
            // Engine 103b8f10: among the registry's textures whose name starts with `Prefix` (ignoring case): Dir 0
            // the current one, -1 the one before it, 1 the one after it (wrapping to the first); otherwise the last
            "GetNextSkin" => {
                let st = |i: usize| match a.get(i) {
                    Some(Str(s)) => s.clone(),
                    _ => String::new(),
                };
                let (prefix, current) = (st(0), st(1));
                let dir = a.get(2).and_then(Value::as_i32).unwrap_or(0);
                let list = self.registry_of("Texture", None);
                let (mut best, mut first, mut last, mut next) = (-1i64, -1i64, -1i64, false);
                for (i, r) in list.iter().enumerate() {
                    let i = i as i64;
                    let starts = r.object.len() >= prefix.len() && r.object.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes());
                    if !starts {
                        continue;
                    }
                    if next {
                        best = i;
                        break;
                    }
                    if first == -1 {
                        first = i;
                    }
                    if r.object.eq_ignore_ascii_case(&current) {
                        if dir == -1 {
                            best = last;
                            break;
                        } else if dir == 0 {
                            best = i;
                            break;
                        }
                        next = true;
                    }
                    last = i;
                }
                if next && best == -1 {
                    best = first;
                }
                if best == -1 {
                    best = last;
                }
                let e = usize::try_from(best).ok().and_then(|i| list.get(i)).cloned().unwrap_or_default();
                if a.len() > 4 {
                    a[3] = Str(e.object);
                    a[4] = Str(e.description);
                }
                Void
            }
            _ => {
                let _ = this;
                return None;
            }
        }))
    }
}

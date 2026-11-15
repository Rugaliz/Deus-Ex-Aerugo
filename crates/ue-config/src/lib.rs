//! Configuration of the engine: layered ini files and localised text.
//!
//! The engine has two configuration files, selected per class by its config name: `System` (the main ini, `DeusEx.ini`)
//! and `User` (`User.ini`); other names refer to ini files of that name (`OpenGlDrv`). Each is a stack of layers, lowest
//! first: the shipped defaults (`Default.ini` / `DefUser.ini`), the ini in the game folder (read only, so the settings of an
//! existing installation carry over), and the port's own file in the per-user configuration directory. Only the last layer
//! is ever written. Localised text comes from `<Package>.int` in the game's `System` folder.

mod ini;

pub use ini::{decode, unquote, Ini, Section};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The per-user configuration directory of this port (`$XDG_CONFIG_HOME/deus-ex-aerugo`, else `~/.config/deus-ex-aerugo`; on
/// Windows `%APPDATA%\deus-ex-aerugo`); never the game's folder.
pub fn user_config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from)?;
    // an empty or relative XDG_CONFIG_HOME counts as unset (XDG Base Directory specification)
    #[cfg(not(windows))]
    let base = {
        let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
        std::env::var_os("XDG_CONFIG_HOME").and_then(absolute).or_else(|| std::env::var_os("HOME").and_then(absolute).map(|h| h.join(".config")))?
    };
    let dir = base.join("deus-ex-aerugo");
    static MOVED: std::sync::Once = std::sync::Once::new();
    MOVED.call_once(|| move_development_config(&base, &dir));
    Some(dir)
}

/// Settings made under the project's development name (`deusex_shin/`, its settings in `DXShin.ini`) move to the
/// current names once, when there is nothing under the new name yet.
fn move_development_config(base: &Path, dir: &Path) {
    let old = base.join("deusex_shin");
    if dir.exists() || !old.is_dir() || std::fs::rename(&old, dir).is_err() {
        return;
    }
    let (from, to) = (dir.join("DXShin.ini"), dir.join("Aerugo.ini"));
    if let Ok(text) = std::fs::read_to_string(&from) {
        if std::fs::write(&to, text.replace("[DXShin.ShinSettings]", "[Aerugo.AerugoSettings]")).is_ok() {
            let _ = std::fs::remove_file(&from);
        }
    }
}

#[derive(Debug, Default)]
struct Stack {
    /// Lowest layer first.
    layers: Vec<Ini>,
    /// The writable per-user layer (also the last element of `layers`).
    user: Ini,
    dirty: bool,
    /// The user layer's file name, as the logical name was first spelled (`DXProbe` -> `DXProbe.ini`).
    user_name: String,
}

#[derive(Debug, Default)]
pub struct Config {
    system_dir: PathBuf,
    user_dir: Option<PathBuf>,
    stacks: HashMap<String, Stack>,
    loc: HashMap<String, Option<Ini>>,
}

/// File names (without extension) of the layers of a logical file, lowest first, and the name of the user layer.
fn layer_names(logical: &str) -> (Vec<String>, String) {
    match logical.to_ascii_lowercase().as_str() {
        "system" | "deusex" => (vec!["Default".into(), "DeusEx".into()], "DeusEx".into()),
        "user" => (vec!["DefUser".into(), "User".into()], "User".into()),
        _ => (vec![logical.to_string()], logical.to_string()),
    }
}

fn canonical(logical: &str) -> String {
    match logical.to_ascii_lowercase().as_str() {
        "system" | "deusex" => "system".into(),
        other => other.to_string(),
    }
}

fn read_ini(path: &Path) -> Ini {
    std::fs::read(path).map(|b| Ini::parse(&decode(&b))).unwrap_or_default()
}

impl Config {
    /// `system_dir` is the game's `System` folder (never written); `user_dir` the per-user directory for the port's own files.
    pub fn new(system_dir: &Path, user_dir: Option<&Path>) -> Config {
        Config { system_dir: system_dir.to_path_buf(), user_dir: user_dir.map(Path::to_path_buf), stacks: HashMap::new(), loc: HashMap::new() }
    }

    /// Reads the stack of a logical file now (idempotent). Call before reading; lookups of files never loaded find nothing.
    pub fn load(&mut self, logical: &str) {
        let key = canonical(logical);
        if self.stacks.contains_key(&key) {
            return;
        }
        let (names, user_name) = layer_names(logical);
        let mut layers: Vec<Ini> = names.iter().map(|n| read_ini(&self.system_dir.join(format!("{n}.ini")))).collect();
        // the mods' ini files of the same names go over the game's (the player's own layer still wins)
        let mods = ue_package::mods::current();
        for n in &names {
            for src in mods.inis.get(&n.to_ascii_lowercase()).into_iter().flatten() {
                if let Ok(b) = src.read() {
                    layers.push(Ini::parse(&decode(&b)));
                }
            }
        }
        let user = self.user_dir.as_ref().map(|d| read_ini(&d.join(format!("{user_name}.ini")))).unwrap_or_default();
        layers.push(user.clone());
        self.stacks.insert(key, Stack { layers, user, dirty: false, user_name });
    }

    /// The value of `section`/`key` in the logical file, from the highest layer that has it. Quotes are kept.
    pub fn value(&self, logical: &str, section: &str, key: &str) -> Option<String> {
        let st = self.stacks.get(&canonical(logical))?;
        st.layers.iter().rev().find_map(|l| l.get(section, key)).map(str::to_string)
    }

    /// The value of `section`/`key` in the port's own (user) layer only: settings this port saved, not the shipped or
    /// game-folder defaults.
    pub fn user_value(&mut self, logical: &str, section: &str, key: &str) -> Option<String> {
        self.load(logical);
        self.stacks.get(&canonical(logical))?.user.get(section, key).map(str::to_string)
    }

    /// Every key of `section` in the logical file with its value from the highest layer that has it, in the order the
    /// keys first appear (what `GConfig->GetSection` hands out). Loads the file if needed.
    pub fn section_entries(&mut self, logical: &str, section: &str) -> Vec<(String, String)> {
        self.load(logical);
        let mut out: Vec<(String, String)> = Vec::new();
        let Some(st) = self.stacks.get(&canonical(logical)) else { return out };
        for l in &st.layers {
            let Some(sec) = l.section(section) else { continue };
            for (k, v) in &sec.entries {
                match out.iter_mut().find(|(ok, _)| ok.eq_ignore_ascii_case(k)) {
                    Some(e) => e.1 = v.clone(),
                    None => out.push((k.clone(), v.clone())),
                }
            }
        }
        out
    }

    /// Every value of a key that may repeat (`Paths=`), from the highest layer that has the key, in file order.
    pub fn values(&mut self, logical: &str, section: &str, key: &str) -> Vec<String> {
        self.load(logical);
        let Some(st) = self.stacks.get(&canonical(logical)) else { return Vec::new() };
        for l in st.layers.iter().rev() {
            let Some(sec) = l.section(section) else { continue };
            let v: Vec<String> = sec.entries.iter().filter(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.clone()).collect();
            if !v.is_empty() {
                return v;
            }
        }
        Vec::new()
    }

    /// Does any layer have this section?
    pub fn has_section(&self, logical: &str, section: &str) -> bool {
        self.stacks.get(&canonical(logical)).map(|st| st.layers.iter().any(|l| l.section(section).is_some())).unwrap_or(false)
    }

    /// Sets a value in the user layer (loads the file if needed). Nothing is written until `save`.
    pub fn set(&mut self, logical: &str, section: &str, key: &str, value: &str) {
        self.load(logical);
        let st = self.stacks.get_mut(&canonical(logical)).unwrap();
        st.user.set(section, key, value);
        let last = st.layers.len() - 1;
        st.layers[last].set(section, key, value);
        st.dirty = true;
    }

    /// Writes the changed user layers to the user directory. Without one, nothing happens.
    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(dir) = self.user_dir.clone() else { return Ok(()) };
        for st in self.stacks.values_mut().filter(|s| s.dirty) {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join(format!("{}.ini", st.user_name)), st.user.to_text())?;
            st.dirty = false;
        }
        Ok(())
    }

    /// Localised text: `[section]` `key` of `<package>.int` in the game's `System` folder.
    pub fn localized(&mut self, package: &str, section: &str, key: &str) -> Option<String> {
        let ini = self
            .loc
            .entry(package.to_ascii_lowercase())
            .or_insert_with(|| {
                if let Some(src) = ue_package::mods::current().ints.get(&package.to_ascii_lowercase()) {
                    return src.read().ok().map(|b| Ini::parse(&decode(&b)));
                }
                let p = std::fs::read_dir(&self.system_dir).ok()?.flatten().map(|e| e.path()).find(|p| {
                    p.extension().map(|e| e.eq_ignore_ascii_case("int")).unwrap_or(false) && p.file_stem().map(|s| s.to_string_lossy().eq_ignore_ascii_case(package)).unwrap_or(false)
                })?;
                Some(read_ini(&p))
            })
            .as_ref()?;
        ini.get(section, key).map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ue-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn layers_override_and_only_the_user_layer_is_written() {
        let sys = temp("sys");
        let usr = temp("usr").join("cfg");
        std::fs::write(sys.join("DefUser.ini"), "[A.B]\nx=1\ny=1\n").unwrap();
        std::fs::write(sys.join("User.ini"), "[A.B]\ny=2\n").unwrap();
        let mut c = Config::new(&sys, Some(&usr));
        c.load("User");
        assert_eq!(c.value("User", "A.B", "x").as_deref(), Some("1"));
        assert_eq!(c.value("user", "a.b", "y").as_deref(), Some("2"));
        c.set("User", "A.B", "y", "3");
        assert_eq!(c.value("User", "A.B", "y").as_deref(), Some("3"));
        c.save().unwrap();
        assert_eq!(std::fs::read_to_string(sys.join("User.ini")).unwrap(), "[A.B]\ny=2\n", "the game folder is never written");
        let mut again = Config::new(&sys, Some(&usr));
        again.load("User");
        assert_eq!(again.value("User", "A.B", "y").as_deref(), Some("3"));
        assert_eq!(again.value("User", "A.B", "x").as_deref(), Some("1"));
    }

    #[test]
    fn int_files_give_localised_text() {
        let sys = temp("int");
        std::fs::write(sys.join("Thing.int"), "[Cls]\nMsg=\"Hello %s\"\n").unwrap();
        let mut c = Config::new(&sys, None);
        assert_eq!(c.localized("thing", "cls", "Msg").map(|v| unquote(&v)).as_deref(), Some("Hello %s"));
        assert_eq!(c.localized("Nope", "Cls", "Msg"), None);
    }
}

#[cfg(test)]
mod development_name {
    #[test]
    fn settings_move_to_the_current_names_once() {
        let base = std::env::temp_dir().join(format!("aerugo-move-test-{}", std::process::id()));
        let (old, new) = (base.join("deusex_shin"), base.join("deus-ex-aerugo"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("DXShin.ini"), "[DXShin.ShinSettings]\nVSync=2\n").unwrap();
        std::fs::write(old.join("Input.ini"), "x").unwrap();
        super::move_development_config(&base, &new);
        assert!(!old.exists());
        assert_eq!(std::fs::read_to_string(new.join("Aerugo.ini")).unwrap(), "[Aerugo.AerugoSettings]\nVSync=2\n");
        assert!(!new.join("DXShin.ini").exists() && new.join("Input.ini").exists());
        // a second folder under the old name is left alone once the new one exists
        std::fs::create_dir_all(&old).unwrap();
        super::move_development_config(&base, &new);
        assert!(old.exists());
        std::fs::remove_dir_all(&base).unwrap();
    }
}

//! Configuration at run time: the `Localize`/`GetConfig` natives, `SaveConfig`, and the `get`/`set` console commands that the
//! options screens use to read and change class defaults (`get DeusExPlayer bAlwaysRun`, `set DeusExPlayer bAlwaysRun False`).

use crate::Vm;

impl Vm {
    /// `Localize(section, key, package)`: text from `<package>.int` (at most 1023 characters), or the engine's "not
    /// found" marker `<?int?Package.Section.Key?>`.
    pub(crate) fn localize(&mut self, section: &str, key: &str, package: &str) -> String {
        match self.reg.config.as_mut().and_then(|c| c.localized(package, section, key)) {
            // read into a 1024-character buffer
            Some(v) => ue_config::unquote(&v).chars().take(1023).collect(),
            // `Localize` (10148430) tries the language's file then `int`, and names the last one tried
            None => format!("<?int?{package}.{section}.{key}?>"),
        }
    }

    /// `GetConfig(section, key)`: a value from the main ini (`System`), falling back to the user ini.
    pub(crate) fn get_config(&mut self, section: &str, key: &str) -> String {
        for file in ["System", "User"] {
            if let Some(v) = self.reg.config_value(file, section, key) {
                return ue_config::unquote(&v);
            }
        }
        String::new()
    }

    /// The menus address the audio settings as `ini:Engine.Engine.AudioDevice MusicVolume`: a section name whose last part
    /// is a key of the section before it (`[Engine.Engine] AudioDevice=Galaxy.GalaxyAudioSubsystem`) stands for the
    /// section named by that key's value. (Read from how the shipped menus and ini fit together; the original's rule is
    /// UNVERIFIED.)
    pub(crate) fn resolve_ini_section(&mut self, section: &str) -> String {
        if let Some((base, key)) = section.rsplit_once('.') {
            if base.contains('.') {
                for file in ["System", "User"] {
                    if let Some(v) = self.reg.config_value(file, base, key) {
                        return ue_config::unquote(&v);
                    }
                }
            }
        }
        section.to_string()
    }
}

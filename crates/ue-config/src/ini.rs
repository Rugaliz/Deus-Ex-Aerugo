//! The engine's ini dialect: `[Section]` headers, `Key=Value` lines (the value is everything after the first `=`),
//! `;` comments, case-insensitive section and key names, keys that may repeat (`Paths=`, `Aliases[0]=`).

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ini {
    pub sections: Vec<Section>,
}

/// Decodes file bytes: UTF-16 with a byte order mark, UTF-8 with or without one, anything else as Latin-1.
pub fn decode(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let le = bytes[0] == 0xFF;
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

impl Ini {
    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        let mut cur: Option<usize> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                let name = rest.split(']').next().unwrap_or("").trim();
                cur = Some(ini.section_index_or_add(name));
                continue;
            }
            let (Some(i), Some((k, v))) = (cur, line.split_once('=')) else { continue };
            ini.sections[i].entries.push((k.trim().to_string(), v.trim().to_string()));
        }
        ini
    }

    fn section_index(&self, name: &str) -> Option<usize> {
        self.sections.iter().position(|s| s.name.eq_ignore_ascii_case(name))
    }

    fn section_index_or_add(&mut self, name: &str) -> usize {
        match self.section_index(name) {
            Some(i) => i,
            None => {
                self.sections.push(Section { name: name.to_string(), entries: Vec::new() });
                self.sections.len() - 1
            }
        }
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.section_index(name).map(|i| &self.sections[i])
    }

    /// The value of `key` (the last occurrence wins, as when a later line overrides an earlier one).
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.section(section)?.entries.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    /// Sets `key`, replacing its first occurrence (further occurrences are dropped) or appending it.
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let i = self.section_index_or_add(section);
        let entries = &mut self.sections[i].entries;
        match entries.iter().position(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some(p) => {
                entries[p].1 = value.to_string();
                let mut seen = false;
                entries.retain(|(k, _)| {
                    if k.eq_ignore_ascii_case(key) {
                        let keep = !seen;
                        seen = true;
                        keep
                    } else {
                        true
                    }
                });
            }
            None => entries.push((key.to_string(), value.to_string())),
        }
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for sec in &self.sections {
            s += &format!("[{}]\n", sec.name);
            for (k, v) in &sec.entries {
                s += &format!("{k}={v}\n");
            }
            s.push('\n');
        }
        s
    }
}

/// Removes one pair of surrounding double quotes (and `\"` escapes inside); other text is returned as is.
pub fn unquote(v: &str) -> String {
    let v = v.trim();
    match v.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner) => inner.replace("\\\"", "\"").replace("\\\\", "\\"),
        None => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_keys_and_repeats() {
        let ini = Ini::parse("\u{feff}; c\r\n[Engine.Engine]\r\nGameRenderDevice=OpenGlDrv.OpenGLRenderDevice\r\nPaths=a\r\nPaths=b\r\n\r\n[user.Thing]\r\nName = Jo=hn \r\n");
        assert_eq!(ini.get("engine.engine", "gamerenderdevice"), Some("OpenGlDrv.OpenGLRenderDevice"));
        assert_eq!(ini.get("Engine.Engine", "Paths"), Some("b"));
        assert_eq!(ini.get("User.Thing", "Name"), Some("Jo=hn"));
        assert_eq!(ini.get("Engine.Engine", "Nope"), None);
    }

    #[test]
    fn set_replaces_appends_and_round_trips() {
        let mut ini = Ini::parse("[A]\nx=1\nx=2\ny=3\n");
        ini.set("a", "X", "9");
        ini.set("A", "z", "4");
        ini.set("B", "k", "v");
        assert_eq!(Ini::parse(&ini.to_text()), ini);
        assert_eq!(ini.section("A").unwrap().entries.len(), 3);
        assert_eq!(ini.get("A", "x"), Some("9"));
    }

    #[test]
    fn quotes_and_encodings() {
        assert_eq!(unquote("\"You can't\""), "You can't");
        assert_eq!(unquote("plain"), "plain");
        assert_eq!(decode(&[0xFF, 0xFE, b'a', 0, b'b', 0]), "ab");
        assert_eq!(decode(&[b'c', 0xE9]), "c\u{e9}");
    }
}

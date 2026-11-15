//! `DeusExTextParser`: the tag reader behind datacubes, books, newspapers, emails, bulletins, the credits and the quotes
//! (spec: `docs/text-parser.md`). The texts are `ExtString` objects, by default in the `DeusExText` package.
//!
//! The scripts open a text, call `ProcessText` until it returns false and after each call ask `GetTag` what came out:
//! a run of plain text (tag 0, read with `GetText`) or one tag (`<P>`, `<B>`, `<FILE=...>`, ...). The rules follow the
//! original parser closely, including its quirks (they decide where blank lines and paragraphs fall in every reader).

use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

/// The tag spellings, in the order of `EDeusExTextTags` (index = tag value). A tag matches the FIRST entry its text starts with.
const TAGS: [&str; 30] = [
    "TEXT", "FILE", "EMAIL", "NOTE", "/NOTE", "GOAL", "/GOAL", "COMMENT", "/COMMENT", "PLAYERNAME", "PLAYERFIRSTNAME", "NP", "JC",
    "JL", "JR", "DC", "C", "/C", "P", "B", "/B", "U", "/U", "I", "/I", "G", "F", "L", "/<", "/>",
];

pub const TT_TEXT: u8 = 0;
const TT_NOTE: u8 = 3;
const TT_END_NOTE: u8 = 4;
const TT_GOAL: u8 = 5;
const TT_END_GOAL: u8 = 6;
const TT_COMMENT: u8 = 7;
const TT_END_COMMENT: u8 = 8;
const TT_DEFAULT_COLOR: u8 = 15;
const TT_END_COLOR: u8 = 17;
pub const TT_NEW_PARAGRAPH: u8 = 18;
const TT_GRAPHIC: u8 = 25;
const TT_LABEL: u8 = 27;
pub const TT_NONE: u8 = 30;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmailInfo {
    pub name: String,
    pub subject: String,
    pub from: String,
    pub to: String,
    pub cc: String,
}

#[derive(Clone, Debug)]
pub struct TextParser {
    /// The open text with a terminating NUL (`None` = closed).
    text: Option<Vec<char>>,
    /// Read position (`None` = past the end).
    pos: Option<usize>,
    pub last_text: String,
    pub last_tag: u8,
    pub last_name: String,
    pub last_color: [u8; 4],
    pub default_color: [u8; 4],
    paragraph_started: bool,
    pub player_name: String,
    pub player_first_name: String,
    pub email: EmailInfo,
    pub file_name: String,
    pub file_description: String,
}

impl Default for TextParser {
    fn default() -> Self {
        TextParser {
            text: None,
            pos: None,
            last_text: String::new(),
            last_tag: TT_NONE,
            last_name: String::new(),
            last_color: [0; 4],
            default_color: [0; 4],
            paragraph_started: false,
            player_name: String::new(),
            player_first_name: String::new(),
            email: EmailInfo::default(),
            file_name: String::new(),
            file_description: String::new(),
        }
    }
}

/// C `atoi`: optional leading blanks and sign, then digits; anything else ends the number.
fn atoi(s: &str) -> i32 {
    let t = s.trim_start_matches([' ', '\t', '\n', '\r']);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v: i32 = 0;
    for c in digits.chars() {
        let Some(d) = c.to_digit(10) else { break };
        v = v.wrapping_mul(10).wrapping_add(d as i32);
    }
    if neg { v.wrapping_neg() } else { v }
}

/// The comma-separated values after the `=` of a tag (`None` if the tag has no `=`). Past the last value, more
/// values read as empty strings.
fn tag_values(body: &str) -> Option<impl Iterator<Item = &str>> {
    let (_, rest) = body.split_once('=')?;
    Some(rest.split(',').chain(std::iter::repeat("")))
}

impl TextParser {
    /// Starts reading `text` (`None`: the text was not found; the parser stays closed and `open` returns false).
    pub fn open(&mut self, text: Option<&str>) -> bool {
        self.close();
        if let Some(t) = text {
            let mut v: Vec<char> = t.chars().collect();
            v.push('\0');
            self.text = Some(v);
            self.pos = Some(0);
        }
        self.paragraph_started = false;
        self.text.is_some()
    }

    pub fn close(&mut self) {
        self.text = None;
        self.pos = None;
    }

    pub fn is_eof(&self) -> bool {
        self.text.is_none() || self.pos.is_none()
    }

    /// The player's name for `<PLAYERNAME>`; `<PLAYERFIRSTNAME>` is the part before the first space (all of it without one).
    pub fn set_player_name(&mut self, name: &str) {
        self.player_name = name.to_string();
        self.player_first_name = match name.find(' ') {
            Some(i) => name[..i].to_string(),
            None => name.to_string(),
        };
    }

    /// Reads the next block (text run or tag) into `last_text`/`last_tag`; false once the end has been reached.
    pub fn process(&mut self) -> bool {
        self.last_text = self.text_block();
        !self.is_eof()
    }

    /// `GetName`: the name of the last `<GOAL=>`, `<G=>`, `<F=>` (and, as in the original, `<NOTE>`/`<L>`, which
    /// leave the previous one), else None.
    pub fn name(&self) -> Option<&str> {
        match self.last_tag {
            TT_NOTE | TT_GRAPHIC..=TT_LABEL => Some(&self.last_name),
            _ => None,
        }
    }

    /// `GetColor`: the colour of the last `<DC=>`/`<C=>`, the default colour after `</C>`, else zero.
    pub fn color(&self) -> [u8; 4] {
        match self.last_tag {
            TT_DEFAULT_COLOR | 16 => self.last_color,
            TT_END_COLOR => self.default_color,
            _ => [0; 4],
        }
    }

    fn ch(&self) -> char {
        match (&self.text, self.pos) {
            (Some(t), Some(p)) => t.get(p).copied().unwrap_or('\0'),
            _ => '\0',
        }
    }

    fn advance(&mut self) {
        if let Some(p) = self.pos.as_mut() {
            *p += 1;
        }
    }

    /// Text before the first `<P>` is skipped. A run of text ends before the next tag; a tag ends the block at once,
    /// except the first `<P>`, which only starts the text (and is not reported). Line breaks read as spaces.
    fn text_block(&mut self) -> String {
        self.last_tag = TT_NONE;
        if self.is_eof() {
            return String::new();
        }
        if self.ch() == '\0' {
            self.pos = None;
            return String::new();
        }
        let mut out = String::new();
        let mut appended = false;
        loop {
            let c = self.ch();
            if c == '\0' {
                break;
            }
            if c == '<' {
                if appended {
                    break;
                }
                let s = self.tag();
                out.push_str(&s);
                if self.last_tag != TT_NEW_PARAGRAPH || self.paragraph_started {
                    break;
                }
                self.paragraph_started = true;
            } else {
                if self.paragraph_started {
                    out.push(if c == '\r' || c == '\n' { ' ' } else { c });
                    self.last_tag = TT_TEXT;
                    appended = true;
                }
                self.advance();
            }
        }
        out
    }

    /// Reads one tag at the read position (`<`); sets `last_tag` when the tag is known and returns its text
    /// (player names, `<` / `>` for the escapes, empty for the rest).
    fn tag(&mut self) -> String {
        let (Some(t), Some(start)) = (self.text.as_ref(), self.pos) else { return String::new() };
        let mut end = start;
        while t[end] != '>' && t[end] != '\0' {
            end += 1;
        }
        let body: String = t[start + 1..end].iter().collect();
        // past the `>`; an unclosed tag runs to the end of the text
        self.pos = Some(if t[end] == '>' { end + 1 } else { end });
        let Some(tag) = TAGS.iter().position(|k| body.starts_with(k)) else { return String::new() };
        self.last_tag = tag as u8;
        self.token(tag as u8, &body)
    }

    fn token(&mut self, tag: u8, body: &str) -> String {
        match tag {
            1 => {
                if let Some(mut v) = tag_values(body) {
                    self.file_name = v.next().unwrap_or("").trim_matches(' ').to_string();
                    self.file_description = v.next().unwrap_or("").trim_matches(' ').to_string();
                }
            }
            2 => {
                if let Some(v) = tag_values(body) {
                    let f: Vec<String> = v.take(5).map(|s| s.trim_matches(' ').to_string()).collect();
                    let [name, subject, from, to, cc] = <[String; 5]>::try_from(f).unwrap_or_default();
                    self.email = EmailInfo { name, subject, from, to, cc };
                }
            }
            // notes, goals and comments swallow everything up to their end tag (which becomes the reported tag)
            TT_NOTE => {
                self.find_end_tag(TT_END_NOTE);
            }
            TT_GOAL => {
                self.parse_name(body);
                self.find_end_tag(TT_END_GOAL);
            }
            TT_COMMENT => {
                self.find_end_tag(TT_END_COMMENT);
            }
            9 => return self.player_name.clone(),
            10 => return self.player_first_name.clone(),
            15..=17 => {
                self.last_color = match tag_values(body) {
                    Some(mut v) => {
                        let mut c = || atoi(v.next().unwrap_or("")) as u8;
                        [c(), c(), c(), 0]
                    }
                    None => [0; 4],
                };
            }
            TT_GRAPHIC | 26 => self.parse_name(body),
            28 => return "<".into(),
            29 => return ">".into(),
            _ => {}
        }
        String::new()
    }

    fn parse_name(&mut self, body: &str) {
        if let Some((_, n)) = body.split_once('=') {
            self.last_name = n.to_string();
        }
    }

    /// Skips to the end tag `want`, returning the text in between (unused by the callers, as in the original). Like
    /// the original, one character after every other tag inside is skipped.
    fn find_end_tag(&mut self, want: u8) -> String {
        let mut out = String::new();
        loop {
            let c = self.ch();
            if c == '\0' {
                break;
            }
            if c == '<' {
                let s = self.tag();
                out.push_str(&s);
                if self.last_tag == want {
                    break;
                }
                if self.ch() == '\0' {
                    break;
                }
            } else {
                out.push(if c == '\r' || c == '\n' { ' ' } else { c });
            }
            self.advance();
        }
        out
    }
}

impl Vm {
    /// The text of `ExtString` `Package.Name` from the game's packages (loaded on demand), if it exists.
    pub fn ext_string_text(&mut self, path: &str) -> Option<String> {
        self.reg.ensure_package_for_path(path);
        let (pi, ei) = self.reg.export_by_path(path)?;
        let pkg = &self.reg.packages[pi].pkg;
        if !pkg.export_class_name(ei).eq_ignore_ascii_case("ExtString") {
            return None;
        }
        ue_assets::text::load_ext_string(pkg, ei).ok()
    }

    /// The natives of `DeusExTextParser` (the parser's state lives in the host, keyed by the parser object).
    pub(crate) fn native_text_parser(&mut self, this: u32, name: &str, a: &mut [Value]) -> Option<Res<Value>> {
        use Value::*;
        const NAMES: [&str; 12] = [
            "OpenText", "CloseText", "ProcessText", "IsEOF", "GetText", "GotoLabel", "GetTag", "GetName", "GetColor", "GetEmailInfo",
            "GetFileInfo", "SetPlayerName",
        ];
        if !NAMES.contains(&name) || !self.is_class(this, "DeusExTextParser") {
            return None;
        }
        let s = |a: &[Value], i: usize| match a.get(i) {
            Some(Str(s)) => s.clone(),
            _ => String::new(),
        };
        let v = match name {
            "OpenText" => {
                let text_name = match a.first() {
                    Some(Name(n)) => self.reg.names.str(*n).to_string(),
                    Some(Str(t)) => t.clone(),
                    _ => String::new(),
                };
                let mut package = s(a, 1);
                if package.is_empty() {
                    package = "DeusExText".into();
                }
                let text = self.ext_string_text(&format!("{package}.{text_name}"));
                Bool(self.text_parsers.entry(this).or_default().open(text.as_deref()))
            }
            "SetPlayerName" => {
                let n = s(a, 0);
                self.text_parsers.entry(this).or_default().set_player_name(&n);
                Void
            }
            "GetName" => {
                let n = self.text_parsers.get(&this).and_then(|p| p.name().map(str::to_string));
                Name(n.map(|n| self.reg.names.intern(&n)).unwrap_or(0))
            }
            "GetColor" => {
                let c = self.text_parsers.get(&this).map(TextParser::color).unwrap_or([0; 4]);
                self.color_value_pub(c)
            }
            _ => {
                let p = self.text_parsers.entry(this).or_default();
                match name {
                    "CloseText" => {
                        p.close();
                        Void
                    }
                    "ProcessText" => Bool(p.process()),
                    "IsEOF" => Bool(p.is_eof()),
                    "GetText" => Str(p.last_text.clone()),
                    // the original's label jump does nothing
                    "GotoLabel" => Void,
                    "GetTag" => Byte(p.last_tag),
                    "GetEmailInfo" => {
                        let e = p.email.clone();
                        for (i, f) in [e.name, e.subject, e.from, e.to, e.cc].into_iter().enumerate() {
                            if i < a.len() {
                                a[i] = Str(f);
                            }
                        }
                        Void
                    }
                    "GetFileInfo" => {
                        let (n, d) = (p.file_name.clone(), p.file_description.clone());
                        if a.len() >= 2 {
                            a[0] = Str(n);
                            a[1] = Str(d);
                        }
                        Void
                    }
                    _ => return None,
                }
            }
        };
        Some(Ok(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(text: &str) -> Vec<(u8, String)> {
        let mut p = TextParser::default();
        p.set_player_name("JC Denton");
        assert!(p.open(Some(text)));
        let mut v = Vec::new();
        while p.process() {
            v.push((p.last_tag, p.last_text.clone()));
        }
        v
    }

    #[test]
    fn text_starts_at_the_first_paragraph_and_tags_split_it() {
        let v = blocks("<COMMENT>hidden</COMMENT>\r\nskipped\r\n<P>Hello <B>there</B>,\r\n<PLAYERFIRSTNAME>.<P>Next");
        // the comment comes out as its end tag; text before the first <P> is dropped; the first <P> is not reported
        assert_eq!(
            v,
            vec![
                (TT_END_COMMENT, String::new()),
                (TT_TEXT, "Hello ".into()),
                (19, String::new()),
                (TT_TEXT, "there".into()),
                (20, String::new()),
                (TT_TEXT, ",  ".into()),
                (10, "JC".into()),
                (TT_TEXT, ".".into()),
                (TT_NEW_PARAGRAPH, String::new()),
                (TT_TEXT, "Next".into()),
            ]
        );
    }

    #[test]
    fn email_and_file_tags_fill_their_fields() {
        let mut p = TextParser::default();
        p.open(Some("<EMAIL=01_Email06, Whupass ,Warlord,AJacobson,>\r\n<FILE=01_Bulletin01,Terrorism>"));
        p.process();
        assert_eq!(p.last_tag, 2);
        assert_eq!(
            p.email,
            EmailInfo { name: "01_Email06".into(), subject: "Whupass".into(), from: "Warlord".into(), to: "AJacobson".into(), cc: String::new() }
        );
        p.process();
        assert_eq!((p.last_tag, p.file_name.as_str(), p.file_description.as_str()), (1, "01_Bulletin01", "Terrorism"));
        assert!(!p.process());
    }

    #[test]
    fn colours_names_escapes_and_unknown_tags() {
        let mut p = TextParser::default();
        p.open(Some("<P><C=255,128,7>x</C><G=Image01></<><WHAT>"));
        p.process();
        assert_eq!((p.last_tag, p.color()), (16, [255, 128, 7, 0]));
        p.process();
        p.process();
        assert_eq!((p.last_tag, p.color()), (TT_END_COLOR, [0; 4]));
        p.process();
        assert_eq!((p.last_tag, p.name()), (TT_GRAPHIC, Some("Image01")));
        p.process();
        assert_eq!((p.last_tag, p.last_text.as_str()), (28, "<"));
        // an unknown tag after the paragraph has started reports no tag
        p.process();
        assert_eq!(p.last_tag, TT_NONE);
        assert!(!p.process());
        assert!(p.is_eof());
    }

    #[test]
    fn a_missing_text_leaves_the_parser_closed() {
        let mut p = TextParser::default();
        assert!(!p.open(None));
        assert!(!p.process());
        assert_eq!(p.last_tag, TT_NONE);
    }

    #[test]
    fn atoi_reads_like_c() {
        assert_eq!((atoi(" 12x"), atoi("-3"), atoi("abc"), atoi("")), (12, -3, 0, 0));
    }
}

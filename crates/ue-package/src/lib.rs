//! Reader for Unreal Engine 1 package files (`.u`, `.dx`, `.utx`, `.uax`, `.umx`), versions 61-68.
//!
//! This crate only understands the container: header, name table, import table and export table.
//! Object payloads are exposed as raw byte slices.

use std::fmt;
use std::path::Path;

pub mod bytecode;
pub mod object;
pub mod mods;

pub const PACKAGE_TAG: u32 = 0x9E2A_83C1;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    BadTag(u32),
    UnsupportedVersion(u16),
    Truncated { what: &'static str, at: usize },
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io error: {e}"),
            Error::BadTag(t) => write!(f, "bad package tag 0x{t:08X}"),
            Error::UnsupportedVersion(v) => write!(f, "unsupported package version {v}"),
            Error::Truncated { what, at } => write!(f, "truncated while reading {what} at offset {at}"),
            Error::Invalid(s) => write!(f, "invalid package: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Little-endian cursor over a byte slice with the primitive encodings used by UE1 packages.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Reader { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        match end {
            Some(end) => {
                let s = &self.data[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => Err(Error::Truncated { what, at: self.pos }),
        }
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1, "u8")?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2, "u16")?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4, "u32")?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8, "u64")?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.take(n, "bytes")
    }

    /// Variable-length signed integer ("compact index"): the first byte carries the sign (bit 7), a continuation
    /// flag (bit 6) and 6 value bits; each following byte carries a continuation flag (bit 7) and 7 value bits.
    pub fn compact_index(&mut self) -> Result<i32> {
        let b0 = self.u8()?;
        let neg = b0 & 0x80 != 0;
        let mut v = (b0 & 0x3F) as i32;
        if b0 & 0x40 != 0 {
            let mut shift = 6;
            loop {
                let b = self.u8()?;
                v |= ((b & 0x7F) as i32) << shift;
                shift += 7;
                if b & 0x80 == 0 || shift > 34 {
                    break;
                }
            }
        }
        Ok(if neg { -v } else { v })
    }

    /// Zero-terminated 8-bit string without a length prefix (name entries before package version 64).
    pub fn cstring(&mut self) -> Result<String> {
        let rest = self.data.get(self.pos..).ok_or(Error::Truncated { what: "cstring", at: self.pos })?;
        let n = rest.iter().position(|&c| c == 0).ok_or(Error::Truncated { what: "cstring", at: self.pos })?;
        let s = rest[..n].iter().map(|&c| c as char).collect();
        self.pos += n + 1;
        Ok(s)
    }

    /// String: compact-index length (including the terminator), then that many 8-bit characters.
    /// A negative length means UTF-16 code units. Returned without the terminator.
    pub fn string(&mut self) -> Result<String> {
        let len = self.compact_index()?;
        if len == 0 {
            return Ok(String::new());
        }
        if len > 0 {
            let b = self.take(len as usize, "string")?;
            let b = b.strip_suffix(&[0]).unwrap_or(b);
            // Latin-1 decoding is lossless for the ANSI text found in these packages.
            Ok(b.iter().map(|&c| c as char).collect())
        } else {
            let n = (-len) as usize;
            let b = self.take(n * 2, "wide string")?;
            let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let units = units.strip_suffix(&[0]).unwrap_or(&units);
            Ok(String::from_utf16_lossy(units))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid(pub [u8; 16]);

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02X}"))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Generation {
    pub export_count: u32,
    pub name_count: u32,
}

#[derive(Debug, Clone)]
pub struct NameEntry {
    pub name: String,
    pub flags: u32,
}

#[derive(Debug, Clone)]
pub struct Import {
    pub class_package: i32,
    pub class_name: i32,
    /// Object reference to the owning import (negative) or 0 for a top-level package.
    pub outer: i32,
    pub object_name: i32,
}

#[derive(Debug, Clone)]
pub struct Export {
    /// Object reference to the class (negative = import, positive = export, 0 = the `Class` class).
    pub class: i32,
    pub super_ref: i32,
    pub outer: i32,
    pub object_name: i32,
    pub object_flags: u32,
    pub serial_size: u32,
    pub serial_offset: u32,
}

#[derive(Debug)]
pub struct Package {
    pub version: u16,
    pub licensee: u16,
    pub flags: u32,
    /// Present for version 68 and later.
    pub guid: Option<Guid>,
    pub generations: Vec<Generation>,
    /// Present before version 68: list of ancestor GUIDs.
    pub heritage: Vec<Guid>,
    pub names: Vec<NameEntry>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
    data: Vec<u8>,
}

/// A decoded object reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectRef {
    None,
    Import(usize),
    Export(usize),
}

impl ObjectRef {
    pub fn from_index(i: i32) -> ObjectRef {
        match i {
            0 => ObjectRef::None,
            i if i < 0 => ObjectRef::Import((-i - 1) as usize),
            i => ObjectRef::Export((i - 1) as usize),
        }
    }
}

impl Package {
    pub fn open(path: impl AsRef<Path>) -> Result<Package> {
        Package::parse(std::fs::read(path)?)
    }

    /// Opens a package from the disk or from a mod's zip file.
    pub fn open_source(source: &mods::Source) -> Result<Package> {
        Package::parse(source.read()?)
    }

    pub fn parse(data: Vec<u8>) -> Result<Package> {
        let mut r = Reader::new(&data, 0);
        let tag = r.u32()?;
        if tag != PACKAGE_TAG {
            return Err(Error::BadTag(tag));
        }
        let version = r.u16()?;
        let licensee = r.u16()?;
        if !(61..=68).contains(&version) {
            return Err(Error::UnsupportedVersion(version));
        }
        let flags = r.u32()?;
        let name_count = r.u32()? as usize;
        let name_offset = r.u32()? as usize;
        let export_count = r.u32()? as usize;
        let export_offset = r.u32()? as usize;
        let import_count = r.u32()? as usize;
        let import_offset = r.u32()? as usize;

        let mut guid = None;
        let mut generations = Vec::new();
        let mut heritage = Vec::new();
        if version >= 68 {
            guid = Some(Guid(r.bytes(16)?.try_into().unwrap()));
            let n = r.u32()? as usize;
            for _ in 0..n.min(1024) {
                generations.push(Generation { export_count: r.u32()?, name_count: r.u32()? });
            }
        } else {
            let count = r.u32()? as usize;
            let off = r.u32()? as usize;
            let mut hr = Reader::new(&data, off);
            for _ in 0..count.min(1024) {
                heritage.push(Guid(hr.bytes(16)?.try_into().unwrap()));
            }
        }

        let check = |what: &str, count: usize, off: usize| -> Result<()> {
            if count > 0 && off >= data.len() {
                return Err(Error::Invalid(format!("{what} table offset {off} beyond end of file")));
            }
            if count > 4_000_000 {
                return Err(Error::Invalid(format!("{what} count {count} implausible")));
            }
            Ok(())
        };
        check("name", name_count, name_offset)?;
        check("import", import_count, import_offset)?;
        check("export", export_count, export_offset)?;

        let mut r = Reader::new(&data, name_offset);
        let mut names = Vec::with_capacity(name_count);
        for _ in 0..name_count {
            let name = if version < 64 { r.cstring()? } else { r.string()? };
            let flags = r.u32()?;
            names.push(NameEntry { name, flags });
        }

        let mut r = Reader::new(&data, import_offset);
        let mut imports = Vec::with_capacity(import_count);
        for _ in 0..import_count {
            imports.push(Import {
                class_package: r.compact_index()?,
                class_name: r.compact_index()?,
                outer: r.i32()?,
                object_name: r.compact_index()?,
            });
        }

        let mut r = Reader::new(&data, export_offset);
        let mut exports = Vec::with_capacity(export_count);
        for _ in 0..export_count {
            let class = r.compact_index()?;
            let super_ref = r.compact_index()?;
            let outer = r.i32()?;
            let object_name = r.compact_index()?;
            let object_flags = r.u32()?;
            let serial_size = r.compact_index()? as u32;
            let serial_offset = if serial_size > 0 { r.compact_index()? as u32 } else { 0 };
            exports.push(Export { class, super_ref, outer, object_name, object_flags, serial_size, serial_offset });
        }

        Ok(Package { version, licensee, flags, guid, generations, heritage, names, imports, exports, data })
    }

    pub fn name(&self, index: i32) -> &str {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.names.get(i))
            .map(|n| n.name.as_str())
            .unwrap_or("<bad name>")
    }

    /// Raw serialized bytes of an export, or an error if its range lies outside the file.
    pub fn export_bytes(&self, index: usize) -> Result<&[u8]> {
        let e = self.exports.get(index).ok_or_else(|| Error::Invalid(format!("no export {index}")))?;
        let start = e.serial_offset as usize;
        let end = start.checked_add(e.serial_size as usize).filter(|&x| x <= self.data.len());
        match end {
            Some(end) => Ok(&self.data[start..end]),
            None => Err(Error::Invalid(format!("export {index} data range out of file"))),
        }
    }

    pub fn export_class_name(&self, index: usize) -> String {
        match ObjectRef::from_index(self.exports[index].class) {
            ObjectRef::None => "Class".to_string(),
            ObjectRef::Import(i) => self.imports.get(i).map(|im| self.name(im.object_name).to_string()).unwrap_or_default(),
            ObjectRef::Export(i) => self.exports.get(i).map(|ex| self.name(ex.object_name).to_string()).unwrap_or_default(),
        }
    }

    /// Dotted path (`Package.Group.Name`) of an object reference, following outer links.
    /// Returns `None` if the chain is broken or cyclic.
    pub fn object_path(&self, r: ObjectRef) -> Option<String> {
        let mut parts = Vec::new();
        let mut cur = r;
        for _ in 0..64 {
            match cur {
                ObjectRef::None => {
                    parts.reverse();
                    return Some(parts.join("."));
                }
                ObjectRef::Import(i) => {
                    let im = self.imports.get(i)?;
                    parts.push(self.name(im.object_name).to_string());
                    cur = ObjectRef::from_index(im.outer);
                }
                ObjectRef::Export(i) => {
                    let ex = self.exports.get(i)?;
                    parts.push(self.name(ex.object_name).to_string());
                    cur = ObjectRef::from_index(ex.outer);
                }
            }
        }
        None
    }

    pub fn file_len(&self) -> usize {
        self.data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_index_values() {
        let cases: &[(&[u8], i32)] = &[
            (&[0x00], 0),
            (&[0x05], 5),
            (&[0x85], -5),
            (&[0x3F], 63),
            (&[0x40 | 0x01, 0x01], 1 | (1 << 6)),       // 65: continuation, 6 bits + 7 bits
            (&[0xC0 | 0x02, 0x80 | 0x03, 0x04], -(2 | (3 << 6) | (4 << 13))),
        ];
        for (bytes, want) in cases {
            let mut r = Reader::new(bytes, 0);
            assert_eq!(r.compact_index().unwrap(), *want, "{bytes:02X?}");
            assert_eq!(r.pos(), bytes.len());
        }
    }

    #[test]
    fn strings() {
        // ANSI: length 4 including terminator
        let mut r = Reader::new(&[0x04, b'a', b'b', b'c', 0], 0);
        assert_eq!(r.string().unwrap(), "abc");
        // UTF-16: length -3 (two characters plus terminator)
        let mut r = Reader::new(&[0x83, b'h', 0, 0xE9, 0, 0, 0], 0);
        assert_eq!(r.string().unwrap(), "h\u{e9}");
        // empty
        assert_eq!(Reader::new(&[0x00], 0).string().unwrap(), "");
        // pre-v64 zero-terminated
        assert_eq!(Reader::new(b"None\0rest", 0).cstring().unwrap(), "None");
    }

    #[test]
    fn rejects_bad_tag() {
        assert!(matches!(Package::parse(vec![0; 64]), Err(Error::BadTag(0))));
    }

    #[test]
    fn object_ref_decoding() {
        assert_eq!(ObjectRef::from_index(0), ObjectRef::None);
        assert_eq!(ObjectRef::from_index(-1), ObjectRef::Import(0));
        assert_eq!(ObjectRef::from_index(1), ObjectRef::Export(0));
    }
}

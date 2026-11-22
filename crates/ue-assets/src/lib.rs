//! Loaders for asset objects whose data is not covered by the tagged property stream.

pub mod audio;
pub mod files;
pub mod font;
pub mod level;
pub mod light;
pub mod mesh;
pub mod mesh_mod;
pub mod model;
pub mod procedural;
pub mod sort;
pub mod text;
pub mod texture;

use ue_package::object::{read_tags, PropType, Tag};
use ue_package::{Package, Reader, Result};

/// RF_HasStack: object data starts with a script-frame header.
const RF_HAS_STACK: u32 = 0x0200_0000;

/// An object's property tags plus the position where its native payload starts.
pub(crate) struct Payload<'a> {
    pub data: &'a [u8],
    pub tags: Vec<Tag>,
    pub native_start: usize,
}

pub(crate) fn payload(pkg: &Package, export: usize) -> Result<Payload<'_>> {
    let data = pkg.export_bytes(export)?;
    let mut r = Reader::new(data, 0);
    if pkg.exports[export].object_flags & RF_HAS_STACK != 0 {
        // script-frame header (node, state node, probe mask, latent action, and a code offset if there is a node)
        let node = r.compact_index()?;
        r.compact_index()?;
        r.u64()?;
        r.u32()?;
        if node != 0 {
            r.compact_index()?;
        }
    }
    let tags = read_tags(&mut r, pkg)?;
    Ok(Payload { data, tags, native_start: r.pos() })
}

/// One tagged property of an object, decoded for tools: name, type, array index, raw value bytes (bool in `bool_value`).
pub struct PropertyValue {
    pub name: String,
    pub ptype: PropType,
    pub array_index: u32,
    pub bool_value: bool,
    pub data: Vec<u8>,
}

/// The tagged properties of an export (tools and probes; loaders use the typed accessors).
pub fn property_values(pkg: &Package, export: usize) -> Result<Vec<PropertyValue>> {
    let p = payload(pkg, export)?;
    Ok(p.tags
        .iter()
        .map(|t| PropertyValue { name: pkg.name(t.name).to_string(), ptype: t.ptype, array_index: t.array_index, bool_value: t.bool_value, data: p.data[t.data.clone()].to_vec() })
        .collect())
}

impl Payload<'_> {
    fn tag(&self, pkg: &Package, name: &str) -> Option<&Tag> {
        self.tags.iter().find(|t| pkg.name(t.name) == name)
    }

    pub fn bool(&self, pkg: &Package, name: &str) -> bool {
        self.tag(pkg, name).map(|t| t.ptype == PropType::Bool && t.bool_value).unwrap_or(false)
    }

    pub fn byte(&self, pkg: &Package, name: &str) -> Option<u8> {
        self.tag(pkg, name).filter(|t| t.ptype == PropType::Byte).map(|t| self.data[t.data.start])
    }

    pub fn int(&self, pkg: &Package, name: &str) -> Option<i32> {
        let t = self.tag(pkg, name).filter(|t| t.ptype == PropType::Int)?;
        Some(i32::from_le_bytes(self.data[t.data.clone()].try_into().ok()?))
    }

    pub fn float(&self, pkg: &Package, name: &str) -> Option<f32> {
        let t = self.tag(pkg, name).filter(|t| t.ptype == PropType::Float)?;
        Some(f32::from_le_bytes(self.data[t.data.clone()].try_into().ok()?))
    }

    /// Element `index` of a static array property (each element is its own tag).
    pub fn element(&self, pkg: &Package, name: &str, index: u32) -> Option<&Tag> {
        self.tags.iter().find(|t| t.array_index == index && pkg.name(t.name) == name)
    }

    /// Raw object reference stored in an object property (compact index).
    pub fn object(&self, pkg: &Package, name: &str) -> Option<i32> {
        let t = self.tag(pkg, name).filter(|t| t.ptype == PropType::Object)?;
        Reader::new(self.data, t.data.start).compact_index().ok()
    }
}

//! Decoding of object payloads: the tagged property stream shared by all objects, and the objects that
//! describe script types (classes, structs, functions, states, enums, constants, properties, text buffers).

use crate::{Error, Guid, Package, Reader, Result};

/// `RF_HasStack`: object data starts with a script-frame header (used by saved games).
const RF_HAS_STACK: u32 = 0x0200_0000;
/// `FUNC_Net`: a replication offset follows the flags.
const FUNC_NET: u32 = 0x40;
/// `CPF_Net`: a replication offset follows the category name.
const CPF_NET: u32 = 0x20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropType {
    Byte,
    Int,
    Bool,
    Float,
    Object,
    Name,
    String,
    Class,
    Array,
    Struct,
    Vector,
    Rotator,
    Str,
    Map,
    FixedArray,
}

impl PropType {
    fn from_id(id: u8) -> Option<PropType> {
        use PropType::*;
        Some(match id {
            1 => Byte,
            2 => Int,
            3 => Bool,
            4 => Float,
            5 => Object,
            6 => Name,
            7 => String,
            8 => Class,
            9 => Array,
            10 => Struct,
            11 => Vector,
            12 => Rotator,
            13 => Str,
            14 => Map,
            15 => FixedArray,
            _ => return None,
        })
    }
}

/// One entry of a tagged property stream. `data` is the byte range of the value inside the object data.
#[derive(Debug, Clone)]
pub struct Tag {
    pub name: i32,
    pub ptype: PropType,
    /// Struct type name (name index) for struct properties.
    pub struct_name: Option<i32>,
    pub size: u32,
    pub array_index: u32,
    /// Value of a bool property (stored in the tag itself).
    pub bool_value: bool,
    pub data: std::ops::Range<usize>,
}

/// Header shared by all objects: optional script frame, then (for non-class objects) tagged properties.
#[derive(Debug, Clone, Default)]
pub struct StackHeader {
    pub node: i32,
    pub state_node: i32,
    pub probe_mask: u64,
    pub latent_action: u32,
    pub offset: i32,
}

/// Reads one tagged property stream starting at the reader's position, up to and including the `None` terminator.
/// `none_name` is the name-table index of "None" (0 in every shipped package, but looked up to be safe).
pub fn read_tags(r: &mut Reader, pkg: &Package) -> Result<Vec<Tag>> {
    let none = pkg.names.iter().position(|n| n.name == "None").unwrap_or(0) as i32;
    let mut tags = Vec::new();
    loop {
        let name = r.compact_index()?;
        if name == none {
            return Ok(tags);
        }
        if name < 0 || name as usize >= pkg.names.len() {
            return Err(Error::Invalid(format!("property tag name index {name} out of range at {}", r.pos())));
        }
        let info = r.u8()?;
        let ptype = PropType::from_id(info & 0x0F)
            .ok_or_else(|| Error::Invalid(format!("unknown property type {} at {}", info & 0x0F, r.pos())))?;
        let is_array = info & 0x80 != 0;
        let struct_name = if ptype == PropType::Struct { Some(r.compact_index()?) } else { None };
        let size = match (info >> 4) & 7 {
            0 => 1,
            1 => 2,
            2 => 4,
            3 => 12,
            4 => 16,
            5 => r.u8()? as u32,
            6 => r.u16()? as u32,
            _ => r.u32()?,
        };
        let mut array_index = 0;
        if is_array && ptype != PropType::Bool {
            let b0 = r.u8()? as u32;
            array_index = if b0 < 0x80 {
                b0
            } else if b0 < 0xC0 {
                ((b0 & 0x7F) << 8) | r.u8()? as u32
            } else {
                let b1 = r.u8()? as u32;
                let b2 = r.u8()? as u32;
                let b3 = r.u8()? as u32;
                ((b0 & 0x3F) << 24) | (b1 << 16) | (b2 << 8) | b3
            };
        }
        let bool_value = ptype == PropType::Bool && is_array;
        let start = r.pos();
        let data_len = if ptype == PropType::Bool { 0 } else { size as usize };
        r.bytes(data_len)?;
        tags.push(Tag { name, ptype, struct_name, size, array_index, bool_value, data: start..start + data_len });
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FieldHeader {
    pub super_ref: i32,
    pub next: i32,
}

/// Common part of classes, structs, functions and states.
#[derive(Debug, Clone)]
pub struct StructBody {
    pub script_text: i32,
    pub children: i32,
    pub friendly_name: i32,
    pub line: i32,
    pub text_pos: i32,
    /// Range of the serialized bytecode inside the object data.
    pub script: std::ops::Range<usize>,
    /// Declared size of the code in memory (object/name operands are wider in memory than when serialized).
    pub script_mem_size: usize,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub class: i32,
    pub deep: u32,
    pub script_text_crc: u32,
}

#[derive(Debug, Clone)]
pub struct ClassObject {
    pub field: FieldHeader,
    pub body: StructBody,
    /// Classes derive from states, so they carry the state fields too.
    pub probe_mask: u64,
    pub ignore_mask: u64,
    pub label_table_offset: u16,
    pub state_flags: u32,
    pub class_flags: u32,
    pub guid: Guid,
    pub dependencies: Vec<Dependency>,
    pub package_imports: Vec<i32>,
    pub within: i32,
    pub config_name: i32,
    /// Default property values of the class.
    pub defaults: Vec<Tag>,
}

#[derive(Debug, Clone)]
pub struct StructObject {
    pub field: FieldHeader,
    pub body: StructBody,
}

#[derive(Debug, Clone)]
pub struct FunctionObject {
    pub field: FieldHeader,
    pub body: StructBody,
    pub native_index: u16,
    pub oper_precedence: u8,
    pub flags: u32,
    pub rep_offset: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct StateObject {
    pub field: FieldHeader,
    pub body: StructBody,
    pub probe_mask: u64,
    pub ignore_mask: u64,
    pub label_table_offset: u16,
    pub state_flags: u32,
}

#[derive(Debug, Clone)]
pub struct EnumObject {
    pub field: FieldHeader,
    pub names: Vec<i32>,
}

#[derive(Debug, Clone)]
pub struct ConstObject {
    pub field: FieldHeader,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyKind {
    Byte,
    Int,
    Bool,
    Float,
    Name,
    Str,
    String,
    Object,
    Class,
    Struct,
    Array,
    FixedArray,
    Map,
}

#[derive(Debug, Clone)]
pub struct PropertyObject {
    pub field: FieldHeader,
    pub kind: PropertyKind,
    pub array_dim: u32,
    pub flags: u32,
    pub category: i32,
    pub rep_offset: Option<u16>,
    /// Enum (byte), object class (object/class), struct (struct), inner property (array/fixed array), key (map).
    pub ref_a: i32,
    /// Metaclass (class property), value property (map).
    pub ref_b: i32,
    /// Element count of fixed arrays.
    pub count: i32,
}

#[derive(Debug, Clone)]
pub struct TextBufferObject {
    pub pos: u32,
    pub top: u32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum Object {
    Class(ClassObject),
    Struct(StructObject),
    Function(FunctionObject),
    State(StateObject),
    Enum(EnumObject),
    Const(ConstObject),
    Property(PropertyObject),
    TextBuffer(TextBufferObject),
    /// Any other object: its tagged properties (native payload, if any, follows at `native_start`).
    Plain { tags: Vec<Tag>, native_start: usize },
}

fn read_stack_header(r: &mut Reader) -> Result<StackHeader> {
    let node = r.compact_index()?;
    let state_node = r.compact_index()?;
    let probe_mask = r.u64()?;
    let latent_action = r.u32()?;
    let offset = if node != 0 { r.compact_index()? } else { 0 };
    Ok(StackHeader { node, state_node, probe_mask, latent_action, offset })
}

fn read_struct_body(r: &mut Reader, pkg: &Package) -> Result<StructBody> {
    let script_text = r.compact_index()?;
    let children = r.compact_index()?;
    let friendly_name = r.compact_index()?;
    let line = r.i32()?;
    let text_pos = r.i32()?;
    let size = r.i32()?;
    if size < 0 {
        return Err(Error::Invalid(format!("negative script size {size}")));
    }
    let none = pkg.names.iter().position(|n| n.name == "None").unwrap_or(0) as i32;
    let scan = crate::bytecode::scan(r, size as usize, none)?;
    Ok(StructBody { script_text, children, friendly_name, line, text_pos, script: scan.file, script_mem_size: size as usize })
}

fn property_kind(class_name: &str) -> Option<PropertyKind> {
    use PropertyKind::*;
    Some(match class_name {
        "ByteProperty" => Byte,
        "IntProperty" => Int,
        "BoolProperty" => Bool,
        "FloatProperty" => Float,
        "NameProperty" => Name,
        "StrProperty" => Str,
        "StringProperty" => String,
        "ObjectProperty" => Object,
        "ClassProperty" => Class,
        "StructProperty" => Struct,
        "ArrayProperty" => Array,
        "FixedArrayProperty" => FixedArray,
        "MapProperty" => Map,
        _ => return None,
    })
}

impl Package {
    /// The script-frame header of an export that has one (actors and other objects with a state): the state being run
    /// (`state_node`, an object reference) and the position in its code.
    pub fn stack_header(&self, export: usize) -> Option<StackHeader> {
        if self.exports[export].object_flags & RF_HAS_STACK == 0 {
            return None;
        }
        let data = self.export_bytes(export).ok()?;
        read_stack_header(&mut Reader::new(data, 0)).ok()
    }

    /// Decodes an export's payload. The whole payload must be consumed, otherwise an error is returned.
    pub fn load_object(&self, export: usize) -> Result<Object> {
        let data = self.export_bytes(export)?;
        let class_name = self.export_class_name(export);
        let flags = self.exports[export].object_flags;
        let mut r = Reader::new(data, 0);
        let is_class = class_name == "Class";
        if flags & RF_HAS_STACK != 0 {
            read_stack_header(&mut r)?;
        }
        let tags = if is_class { Vec::new() } else { read_tags(&mut r, self)? };
        let field = |r: &mut Reader| -> Result<FieldHeader> {
            Ok(FieldHeader { super_ref: r.compact_index()?, next: r.compact_index()? })
        };
        let obj = match class_name.as_str() {
            "Class" => {
                let field = field(&mut r)?;
                let body = read_struct_body(&mut r, self)?;
                let probe_mask = r.u64()?;
                let ignore_mask = r.u64()?;
                let label_table_offset = r.u16()?;
                let state_flags = r.u32()?;
                let class_flags = r.u32()?;
                let guid = Guid(r.bytes(16)?.try_into().unwrap());
                let n = r.compact_index()?;
                let mut dependencies = Vec::new();
                for _ in 0..n.max(0) {
                    dependencies.push(Dependency { class: r.compact_index()?, deep: r.u32()?, script_text_crc: r.u32()? });
                }
                let n = r.compact_index()?;
                let mut package_imports = Vec::new();
                for _ in 0..n.max(0) {
                    package_imports.push(r.compact_index()?);
                }
                let within = r.compact_index()?;
                let config_name = r.compact_index()?;
                let defaults = read_tags(&mut r, self)?;
                Object::Class(ClassObject { field, body, probe_mask, ignore_mask, label_table_offset, state_flags, class_flags, guid, dependencies, package_imports, within, config_name, defaults })
            }
            "Struct" => {
                let field = field(&mut r)?;
                Object::Struct(StructObject { field, body: read_struct_body(&mut r, self)? })
            }
            "Function" => {
                let field = field(&mut r)?;
                let body = read_struct_body(&mut r, self)?;
                let native_index = r.u16()?;
                let oper_precedence = r.u8()?;
                let fl = r.u32()?;
                let rep_offset = if fl & FUNC_NET != 0 { Some(r.u16()?) } else { None };
                Object::Function(FunctionObject { field, body, native_index, oper_precedence, flags: fl, rep_offset })
            }
            "State" => {
                let field = field(&mut r)?;
                let body = read_struct_body(&mut r, self)?;
                Object::State(StateObject {
                    field,
                    body,
                    probe_mask: r.u64()?,
                    ignore_mask: r.u64()?,
                    label_table_offset: r.u16()?,
                    state_flags: r.u32()?,
                })
            }
            "Enum" => {
                let field = field(&mut r)?;
                let n = r.compact_index()?;
                let mut names = Vec::new();
                for _ in 0..n.max(0) {
                    names.push(r.compact_index()?);
                }
                Object::Enum(EnumObject { field, names })
            }
            "Const" => {
                let field = field(&mut r)?;
                Object::Const(ConstObject { field, value: r.string()? })
            }
            "TextBuffer" => {
                let pos = r.u32()?;
                let top = r.u32()?;
                Object::TextBuffer(TextBufferObject { pos, top, text: r.string()? })
            }
            other => {
                if let Some(kind) = property_kind(other) {
                    let field = field(&mut r)?;
                    let array_dim = r.u32()?;
                    let pflags = r.u32()?;
                    let category = r.compact_index()?;
                    let rep_offset = if pflags & CPF_NET != 0 { Some(r.u16()?) } else { None };
                    let (mut ref_a, mut ref_b, mut count) = (0, 0, 0);
                    match kind {
                        PropertyKind::Byte | PropertyKind::Object | PropertyKind::Struct | PropertyKind::Array => ref_a = r.compact_index()?,
                        PropertyKind::Class => {
                            ref_a = r.compact_index()?;
                            ref_b = r.compact_index()?;
                        }
                        PropertyKind::FixedArray => {
                            ref_a = r.compact_index()?;
                            count = r.i32()?;
                        }
                        PropertyKind::Map => {
                            ref_a = r.compact_index()?;
                            ref_b = r.compact_index()?;
                        }
                        _ => {}
                    }
                    Object::Property(PropertyObject { field, kind, array_dim, flags: pflags, category, rep_offset, ref_a, ref_b, count })
                } else {
                    return Ok(Object::Plain { tags, native_start: r.pos() });
                }
            }
        };
        if r.pos() != data.len() {
            return Err(Error::Invalid(format!(
                "export {export} ({class_name}): decoded {} of {} bytes",
                r.pos(),
                data.len()
            )));
        }
        Ok(obj)
    }

    /// Exports reachable through a struct's child list, in list order (grouped by member kind).
    pub fn children_of(&self, first: i32) -> Result<Vec<usize>> {
        let mut out = Vec::new();
        let mut cur = crate::ObjectRef::from_index(first);
        while let crate::ObjectRef::Export(i) = cur {
            if out.len() > self.exports.len() {
                return Err(Error::Invalid("child list cycle".into()));
            }
            out.push(i);
            cur = crate::ObjectRef::from_index(match self.load_object(i)? {
                Object::Property(p) => p.field.next,
                Object::Function(f) => f.field.next,
                Object::State(s) => s.field.next,
                Object::Struct(s) => s.field.next,
                Object::Enum(e) => e.field.next,
                Object::Const(c) => c.field.next,
                other => return Err(Error::Invalid(format!("unexpected {other:?} in child list"))),
            });
        }
        Ok(out)
    }
}

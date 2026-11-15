//! Decoding of default property values into an instance's memory image.

use crate::*;
use ue_package::object::{PropType, PropertyKind, Tag};
use ue_package::Reader;

#[derive(Debug)]
pub struct DefaultError {
    pub class: String,
    pub detail: String,
}

impl std::fmt::Display for DefaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class, self.detail)
    }
}

impl std::error::Error for DefaultError {}

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

impl Registry {
    /// Creates an instance of `class` with all default values applied, ancestors first.
    pub fn instantiate(&mut self, class: ClassId) -> std::result::Result<Instance, DefaultError> {
        let size = self.class(class).size as usize;
        let mut inst = Instance { class, data: vec![0; size], strings: Vec::new() };
        for c in self.chain(class) {
            let (pi, ei) = self.class(c).export;
            let tags = self.class(c).default_tags.clone();
            let data = self.packages[pi].pkg.export_bytes(ei).map_err(|e| self.err(c, e.to_string()))?.to_vec();
            for tag in &tags {
                self.apply_tag(&mut inst, c, pi, &data, tag).map_err(|d| self.err(c, d))?;
            }
            self.apply_config_and_text(&mut inst, c);
        }
        Ok(inst)
    }

    /// Applies one class's own default block (its `defaultproperties`, as tags) to `inst` (`UClass::Serialize`
    /// after `InitClassDefaultObject` copied the super class's defaults).
    pub fn apply_class_tags(&mut self, inst: &mut Instance, c: ClassId) -> std::result::Result<(), DefaultError> {
        let (pi, ei) = self.class(c).export;
        let tags = self.class(c).default_tags.clone();
        let data = self.packages[pi].pkg.export_bytes(ei).map_err(|e| self.err(c, e.to_string()))?.to_vec();
        for tag in &tags {
            self.apply_tag(inst, c, pi, &data, tag).map_err(|d| self.err(c, d))?;
        }
        Ok(())
    }

    /// Applies the saved object `export` of package `pi`'s own property tags to `inst` (which holds the class's
    /// defaults). Tags that cannot be applied are returned as warnings.
    pub fn apply_object_tags(&mut self, inst: &mut Instance, class: ClassId, pi: usize, export: usize) -> std::result::Result<Vec<String>, DefaultError> {
        let obj = self.packages[pi].pkg.load_object(export).map_err(|e| self.err(class, e.to_string()))?;
        let ue_package::object::Object::Plain { tags, .. } = obj else {
            return Err(self.err(class, "object is not a plain tagged object".into()));
        };
        let data = self.packages[pi].pkg.export_bytes(export).map_err(|e| self.err(class, e.to_string()))?.to_vec();
        let mut warnings = Vec::new();
        for tag in &tags {
            if let Err(d) = self.apply_tag(inst, class, pi, &data, tag) {
                warnings.push(d);
            }
        }
        Ok(warnings)
    }

    /// Creates an instance of `class` from the saved object `export` of package `pi` (an actor in a map): class
    /// defaults first, then the object's own property tags. Tags that cannot be applied are reported in the second
    /// result instead of failing the whole object. Object references come out as interned path ids; the caller maps
    /// the ones that designate other saved objects.
    pub fn instantiate_object(&mut self, class: ClassId, pi: usize, export: usize) -> std::result::Result<(Instance, Vec<String>), DefaultError> {
        let mut inst = self.instantiate(class)?;
        let obj = self.packages[pi].pkg.load_object(export).map_err(|e| self.err(class, e.to_string()))?;
        let ue_package::object::Object::Plain { tags, .. } = obj else {
            return Err(self.err(class, "object is not a plain tagged object".into()));
        };
        let data = self.packages[pi].pkg.export_bytes(export).map_err(|e| self.err(class, e.to_string()))?.to_vec();
        let mut warnings = Vec::new();
        for tag in &tags {
            if let Err(d) = self.apply_tag(&mut inst, class, pi, &data, tag) {
                warnings.push(d);
            }
        }
        Ok((inst, warnings))
    }

    fn err(&self, c: ClassId, detail: String) -> DefaultError {
        DefaultError { class: self.class(c).path.clone(), detail }
    }

    /// `declaring` is the class whose default block holds the tag: names bind from there upwards, so a subclass that
    /// redeclares a variable does not capture the parent's default for the parent's own variable.
    fn apply_tag(&mut self, inst: &mut Instance, declaring: ClassId, pi: usize, data: &[u8], tag: &Tag) -> std::result::Result<(), String> {
        let name_id = self.packages[pi].name_map[tag.name as usize];
        let prop = self
            .chain(declaring)
            .into_iter()
            .rev()
            .find_map(|c| self.class(c).props.iter().find(|p| p.name == name_id).cloned())
            .ok_or_else(|| format!("unknown property {}", self.names.str(name_id)))?;
        let pname = self.names.str(name_id).to_string();
        let expected = match prop.kind {
            PropertyKind::Byte => PropType::Byte,
            PropertyKind::Int => PropType::Int,
            PropertyKind::Bool => PropType::Bool,
            PropertyKind::Float => PropType::Float,
            PropertyKind::Object => PropType::Object,
            PropertyKind::Name => PropType::Name,
            PropertyKind::Class => PropType::Class,
            PropertyKind::Struct => PropType::Struct,
            PropertyKind::Str => PropType::Str,
            PropertyKind::String => PropType::String,
            PropertyKind::Array => PropType::Array,
            _ => return Err(format!("{pname}: unsupported kind")),
        };
        // Class-reference properties are tagged with the object type id.
        let class_as_object = prop.kind == PropertyKind::Class && tag.ptype == PropType::Object;
        if tag.ptype != expected && !class_as_object {
            return Err(format!("{pname}: tag type {:?} does not match property kind {:?}", tag.ptype, prop.kind));
        }
        if tag.array_index >= prop.dim {
            return Err(format!("{pname}: array index {} out of range {}", tag.array_index, prop.dim));
        }
        if prop.kind == PropertyKind::Bool {
            set_bit(inst, prop.offset, prop.bit_mask, tag.bool_value);
            return Ok(());
        }
        let off = prop.offset + tag.array_index * prop.elem_size;
        let mut r = Reader::new(&data[..tag.data.end], tag.data.start);
        self.read_value(inst, pi, &prop, &mut r, off).map_err(|e| format!("{pname}: {e}"))?;
        if r.pos() != tag.data.end {
            return Err(format!("{pname}: value consumed {} of {} bytes", r.pos() - tag.data.start, tag.size));
        }
        Ok(())
    }

    /// Reads one element of `prop` into `inst` at `off`. Used for tag values and for struct members (binary encoding).
    fn read_value(
        &mut self,
        inst: &mut Instance,
        pi: usize,
        prop: &PropDef,
        r: &mut Reader,
        off: u32,
    ) -> std::result::Result<(), String> {
        let e = |x: ue_package::Error| x.to_string();
        match prop.kind {
            PropertyKind::Byte => {
                let v = r.u8().map_err(e)?;
                put(inst, off, &[v]);
            }
            PropertyKind::Int | PropertyKind::Float => {
                let v = r.u32().map_err(e)?;
                put(inst, off, &v.to_le_bytes());
            }
            PropertyKind::Name => {
                let local = r.compact_index().map_err(e)?;
                let g = *self.packages[pi].name_map.get(local as usize).ok_or("name index out of range")?;
                put(inst, off, &g.to_le_bytes());
            }
            PropertyKind::Object | PropertyKind::Class => {
                let rf = r.compact_index().map_err(e)?;
                let id = match self.ref_path(pi, rf) {
                    Some(p) => self.obj_id(&p).0,
                    None => 0,
                };
                put(inst, off, &id.to_le_bytes());
            }
            PropertyKind::Str | PropertyKind::String => {
                let s = r.string().map_err(e)?;
                inst.strings.push(s);
                put(inst, off, &(inst.strings.len() as u32).to_le_bytes());
            }
            PropertyKind::Bool => {
                // Struct member: a single byte; the bit lives in the member's shared word.
                let v = r.u8().map_err(e)?;
                set_bit(inst, off, prop.bit_mask, v != 0);
            }
            PropertyKind::Struct => {
                let Target::Struct(sid) = prop.target else { return Err("struct without definition".into()) };
                self.read_struct(inst, pi, sid, r, off)?;
            }
            other => return Err(format!("cannot decode {other:?} values")),
        }
        Ok(())
    }

    /// Binary struct encoding: members in declaration order (superstruct first), each element in turn.
    fn read_struct(
        &mut self,
        inst: &mut Instance,
        pi: usize,
        sid: StructId,
        r: &mut Reader,
        base: u32,
    ) -> std::result::Result<(), String> {
        let mut chain = vec![sid];
        while let Some(s) = self.struct_def(*chain.last().unwrap()).super_id {
            chain.push(s);
        }
        chain.reverse();
        for s in chain {
            let props = self.struct_def(s).props.clone();
            for m in &props {
                for i in 0..m.dim {
                    if m.kind == PropertyKind::Bool {
                        // Bool members share a word starting at the member's own offset.
                        let v = r.u8().map_err(|x| x.to_string())?;
                        set_bit(inst, base + m.offset, m.bit_mask, v != 0);
                    } else {
                        self.read_value(inst, pi, m, r, base + m.offset + i * m.elem_size)?;
                    }
                }
            }
        }
        Ok(())
    }
}

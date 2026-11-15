//! Memory layout of classes, structs and function frames (x86-32, MSVC 6 conventions as used by the original).

use crate::*;
use ue_package::object::{Object, PropertyKind, PropertyObject};

fn align(x: u32, a: u32) -> u32 {
    x.div_ceil(a) * a
}

impl Registry {
    /// Size and alignment of one element of a property.
    fn kind_size(&self, kind: PropertyKind, target: &Target) -> Result<(u32, u32)> {
        use PropertyKind::*;
        Ok(match kind {
            Byte => (1, 1),
            Int | Float | Name | Object | Class | Bool => (4, 4),
            Str | String | Array => (12, 4),
            Struct => match target {
                Target::Struct(id) => {
                    let s = self.struct_def(*id);
                    (s.size, s.align)
                }
                _ => return Err(Error::Invalid("struct property without struct".into())),
            },
            FixedArray | Map => return Err(Error::Invalid(format!("unsupported property kind {kind:?}"))),
        })
    }

    fn prop_target(&mut self, pi: usize, p: &PropertyObject) -> Result<Target> {
        Ok(match p.kind {
            PropertyKind::Byte => match self.ref_path(pi, p.ref_a) {
                Some(path) => Target::Enum(path),
                None => Target::None,
            },
            PropertyKind::Object => {
                let path = self.ref_path(pi, p.ref_a);
                Target::Object(path.map(|p| self.obj_id(&p)).unwrap_or(ObjId(0)))
            }
            PropertyKind::Class => {
                let c = self.ref_path(pi, p.ref_a).map(|p| self.obj_id(&p)).unwrap_or(ObjId(0));
                let m = self.ref_path(pi, p.ref_b).map(|p| self.obj_id(&p)).unwrap_or(ObjId(0));
                Target::Class { class: c, meta: m }
            }
            PropertyKind::Struct => {
                let path = self
                    .ref_path(pi, p.ref_a)
                    .ok_or_else(|| Error::Invalid("struct property without struct ref".into()))?;
                let (pj, ej) = *self
                    .exports_by_path
                    .get(&path.to_ascii_lowercase())
                    .ok_or_else(|| Error::Invalid(format!("struct {path} not found in loaded packages")))?;
                Target::Struct(self.struct_for(pj, ej)?)
            }
            PropertyKind::Array => {
                // Inner property is an export of the same package.
                let inner = match ObjectRef::from_index(p.ref_a) {
                    ObjectRef::Export(i) => i,
                    _ => return Err(Error::Invalid("array inner property is not an export".into())),
                };
                let Object::Property(ip) = self.packages[pi].pkg.load_object(inner).map_err(Error::from)? else {
                    return Err(Error::Invalid("array inner is not a property".into()));
                };
                let name = self.packages[pi].name_map[self.packages[pi].pkg.exports[inner].object_name as usize];
                let t = self.prop_target(pi, &ip)?;
                let (sz, _) = self.kind_size(ip.kind, &t)?;
                Target::Inner(Box::new(PropDef {
                    name,
                    kind: ip.kind,
                    dim: 1,
                    flags: ip.flags,
                    offset: 0,
                    elem_size: sz,
                    bit_mask: 0,
                    target: t,
                    public: true,
                    category: 0,
                }))
            }
            _ => Target::None,
        })
    }

    /// Lays out `objs` sequentially starting at `base`. Returns the definitions, the end offset and the largest alignment.
    pub(crate) fn layout_members(
        &mut self,
        pi: usize,
        objs: &[PropertyObject],
        names: &[u32],
        exports: &[usize],
        base: u32,
        min_align: u32,
    ) -> Result<(Vec<PropDef>, u32, u32)> {
        let mut top = base;
        let mut maxal = min_align;
        let mut prev_bool: Option<(u32, u32)> = None;
        let mut out = Vec::with_capacity(objs.len());
        for ((p, &name), &ex) in objs.iter().zip(names).zip(exports) {
            const RF_PUBLIC: u32 = 0x4;
            let public = self.packages[pi].pkg.exports[ex].object_flags & RF_PUBLIC != 0;
            let category = self.packages[pi].name_map.get(p.category.max(0) as usize).copied().unwrap_or(0);
            let category = if self.names.str(category).eq_ignore_ascii_case("None") { 0 } else { category };
            let target = self.prop_target(pi, p)?;
            let (sz, al) = self.kind_size(p.kind, &target)?;
            let dim = (p.array_dim & 0xFFFF).max(1);
            let (offset, mask);
            if p.kind == PropertyKind::Bool && dim == 1 && matches!(prev_bool, Some((_, m)) if m != 0x8000_0000) {
                let (o, m) = prev_bool.unwrap();
                offset = o;
                mask = m << 1;
            } else {
                offset = align(top, al);
                top = offset + sz * dim;
                mask = 1;
            }
            prev_bool = if p.kind == PropertyKind::Bool && dim == 1 { Some((offset, mask)) } else { None };
            maxal = maxal.max(al);
            out.push(PropDef {
                name,
                kind: p.kind,
                dim,
                flags: p.flags,
                offset,
                elem_size: sz,
                bit_mask: if p.kind == PropertyKind::Bool { mask } else { 0 },
                target,
                public,
                category,
            });
        }
        Ok((out, top, maxal))
    }

    pub(crate) fn struct_for(&mut self, pi: usize, ei: usize) -> Result<StructId> {
        let path = self.ref_path(pi, (ei + 1) as i32).unwrap_or_default();
        if let Some(&id) = self.struct_by_path.get(&path.to_ascii_lowercase()) {
            return Ok(id);
        }
        let Object::Struct(so) = self.packages[pi].pkg.load_object(ei).map_err(Error::from)? else {
            return Err(Error::Invalid(format!("{path} is not a struct")));
        };
        let (mut base, mut align_) = (0, 1);
        let mut super_id = None;
        if let Some(sp) = self.ref_path(pi, so.field.super_ref) {
            if let Some(&(pj, ej)) = self.exports_by_path.get(&sp.to_ascii_lowercase()) {
                let sid = self.struct_for(pj, ej)?;
                base = self.struct_def(sid).size;
                align_ = self.struct_def(sid).align;
                super_id = Some(sid);
            }
        }
        let kids = self.packages[pi].pkg.children_of(so.body.children).map_err(Error::from)?;
        let mut objs = Vec::new();
        let mut names = Vec::new();
        let mut exs = Vec::new();
        for k in kids {
            if let Object::Property(p) = self.packages[pi].pkg.load_object(k).map_err(Error::from)? {
                names.push(self.packages[pi].name_map[self.packages[pi].pkg.exports[k].object_name as usize]);
                objs.push(p);
                exs.push(k);
            }
        }
        let (props, top, maxal) = self.layout_members(pi, &objs, &names, &exs, base, align_)?;
        let name = self.packages[pi].name_map[self.packages[pi].pkg.exports[ei].object_name as usize];
        let id = StructId(self.structs.len() as u32);
        for (i, &k) in exs.iter().enumerate() {
            self.prop_exports.insert((pi, k), PropLoc { owner: Owner::Struct(id), index: i, def: props[i].clone() });
        }
        self.structs.push(StructDef { path: path.clone(), name, super_id, props, size: align(top, maxal), align: maxal });
        self.struct_by_path.insert(path.to_ascii_lowercase(), id);
        Ok(id)
    }
}

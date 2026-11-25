//! Interpreter: evaluates compiled expression trees against object and frame memory.

use crate::ir::*;
use crate::{inst_ref, natives, ref_instance, Mem, Value, Vm, VmError};
use ue_package::object::PropertyKind;
use ue_registry::{ClassId, FuncLoc, PropDef, Target};

type Res<T> = Result<T, VmError>;

const CPF_PARM: u32 = 0x80;
const CPF_OUT: u32 = 0x100;
const CPF_RETURN: u32 = 0x400;
const FUNC_NATIVE: u32 = 0x400;
const MAX_DEPTH: usize = 128;
const MAX_STEPS: u64 = 5_000_000;

/// A function's call data (`Vm::call_plan`): its definition's fields, the parameter slots (property, frame offset) in
/// argument order, the return property and the name natives are dispatched by.
pub(crate) struct CallPlan {
    name: u32,
    flags: u32,
    export: (usize, usize),
    mem_size: u32,
    frame_size: u32,
    slots: Vec<(PropDef, usize)>,
    /// Per declared parameter (not the return value): its flags and element count, in order.
    params: Vec<(u32, u32)>,
    /// A native short-circuit operator: 1 `&&`, 2 `||` (the second operand is evaluated only when needed).
    short: u8,
    ret: Option<PropDef>,
    native_name: std::rc::Rc<str>,
    /// The compiled body, once first called (the code cache never changes an entry).
    code: std::cell::OnceCell<std::rc::Rc<crate::ir::Code>>,
    /// An Aerugo repair of the original's script to run before the body (`Vm::port_repair`; 0 none).
    repair: u8,
}

pub struct Frame {
    pub mem: Mem,
    /// Object the code runs on (`Self`); 0 when there is none.
    pub this: u32,
    /// While running inside `obj.expr` this is the object that wrote the whole expression: operands such as call
    /// arguments and array indices are evaluated against it, only the member access targets `this`.
    caller: Option<u32>,
    ret: Value,
}

impl Frame {
    pub(crate) fn new(mem: Mem, this: u32) -> Frame {
        Frame { mem, this, caller: None, ret: Value::Void }
    }
}

/// How a run of statements ended.
pub(crate) enum RunEnd {
    Done,
    /// State code stopped at a latent call; continue at this statement.
    Suspended(usize),
    /// State code was replaced by `GotoState` while running.
    Aborted,
}

struct IterState<'c> {
    /// One entry per visited item: the values of the iterator's out parameters, in declaration order (a walking
    /// iterator holds only its current item).
    items: Vec<Vec<Value>>,
    /// A walking iterator over the level's actor list: the next item is found when the loop comes round.
    walk: Option<crate::iterators::ActorWalk>,
    /// The out-parameter values when the loop runs out (`Void` = unchanged).
    end: Vec<Value>,
    idx: usize,
    outs: Vec<Option<&'c Expr>>,
    body_start: usize,
    end_idx: usize,
}

#[derive(Clone, Copy)]
enum MemId {
    Frame,
    Obj(usize),
    Def(ClassId),
    /// The header members of a package object (`Vm::headers`).
    Header(u32),
}

struct Loc<'c> {
    mem: MemId,
    off: u32,
    def: &'c PropDef,
}

fn rt<T: Into<String>>(s: T) -> VmError {
    VmError::Runtime(s.into())
}

fn rd4(m: &Mem, off: usize) -> Res<[u8; 4]> {
    m.data.get(off..off + 4).and_then(|s| s.try_into().ok()).ok_or_else(|| rt("memory read out of range"))
}

fn put(m: &mut Mem, off: usize, b: &[u8]) -> Res<()> {
    m.data.get_mut(off..off + b.len()).ok_or_else(|| rt("memory write out of range"))?.copy_from_slice(b);
    Ok(())
}

impl Vm {
    fn mem_ref<'a>(&'a self, f: &'a Frame, id: MemId) -> Res<&'a Mem> {
        match id {
            MemId::Frame => Ok(&f.mem),
            MemId::Obj(i) => self.objects.get(i).map(|o| &o.mem).ok_or_else(|| rt("bad object")),
            MemId::Def(c) => self.defaults.get(&c).ok_or_else(|| rt("defaults not loaded")),
            MemId::Header(r) => self.headers.get(&r).ok_or_else(|| rt("no header")),
        }
    }

    fn mem_mut<'a>(&'a mut self, f: &'a mut Frame, id: MemId) -> Res<&'a mut Mem> {
        match id {
            MemId::Frame => Ok(&mut f.mem),
            MemId::Obj(i) => self.objects.get_mut(i).map(|o| &mut o.mem).ok_or_else(|| rt("bad object")),
            MemId::Def(c) => self.defaults.get_mut(&c).ok_or_else(|| rt("defaults not loaded")),
            MemId::Header(r) => self.headers.get_mut(&r).ok_or_else(|| rt("no header")),
        }
    }

    /// Class of an object reference: live instances and class-path references.
    pub fn class_of_ref(&self, r: u32) -> Option<ClassId> {
        if r == 0 {
            return None;
        }
        match ref_instance(r) {
            // a destroyed actor waiting for `CleanupDestroyed` still answers calls (`execContext` only rejects null)
            Some(i) => self.objects.get(i).filter(|o| o.alive || self.pending_deleted_set.contains(&r)).map(|o| o.class),
            None => self.reg.class_by_path(self.reg.obj_path(ue_registry::ObjId(r))),
        }
    }

    /// The class of the object a reference designates: live instances, and resources of loaded packages (a font, a texture, a
    /// sound: their class comes from the export), unlike `class_of_ref`, which resolves class references.
    pub fn class_of_object_ref(&self, r: u32) -> Option<ClassId> {
        if r == 0 {
            return None;
        }
        if ref_instance(r).is_some() {
            return self.class_of_ref(r);
        }
        let path = self.reg.obj_path(ue_registry::ObjId(r));
        if let Some((pi, ei)) = self.reg.export_by_path(path) {
            if let Some(c) = self.reg.class_of_export(pi, ei) {
                return Some(c);
            }
        }
        self.class_of_ref(r)
    }

    /// The class path (`Engine.Font`) recorded for a resource of a loaded package.
    pub fn resource_class_path(&self, r: u32) -> Option<String> {
        if ref_instance(r).is_some() {
            return None;
        }
        let (pi, ei) = self.reg.export_by_path(self.reg.obj_path(ue_registry::ObjId(r)))?;
        self.reg.ref_path(pi, self.reg.packages[pi].pkg.exports.get(ei)?.class)
    }

    /// False for none and for destroyed objects.
    pub fn is_live(&self, r: u32) -> bool {
        r != 0 && ref_instance(r).map(|i| self.objects.get(i).map(|o| o.alive).unwrap_or(false)).unwrap_or(true)
    }

    pub fn is_a(&self, class: ClassId, ancestor: ClassId) -> bool {
        let (c, a) = (class.0 as usize, ancestor.0 as usize);
        let mut anc = self.ancestry.borrow_mut();
        if anc.len() <= c {
            anc.resize(c + 1, Vec::new());
        }
        if anc[c].is_empty() {
            // the class and everything above it, as bits indexed by class id
            let mut bits = vec![0u64; c / 64 + 1];
            let mut cur = Some(class);
            while let Some(k) = cur {
                let i = k.0 as usize;
                if bits.len() <= i / 64 {
                    bits.resize(i / 64 + 1, 0);
                }
                bits[i / 64] |= 1 << (i % 64);
                cur = self.reg.class(k).super_id;
            }
            anc[c] = bits;
        }
        anc[c].get(a / 64).map(|w| (w >> (a % 64)) & 1 == 1).unwrap_or(false)
    }

    // ---- memory <-> values -------------------------------------------------------------------------------

    pub(crate) fn read_from(&self, m: &Mem, def: &PropDef, off: usize) -> Res<Value> {
        Ok(match def.kind {
            PropertyKind::Byte => Value::Byte(*m.data.get(off).ok_or_else(|| rt("memory read out of range"))?),
            PropertyKind::Int => Value::Int(i32::from_le_bytes(rd4(m, off)?)),
            PropertyKind::Float => Value::Float(f32::from_le_bytes(rd4(m, off)?)),
            PropertyKind::Name => Value::Name(u32::from_le_bytes(rd4(m, off)?)),
            PropertyKind::Object | PropertyKind::Class => Value::Obj(u32::from_le_bytes(rd4(m, off)?)),
            PropertyKind::Bool => Value::Bool(u32::from_le_bytes(rd4(m, off)?) & def.bit_mask != 0),
            PropertyKind::Str | PropertyKind::String => {
                let h = u32::from_le_bytes(rd4(m, off)?) as usize;
                Value::Str(if h == 0 { String::new() } else { m.strings.get(h - 1).cloned().unwrap_or_default() })
            }
            PropertyKind::Struct => {
                let Target::Struct(sid) = def.target else { return Err(rt("struct property without type")) };
                if Some(sid) == self.vector {
                    let f = |i: usize| rd4(m, off + 4 * i).map(f32::from_le_bytes);
                    Value::Vector([f(0)?, f(1)?, f(2)?])
                } else if Some(sid) == self.rotator {
                    let f = |i: usize| rd4(m, off + 4 * i).map(i32::from_le_bytes);
                    Value::Rotator([f(0)?, f(1)?, f(2)?])
                } else {
                    let size = self.reg.struct_def(sid).size as usize;
                    let b = m.data.get(off..off + size).ok_or_else(|| rt("memory read out of range"))?;
                    Value::Struct(sid, b.to_vec())
                }
            }
            other => return Err(VmError::Unsupported(format!("reading {other:?} properties"))),
        })
    }

    pub(crate) fn write_to(m: &mut Mem, def: &PropDef, off: usize, v: Value) -> Res<()> {
        if v == Value::Void {
            // Accessed-none or omitted value: store zero of the right width.
            let zeros = vec![0u8; if def.kind == PropertyKind::Struct { 0 } else { def.elem_size as usize }];
            if def.kind == PropertyKind::Bool {
                let mut w = u32::from_le_bytes(rd4(m, off)?);
                w &= !def.bit_mask;
                return put(m, off, &w.to_le_bytes());
            }
            if def.kind == PropertyKind::Struct {
                return Ok(());
            }
            return put(m, off, &zeros);
        }
        match (def.kind, v) {
            (PropertyKind::Byte, v) => put(m, off, &[v.as_i32().ok_or_else(|| rt("expected byte"))? as u8]),
            (PropertyKind::Int, v) => put(m, off, &v.as_i32().ok_or_else(|| rt("expected int"))?.to_le_bytes()),
            (PropertyKind::Float, v) => put(m, off, &v.as_f32().ok_or_else(|| rt("expected float"))?.to_le_bytes()),
            (PropertyKind::Name, Value::Name(n)) => put(m, off, &n.to_le_bytes()),
            (PropertyKind::Object | PropertyKind::Class, Value::Obj(o)) => put(m, off, &o.to_le_bytes()),
            (PropertyKind::Bool, v) => {
                let b = v.as_bool().ok_or_else(|| rt("expected bool"))?;
                let mut w = u32::from_le_bytes(rd4(m, off)?);
                if b {
                    w |= def.bit_mask;
                } else {
                    w &= !def.bit_mask;
                }
                put(m, off, &w.to_le_bytes())
            }
            (PropertyKind::Str | PropertyKind::String, Value::Str(s)) => {
                let h = u32::from_le_bytes(rd4(m, off)?) as usize;
                if h != 0 && h <= m.strings.len() {
                    m.strings[h - 1] = s;
                } else {
                    m.strings.push(s);
                    let nh = m.strings.len() as u32;
                    put(m, off, &nh.to_le_bytes())?;
                }
                Ok(())
            }
            (PropertyKind::Struct, Value::Vector(v)) => {
                for (i, x) in v.iter().enumerate() {
                    put(m, off + 4 * i, &x.to_le_bytes())?;
                }
                Ok(())
            }
            (PropertyKind::Struct, Value::Rotator(v)) => {
                for (i, x) in v.iter().enumerate() {
                    put(m, off + 4 * i, &x.to_le_bytes())?;
                }
                Ok(())
            }
            (PropertyKind::Struct, Value::Struct(_, b)) => put(m, off, &b),
            (k, v) => Err(rt(format!("cannot store {v:?} into a {k:?} property"))),
        }
    }

    fn read_loc(&self, f: &Frame, l: &Loc) -> Res<Value> {
        self.read_from(self.mem_ref(f, l.mem)?, l.def, l.off as usize)
    }

    fn write_loc(&mut self, f: &mut Frame, l: &Loc, v: Value) -> Res<()> {
        let m = self.mem_mut(f, l.mem)?;
        Self::write_to(m, l.def, l.off as usize, v)
    }

    pub(crate) fn read_prop(&self, m: &Mem, def: &PropDef, off: usize) -> Res<Value> {
        self.read_from(m, def, off)
    }

    pub(crate) fn write_prop(&mut self, obj: usize, def: &PropDef, off: usize, v: Value) -> Res<()> {
        Self::write_to(&mut self.objects[obj].mem, def, off, v)
    }

    /// The zero of the type of the variable at `p` (what a read gives when the place is inside a null object).
    fn zero_of_place(&self, p: &Place) -> Res<Value> {
        let def = match p {
            Place::Local(d) | Place::Instance(d) | Place::Default(d) | Place::Member(_, d) => d,
            Place::Elem(base, _) | Place::Ctx(_, base) => return self.zero_of_place(base),
        };
        let size = match (def.kind, &def.target) {
            (PropertyKind::Struct, Target::Struct(sid)) => self.reg.struct_def(*sid).size as usize,
            _ => def.elem_size as usize,
        };
        self.read_from(&Mem { data: vec![0; size.max(4)], strings: Vec::new() }, def, 0)
    }

    // ---- places --------------------------------------------------------------------------------------------

    /// Runs `body` with `Self` set to `o` (a context expression's target); operands go back to the writer's object.
    fn in_context<T>(&mut self, f: &mut Frame, o: u32, body: impl FnOnce(&mut Self, &mut Frame) -> T) -> T {
        let (saved, saved_caller) = (f.this, f.caller);
        if f.caller.is_none() {
            f.caller = Some(saved);
        }
        f.this = o;
        let r = body(self, f);
        f.this = saved;
        f.caller = saved_caller;
        r
    }

    /// Runs `body` as an operand of a (possibly context-qualified) expression: against the writer's object.
    fn as_operand<T>(&mut self, f: &mut Frame, body: impl FnOnce(&mut Self, &mut Frame) -> T) -> T {
        match f.caller.take() {
            Some(c) => {
                let this = std::mem::replace(&mut f.this, c);
                let r = body(self, f);
                f.this = this;
                f.caller = Some(c);
                r
            }
            None => body(self, f),
        }
    }

    /// Resolves a place; `None` means the place is inside a null object (reads give void, writes are dropped).
    fn place<'c>(&mut self, f: &mut Frame, p: &'c Place) -> Res<Option<Loc<'c>>> {
        Ok(match p {
            Place::Local(d) => Some(Loc { mem: MemId::Frame, off: d.offset, def: d }),
            Place::Instance(d) => match ref_instance(f.this) {
                Some(i) => Some(Loc { mem: MemId::Obj(i), off: d.offset, def: d }),
                // a package object (a class, a texture...): only Object's header members can be read
                None if f.this != 0 && self.package_header(f.this, d.offset) => Some(Loc { mem: MemId::Header(f.this), off: d.offset, def: d }),
                None => {
                    self.log.push("Accessed None (instance variable without an object)".into());
                    None
                }
            },
            Place::Default(d) => {
                let class = self.class_of_ref(f.this).ok_or_else(|| rt("default variable without a class"))?;
                self.class_default_mem(class)?;
                Some(Loc { mem: MemId::Def(class), off: d.offset, def: d })
            }
            Place::Elem(base, idx) => {
                let i = self.as_operand(f, |s, f| s.eval(f, idx))?.as_i32().ok_or_else(|| rt("array index is not an int"))?;
                let Some(l) = self.place(f, base)? else { return Ok(None) };
                // an index out of range is warned about and clamped into the array (probes `vm.array.*`)
                let dim = l.def.dim.max(1) as i32;
                let i = if i < 0 || i >= dim {
                    self.log.push(format!("Accessed array out of bounds ({i}/{dim})"));
                    i.clamp(0, dim - 1)
                } else {
                    i
                };
                Some(Loc { mem: l.mem, off: l.off + i as u32 * l.def.elem_size, def: l.def })
            }
            Place::Member(base, m) => {
                let Some(l) = self.place(f, base)? else { return Ok(None) };
                Some(Loc { mem: l.mem, off: l.off + m.offset, def: m })
            }
            Place::Ctx(obj, inner) => {
                let o = match self.eval(f, obj)? {
                    Value::Obj(o) => o,
                    _ => 0,
                };
                if !self.is_live(o) && !self.is_pending_deleted(o) {
                    let cls = self.accessed_none_place(f.this);
                    if trace_enabled() { eprintln!("!! Accessed None in {cls}"); }
                    self.log.push(format!("Accessed None in {cls}"));
                    return Ok(None);
                }
                self.in_context(f, o, |s, f| s.place(f, inner))?
            }
        })
    }

    fn place_of_expr<'c>(&mut self, f: &mut Frame, e: &'c Expr) -> Res<Option<Loc<'c>>> {
        match e {
            Expr::Load(p) => self.place(f, p),
            Expr::Context { obj, inner } => {
                let Expr::Load(p) = &**inner else { return Err(rt("out argument is not a variable")) };
                let o = match self.eval(f, obj)? {
                    Value::Obj(o) => o,
                    _ => 0,
                };
                if !self.is_live(o) && !self.is_pending_deleted(o) {
                    return Ok(None);
                }
                self.in_context(f, o, |s, f| s.place(f, p))
            }
            _ => Err(rt("out argument is not a variable")),
        }
    }

    // ---- expressions ---------------------------------------------------------------------------------------

    pub(crate) fn eval<'c>(&mut self, f: &mut Frame, e: &'c Expr) -> Res<Value> {
        Ok(match e {
            Expr::Nothing => Value::Void,
            Expr::Int(i) => Value::Int(*i),
            Expr::Float(x) => Value::Float(*x),
            Expr::Str(s) => Value::Str(s.clone()),
            Expr::Name(n) => Value::Name(*n),
            Expr::Byte(b) => Value::Byte(*b),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Vector(v) => Value::Vector(*v),
            Expr::Rotator(v) => Value::Rotator(*v),
            Expr::NoObject => Value::Obj(0),
            Expr::SelfObj => Value::Obj(f.this),
            Expr::ObjConst(id) => Value::Obj(id.0),
            Expr::Load(p) => match self.place(f, p)? {
                Some(l) => self.read_loc(f, &l)?,
                None => self.zero_of_place(p)?,
            },
            Expr::Let(p, src) => {
                let v = self.eval(f, src)?;
                if let Some(l) = self.place(f, p)? {
                    self.write_loc(f, &l, v.clone())?;
                }
                v
            }
            Expr::Cast(op, x) => {
                let v = self.eval(f, x)?;
                natives::cast(self, *op, v)?
            }
            // 10130fa0: the value if it is a class derived from the target, else None
            Expr::MetaCast(id, x) => {
                let v = self.eval(f, x)?;
                let target = self.reg.class_by_path(self.reg.obj_path(*id));
                match (&v, target) {
                    (Value::Obj(r), Some(t)) if *r != 0 && ref_instance(*r).is_none() => match self.reg.class_by_path(self.reg.obj_path(ue_registry::ObjId(*r))) {
                        Some(c) if self.is_a(c, t) => v,
                        _ => Value::Obj(0),
                    },
                    (Value::Obj(_), _) => Value::Obj(0),
                    _ => v,
                }
            }
            Expr::DynCast(id, x) => {
                let v = self.eval(f, x)?;
                match v {
                    Value::Obj(r) if r != 0 => {
                        let target = self.reg.class_by_path(self.reg.obj_path(*id));
                        match (self.class_of_object_ref(r), target) {
                            (Some(c), Some(t)) if self.is_a(c, t) => Value::Obj(r),
                            // a native-only class without a class definition (`Font`): a resource of exactly that class passes
                            (_, None) if self.resource_class_path(r).map(|p| p.eq_ignore_ascii_case(self.reg.obj_path(*id))).unwrap_or(false) => Value::Obj(r),
                            _ => Value::Obj(0),
                        }
                    }
                    other => other,
                }
            }
            Expr::Skip(x) => self.eval(f, x)?,
            Expr::Context { obj, inner } => {
                let o = match self.eval(f, obj)? {
                    Value::Obj(o) => o,
                    _ => 0,
                };
                if !self.is_live(o) && !self.is_pending_deleted(o) {
                    let cls = self.accessed_none_place(f.this);
                    if trace_enabled() { eprintln!("!! Accessed None in {cls}"); }
                    self.log.push(format!("Accessed None in {cls}"));
                    // `UObject::execContext` (Core 10102964) skips the expression and zeroes the result (its size is
                    // stored after the skip): a variable read through None is its type's zero (a name reads None)
                    return match &**inner {
                        Expr::Load(p) => self.zero_of_place(p),
                        _ => Ok(Value::Void),
                    };
                }
                self.in_context(f, o, |s, f| s.eval(f, inner))?
            }
            Expr::Call(kind, args, site) => {
                // the site's last resolution, when the receiver's class and state are the same
                // (the receiver's class as `class_of_ref` gives it: none for a dead object; its state as
                // `resolve_function` reads it)
                let (class, state) = match ref_instance(f.this).and_then(|i| self.objects.get(i)) {
                    Some(o) => ((o.alive || self.pending_deleted_set.contains(&f.this)).then_some(o.class), o.state),
                    None => (self.class_of_ref(f.this), None),
                };
                let hit = site.0.borrow().as_ref().filter(|h| Some(h.class) == class && (h.state == state || !matches!(kind, Call::Virtual(_)))).map(|h| (h.loc, h.plan.clone()));
                if let Some((loc, plan)) = hit {
                    return self.invoke_planned(f, loc, &plan, args);
                }
                let loc = match kind {
                    Call::Final(l) => *l,
                    Call::Virtual(n) => self.resolve_function(f.this, *n).ok_or_else(|| {
                        rt(format!("function {} not found on {}", self.reg.names.str(*n), self.obj_to_string(f.this)))
                    })?,
                    Call::Global(n) => {
                        let class = self.class_of_ref(f.this).ok_or_else(|| rt("function call without an object"))?;
                        self.reg.find_function(class, *n).ok_or_else(|| {
                            rt(format!("function {} not found on {}", self.reg.names.str(*n), self.reg.class(class).path))
                        })?
                    }
                };
                let plan = self.call_plan(loc);
                if let Some(class) = class {
                    *site.0.borrow_mut() = Some(crate::ir::SiteHit { class, state, loc, plan: plan.clone() });
                }
                self.invoke_planned(f, loc, &plan, args)?
            }
            Expr::Native(idx, args, site) => {
                let hit = site.0.borrow().as_ref().map(|h| (h.loc, h.plan.clone()));
                let (loc, plan) = match hit {
                    Some(h) => h,
                    None => {
                        let locs = self.reg.natives_by_index(*idx);
                        let loc = *locs.first().ok_or_else(|| VmError::Unsupported(format!("native function index {idx}")))?;
                        let plan = self.call_plan(loc);
                        let class = loc.class;
                        *site.0.borrow_mut() = Some(crate::ir::SiteHit { class, state: None, loc, plan: plan.clone() });
                        (loc, plan)
                    }
                };
                self.invoke_planned(f, loc, &plan, args)?
            }
            Expr::Op(op, args) => self.eval_op(f, *op, args)?,
            Expr::StructCmp { eq, a, b, .. } => {
                let (x, y) = (self.eval(f, a)?, self.eval(f, b)?);
                Value::Bool((x == y) == *eq)
            }
            Expr::MemberOf(x, def) => {
                let v = self.eval(f, x)?;
                let bytes = match v {
                    Value::Vector(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
                    Value::Rotator(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
                    Value::Struct(_, b) => b,
                    Value::Void => return Ok(Value::Void),
                    _ => return Err(rt("member access on a non-struct value")),
                };
                self.read_from(&Mem { data: bytes, strings: Vec::new() }, def, def.offset as usize)?
            }
            Expr::New(args) => {
                // `execNew`: the outer defaults to the running object (an omitted argument leaves it), then the name,
                // the flags (only warned about) and the class
                let mut outer = if ref_instance(f.this).is_some() { f.this } else { 0 };
                let mut name = 0;
                let mut class_ref = 0;
                for (i, a) in args.iter().enumerate() {
                    if matches!(a, Expr::Nothing) {
                        continue;
                    }
                    let v = self.eval(f, a)?;
                    match (i, v) {
                        (0, Value::Obj(o)) => outer = o,
                        (1, Value::Name(n)) => name = n,
                        (1, Value::Str(s)) => name = if s.is_empty() { 0 } else { self.reg.names.intern(&s) },
                        (3, Value::Obj(c)) => class_ref = c,
                        _ => {}
                    }
                }
                let Some(class) = self.class_of_ref(class_ref) else {
                    self.log.push("new: no class".into());
                    return Ok(Value::Obj(0));
                };
                Value::Obj(self.new_script_object(class, outer, name)?)
            }
        })
    }

    /// A native operator in place (`Expr::Op`): its operands as `invoke` evaluates them (against the writer's object;
    /// `&&`/`||` evaluate the second only when needed, as the short-circuit call does), then the native's arithmetic.
    fn eval_op(&mut self, f: &mut Frame, op: Op, args: &[Expr]) -> Res<Value> {
        if matches!(op, Op::AndB | Op::OrB) {
            let a = self.eval(f, &args[0])?.as_bool().unwrap_or(false);
            return Ok(Value::Bool(if op == Op::AndB {
                a && self.eval(f, &args[1])?.as_bool().unwrap_or(false)
            } else {
                a || self.eval(f, &args[1])?.as_bool().unwrap_or(false)
            }));
        }
        let a = self.as_operand(f, |s, f| s.eval(f, &args[0]))?;
        let b = match args.get(1) {
            Some(x) => self.as_operand(f, |s, f| s.eval(f, x))?,
            None => Value::Void,
        };
        natives::op(op, a, b)
    }

    /// Evaluates arguments (resolving out parameters to places), calls, and copies out parameters back.
    #[allow(dead_code)]
    fn invoke<'c>(&mut self, f: &mut Frame, loc: FuncLoc, args: &'c [Expr]) -> Res<Value> {
        let plan = self.call_plan(loc);
        self.invoke_planned(f, loc, &plan, args)
    }

    /// `invoke` with the callee's plan in hand.
    fn invoke_planned<'c>(&mut self, f: &mut Frame, loc: FuncLoc, plan: &CallPlan, args: &'c [Expr]) -> Res<Value> {
        let flags = plan.flags;
        // Short-circuit operators evaluate their second operand only when needed.
        if plan.short != 0 && args.len() == 2 {
            let a = self.eval(f, &args[0])?.as_bool().unwrap_or(false);
            return Ok(Value::Bool(if plan.short == 1 {
                a && self.eval(f, &args[1])?.as_bool().unwrap_or(false)
            } else {
                a || self.eval(f, &args[1])?.as_bool().unwrap_or(false)
            }));
        }
        let this = f.this;
        let mut vals: Vec<Value> = self.arg_pool.pop().unwrap_or_default();
        vals.clear();
        // (value index, element count, place) of each out parameter
        let mut outs: Vec<(usize, u32, Option<Loc<'c>>)> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let (is_out, dim) = plan.params.get(i).map(|&(fl, d)| (fl & CPF_OUT != 0, d)).unwrap_or((false, 1));
            if dim > 1 {
                // a static array parameter takes the whole array (the instance-variable opcode copies the complete
                // value, `CopyCompleteValue`): its elements are consecutive values, natives see them so too
                let l = if matches!(a, Expr::Nothing) { None } else { self.as_operand(f, |s, f| s.place_of_expr(f, a))? };
                let start = vals.len();
                for k in 0..dim {
                    let v = match &l {
                        Some(l) if k < l.def.dim.max(1) => self.read_loc(f, &Loc { mem: l.mem, off: l.off + k * l.def.elem_size, def: l.def })?,
                        _ => Value::Void,
                    };
                    vals.push(v);
                }
                if is_out {
                    outs.push((start, dim, l));
                }
            } else if is_out && !matches!(a, Expr::Nothing) {
                let l = self.as_operand(f, |s, f| s.place_of_expr(f, a))?;
                let cur = match &l {
                    Some(l) => self.read_loc(f, l)?,
                    None => Value::Void,
                };
                outs.push((vals.len(), 1, l));
                vals.push(cur);
            } else {
                vals.push(self.as_operand(f, |s, f| s.eval(f, a))?);
            }
        }
        self.native_array_out = None;
        let ret = if self.profile_on { self.call_function(this, loc, &mut vals)? } else { self.call_planned(this, &plan, &mut vals)? };
        for (i, n, l) in outs {
            if let Some(l) = l {
                for k in 0..n.min(l.def.dim.max(1)) {
                    let e = Loc { mem: l.mem, off: l.off + k * l.def.elem_size, def: l.def };
                    self.write_loc(f, &e, std::mem::replace(&mut vals[i + k as usize], Value::Void))?;
                }
            }
        }
        if self.arg_pool.len() < 64 {
            vals.clear();
            self.arg_pool.push(vals);
        }
        if flags & FUNC_NATIVE != 0 {
            if let Some((i, elems)) = self.native_array_out.take() {
                if let Some(a) = args.get(i).filter(|a| !matches!(a, Expr::Nothing)) {
                    if let Some(l) = self.as_operand(f, |s, f| s.place_of_expr(f, a))? {
                        let n = l.def.dim.max(1) as usize;
                        for (k, v) in elems.into_iter().take(n).enumerate() {
                            let e = Loc { mem: l.mem, off: l.off + k as u32 * l.def.elem_size, def: l.def };
                            self.write_loc(f, &e, v)?;
                        }
                    }
                }
            }
        }
        Ok(ret)
    }

    /// Does the function contain real code? Declarations without a body and `ignores` stubs do not.
    pub(crate) fn function_has_body(&mut self, loc: FuncLoc) -> bool {
        let (flags, export, mem_size) = {
            let d = self.reg.function(loc);
            (d.flags, d.export, d.script_mem_size)
        };
        if flags & FUNC_NATIVE != 0 {
            return true;
        }
        if mem_size == 0 {
            return false;
        }
        match self.code_for(export, mem_size) {
            Ok(code) => code.stmts.iter().any(|s| !matches!(s, Stmt::Nothing | Stmt::Return(Expr::Nothing))),
            Err(_) => true,
        }
    }

    /// `UObject::IsProbing`: will sending `event` to the object run any code in its current state?
    pub fn is_probing(&mut self, obj: u32, event: &str) -> bool {
        let Some(id) = self.reg.names.get(event) else { return false };
        match self.resolve_function(obj, id) {
            Some(loc) => self.function_has_body(loc),
            None => false,
        }
    }

    /// Calls a function with evaluated arguments. On return `args` holds the final value of each parameter
    /// (out parameters carry results back).
    pub fn call_function(&mut self, this: u32, loc: FuncLoc, args: &mut Vec<Value>) -> Res<Value> {
        if !self.profile_on {
            return self.call_function_inner(this, loc, args);
        }
        // `DX_VM_PROFILE=1`: self time and call counts per script function / native (see `Vm::profile_report`)
        let (name, native) = {
            let d = self.reg.function(loc);
            (d.name, d.flags & FUNC_NATIVE != 0)
        };
        let key = if native {
            format!("native {}", self.reg.names.str(name))
        } else {
            format!("{}.{}", self.class_of(this).map(|c| self.reg.class(c).path.clone()).unwrap_or_default(), self.reg.names.str(name))
        };
        self.profile_stack.push(0.0);
        let t0 = std::time::Instant::now();
        let r = self.call_function_inner(this, loc, args);
        let total = t0.elapsed().as_secs_f64();
        let child = self.profile_stack.pop().unwrap_or(0.0);
        if let Some(top) = self.profile_stack.last_mut() {
            *top += total;
        }
        let e = self.profile.entry(key).or_insert((0, 0.0));
        e.0 += 1;
        e.1 += total - child;
        r
    }

    /// The slowest functions by self time, one per line: seconds, calls, name.
    pub fn profile_report(&self, top: usize) -> String {
        let mut v: Vec<(&String, &(u64, f64))> = self.profile.iter().collect();
        v.sort_by(|a, b| b.1 .1.total_cmp(&a.1 .1));
        let total: f64 = v.iter().map(|x| x.1 .1).sum();
        let mut out = format!("total self time {total:.2} s\n");
        for (k, (n, s)) in v.into_iter().take(top) {
            out.push_str(&format!("{s:8.3} s {n:9} calls {k}\n"));
        }
        out
    }

    /// What a call of `loc` needs from its definition, worked out once (`call_plans`).
    fn call_plan(&mut self, loc: FuncLoc) -> std::rc::Rc<CallPlan> {
        if let Some(p) = self.call_plans.get(&loc) {
            return p.clone();
        }
        let d = self.reg.function(loc);
        // one value per parameter element (a static array parameter takes `dim` consecutive values)
        let slots: Vec<(PropDef, usize)> = d
            .props
            .iter()
            .filter(|p| p.flags & CPF_PARM != 0 && p.flags & CPF_RETURN == 0)
            .flat_map(|p| (0..p.dim.max(1) as usize).map(move |k| (p.clone(), p.offset as usize + k * p.elem_size as usize)))
            .collect();
        let params: Vec<(u32, u32)> = d.props.iter().filter(|p| p.flags & CPF_PARM != 0 && p.flags & CPF_RETURN == 0).map(|p| (p.flags, p.dim.max(1))).collect();
        let short = if d.flags & FUNC_NATIVE != 0 && params.len() == 2 {
            match self.reg.names.str(d.name) {
                "AndAnd_BoolBool" => 1,
                "OrOr_BoolBool" => 2,
                _ => 0,
            }
        } else {
            0
        };
        let plan = std::rc::Rc::new(CallPlan {
            params,
            short,
            name: d.name,
            flags: d.flags,
            export: d.export,
            mem_size: d.script_mem_size,
            frame_size: d.frame_size,
            slots,
            ret: d.props.iter().find(|p| p.flags & CPF_RETURN != 0).cloned(),
            native_name: self.reg.names.str(d.name).into(),
            code: Default::default(),
            repair: self.port_repair_of(d.export),
        });
        self.call_plans.insert(loc, plan.clone());
        plan
    }

    fn call_function_inner(&mut self, this: u32, loc: FuncLoc, args: &mut Vec<Value>) -> Res<Value> {
        let plan = self.call_plan(loc);
        self.call_planned(this, &plan, args)
    }

    /// `call_function` with the callee's plan in hand.
    fn call_planned(&mut self, this: u32, plan: &CallPlan, args: &mut Vec<Value>) -> Res<Value> {
        let (name, flags, export, mem_size, frame_size) = (plan.name, plan.flags, plan.export, plan.mem_size, plan.frame_size);
        if flags & FUNC_NATIVE != 0 {
            return natives::call(self, this, &plan.native_name, args);
        }
        if mem_size == 0 {
            return Ok(Value::Void);
        }
        if plan.repair != 0 {
            self.port_repair(plan.repair, this, args);
        }
        if self.depth >= MAX_DEPTH {
            return Err(rt("script call depth exceeded"));
        }
        if self.depth == 0 {
            self.steps = 0;
        }
        if trace_enabled() {
            let class = self.class_of(this).map(|c| self.reg.class(c).path.clone()).unwrap_or_default();
            eprintln!("{:indent$}{}.{}", "", class, self.reg.names.str(name), indent = self.depth * 2);
        }
        let code = match plan.code.get() {
            Some(c) => c.clone(),
            None => {
                let c = self.code_for(export, mem_size)?;
                let _ = plan.code.set(c.clone());
                c
            }
        };
        let mut data = self.frame_pool.pop().unwrap_or_default();
        data.clear();
        data.resize(frame_size as usize, 0);
        let mut frame = Frame { mem: Mem { data, strings: Vec::new() }, this, caller: None, ret: Value::Void };
        for (i, (p, off)) in plan.slots.iter().enumerate() {
            if let Some(v) = args.get(i) {
                Self::write_to(&mut frame.mem, p, *off, v.clone())?;
            }
        }
        self.depth += 1;
        self.func_stack.push(name);
        let r = self.run(&mut frame, &code);
        self.func_stack.pop();
        self.depth -= 1;
        let out = r.and_then(|_| {
            for (i, (p, off)) in plan.slots.iter().enumerate() {
                if i < args.len() {
                    args[i] = self.read_from(&frame.mem, p, *off)?;
                }
            }
            if frame.ret != Value::Void {
                return Ok(std::mem::replace(&mut frame.ret, Value::Void));
            }
            match &plan.ret {
                Some(p) => self.read_from(&frame.mem, p, p.offset as usize),
                None => Ok(Value::Void),
            }
        });
        if self.frame_pool.len() < 64 {
            self.frame_pool.push(std::mem::take(&mut frame.mem.data));
        }
        out
    }

    /// `Class.Function` of the script code running on `this` (for "Accessed None" messages); state code shows the
    /// class only.
    fn accessed_none_place(&self, this: u32) -> String {
        let cls = self.class_of(this).map(|c| self.reg.class(c).path.clone()).unwrap_or_default();
        match self.func_stack.last() {
            Some(&n) => format!("{cls} ({})", self.reg.names.str(n)),
            None => cls,
        }
    }

    fn jump(code: &Code, target: u32) -> Res<usize> {
        code.index_of(target).ok_or_else(|| rt(format!("jump to bad offset {target}")))
    }

    fn run(&mut self, f: &mut Frame, code: &Code) -> Res<()> {
        self.run_at(f, code, 0, false).map(|_| ())
    }

    /// Runs a suspended or fresh state-code execution.
    pub(crate) fn run_state(&mut self, ex: &mut crate::StateExec) -> Res<RunEnd> {
        let code = ex.code.clone();
        self.run_at(&mut ex.frame, &code, ex.pc, true)
    }

    /// Source of an iterator loop: the items to visit and the expressions of the out variables they are stored in.
    #[allow(clippy::type_complexity)]
    fn iter_items<'c>(&mut self, f: &mut Frame, e: &'c Expr) -> Res<(Vec<Vec<Value>>, Vec<Value>, Vec<Option<&'c Expr>>, Option<crate::iterators::ActorWalk>)> {
        // `foreach other.Iterator(...)`: the iterator runs on `other`, its arguments belong to the writer
        let (e, target) = match e {
            Expr::Context { obj, inner } => match self.eval(f, obj)? {
                Value::Obj(r) if self.is_live(r) || self.is_pending_deleted(r) => (&**inner, r),
                _ => return Ok((Vec::new(), Vec::new(), Vec::new(), None)),
            },
            other => (other, f.this),
        };
        let Expr::Native(idx, args, _) = e else { return Err(VmError::Unsupported("iterator source is not a native call".into())) };
        let loc = *self
            .reg
            .natives_by_index(*idx)
            .first()
            .ok_or_else(|| VmError::Unsupported(format!("iterator native index {idx}")))?;
        let (name, param_flags) = {
            let d = self.reg.function(loc);
            let pf: Vec<u32> = d.props.iter().filter(|p| p.flags & CPF_PARM != 0 && p.flags & CPF_RETURN == 0).map(|p| p.flags).collect();
            (self.reg.names.str(d.name).to_string(), pf)
        };
        // in-parameters are evaluated now, out-parameters are kept as places
        let mut vals: Vec<Value> = Vec::with_capacity(args.len());
        let mut outs: Vec<Option<&'c Expr>> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            if param_flags.get(i).map(|fl| fl & CPF_OUT != 0).unwrap_or(false) {
                // out-parameters are places; their current values go in too (`CycleActors` reads its index)
                vals.push(if matches!(a, Expr::Nothing) { Value::Void } else { self.eval(f, a)? });
                outs.push(if matches!(a, Expr::Nothing) { None } else { Some(a) });
            } else {
                vals.push(self.eval(f, a)?);
            }
        }
        if let Some(w) = self.actor_walk(&name, target, &vals)? {
            // the engine clears the out actor before every search: a finished loop leaves it None
            return Ok((Vec::new(), vec![Value::Obj(0)], outs, Some(w)));
        }
        let (items, end) = self.native_iterator(&name, target, &vals)?;
        Ok((items, end, outs, None))
    }

    fn iter_assign<'c>(&mut self, f: &mut Frame, items: &[Vec<Value>], idx: usize, outs: &[Option<&'c Expr>]) -> Res<()> {
        for (k, e) in outs.iter().enumerate() {
            if let (Some(e), Some(v)) = (e, items[idx].get(k)) {
                if let Some(l) = self.place_of_expr(f, e)? {
                    self.write_loc(f, &l, v.clone())?;
                }
            }
        }
        Ok(())
    }

    /// Writes the values an iterator leaves in its out-parameters when it runs out (`Void` entries are left alone).
    fn iter_assign_end<'c>(&mut self, f: &mut Frame, end: &[Value], outs: &[Option<&'c Expr>]) -> Res<()> {
        for (k, e) in outs.iter().enumerate() {
            if let (Some(e), Some(v)) = (e, end.get(k)) {
                if *v == Value::Void {
                    continue;
                }
                if let Some(l) = self.place_of_expr(f, e)? {
                    self.write_loc(f, &l, v.clone())?;
                }
            }
        }
        Ok(())
    }

    /// Number of `GotoState` calls the object has seen (0 for anything that is not a live object).
    fn goto_serial_of(&self, obj: u32) -> u64 {
        ref_instance(obj).and_then(|i| self.objects.get(i)).map(|o| o.goto_serial).unwrap_or(0)
    }

    pub(crate) fn run_at<'c>(&mut self, f: &mut Frame, code: &'c Code, start: usize, is_state: bool) -> Res<RunEnd> {
        let serial = self.goto_serial_of(f.this);
        let mut iters: Vec<IterState<'c>> = Vec::new();
        let mut pc = start;
        while pc < code.stmts.len() {
            self.steps += 1;
            if self.steps > MAX_STEPS {
                return Err(rt("script step limit exceeded"));
            }
            match &code.stmts[pc] {
                Stmt::Expr(e) => {
                    self.eval(f, e)?;
                    if is_state {
                        if self.goto_serial_of(f.this) != serial {
                            return Ok(RunEnd::Aborted);
                        }
                        if let Some(l) = self.pending_latent.take() {
                            if !iters.is_empty() {
                                return Err(VmError::Unsupported("latent call inside foreach".into()));
                            }
                            if let Some(i) = ref_instance(f.this) {
                                self.objects[i].latent = Some(l);
                            }
                            return Ok(RunEnd::Suspended(pc + 1));
                        }
                    } else {
                        self.pending_latent = None;
                    }
                }
                Stmt::Return(e) => {
                    let v = self.eval(f, e)?;
                    if v != Value::Void {
                        f.ret = v;
                    }
                    return Ok(RunEnd::Done);
                }
                Stmt::Jump(t) => {
                    pc = Self::jump(code, *t)?;
                    continue;
                }
                Stmt::JumpIfNot(t, e) => {
                    if !self.eval(f, e)?.as_bool().unwrap_or(false) {
                        pc = Self::jump(code, *t)?;
                        continue;
                    }
                }
                Stmt::Switch(e) => {
                    // the switch itself walks the case labels that follow it: a matching label starts execution right
                    // after it (labels with an empty body flow into the next label's body), no match goes to the next
                    // label's test, the default label matches anything
                    let v = self.eval(f, e)?;
                    let mut p = pc + 1;
                    loop {
                        match code.stmts.get(p) {
                            Some(Stmt::Case(next, Some(ce))) => {
                                let c = self.eval(f, ce)?;
                                if case_matches(&v, &c) {
                                    p += 1;
                                    break;
                                }
                                p = Self::jump(code, *next)?;
                            }
                            Some(Stmt::Case(_, None)) => {
                                p += 1;
                                break;
                            }
                            _ => break,
                        }
                    }
                    pc = p;
                    continue;
                }
                // reached by falling through from the previous label's body: nothing to test
                Stmt::Case(_, _) => {}
                Stmt::Stop => return Ok(RunEnd::Done),
                Stmt::Assert(line, e) => {
                    if !self.eval(f, e)?.as_bool().unwrap_or(false) {
                        return Err(rt(format!("assertion failed (line {line})")));
                    }
                }
                Stmt::Nothing | Stmt::LabelTable(_) => {}
                Stmt::EatString(e) => {
                    self.eval(f, e)?;
                }
                Stmt::GotoLabel(e) => {
                    let n = match self.eval(f, e)? {
                        Value::Name(n) => n,
                        _ => return Err(rt("Goto expects a name")),
                    };
                    pc = code.label_index(n).ok_or_else(|| rt(format!("label {} not found", self.reg.names.str(n))))?;
                    continue;
                }
                Stmt::Iterator(src, end_off) => {
                    let end_idx = Self::jump(code, *end_off)?;
                    let (items, end, outs, walk) = self.iter_items(f, src)?;
                    if let Some(mut w) = walk {
                        let Some(row) = self.walk_next(&mut w) else {
                            let end = Vm::walk_end(&w).unwrap_or(end);
                            self.iter_assign_end(f, &end, &outs)?;
                            pc = end_idx;
                            continue;
                        };
                        let items = vec![row];
                        self.iter_assign(f, &items, 0, &outs)?;
                        iters.push(IterState { items, walk: Some(w), end, idx: 0, outs, body_start: pc + 1, end_idx });
                        pc += 1;
                        continue;
                    }
                    let Some(first) = self.next_live_item(&items, 0) else {
                        self.iter_assign_end(f, &end, &outs)?;
                        pc = end_idx;
                        continue;
                    };
                    self.iter_assign(f, &items, first, &outs)?;
                    iters.push(IterState { items, walk: None, end, idx: first, outs, body_start: pc + 1, end_idx });
                }
                Stmt::IteratorNext => {
                    let Some(top) = iters.last_mut() else { return Err(rt("iterator next outside a loop")) };
                    if let Some(mut w) = top.walk.take() {
                        let (outs, body, end) = (top.outs.clone(), top.body_start, top.end_idx);
                        match self.walk_next(&mut w) {
                            Some(row) => {
                                let items = vec![row];
                                self.iter_assign(f, &items, 0, &outs)?;
                                let top = iters.last_mut().unwrap();
                                top.items = items;
                                top.walk = Some(w);
                                pc = body;
                            }
                            None => {
                                let end_vals = std::mem::take(&mut iters.last_mut().unwrap().end);
                                let end_vals = Vm::walk_end(&w).unwrap_or(end_vals);
                                self.iter_assign_end(f, &end_vals, &outs)?;
                                pc = end;
                            }
                        }
                        continue;
                    }
                    let (items, outs, from, body, end) = (std::mem::take(&mut top.items), top.outs.clone(), top.idx + 1, top.body_start, top.end_idx);
                    let next = self.next_live_item(&items, from);
                    match next {
                        Some(i) => {
                            self.iter_assign(f, &items, i, &outs)?;
                            let top = iters.last_mut().unwrap();
                            top.items = items;
                            top.idx = i;
                            pc = body;
                        }
                        None => {
                            let end_vals = std::mem::take(&mut iters.last_mut().unwrap().end);
                            self.iter_assign_end(f, &end_vals, &outs)?;
                            pc = end;
                        }
                    }
                    continue;
                }
                Stmt::IteratorPop => {
                    iters.pop();
                }
            }
            pc += 1;
        }
        Ok(RunEnd::Done)
    }
}

impl Vm {
    /// Index of the first item at or after `from` whose leading object value (if any) still exists: iterators over
    /// actors skip actors destroyed while the loop runs.
    fn next_live_item(&self, items: &[Vec<Value>], from: usize) -> Option<usize> {
        (from..items.len()).find(|&i| match items[i].first() {
            Some(Value::Obj(r)) => self.is_live(*r),
            _ => true,
        })
    }
}

pub(crate) fn _keep(_: u32) -> u32 {
    inst_ref(0)
}

/// `DX_VM_TRACE=1` prints every script function call (entry only) to stderr; for debugging script flow.
fn trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("DX_VM_TRACE").is_some())
}

impl Vm {
    /// Makes the header of package object `r` (`Name`: the last part of its path, `Class`: its class, `Outer`: the
    /// object its path leads to) when `off` lies in Object's native header; false for other members.
    fn package_header(&mut self, r: u32, off: u32) -> bool {
        let Some(obj) = self.reg.class_by_name("Object") else { return false };
        let defs: Vec<_> = ["Outer", "Name", "Class"].iter().filter_map(|n| self.reg.find_prop(obj, n).cloned()).collect();
        let end = defs.iter().map(|d| d.offset + d.elem_size.max(4)).max().unwrap_or(0);
        if off >= end {
            return false;
        }
        if self.headers.contains_key(&r) {
            return true;
        }
        let path = self.reg.obj_path(ue_registry::ObjId(r)).to_string();
        if path.is_empty() {
            return false;
        }
        let (outer_path, name) = match path.rsplit_once('.') {
            Some((o, n)) => (Some(o.to_string()), n.to_string()),
            None => (None, path.clone()),
        };
        let name = self.reg.names.intern(&name);
        let outer = outer_path.map(|o| self.reg.obj_id(&o).0).unwrap_or(0);
        // a class object's class is `Class`; other resources have their export's class
        let class = if self.reg.class_by_path(&path).is_some() { self.reg.class_by_name("Class") } else { self.class_of_object_ref(r) };
        let class = class.map(|c| {
            let p = self.reg.class(c).path.clone();
            self.reg.obj_id(&p).0
        }).unwrap_or(0);
        let mut mem = Mem { data: vec![0; end as usize], strings: Vec::new() };
        for d in &defs {
            let v = match self.reg.names.str(d.name) {
                "Outer" => Value::Obj(outer),
                "Name" => Value::Name(name),
                _ => Value::Obj(class),
            };
            let _ = Self::write_to(&mut mem, d, d.offset as usize, v);
        }
        self.headers.insert(r, mem);
        true
    }
}

/// `execCase`: a string label matches without regard to case (`appStricmp`), other values by their bytes (a float's
/// bits: -0 is not 0); bytes and ints compare as numbers (probes `vm.switch.*`).
fn case_matches(v: &Value, c: &Value) -> bool {
    match (v, c) {
        (Value::Str(a), Value::Str(b)) => a.eq_ignore_ascii_case(b),
        (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
        _ => v == c || v.as_i32().is_some() && v.as_i32() == c.as_i32(),
    }
}

/// Aerugo repairs of the original's scripts: defects of the shipped game code that hurt play (as the map repairs
/// do for its data). Each is a step run before one script function's body, chosen once per function.
impl Vm {
    /// Which repair, if any, function `export` (package, export of its code) gets.
    fn port_repair_of(&self, export: (usize, usize)) -> u8 {
        let Some(pkg) = self.reg.packages.get(export.0) else { return 0 };
        let path = pkg.pkg.object_path(ue_package::ObjectRef::Export(export.1)).unwrap_or_default();
        match (pkg.name.as_str(), path.as_str()) {
            // `Barrel1.TakeDamage` (state Active) gives its gas clouds the barrel's `Instigator` before
            // `Super.TakeDamage` sets it to the attacker: a barrel broken by its first hit let out gas that belonged to
            // nobody, so the people it hurt never reacted (no pain animation, no anger, no flight: `ReactToInjury`
            // needs an instigator). The barrel takes the attacker as its `Instigator` first.
            ("DeusEx", "Barrel1.Active.TakeDamage") => 1,
            _ => 0,
        }
    }

    fn port_repair(&mut self, repair: u8, this: u32, args: &[Value]) {
        if repair == 1 {
            if let (Some(Value::Obj(by)), Ok(Value::Obj(0))) = (args.get(1), self.get_prop(this, "Instigator", 0)) {
                if *by != 0 {
                    let _ = self.set_prop(this, "Instigator", 0, Value::Obj(*by));
                }
            }
        }
    }
}

//! Typed expression tree compiled from the serialized token stream of a function or state.

use crate::VmError;
use ue_package::bytecode::*;
use ue_package::Reader;
use ue_registry::{FuncLoc, ObjId, Owner, PropDef, Registry, StructId};

#[derive(Debug, Clone)]
pub enum Place {
    /// Parameter, return value or local of the running function.
    Local(PropDef),
    /// Variable of the object the code runs on.
    Instance(PropDef),
    /// Variable of the class default object.
    Default(PropDef),
    Elem(Box<Place>, Box<Expr>),
    Member(Box<Place>, PropDef),
    /// Place inside another object (`obj.var = ...`).
    Ctx(Box<Expr>, Box<Place>),
}

#[derive(Debug, Clone)]
pub enum Call {
    Virtual(u32),
    Global(u32),
    Final(FuncLoc),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Nothing,
    Int(i32),
    Float(f32),
    Str(String),
    Name(u32),
    Byte(u8),
    Bool(bool),
    Vector([f32; 3]),
    Rotator([i32; 3]),
    NoObject,
    SelfObj,
    ObjConst(ObjId),
    Load(Place),
    Let(Place, Box<Expr>),
    Cast(u8, Box<Expr>),
    MetaCast(ObjId, Box<Expr>),
    DynCast(ObjId, Box<Expr>),
    Call(Call, Vec<Expr>),
    Native(u16, Vec<Expr>),
    /// Argument evaluated only if the callee asks for it (short-circuit operators).
    Skip(Box<Expr>),
    Context { obj: Box<Expr>, inner: Box<Expr> },
    StructCmp { eq: bool, sid: StructId, a: Box<Expr>, b: Box<Expr> },
    MemberOf(Box<Expr>, PropDef),
    New(Vec<Expr>),
    /// A native operator resolved when the code was compiled (`Op::of`): evaluated in place, without the general
    /// call path. Same operands, conversions and errors as the native.
    Op(Op, Vec<Expr>),
}

/// The pure native operators `Expr::Op` stands for (no out parameters, no byte typing of the result).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    NotB,
    EqB,
    NeB,
    XorB,
    AndB,
    OrB,
    NegI,
    ComplI,
    AddI,
    SubI,
    MulI,
    LtI,
    GtI,
    LeI,
    GeI,
    EqI,
    NeI,
    AndI,
    XorI,
    OrI,
    NegF,
    AddF,
    SubF,
    MulF,
    LtF,
    GtF,
    LeF,
    GeF,
    EqF,
    NeF,
    EqN,
    NeN,
    EqO,
    NeO,
}

impl Op {
    /// The kinds of the operands and of the result (the native's parameter and return types, for diagnostics).
    pub fn kinds(self) -> (&'static str, &'static str) {
        use Op::*;
        match self {
            NotB | EqB | NeB | XorB | AndB | OrB => ("bool", "bool"),
            NegI | ComplI | AddI | SubI | MulI | AndI | XorI | OrI => ("int", "int"),
            LtI | GtI | LeI | GeI | EqI | NeI => ("int", "bool"),
            NegF | AddF | SubF | MulF => ("float", "float"),
            LtF | GtF | LeF | GeF | EqF | NeF => ("float", "bool"),
            EqN | NeN => ("name", "bool"),
            EqO | NeO => ("object", "bool"),
        }
    }

    /// The operator of a native function name and its operand count.
    pub fn of(name: &str) -> Option<(Op, usize)> {
        use Op::*;
        Some(match name {
            "Not_PreBool" => (NotB, 1),
            "EqualEqual_BoolBool" => (EqB, 2),
            "NotEqual_BoolBool" => (NeB, 2),
            "XorXor_BoolBool" => (XorB, 2),
            "AndAnd_BoolBool" => (AndB, 2),
            "OrOr_BoolBool" => (OrB, 2),
            "Subtract_PreInt" => (NegI, 1),
            "Complement_PreInt" => (ComplI, 1),
            "Add_IntInt" => (AddI, 2),
            "Subtract_IntInt" => (SubI, 2),
            "Multiply_IntInt" => (MulI, 2),
            "Less_IntInt" => (LtI, 2),
            "Greater_IntInt" => (GtI, 2),
            "LessEqual_IntInt" => (LeI, 2),
            "GreaterEqual_IntInt" => (GeI, 2),
            "EqualEqual_IntInt" => (EqI, 2),
            "NotEqual_IntInt" => (NeI, 2),
            "And_IntInt" => (AndI, 2),
            "Xor_IntInt" => (XorI, 2),
            "Or_IntInt" => (OrI, 2),
            "Subtract_PreFloat" => (NegF, 1),
            "Add_FloatFloat" => (AddF, 2),
            "Subtract_FloatFloat" => (SubF, 2),
            "Multiply_FloatFloat" => (MulF, 2),
            "Less_FloatFloat" => (LtF, 2),
            "Greater_FloatFloat" => (GtF, 2),
            "LessEqual_FloatFloat" => (LeF, 2),
            "GreaterEqual_FloatFloat" => (GeF, 2),
            "EqualEqual_FloatFloat" => (EqF, 2),
            "NotEqual_FloatFloat" => (NeF, 2),
            "EqualEqual_NameName" => (EqN, 2),
            "NotEqual_NameName" => (NeN, 2),
            "EqualEqual_ObjectObject" => (EqO, 2),
            "NotEqual_ObjectObject" => (NeO, 2),
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Expr(Expr),
    Return(Expr),
    Jump(u32),
    JumpIfNot(u32, Expr),
    Switch(Expr),
    /// Next-case offset and the value to compare (none for `default`).
    Case(u32, Option<Expr>),
    Stop,
    Assert(u16, Expr),
    Nothing,
    LabelTable(Vec<(u32, u32)>),
    GotoLabel(Expr),
    Iterator(Expr, u32),
    IteratorNext,
    IteratorPop,
    EatString(Expr),
}

#[derive(Debug, Clone, Default)]
pub struct Code {
    pub end: u32,
    pub stmts: Vec<Stmt>,
    /// In-memory offset of each statement.
    /// (Increasing: statements are compiled in order, so jumps find their target by binary search.)
    pub offsets: Vec<u32>,
}

impl Code {
    /// Statement index a label points at.
    pub fn label_index(&self, name: u32) -> Option<usize> {
        for st in &self.stmts {
            if let Stmt::LabelTable(v) = st {
                if let Some((_, off)) = v.iter().find(|(n, _)| *n == name) {
                    return self.index_of(*off);
                }
            }
        }
        None
    }

    pub fn index_of(&self, mem_offset: u32) -> Option<usize> {
        if mem_offset == self.end {
            return Some(self.stmts.len());
        }
        self.offsets.binary_search(&mem_offset).ok()
    }
}

struct Compiler<'a> {
    reg: &'a mut Registry,
    pi: usize,
    r: Reader<'a>,
    mem: u32,
    none_name: i32,
}

type Res<T> = Result<T, VmError>;

fn e<T: std::fmt::Display>(x: T) -> VmError {
    VmError::Compile(x.to_string())
}

/// Compiles the code of the function or state at `export`.
pub fn compile(reg: &mut Registry, export: (usize, usize), mem_size: u32) -> Res<Code> {
    let (data, range) = reg.function_code(export).map_err(e)?;
    let none_name = reg.packages[export.0].pkg.names.iter().position(|n| n.name == "None").unwrap_or(0) as i32;
    let pi = export.0;
    let data_ref: &[u8] = &data;
    let mut c = Compiler { reg, pi, r: Reader::new(data_ref, range.start), mem: 0, none_name };
    let mut code = Code::default();
    while c.mem < mem_size {
        let off = c.mem;
        let s = c.stmt()?;
        code.offsets.push(off);
        code.stmts.push(s);
    }
    if c.mem != mem_size {
        return Err(e(format!("compiled {} memory bytes, expected {mem_size}", c.mem)));
    }
    code.end = mem_size;
    Ok(code)
}

impl<'a> Compiler<'a> {
    /// A call of native `idx`: an operator node when it is one of `Op`'s (the engine's natives table, first entry).
    fn native(&mut self, idx: u16) -> Result<Expr, VmError> {
        let args = self.params()?;
        let op = self.reg.natives_by_index(idx).first().copied().and_then(|loc| self.native_op(loc, args.len()));
        Ok(match op {
            Some(op) => Expr::Op(op, args),
            None => Expr::Native(idx, args),
        })
    }

    /// The operator a call of `loc` with `nargs` operands stands for, if it is a native one of `Op`'s.
    fn native_op(&self, loc: FuncLoc, nargs: usize) -> Option<Op> {
        const FUNC_NATIVE: u32 = 0x400;
        let d = self.reg.function(loc);
        if d.flags & FUNC_NATIVE == 0 {
            return None;
        }
        Op::of(self.reg.names.str(d.name)).filter(|&(_, n)| n == nargs).map(|(op, _)| op)
    }

    fn u8(&mut self) -> Res<u8> {
        self.mem += 1;
        self.r.u8().map_err(e)
    }

    fn u16(&mut self) -> Res<u16> {
        self.mem += 2;
        self.r.u16().map_err(e)
    }

    fn i32(&mut self) -> Res<i32> {
        self.mem += 4;
        self.r.i32().map_err(e)
    }

    fn compact(&mut self) -> Res<i32> {
        self.mem += 4;
        self.r.compact_index().map_err(e)
    }

    fn name(&mut self) -> Res<u32> {
        let l = self.compact()?;
        Ok(self.reg.global_name(self.pi, l))
    }

    fn obj_path_id(&mut self, r: i32) -> ObjId {
        match self.reg.ref_path(self.pi, r) {
            Some(p) => self.reg.obj_id(&p),
            None => ObjId(0),
        }
    }

    fn prop(&mut self) -> Res<ue_registry::PropLoc> {
        let r = self.compact()?;
        self.reg.prop_loc(self.pi, r).cloned().ok_or_else(|| {
            let p = self.reg.ref_path(self.pi, r).unwrap_or_else(|| format!("#{r}"));
            e(format!("unresolved property reference {p}"))
        })
    }

    fn stmt(&mut self) -> Res<Stmt> {
        let op = self.u8()?;
        Ok(match op {
            EX_RETURN => Stmt::Return(self.expr()?),
            EX_SWITCH => {
                self.u8()?;
                Stmt::Switch(self.expr()?)
            }
            EX_JUMP => Stmt::Jump(self.u16()? as u32),
            EX_JUMP_IF_NOT => {
                let t = self.u16()? as u32;
                Stmt::JumpIfNot(t, self.expr()?)
            }
            EX_STOP => Stmt::Stop,
            EX_ASSERT => {
                let line = self.u16()?;
                Stmt::Assert(line, self.expr()?)
            }
            EX_CASE => {
                let next = self.u16()?;
                if next != 0xFFFF {
                    Stmt::Case(next as u32, Some(self.expr()?))
                } else {
                    Stmt::Case(0xFFFF, None)
                }
            }
            EX_NOTHING => Stmt::Nothing,
            EX_LABEL_TABLE => {
                let mut v = Vec::new();
                loop {
                    let l = self.compact()?;
                    let off = self.i32()? as u32;
                    if l == self.none_name {
                        break;
                    }
                    v.push((self.reg.global_name(self.pi, l), off));
                }
                Stmt::LabelTable(v)
            }
            EX_GOTO_LABEL => Stmt::GotoLabel(self.expr()?),
            EX_EAT_STRING => Stmt::EatString(self.expr()?),
            EX_ITERATOR => {
                let x = self.expr()?;
                Stmt::Iterator(x, self.u16()? as u32)
            }
            EX_ITERATOR_POP => Stmt::IteratorPop,
            EX_ITERATOR_NEXT => Stmt::IteratorNext,
            other => Stmt::Expr(self.expr_op(other)?),
        })
    }

    fn expr(&mut self) -> Res<Expr> {
        let op = self.u8()?;
        self.expr_op(op)
    }

    fn params(&mut self) -> Res<Vec<Expr>> {
        let mut v = Vec::new();
        loop {
            let op = self.u8()?;
            if op == EX_END_FUNCTION_PARMS {
                return Ok(v);
            }
            v.push(self.expr_op(op)?);
        }
    }

    fn to_place(x: Expr) -> Res<Place> {
        match x {
            Expr::Load(p) => Ok(p),
            Expr::Context { obj, inner } => match *inner {
                Expr::Load(p) => Ok(Place::Ctx(obj, Box::new(p))),
                _ => Err(e("context target is not a variable")),
            },
            other => Err(e(format!("expression is not assignable: {other:?}"))),
        }
    }

    fn expr_op(&mut self, op: u8) -> Res<Expr> {
        Ok(match op {
            EX_LOCAL_VARIABLE | EX_NATIVE_PARM => {
                let p = self.prop()?;
                match p.owner {
                    Owner::Function(_) => Expr::Load(Place::Local(p.def)),
                    _ => Expr::Load(Place::Instance(p.def)),
                }
            }
            EX_INSTANCE_VARIABLE => {
                let p = self.prop()?;
                Expr::Load(Place::Instance(p.def))
            }
            EX_DEFAULT_VARIABLE => {
                let p = self.prop()?;
                Expr::Load(Place::Default(p.def))
            }
            EX_BOOL_VARIABLE => self.expr()?,
            EX_ARRAY_ELEMENT => {
                let idx = self.expr()?;
                let base = Self::to_place(self.expr()?)?;
                Expr::Load(Place::Elem(Box::new(base), Box::new(idx)))
            }
            EX_STRUCT_MEMBER => {
                let m = self.prop()?;
                match self.expr()? {
                    Expr::Load(p) => Expr::Load(Place::Member(Box::new(p), m.def)),
                    Expr::Context { obj, inner } => match *inner {
                        Expr::Load(p) => Expr::Load(Place::Ctx(obj, Box::new(Place::Member(Box::new(p), m.def)))),
                        other => Expr::MemberOf(Box::new(Expr::Context { obj, inner: Box::new(other) }), m.def),
                    },
                    other => Expr::MemberOf(Box::new(other), m.def),
                }
            }
            EX_CONTEXT | EX_CLASS_CONTEXT => {
                let obj = self.expr()?;
                self.u16()?;
                self.u8()?;
                let inner = self.expr()?;
                Expr::Context { obj: Box::new(obj), inner: Box::new(inner) }
            }
            EX_LET | EX_LET_BOOL => {
                let dst = Self::to_place(self.expr()?)?;
                let src = self.expr()?;
                Expr::Let(dst, Box::new(src))
            }
            EX_NEW => {
                let mut v = Vec::new();
                for _ in 0..4 {
                    v.push(self.expr()?);
                }
                Expr::New(v)
            }
            EX_META_CAST | EX_DYNAMIC_CAST => {
                let r = self.compact()?;
                let id = self.obj_path_id(r);
                let x = self.expr()?;
                if op == EX_META_CAST { Expr::MetaCast(id, Box::new(x)) } else { Expr::DynCast(id, Box::new(x)) }
            }
            EX_VIRTUAL_FUNCTION => {
                let n = self.name()?;
                Expr::Call(Call::Virtual(n), self.params()?)
            }
            EX_GLOBAL_FUNCTION => {
                let n = self.name()?;
                Expr::Call(Call::Global(n), self.params()?)
            }
            EX_FINAL_FUNCTION => {
                let r = self.compact()?;
                let loc = self.reg.func_loc(self.pi, r).ok_or_else(|| {
                    e(format!("unresolved function {}", self.reg.ref_path(self.pi, r).unwrap_or_default()))
                })?;
                let args = self.params()?;
                match self.native_op(loc, args.len()) {
                    Some(op) => Expr::Op(op, args),
                    None => Expr::Call(Call::Final(loc), args),
                }
            }
            EX_INT_CONST => Expr::Int(self.i32()?),
            EX_FLOAT_CONST => Expr::Float(f32::from_bits(self.i32()? as u32)),
            EX_STRING_CONST => {
                let mut s = String::new();
                loop {
                    let b = self.u8()?;
                    if b == 0 {
                        break;
                    }
                    s.push(b as char);
                }
                Expr::Str(s)
            }
            EX_UNICODE_STRING_CONST => {
                let mut u = Vec::new();
                loop {
                    let c = self.u16()?;
                    if c == 0 {
                        break;
                    }
                    u.push(c);
                }
                Expr::Str(String::from_utf16_lossy(&u))
            }
            EX_OBJECT_CONST => {
                let r = self.compact()?;
                Expr::ObjConst(self.obj_path_id(r))
            }
            EX_NAME_CONST => Expr::Name(self.name()?),
            EX_ROTATION_CONST => Expr::Rotator([self.i32()?, self.i32()?, self.i32()?]),
            EX_VECTOR_CONST => {
                let mut v = [0.0f32; 3];
                for x in &mut v {
                    *x = f32::from_bits(self.i32()? as u32);
                }
                Expr::Vector(v)
            }
            EX_BYTE_CONST => Expr::Byte(self.u8()?),
            EX_INT_CONST_BYTE => Expr::Int(self.u8()? as i32),
            EX_INT_ZERO => Expr::Int(0),
            EX_INT_ONE => Expr::Int(1),
            EX_TRUE => Expr::Bool(true),
            EX_FALSE => Expr::Bool(false),
            EX_NO_OBJECT => Expr::NoObject,
            EX_SELF => Expr::SelfObj,
            EX_NOTHING | EX_END_PARM_VALUE => Expr::Nothing,
            EX_SKIP => {
                self.u16()?;
                Expr::Skip(Box::new(self.expr()?))
            }
            EX_STRUCT_CMP_EQ | EX_STRUCT_CMP_NE => {
                let r = self.compact()?;
                let path = self.reg.ref_path(self.pi, r).unwrap_or_default();
                let sid = self.reg.struct_id_by_path(&path).ok_or_else(|| e(format!("unknown struct {path}")))?;
                let a = self.expr()?;
                let b = self.expr()?;
                Expr::StructCmp { eq: op == EX_STRUCT_CMP_EQ, sid, a: Box::new(a), b: Box::new(b) }
            }
            EX_CAST_FIRST..=EX_CAST_LAST => Expr::Cast(op, Box::new(self.expr()?)),
            EX_EXTENDED_NATIVE..=0x6F => {
                let lo = self.u8()? as u16;
                let idx = (((op & 0x0F) as u16) << 8) | lo;
                self.native(idx)?
            }
            EX_FIRST_NATIVE..=0xFF => self.native(op as u16)?,
            other => return Err(e(format!("unsupported token 0x{other:02X}"))),
        })
    }
}

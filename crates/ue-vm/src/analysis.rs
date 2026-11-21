//! Whole-game checks and statistics over compiled script code.

use crate::ir::*;
use crate::{Vm, VmError};
use std::collections::BTreeMap;
use ue_package::object::PropertyKind;
use ue_registry::{PropDef, Target};

#[derive(Debug, Default)]
pub struct CompileReport {
    pub functions: usize,
    pub states: usize,
    pub failed: usize,
    pub statements: usize,
    pub errors: BTreeMap<String, (usize, String)>,
}

fn kind_name(vm: &Vm, d: &PropDef) -> String {
    match d.kind {
        PropertyKind::Byte => "byte".into(),
        PropertyKind::Int => "int".into(),
        PropertyKind::Bool => "bool".into(),
        PropertyKind::Float => "float".into(),
        PropertyKind::Name => "name".into(),
        PropertyKind::Str | PropertyKind::String => "string".into(),
        PropertyKind::Object | PropertyKind::Class => "object".into(),
        PropertyKind::Struct => match d.target {
            Target::Struct(s) => vm.reg.struct_def(s).path.rsplit('.').next().unwrap_or("struct").to_string(),
            _ => "struct".into(),
        },
        other => format!("{other:?}").to_lowercase(),
    }
}

fn place_kind(vm: &Vm, p: &Place) -> Option<String> {
    Some(match p {
        Place::Local(d) | Place::Instance(d) | Place::Default(d) | Place::Member(_, d) => kind_name(vm, d),
        Place::Elem(b, _) => return place_kind(vm, b),
        Place::Ctx(_, p) => return place_kind(vm, p),
    })
}

fn expr_kind(vm: &Vm, e: &Expr) -> Option<String> {
    Some(match e {
        Expr::Int(_) => "int".into(),
        Expr::Float(_) => "float".into(),
        Expr::Str(_) => "string".into(),
        Expr::Name(_) => "name".into(),
        Expr::Byte(_) => "byte".into(),
        Expr::Bool(_) => "bool".into(),
        Expr::Vector(_) => "Vector".into(),
        Expr::Rotator(_) => "Rotator".into(),
        Expr::NoObject | Expr::SelfObj | Expr::ObjConst(_) => "object".into(),
        Expr::Load(p) => return place_kind(vm, p),
        Expr::Context { inner, .. } => return expr_kind(vm, inner),
        Expr::Call(Call::Final(loc), _, _) => return return_kind(vm, *loc),
        Expr::Native(i, _, _) => return vm.reg.natives_by_index(*i).first().and_then(|l| return_kind(vm, *l)),
        Expr::Cast(op, _) => format!("cast{op:02X}"),
        Expr::Op(op, _) => op.kinds().1.into(),
        _ => return None,
    })
}

fn return_kind(vm: &Vm, loc: ue_registry::FuncLoc) -> Option<String> {
    vm.reg.function(loc).props.iter().find(|p| p.flags & 0x400 != 0).map(|p| kind_name(vm, p))
}

fn param_kinds(vm: &Vm, loc: ue_registry::FuncLoc) -> Vec<String> {
    vm.reg.function(loc).props.iter().filter(|p| p.flags & 0x80 != 0 && p.flags & 0x400 == 0).map(|p| kind_name(vm, p)).collect()
}

/// For each cast opcode: counts of (operand kind -> kind expected by the context).
pub type CastStats = BTreeMap<u8, BTreeMap<(String, String), usize>>;

fn walk(vm: &Vm, e: &Expr, expected: Option<String>, st: &mut CastStats) {
    match e {
        Expr::Cast(op, x) => {
            let operand = expr_kind(vm, x).unwrap_or_else(|| "?".into());
            *st.entry(*op).or_default().entry((operand, expected.unwrap_or_else(|| "?".into()))).or_default() += 1;
            walk(vm, x, None, st);
        }
        Expr::Let(p, src) => walk(vm, src, place_kind(vm, p), st),
        Expr::Call(call, args, _) => {
            let pk = match call {
                Call::Final(l) => param_kinds(vm, *l),
                _ => Vec::new(),
            };
            for (i, a) in args.iter().enumerate() {
                walk(vm, a, pk.get(i).cloned(), st);
            }
        }
        Expr::Native(idx, args, _) => {
            let pk = vm.reg.natives_by_index(*idx).first().map(|l| param_kinds(vm, *l)).unwrap_or_default();
            for (i, a) in args.iter().enumerate() {
                walk(vm, a, pk.get(i).cloned(), st);
            }
        }
        Expr::Op(op, args) => {
            for a in args {
                walk(vm, a, Some(op.kinds().0.into()), st);
            }
        }
        Expr::Skip(x) | Expr::MetaCast(_, x) | Expr::DynCast(_, x) | Expr::MemberOf(x, _) => walk(vm, x, None, st),
        Expr::Context { obj, inner } => {
            walk(vm, obj, None, st);
            walk(vm, inner, expected, st);
        }
        Expr::StructCmp { a, b, .. } => {
            walk(vm, a, None, st);
            walk(vm, b, None, st);
        }
        Expr::New(v) => v.iter().for_each(|x| walk(vm, x, None, st)),
        _ => {}
    }
}

fn walk_stmt(vm: &Vm, s: &Stmt, ret: &Option<String>, st: &mut CastStats) {
    match s {
        Stmt::Expr(e) | Stmt::Switch(e) | Stmt::Case(_, Some(e)) | Stmt::GotoLabel(e) | Stmt::Iterator(e, _) | Stmt::EatString(e) => {
            walk(vm, e, None, st)
        }
        Stmt::Return(e) => walk(vm, e, ret.clone(), st),
        Stmt::JumpIfNot(_, e) | Stmt::Assert(_, e) => walk(vm, e, Some("bool".into()), st),
        _ => {}
    }
}

impl Vm {
    /// Compiles every function and state of every class; returns statistics and failures.
    pub fn compile_all(&mut self) -> (CompileReport, CastStats) {
        let mut rep = CompileReport::default();
        let mut stats = CastStats::new();
        let mut jobs: Vec<((usize, usize), u32, Option<String>, String)> = Vec::new();
        for c in &self.reg.classes {
            let cname = self.reg.names.str(c.name).to_string();
            for f in &c.functions {
                let ret = f.props.iter().find(|p| p.flags & 0x400 != 0).map(|p| (p.kind, p.clone()));
                let _ = ret;
                jobs.push((f.export, f.script_mem_size, None, format!("{cname}.{}", self.reg.names.str(f.name))));
            }
            for s in &c.states {
                for f in &s.functions {
                    jobs.push((f.export, f.script_mem_size, None, format!("{cname}.{}.{}", self.reg.names.str(s.name), self.reg.names.str(f.name))));
                }
                jobs.push((s.export, s.script_mem_size, Some("state".into()), format!("{cname}.{} (state code)", self.reg.names.str(s.name))));
            }
        }
        for (export, size, kind, name) in jobs {
            if size == 0 {
                continue;
            }
            if kind.is_some() { rep.states += 1 } else { rep.functions += 1 }
            match self.code_for(export, size) {
                Ok(code) => {
                    rep.statements += code.stmts.len();
                    for s in &code.stmts {
                        walk_stmt(self, s, &None, &mut stats);
                    }
                }
                Err(e) => {
                    rep.failed += 1;
                    let key = match &e {
                        VmError::Compile(m) => m.chars().take(60).collect::<String>(),
                        other => other.to_string(),
                    };
                    let ent = rep.errors.entry(key).or_insert((0, name));
                    ent.0 += 1;
                }
            }
        }
        (rep, stats)
    }
}

fn expr_pure(vm: &Vm, e: &Expr, memo: &mut std::collections::HashMap<ue_registry::FuncLoc, Option<bool>>) -> bool {
    match e {
        Expr::Nothing | Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Name(_) | Expr::Byte(_) | Expr::Bool(_)
        | Expr::Vector(_) | Expr::Rotator(_) => true,
        Expr::Load(p) => place_pure(vm, p, memo),
        Expr::Let(p, x) => place_pure(vm, p, memo) && expr_pure(vm, x, memo),
        Expr::Cast(_, x) | Expr::Skip(x) => expr_pure(vm, x, memo),
        Expr::Native(i, args, _) => {
            let ok = vm.reg.natives_by_index(*i).first().map(|l| crate::natives::is_implemented(vm.reg.names.str(vm.reg.function(*l).name))).unwrap_or(false);
            ok && args.iter().all(|a| expr_pure(vm, a, memo))
        }
        Expr::Call(Call::Final(l), args, _) => func_pure(vm, *l, memo) && args.iter().all(|a| expr_pure(vm, a, memo)),
        // implemented natives
        Expr::Op(_, args) => args.iter().all(|a| expr_pure(vm, a, memo)),
        _ => false,
    }
}

fn place_pure(vm: &Vm, p: &Place, memo: &mut std::collections::HashMap<ue_registry::FuncLoc, Option<bool>>) -> bool {
    match p {
        Place::Local(_) => true,
        Place::Elem(b, i) => place_pure(vm, b, memo) && expr_pure(vm, i, memo),
        Place::Member(b, _) => place_pure(vm, b, memo),
        _ => false,
    }
}

fn func_pure(vm: &Vm, l: ue_registry::FuncLoc, memo: &mut std::collections::HashMap<ue_registry::FuncLoc, Option<bool>>) -> bool {
    match memo.get(&l) {
        Some(Some(b)) => return *b,
        Some(None) => return true, // recursion: assume pure while checking
        None => {}
    }
    memo.insert(l, None);
    let d = vm.reg.function(l);
    let ok = if d.flags & 0x400 != 0 {
        crate::natives::is_implemented(vm.reg.names.str(d.name))
    } else if d.script_mem_size == 0 {
        false
    } else {
        match vm.code.get(&d.export) {
            Some(code) => code.stmts.iter().all(|s| match s {
                Stmt::Expr(e) | Stmt::Return(e) | Stmt::Switch(e) | Stmt::Case(_, Some(e)) | Stmt::JumpIfNot(_, e) => expr_pure(vm, e, memo),
                Stmt::Jump(_) | Stmt::Nothing | Stmt::Stop | Stmt::Case(_, None) => true,
                _ => false,
            }),
            None => false,
        }
    };
    memo.insert(l, Some(ok));
    ok
}

impl Vm {
    /// Functions that touch only their own parameters/locals, constants, implemented natives and other such functions.
    /// Call `compile_all` first.
    pub fn pure_functions(&self) -> Vec<(ue_registry::FuncLoc, String)> {
        let mut memo = std::collections::HashMap::new();
        let mut out = Vec::new();
        for (ci, c) in self.reg.classes.iter().enumerate() {
            for (fi, f) in c.functions.iter().enumerate() {
                if f.flags & 0x400 != 0 || f.script_mem_size == 0 {
                    continue;
                }
                let loc = ue_registry::FuncLoc { class: ue_registry::ClassId(ci as u32), state: None, index: fi };
                if func_pure(self, loc, &mut memo) {
                    let sig: Vec<String> = f.props.iter().filter(|p| p.flags & 0x80 != 0 && p.flags & 0x400 == 0).map(|p| kind_name(self, p)).collect();
                    let ret = f.props.iter().find(|p| p.flags & 0x400 != 0).map(|p| kind_name(self, p)).unwrap_or_else(|| "void".into());
                    out.push((loc, format!("{}.{}({}) -> {ret}  [{} stmts]", self.reg.names.str(c.name), self.reg.names.str(f.name), sig.join(", "), self.code.get(&f.export).map(|c| c.stmts.len()).unwrap_or(0))));
                }
            }
        }
        out
    }
}

impl Vm {
    /// Calls every pure function with zero-valued arguments; returns (ok count, error message -> (count, example)).
    pub fn run_pure_functions(&mut self) -> (usize, std::collections::BTreeMap<String, (usize, String)>) {
        let list = self.pure_functions();
        let mut ok = 0;
        let mut errs = std::collections::BTreeMap::new();
        for (loc, desc) in list {
            let kinds = param_kinds(self, loc);
            let mut args: Vec<crate::Value> = kinds
                .iter()
                .map(|k| match k.as_str() {
                    "byte" => crate::Value::Byte(0),
                    "int" => crate::Value::Int(0),
                    "float" => crate::Value::Float(0.0),
                    "bool" => crate::Value::Bool(false),
                    "string" => crate::Value::Str(String::new()),
                    "name" => crate::Value::Name(0),
                    "object" => crate::Value::Obj(0),
                    "Vector" => crate::Value::Vector([0.0; 3]),
                    "Rotator" => crate::Value::Rotator([0; 3]),
                    _ => crate::Value::Void,
                })
                .collect();
            match self.call_function(0, loc, &mut args) {
                Ok(_) => ok += 1,
                Err(e) => {
                    let key = e.to_string();
                    let ent = errs.entry(key).or_insert((0, desc));
                    ent.0 += 1;
                }
            }
        }
        (ok, errs)
    }
}

impl Vm {
    /// Human-readable listing of a function's compiled statements (offset: statement).
    pub fn dump_function(&mut self, class: &str, func: &str) -> Result<String, VmError> {
        let cid = self.reg.class_by_name(class).ok_or_else(|| VmError::Runtime(format!("no class {class}")))?;
        let id = self.reg.names.get(func).ok_or_else(|| VmError::Runtime(format!("no name {func}")))?;
        let loc = self.reg.find_function(cid, id).ok_or_else(|| VmError::Runtime(format!("no function {func}")))?;
        let (export, size) = {
            let d = self.reg.function(loc);
            (d.export, d.script_mem_size)
        };
        let code = self.code_for(export, size)?;
        let mut out = String::new();
        for (i, s) in code.stmts.iter().enumerate() {
            out.push_str(&format!("{:4} @{:<4} {:?}\n", i, code.offsets[i], s));
        }
        Ok(out)
    }
}

impl Vm {
    /// Spawns one instance of every concrete actor class. Returns (spawned ok, failures grouped by message).
    /// Failures point at engine natives and features the VM still lacks.
    pub fn spawn_all_actor_classes(&mut self) -> (usize, std::collections::BTreeMap<String, (usize, Vec<String>)>) {
        let actor = self.reg.class_by_name("Actor");
        let mut ok = 0;
        let mut errs: std::collections::BTreeMap<String, (usize, Vec<String>)> = std::collections::BTreeMap::new();
        let ids: Vec<ue_registry::ClassId> = (0..self.reg.classes.len() as u32).map(ue_registry::ClassId).collect();
        for id in ids {
            let c = self.reg.class(id);
            // skip abstract classes (class flag 0x1) and anything that is not an actor
            if c.flags & 0x1 != 0 || !actor.map(|a| self.is_a(id, a)).unwrap_or(false) {
                continue;
            }
            let name = self.reg.names.str(c.name).to_string();
            match self.spawn(id, 0, None, None, None) {
                Ok(_) => ok += 1,
                Err(e) => {
                    let key = e.to_string();
                    let ent = errs.entry(key).or_default();
                    ent.0 += 1;
                    if ent.1.len() < 4 {
                        ent.1.push(name);
                    }
                }
            }
        }
        (ok, errs)
    }
}

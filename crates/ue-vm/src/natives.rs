//! Built-in functions of the `Object` class (operators, math, strings) and primitive casts.

use crate::{ref_instance, Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

fn rt<T: Into<String>>(s: T) -> VmError {
    VmError::Runtime(s.into())
}

fn int(a: &[Value], i: usize) -> Res<i32> {
    a.get(i).and_then(Value::as_i32).ok_or_else(|| rt(format!("argument {i} is not an int")))
}

fn flt(a: &[Value], i: usize) -> Res<f32> {
    a.get(i).and_then(Value::as_f32).ok_or_else(|| rt(format!("argument {i} is not a float")))
}

fn boo(a: &[Value], i: usize) -> Res<bool> {
    a.get(i).and_then(Value::as_bool).ok_or_else(|| rt(format!("argument {i} is not a bool")))
}

fn strv(a: &[Value], i: usize) -> Res<String> {
    match a.get(i) {
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(Value::Void) | None => Ok(String::new()),
        _ => Err(rt(format!("argument {i} is not a string"))),
    }
}

fn vecv(a: &[Value], i: usize) -> Res<[f32; 3]> {
    match a.get(i) {
        Some(Value::Vector(v)) => Ok(*v),
        Some(Value::Void) | None => Ok([0.0; 3]),
        _ => Err(rt(format!("argument {i} is not a vector"))),
    }
}

fn rotv(a: &[Value], i: usize) -> Res<[i32; 3]> {
    match a.get(i) {
        Some(Value::Rotator(v)) => Ok(*v),
        Some(Value::Void) | None => Ok([0; 3]),
        _ => Err(rt(format!("argument {i} is not a rotator"))),
    }
}

fn is_byte(a: &[Value], i: usize) -> bool {
    matches!(a.get(i), Some(Value::Byte(_)))
}

fn byte_or_int(was_byte: bool, v: i32) -> Value {
    if was_byte { Value::Byte(v as u8) } else { Value::Int(v) }
}

/// Text form of a float as the original prints it: msvcrt's `%f` of the value widened to double. msvcrt makes 17
/// significant digits (correctly rounded), rounds that digit string half up at the sixth decimal and pads with zeros
/// beyond it; infinities and NaNs print as `1.#INF00`, `1.#QNAN0` and `-1.#IND00` (the x87's default NaN).
pub fn float_str(f: f32) -> String {
    crate::crt::fixed(f as f64, 6)
}

fn vec_mix(a: [f32; 3], b: [f32; 3], f: impl Fn(f32, f32) -> f32) -> [f32; 3] {
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

fn rot_mix(a: [i32; 3], b: [i32; 3], f: impl Fn(i32, i32) -> i32) -> [i32; 3] {
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

impl Vm {
    /// Text form of a value as `GetPropertyText` returns it.
    pub fn value_text(&self, v: &Value) -> String {
        match v {
            Value::Byte(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Bool(b) => if *b { "True" } else { "False" }.into(),
            Value::Float(f) => float_str(*f),
            Value::Str(s) => s.clone(),
            Value::Name(n) => self.reg.names.str(*n).to_string(),
            Value::Obj(r) => self.obj_to_string(*r),
            Value::Vector(x) => format!("(X={},Y={},Z={})", float_str(x[0]), float_str(x[1]), float_str(x[2])),
            Value::Rotator(x) => format!("(Pitch={},Yaw={},Roll={})", x[0], x[1], x[2]),
            Value::Void | Value::Struct(..) => String::new(),
        }
    }

    /// `appRand` = the MSVC C runtime `rand()`: a linear congruential generator, 15-bit results (0..32767). The
    /// engine seeds it from the clock at start-up; here the seed is fixed (and saved with the game).
    pub(crate) fn next_rand(&mut self) -> u32 {
        self.rng = self.rng.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.rng >> 16) & 0x7fff
    }

    /// `appFrand` = `rand() / 32767`: 0..1, both ends included.
    pub fn frand(&mut self) -> f32 {
        self.next_rand() as f32 * 3.051_850_9e-5
    }

    /// `ObjectToString`: "None", or the object's path name (`GetPathName`).
    pub fn obj_to_string(&self, r: u32) -> String {
        if r == 0 {
            return "None".into();
        }
        match ref_instance(r) {
            // `GetPathName`: an actor lives in the map package, a `new` object in the transient package
            Some(i) => match self.objects.get(i) {
                Some(o) => {
                    let name = self.reg.find_prop(o.class, "Name").and_then(|d| match self.read_prop(&o.mem, d, d.offset as usize) {
                        Ok(Value::Name(n)) if n != 0 => Some(self.reg.names.str(n).to_string()),
                        _ => None,
                    });
                    let name = name.unwrap_or_else(|| format!("{}{}", self.reg.names.str(self.reg.class(o.class).name), i));
                    // `GetPathName` (1014fc10): the outer's path, a dot, the name; objects the engine made without an
                    // `Outer` live in the map package (actors) or the transient package
                    let outer_field = self.reg.find_prop(o.class, "Outer").and_then(|d| match self.read_prop(&o.mem, d, d.offset as usize) {
                        Ok(Value::Obj(r)) if r != 0 && r != obj_self_guard(i) => Some(r),
                        _ => None,
                    });
                    let outer = match outer_field {
                        Some(r) => self.obj_to_string(r),
                        None if o.actor => self.level_paths.get(&self.level).and_then(|p| p.split('.').next()).unwrap_or("").to_string(),
                        None => "Transient".to_string(),
                    };
                    format!("{outer}.{name}")
                }
                None => "None".into(),
            },
            // `GetPathName`: the outers' names joined by dots ("Engine.Pawn")
            None => self.reg.obj_path(ue_registry::ObjId(r)).to_string(),
        }
    }
}

/// An object's own reference (an `Outer` pointing at itself would loop).
fn obj_self_guard(i: usize) -> u32 {
    crate::inst_ref(i)
}

/// Primitive conversion tokens (opcodes 0x39-0x5F). UNVERIFIED mapping for opcodes above 0x59.
/// MSVC `_wtoi` (`appAtoi`): leading white space, an optional sign, then decimal digits up to the first other
/// character; nothing numeric gives 0.
pub(crate) fn c_atoi(s: &str) -> i32 {
    let b = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']).as_bytes();
    let (neg, mut i) = match b.first() {
        Some(b'-') => (true, 1),
        Some(b'+') => (false, 1),
        _ => (false, 0),
    };
    // the system C runtime's `_wtol` saturates at the int range (probe `vm.cast.string.int.over`)
    let mut v: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        v = (v * 10 + (b[i] - b'0') as i64).min(1 << 40);
        i += 1;
    }
    let v = if neg { -v } else { v };
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// `appAtof` (msvcrt `atof`): the longest leading decimal number (sign, digits, point, digits, exponent with `e`, `E`,
/// `d` or `D`); 0 if none.
pub(crate) fn c_atof(s: &str) -> f32 {
    let t = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let b = t.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+') | Some(b'-')) {
        i = 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == digits_start || (i == digits_start + 1 && b[digits_start] == b'.') {
        return 0.0;
    }
    let mantissa_end = i;
    // msvcrt also takes `d`/`D` for the exponent
    if i < b.len() && matches!(b[i], b'e' | b'E' | b'd' | b'D') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let e0 = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > e0 {
            i = j;
        }
    }
    let end = if i > mantissa_end { i } else { mantissa_end };
    t[..end].replace(['d', 'D'], "e").parse::<f64>().map(|x| x as f32).unwrap_or(0.0)
}

/// The zero value of a conversion token's operand type.
fn cast_source_zero(op: u8) -> Value {
    match op {
        0x39 | 0x51 | 0x59 => Value::Rotator([0; 3]),
        0x3A..=0x3C | 0x52 => Value::Byte(0),
        0x3D..=0x3F | 0x53 => Value::Int(0),
        0x40..=0x42 | 0x54 => Value::Bool(false),
        0x43..=0x45 | 0x55 => Value::Float(0.0),
        0x46 | 0x49..=0x4E => Value::Str(String::new()),
        0x47 | 0x56 => Value::Obj(0),
        0x48 | 0x57 => Value::Name(0),
        0x4F | 0x50 | 0x58 => Value::Vector([0.0; 3]),
        _ => Value::Void,
    }
}

pub fn cast(vm: &mut Vm, op: u8, v: Value) -> Res<Value> {
    // a call or variable through None leaves a zeroed result: the zero of the cast's source type
    let v = if v == Value::Void { cast_source_zero(op) } else { v };
    let bad = || rt(format!("cast 0x{op:02X} applied to {v:?}"));
    Ok(match op {
        0x39 => {
            // rotator -> vector: the direction the rotator points along
            let r = match &v { Value::Rotator(r) => *r, Value::Void => [0; 3], _ => return Err(bad()) };
            // `FRotator::Vector`: the X axis of the rotation, from the engine's sine table
            Value::Vector(rotate_vector([1.0, 0.0, 0.0], r, false))
        }
        0x3A => Value::Int(v.as_i32().ok_or_else(bad)?),
        0x3B | 0x3E => Value::Bool(v.as_i32().ok_or_else(bad)? != 0),
        0x3C | 0x3F => Value::Float(v.as_i32().ok_or_else(bad)? as f32),
        0x3D | 0x40 => Value::Byte(v.as_i32().ok_or_else(bad)? as u8),
        0x41 => Value::Int(v.as_i32().ok_or_else(bad)?),
        0x42 => Value::Float(v.as_i32().ok_or_else(bad)? as f32),
        // `ftol` truncation (out of range: the "integer indefinite" 0x80000000)
        0x43 => Value::Byte(crate::route::ftol(v.as_f32().ok_or_else(bad)? as f64) as u8),
        0x44 => Value::Int(crate::route::ftol(v.as_f32().ok_or_else(bad)? as f64)),
        0x45 => Value::Bool(v.as_f32().ok_or_else(bad)? != 0.0),
        0x46 => match v {
            Value::Str(s) => Value::Name(vm.reg.names.intern(&s)),
            _ => return Err(bad()),
        },
        0x47 => Value::Bool(matches!(v, Value::Obj(r) if r != 0)),
        0x48 => Value::Bool(matches!(v, Value::Name(n) if n != 0)),
        // `appAtoi` / `appAtof` (C `atoi` / `atof`: the leading number, "12abc" is 12)
        0x49 => Value::Byte(match &v { Value::Str(s) => c_atoi(s) as u8, _ => return Err(bad()) }),
        0x4A => Value::Int(match &v { Value::Str(s) => c_atoi(s), _ => return Err(bad()) }),
        // "True"/"False" (also the localized words), else a number
        0x4B => Value::Bool(match &v {
            Value::Str(s) => {
                if s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case(&vm.localize("General", "True", "Core")) {
                    true
                } else if s.eq_ignore_ascii_case("false") || s.eq_ignore_ascii_case(&vm.localize("General", "False", "Core")) {
                    false
                } else {
                    c_atoi(s) != 0
                }
            }
            _ => return Err(bad()),
        }),
        0x4C => Value::Float(match &v { Value::Str(s) => c_atof(s), _ => return Err(bad()) }),
        // StringToVector / StringToRotator (10136940 / 10136b10): the first number, then one after each of the next two
        // commas (missing ones 0); nothing else is skipped (a "(X=..." text reads as zeros)
        0x4D | 0x4E => {
            let Value::Str(t) = &v else { return Err(bad()) };
            let mut parts = [t.as_str(), "", ""];
            if let Some((_, r)) = t.split_once(',') {
                parts[1] = r;
                if let Some((_, r2)) = r.split_once(',') {
                    parts[2] = r2;
                }
            }
            let has = [true, t.contains(','), t.matches(',').count() >= 2];
            if op == 0x4D {
                Value::Vector(std::array::from_fn(|k| if has[k] { c_atof(parts[k]) } else { 0.0 }))
            } else {
                Value::Rotator(std::array::from_fn(|k| if has[k] { c_atoi(parts[k]) } else { 0 }))
            }
        }
        0x4F => Value::Bool(match v { Value::Vector(x) => x != [0.0; 3], Value::Void => false, _ => return Err(bad()) }),
        0x50 => {
            // direction vector -> rotator (yaw around Z, pitch up); 65536 units per turn
            let d = match &v { Value::Vector(x) => *x, Value::Void => [0.0; 3], _ => return Err(bad()) };
            Value::Rotator(crate::ai_move::vec_to_rot(d))
        }
        0x51 => Value::Bool(match v { Value::Rotator(x) => x != [0; 3], Value::Void => false, _ => return Err(bad()) }),
        0x52 | 0x53 => Value::Str(v.as_i32().ok_or_else(bad)?.to_string()),
        0x54 => Value::Str(if v.as_bool().ok_or_else(bad)? { "True" } else { "False" }.into()),
        0x55 => Value::Str(float_str(v.as_f32().ok_or_else(bad)?)),
        0x56 => match v {
            Value::Obj(r) => Value::Str(vm.obj_to_string(r)),
            _ => return Err(bad()),
        },
        0x57 => match v {
            Value::Name(n) => Value::Str(vm.reg.names.str(n).to_string()),
            _ => return Err(bad()),
        },
        0x58 => match v {
            Value::Vector(x) => Value::Str(format!("{},{},{}", float_str(x[0]), float_str(x[1]), float_str(x[2]))),
            _ => return Err(bad()),
        },
        0x59 => match v {
            // `RotatorToString`: "%i,%i,%i" of the components masked to 16 bits
            Value::Rotator(x) => Value::Str(format!("{},{},{}", x[0] & 0xffff, x[1] & 0xffff, x[2] & 0xffff)),
            _ => return Err(bad()),
        },
        other => return Err(VmError::Unsupported(format!("cast opcode 0x{other:02X}"))),
    })
}

/// A native operator resolved at compile time (`ir::Op`), on its evaluated operands (`b` is void for unary ones): the
/// same arithmetic, conversions and errors as its arm in `call`.
pub(crate) fn op(op: crate::ir::Op, x: Value, y: Value) -> Res<Value> {
    use crate::ir::Op::*;
    use Value::*;
    let a = [x, y];
    let a = &a[..];
    Ok(match op {
        NotB => Bool(!boo(a, 0)?),
        EqB => Bool(boo(a, 0)? == boo(a, 1)?),
        NeB | XorB => Bool(boo(a, 0)? != boo(a, 1)?),
        AndB => Bool(boo(a, 0)? && boo(a, 1)?),
        OrB => Bool(boo(a, 0)? || boo(a, 1)?),
        NegI => Int(int(a, 0)?.wrapping_neg()),
        ComplI => Int(!int(a, 0)?),
        AddI => Int(int(a, 0)?.wrapping_add(int(a, 1)?)),
        SubI => Int(int(a, 0)?.wrapping_sub(int(a, 1)?)),
        MulI => Int(int(a, 0)?.wrapping_mul(int(a, 1)?)),
        LtI => Bool(int(a, 0)? < int(a, 1)?),
        GtI => Bool(int(a, 0)? > int(a, 1)?),
        LeI => Bool(int(a, 0)? <= int(a, 1)?),
        GeI => Bool(int(a, 0)? >= int(a, 1)?),
        EqI => Bool(int(a, 0)? == int(a, 1)?),
        NeI => Bool(int(a, 0)? != int(a, 1)?),
        AndI => Int(int(a, 0)? & int(a, 1)?),
        XorI => Int(int(a, 0)? ^ int(a, 1)?),
        OrI => Int(int(a, 0)? | int(a, 1)?),
        NegF => Float(-flt(a, 0)?),
        AddF => Float(flt(a, 0)? + flt(a, 1)?),
        SubF => Float(flt(a, 0)? - flt(a, 1)?),
        MulF => Float(flt(a, 0)? * flt(a, 1)?),
        LtF => Bool(flt(a, 0)? < flt(a, 1)?),
        GtF => Bool(flt(a, 0)? > flt(a, 1)?),
        LeF => Bool(flt(a, 0)? <= flt(a, 1)?),
        GeF => Bool(flt(a, 0)? >= flt(a, 1)?),
        EqF => Bool(flt(a, 0)? == flt(a, 1)?),
        NeF => Bool(flt(a, 0)? != flt(a, 1)?),
        EqN | EqO => Bool(a.first() == a.get(1)),
        NeN | NeO => Bool(a.first() != a.get(1)),
    })
}

/// Calls a native function by name. `args` holds evaluated arguments; out parameters are updated in place.
pub fn call(vm: &mut Vm, this: u32, name: &str, a: &mut Vec<Value>) -> Res<Value> {
    use Value::*;
    let was_byte = is_byte(a, 0);
    Ok(match name {
        // ---- bool
        "Not_PreBool" => Bool(!boo(a, 0)?),
        "EqualEqual_BoolBool" => Bool(boo(a, 0)? == boo(a, 1)?),
        "NotEqual_BoolBool" => Bool(boo(a, 0)? != boo(a, 1)?),
        "XorXor_BoolBool" => Bool(boo(a, 0)? != boo(a, 1)?),
        "AndAnd_BoolBool" => Bool(boo(a, 0)? && boo(a, 1)?),
        "OrOr_BoolBool" => Bool(boo(a, 0)? || boo(a, 1)?),

        // ---- int
        "Complement_PreInt" => Int(!int(a, 0)?),
        "Subtract_PreInt" => Int(int(a, 0)?.wrapping_neg()),
        "Multiply_IntInt" => Int(int(a, 0)?.wrapping_mul(int(a, 1)?)),
        "Divide_IntInt" => {
            let d = int(a, 1)?;
            if d == 0 {
                vm.log.push("Divide by zero".into());
                Int(0)
            } else {
                Int(int(a, 0)?.wrapping_div(d))
            }
        }
        "Add_IntInt" => Int(int(a, 0)?.wrapping_add(int(a, 1)?)),
        "Subtract_IntInt" => Int(int(a, 0)?.wrapping_sub(int(a, 1)?)),
        "LessLess_IntInt" => Int(int(a, 0)?.wrapping_shl(int(a, 1)? as u32)),
        "GreaterGreater_IntInt" => Int(int(a, 0)?.wrapping_shr(int(a, 1)? as u32)),
        "GreaterGreaterGreater_IntInt" => Int(((int(a, 0)? as u32).wrapping_shr(int(a, 1)? as u32)) as i32),
        "Less_IntInt" => Bool(int(a, 0)? < int(a, 1)?),
        "Greater_IntInt" => Bool(int(a, 0)? > int(a, 1)?),
        "LessEqual_IntInt" => Bool(int(a, 0)? <= int(a, 1)?),
        "GreaterEqual_IntInt" => Bool(int(a, 0)? >= int(a, 1)?),
        "EqualEqual_IntInt" => Bool(int(a, 0)? == int(a, 1)?),
        "NotEqual_IntInt" => Bool(int(a, 0)? != int(a, 1)?),
        "And_IntInt" => Int(int(a, 0)? & int(a, 1)?),
        "Xor_IntInt" => Int(int(a, 0)? ^ int(a, 1)?),
        "Or_IntInt" => Int(int(a, 0)? | int(a, 1)?),
        "MultiplyEqual_IntFloat" => {
            let r = (int(a, 0)? as f32 * flt(a, 1)?) as i32;
            a[0] = Int(r);
            Int(r)
        }
        "DivideEqual_IntFloat" => {
            let d = flt(a, 1)?;
            let r = if d == 0.0 { 0 } else { (int(a, 0)? as f32 / d) as i32 };
            a[0] = Int(r);
            Int(r)
        }
        "AddEqual_IntInt" | "SubtractEqual_IntInt" | "MultiplyEqual_ByteByte" | "DivideEqual_ByteByte" | "AddEqual_ByteByte" | "SubtractEqual_ByteByte" => {
            let (x, y) = (int(a, 0)?, int(a, 1)?);
            let r = if name.starts_with("Add") {
                x.wrapping_add(y)
            } else if name.starts_with("Subtract") {
                x.wrapping_sub(y)
            } else if name.starts_with("Multiply") {
                x.wrapping_mul(y)
            } else if y == 0 {
                0
            } else {
                x / y
            };
            a[0] = byte_or_int(was_byte, r);
            byte_or_int(was_byte, r)
        }
        "AddAdd_PreInt" | "AddAdd_PreByte" => {
            let r = int(a, 0)?.wrapping_add(1);
            a[0] = byte_or_int(was_byte, r);
            byte_or_int(was_byte, r)
        }
        "SubtractSubtract_PreInt" | "SubtractSubtract_PreByte" => {
            let r = int(a, 0)?.wrapping_sub(1);
            a[0] = byte_or_int(was_byte, r);
            byte_or_int(was_byte, r)
        }
        "AddAdd_Int" | "AddAdd_Byte" => {
            let old = byte_or_int(was_byte, int(a, 0)?);
            a[0] = byte_or_int(was_byte, int(a, 0)?.wrapping_add(1));
            old
        }
        "SubtractSubtract_Int" | "SubtractSubtract_Byte" => {
            let old = byte_or_int(was_byte, int(a, 0)?);
            a[0] = byte_or_int(was_byte, int(a, 0)?.wrapping_sub(1));
            old
        }
        "Rand" => {
            let m = int(a, 0)?;
            Int(if m <= 0 { 0 } else { vm.next_rand() as i32 % m })
        }
        "Min" => Int(int(a, 0)?.min(int(a, 1)?)),
        "Max" => Int(int(a, 0)?.max(int(a, 1)?)),
        "Clamp" => Int(int(a, 0)?.max(int(a, 1)?).min(int(a, 2)?)),

        // ---- float
        "Subtract_PreFloat" => Float(-flt(a, 0)?),
        // the C library in double precision (`appPow`, `appSin`, ...), stored as float
        "MultiplyMultiply_FloatFloat" => Float((flt(a, 0)? as f64).powf(flt(a, 1)? as f64) as f32),
        "Multiply_FloatFloat" => Float(flt(a, 0)? * flt(a, 1)?),
        "Divide_FloatFloat" => {
            let d = flt(a, 1)?;
            if d == 0.0 {
                vm.log.push("Divide by zero".into());
            }
            Float(flt(a, 0)? / d)
        }
        "Percent_FloatFloat" => Float(flt(a, 0)? % flt(a, 1)?),
        "Add_FloatFloat" => Float(flt(a, 0)? + flt(a, 1)?),
        "Subtract_FloatFloat" => Float(flt(a, 0)? - flt(a, 1)?),
        "Less_FloatFloat" => Bool(flt(a, 0)? < flt(a, 1)?),
        "Greater_FloatFloat" => Bool(flt(a, 0)? > flt(a, 1)?),
        "LessEqual_FloatFloat" => Bool(flt(a, 0)? <= flt(a, 1)?),
        "GreaterEqual_FloatFloat" => Bool(flt(a, 0)? >= flt(a, 1)?),
        "EqualEqual_FloatFloat" => Bool(flt(a, 0)? == flt(a, 1)?),
        "NotEqual_FloatFloat" => Bool(flt(a, 0)? != flt(a, 1)?),
        // the difference in a register, compared with the double 1e-4 (101357e0)
        "ComplementEqual_FloatFloat" => Bool((flt(a, 0)? as f64 - flt(a, 1)? as f64).abs() < 0.0001),
        "MultiplyEqual_FloatFloat" | "DivideEqual_FloatFloat" | "AddEqual_FloatFloat" | "SubtractEqual_FloatFloat" => {
            let (x, y) = (flt(a, 0)?, flt(a, 1)?);
            let r = if name.starts_with("Multiply") {
                x * y
            } else if name.starts_with("Divide") {
                x / y
            } else if name.starts_with("Add") {
                x + y
            } else {
                x - y
            };
            a[0] = Float(r);
            Float(r)
        }
        "Abs" => Float(flt(a, 0)?.abs()),
        "Sin" => Float((flt(a, 0)? as f64).sin() as f32),
        "Cos" => Float((flt(a, 0)? as f64).cos() as f32),
        "Tan" => Float((flt(a, 0)? as f64).tan() as f32),
        "Atan" => Float((flt(a, 0)? as f64).atan() as f32),
        "Exp" => Float((flt(a, 0)? as f64).exp() as f32),
        "Loge" => Float((flt(a, 0)? as f64).ln() as f32),
        "Sqrt" => Float((flt(a, 0)? as f64).sqrt() as f32),
        "Square" => Float(flt(a, 0)? * flt(a, 0)?),
        "FRand" => Float(vm.frand()),
        // the local player: the first live PlayerPawn, or none (the original warns and returns none as well)
        "GetPlayerPawn" => {
            let o = vm.offsets();
            let cached = vm.player_cache;
            if cached != 0 && vm.is_live(cached) && vm.is_instance_of(cached, o.class_player_pawn) {
                return Ok(Obj(cached));
            }
            let found = (0..vm.objects.len()).map(crate::inst_ref).find(|&r| vm.is_live(r) && vm.is_instance_of(r, o.class_player_pawn)).unwrap_or(0);
            vm.player_cache = found;
            Obj(found)
        }
        "FMin" => Float(flt(a, 0)?.min(flt(a, 1)?)),
        "FMax" => Float(flt(a, 0)?.max(flt(a, 1)?)),
        "FClamp" => Float(flt(a, 0)?.max(flt(a, 1)?).min(flt(a, 2)?)),
        // `(B - A) * Alpha + A` in a register (10136610)
        "Lerp" => Float(((flt(a, 2)? as f64 - flt(a, 1)? as f64) * flt(a, 0)? as f64 + flt(a, 1)? as f64) as f32),
        "Smerp" => {
            // `(B - A) * (a^2 * 3 - (a^3 + a^3)) + A`, all in registers (10136710)
            let al = flt(a, 0)? as f64;
            let (lo, hi) = (flt(a, 1)? as f64, flt(a, 2)? as f64);
            let a2 = al * al;
            let a3 = al * a2;
            Float(((hi - lo) * (a2 * 3.0 - (a3 + a3)) + lo) as f32)
        }

        // ---- vector
        "Subtract_PreVector" => {
            let v = vecv(a, 0)?;
            Vector([-v[0], -v[1], -v[2]])
        }
        "Multiply_VectorFloat" => {
            let (v, s) = (vecv(a, 0)?, flt(a, 1)?);
            Vector([v[0] * s, v[1] * s, v[2] * s])
        }
        "Multiply_FloatVector" => {
            let (s, v) = (flt(a, 0)?, vecv(a, 1)?);
            Vector([v[0] * s, v[1] * s, v[2] * s])
        }
        "Divide_VectorFloat" => {
            // the engine multiplies by the (float) reciprocal
            let (v, s) = (vecv(a, 0)?, flt(a, 1)?);
            let r = 1.0 / s;
            Vector([v[0] * r, v[1] * r, v[2] * r])
        }
        "Add_VectorVector" => Vector(vec_mix(vecv(a, 0)?, vecv(a, 1)?, |x, y| x + y)),
        "Subtract_VectorVector" => Vector(vec_mix(vecv(a, 0)?, vecv(a, 1)?, |x, y| x - y)),
        "Multiply_VectorVector" => Vector(vec_mix(vecv(a, 0)?, vecv(a, 1)?, |x, y| x * y)),
        "EqualEqual_VectorVector" => Bool(vecv(a, 0)? == vecv(a, 1)?),
        "NotEqual_VectorVector" => Bool(vecv(a, 0)? != vecv(a, 1)?),
        "Dot_VectorVector" => {
            let (x, y) = (vecv(a, 0)?, vecv(a, 1)?);
            Float(x[0] * y[0] + x[1] * y[1] + x[2] * y[2])
        }
        "Cross_VectorVector" => {
            let (x, y) = (vecv(a, 0)?, vecv(a, 1)?);
            Vector([x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]])
        }
        "MultiplyEqual_VectorFloat" | "DivideEqual_VectorFloat" | "AddEqual_VectorVector" | "SubtractEqual_VectorVector" | "MultiplyEqual_VectorVector" => {
            let v = vecv(a, 0)?;
            let r = match name {
                "MultiplyEqual_VectorFloat" => { let s = flt(a, 1)?; [v[0] * s, v[1] * s, v[2] * s] }
                "DivideEqual_VectorFloat" => { let r = 1.0 / flt(a, 1)?; [v[0] * r, v[1] * r, v[2] * r] }
                "AddEqual_VectorVector" => vec_mix(v, vecv(a, 1)?, |x, y| x + y),
                "SubtractEqual_VectorVector" => vec_mix(v, vecv(a, 1)?, |x, y| x - y),
                _ => vec_mix(v, vecv(a, 1)?, |x, y| x * y),
            };
            a[0] = Vector(r);
            Vector(r)
        }
        "VSize" => {
            let v = vecv(a, 0)?;
            Float((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt())
        }
        // `Invert(out X, out Y, out Z)`: the inverse (transpose) of the axis system
        "Invert" => {
            let (x, y, z) = (vecv(a, 0)?, vecv(a, 1)?, vecv(a, 2)?);
            a[0] = Vector([x[0], y[0], z[0]]);
            a[1] = Vector([x[1], y[1], z[1]]);
            a[2] = Vector([x[2], y[2], z[2]]);
            Void
        }
        // `DeusExPlayer.SetBoolFlagFromString(name, value)` (DeusEx 100144e0): `FlagBase.SetBool(name, value, bAdd = true,
        // expiration -1)`; returns the name
        "SetBoolFlagFromString" => {
            let n = strv(a, 0)?;
            let v = matches!(a.get(1), Some(Bool(true)));
            let name = vm.reg.names.intern(&n);
            let fb = vm.op(this, "FlagBase");
            if vm.is_live(fb) {
                vm.call_by_name(fb, "SetBool", vec![Name(name), Bool(v), Bool(true), Int(-1)])?;
            }
            Name(name)
        }
        // `UpdateURL(option, value, bSaveDefault)`: the option goes into the current URL; with bSaveDefault also into
        // User.ini [DefaultPlayer] (`FURL::SaveURLConfig`)
        "UpdateURL" => {
            let (opt, val) = (strv(a, 0)?, strv(a, 1)?);
            vm.url_options.retain(|(k, _)| !k.eq_ignore_ascii_case(&opt));
            vm.url_options.push((opt.clone(), val.clone()));
            if matches!(a.get(2), Some(Bool(true))) {
                if let Some(c) = vm.reg.config.as_mut() {
                    c.set("User", "DefaultPlayer", &opt, &val);
                }
                vm.reg.flush_config();
            }
            Void
        }
        // `GetDefaultURL(option)`: the option's value from User.ini [DefaultPlayer] (`FURL::LoadURLConfig`), or ""
        "GetDefaultURL" => {
            let opt = strv(a, 0)?;
            Str(vm.reg.config_value("User", "DefaultPlayer", &opt).map(|v| ue_config::unquote(&v)).unwrap_or_default())
        }
        // `GetURLMap()`: the map of the level's URL
        "GetURLMap" => {
            let path = if vm.level != 0 { vm.reg.obj_path(ue_registry::ObjId(vm.level)).to_string() } else { String::new() };
            Str(path.split('.').next().unwrap_or("").to_string())
        }
        "Normal" => {
            // `FVector::SafeNormal` (10138de0)
            Vector(ue_bsp::safe_normal(vecv(a, 0)?))
        }
        "VRand" => {
            // 10139080: x, y, z drawn as `2 FRand - 1` until the squared length is at most 1, then scaled by
            // `1 / sqrt` in a register
            loop {
                let v = [vm.frand() * 2.0 - 1.0, vm.frand() * 2.0 - 1.0, vm.frand() * 2.0 - 1.0];
                let l = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
                if l <= 1.0 {
                    let k = 1.0 / (l as f64).sqrt();
                    break Vector(v.map(|c| (k * c as f64) as f32));
                }
            }
        }

        // ---- rotator
        "EqualEqual_RotatorRotator" | "NotEqual_RotatorRotator" => {
            let (x, y) = (rotv(a, 0)?, rotv(a, 1)?);
            // raw components (no 16-bit masking in the engine)
            let eq = x == y;
            Bool(eq == (name == "EqualEqual_RotatorRotator"))
        }
        // `ftol` of the product (x87 precision)
        "Multiply_RotatorFloat" => { let (r, s) = (rotv(a, 0)?, flt(a, 1)?); Rotator(r.map(|x| crate::route::ftol(x as f64 * s as f64))) }
        "Multiply_FloatRotator" => { let (s, r) = (flt(a, 0)?, rotv(a, 1)?); Rotator(r.map(|x| crate::route::ftol(x as f64 * s as f64))) }
        "Divide_RotatorFloat" => { let (r, s) = (rotv(a, 0)?, flt(a, 1)?); Rotator(r.map(|x| crate::route::ftol(x as f64 / s as f64))) }
        // `Normalize(rotator)`: each component folded into -32768..32767
        "Normalize" => {
            let r = rotv(a, 0)?;
            Rotator(r.map(|x| { let m = x & 0xffff; if m > 0x7fff { m - 0x10000 } else { m } }))
        }
        "Add_RotatorRotator" => Rotator(rot_mix(rotv(a, 0)?, rotv(a, 1)?, |x, y| x.wrapping_add(y))),
        "Subtract_RotatorRotator" => Rotator(rot_mix(rotv(a, 0)?, rotv(a, 1)?, |x, y| x.wrapping_sub(y))),
        "MultiplyEqual_RotatorFloat" | "DivideEqual_RotatorFloat" | "AddEqual_RotatorRotator" | "SubtractEqual_RotatorRotator" => {
            let r = rotv(a, 0)?;
            let out = match name {
                "MultiplyEqual_RotatorFloat" => { let s = flt(a, 1)?; r.map(|x| crate::route::ftol(x as f64 * s as f64)) }
                "DivideEqual_RotatorFloat" => { let s = flt(a, 1)?; r.map(|x| crate::route::ftol(x as f64 / s as f64)) }
                "AddEqual_RotatorRotator" => rot_mix(r, rotv(a, 1)?, |x, y| x.wrapping_add(y)),
                _ => rot_mix(r, rotv(a, 1)?, |x, y| x.wrapping_sub(y)),
            };
            a[0] = Rotator(out);
            Rotator(out)
        }

        // ---- name / object
        "EqualEqual_NameName" => Bool(a.first() == a.get(1)),
        "NotEqual_NameName" => Bool(a.first() != a.get(1)),
        "EqualEqual_ObjectObject" => Bool(a.first() == a.get(1)),
        "NotEqual_ObjectObject" => Bool(a.first() != a.get(1)),
        "IsA" => {
            let n = match a.first() { Some(Name(n)) => *n, _ => return Err(rt("IsA expects a name")) };
            let mut cur = vm.class_of_ref(this);
            let mut found = false;
            while let Some(c) = cur {
                if vm.reg.class(c).name == n {
                    found = true;
                    break;
                }
                cur = vm.reg.class(c).super_id;
            }
            Bool(found)
        }
        "ClassIsChildOf" => {
            match (a.first(), a.get(1)) {
                (Some(Obj(x)), Some(Obj(y))) => match (vm.class_of_ref(*x), vm.class_of_ref(*y)) {
                    (Some(c), Some(p)) => Bool(vm.is_a(c, p)),
                    _ => Bool(false),
                },
                _ => Bool(false),
            }
        }

        // ---- string
        "Concat_StrStr" => Str(strv(a, 0)? + &strv(a, 1)?),
        "At_StrStr" => Str(format!("{} {}", strv(a, 0)?, strv(a, 1)?)),
        "Less_StrStr" => Bool(strv(a, 0)? < strv(a, 1)?),
        "Greater_StrStr" => Bool(strv(a, 0)? > strv(a, 1)?),
        "LessEqual_StrStr" => Bool(strv(a, 0)? <= strv(a, 1)?),
        "GreaterEqual_StrStr" => Bool(strv(a, 0)? >= strv(a, 1)?),
        "EqualEqual_StrStr" => Bool(strv(a, 0)? == strv(a, 1)?),
        "NotEqual_StrStr" => Bool(strv(a, 0)? != strv(a, 1)?),
        "ComplementEqual_StrStr" => Bool(strv(a, 0)?.eq_ignore_ascii_case(&strv(a, 1)?)),
        "Len" => Int(strv(a, 0)?.chars().count() as i32),
        "InStr" => {
            let (s, t) = (strv(a, 0)?, strv(a, 1)?);
            Int(s.find(&t).map(|b| s[..b].chars().count() as i32).unwrap_or(-1))
        }
        "Mid" => {
            // FString::Mid: the end is start + count (count defaults to 65535), then both are clamped to the string
            // (FString::Mid 101086e0: unsigned comparisons; a negative start lies past the end, a negative end before
            // the start, which then keeps the rest of the string)
            let s: Vec<char> = strv(a, 0)?.chars().collect();
            let i = int(a, 1)? as u32;
            let n = if matches!(a.get(2), Some(Void) | None) { 65535 } else { int(a, 2)? as u32 };
            let len = s.len() as u32;
            let start = i.min(len);
            let end = i.wrapping_add(n);
            let end = if end < start { start } else { end.min(len) };
            Str(s[start as usize..end as usize].iter().collect())
        }
        "Left" => Str(strv(a, 0)?.chars().take(int(a, 1)?.max(0) as usize).collect()),
        "Right" => {
            let s: Vec<char> = strv(a, 0)?.chars().collect();
            let n = int(a, 1)?.max(0) as usize;
            Str(s[s.len().saturating_sub(n)..].iter().collect())
        }
        // appToUpper: ASCII letters only
        "Caps" => Str(strv(a, 0)?.to_ascii_uppercase()),
        // a 16-bit character; 0 gives the empty string
        "Chr" => Str(match int(a, 0)? as u32 & 0xFFFF { 0 => String::new(), c => char::from_u32(c).map(String::from).unwrap_or_default() }),
        "Asc" => Int(strv(a, 0)?.chars().next().map(|c| c as i32).unwrap_or(0)),

        // ---- states and events (act on the calling object)
        // exec 1013dd00: an omitted state is the current one; going to the current state only jumps to the label
        "GotoState" => {
            let as_name = |v: Option<&Value>| match v { Some(Name(n)) if *n != 0 => Some(*n), _ => None };
            let state = match a.first() {
                None | Some(Void) => vm.state_name(this).map(|n| n.to_string()).map(|n| vm.reg.names.intern(&n)),
                other => as_name(other),
            };
            vm.goto_state(this, state, as_name(a.get(1)))?;
            Void
        }
        // ---- pawn AI (see ai_move.rs)
        "IsValidEnemy" => {
            let other = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            // bCheckAlliance defaults to true in the exec wrapper
            let check = match a.get(1) { None | Some(Void) => true, Some(v) => v.as_bool().unwrap_or(true) };
            Bool(vm.is_valid_enemy(this, other, check))
        }
        "GetAllianceType" => {
            let alliance = match a.first() { Some(Name(n)) => *n, _ => 0 };
            Byte(vm.alliance_type(this, alliance) as u8)
        }
        "GetPawnAllianceType" => {
            let other = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            Byte(vm.pawn_alliance_type(this, other) as u8)
        }
        "TurnTo" => {
            vm.native_turn_to(this, vecv(a, 0)?);
            Void
        }
        "TurnToward" => {
            let t = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            vm.native_turn_toward(this, t);
            Void
        }
        "MoveTo" => {
            let speed = a.get(1).and_then(Value::as_f32).filter(|_| !matches!(a.get(1), Some(Void))).unwrap_or(1.0);
            vm.native_move_to(this, vecv(a, 0)?, speed)?;
            Void
        }
        "StrafeTo" => {
            let speed = a.get(2).and_then(Value::as_f32).filter(|_| !matches!(a.get(2), Some(Void))).unwrap_or(1.0);
            vm.native_strafe_to(this, vecv(a, 0)?, vecv(a, 1)?, speed)?;
            Void
        }
        "StrafeFacing" => {
            let t = match a.get(1) { Some(Obj(r)) => *r, _ => 0 };
            let speed = a.get(2).and_then(Value::as_f32).filter(|_| !matches!(a.get(2), Some(Void))).unwrap_or(1.0);
            vm.native_strafe_facing(this, vecv(a, 0)?, t, speed)?;
            Void
        }
        "MoveToward" => {
            let t = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            let speed = a.get(1).and_then(Value::as_f32).filter(|_| !matches!(a.get(1), Some(Void))).unwrap_or(1.0);
            vm.native_move_toward(this, t, speed)?;
            Void
        }
        // `ParabolicTrace(out finalLocation, startVelocity, startLocation, bCheckActors, Cylinder, maxTime, elasticity, bBounce,
        // landingSpeed, granularity)`; the exec wrapper's defaults: maxTime 5, elasticity 0.9, landing speed 60, step 0.025
        "ParabolicTrace" => {
            let f = |i: usize, d: f32| match a.get(i) { None | Some(Void) => d, Some(v) => v.as_f32().unwrap_or(d) };
            let b = |i: usize| matches!(a.get(i), Some(Bool(true)));
            // start velocity and location default to the actor's own
            let start_vel = match a.get(1) { Some(Vector(v)) => *v, _ => { let off = vm.offsets().velocity; vm.vec_at(this, off) } };
            let start_loc = match a.get(2) { Some(Vector(v)) => *v, _ => vm.location(this) };
            let (t, end) = vm.parabolic_trace(this, start_loc, start_vel, f(5, 5.0), f(6, 0.9), b(7), f(8, 60.0), vecv(a, 4)?, b(3), f(9, 0.025))?;
            a[0] = Vector(end);
            Float(t)
        }
        "UpdateParticles" => {
            vm.update_particles(this, flt(a, 0)?);
            Void
        }
        "AIDirectionReachable" => {
            // the out vector is the script's own variable: left alone when nothing is tested
            let mut best = match a.get(5) { Some(Vector(v)) => *v, _ => [0.0; 3] };
            let r = vm.ai_direction_reachable(this, vecv(a, 0)?, a[1].as_i32().unwrap_or(0), a[2].as_i32().unwrap_or(0), flt(a, 3)?, flt(a, 4)?, &mut best)?;
            a[5] = Vector(best);
            Bool(r)
        }
        "AIPickRandomDestination" => {
            let mut dest = [0.0; 3];
            let g = |i: usize| a.get(i).and_then(Value::as_i32).unwrap_or(0);
            let r = vm.ai_pick_random_destination(this, flt(a, 0)?, flt(a, 1)?, g(2), flt(a, 3)?, g(4), flt(a, 5)?, g(6), flt(a, 7)?, &mut dest)?;
            a[8] = Vector(dest);
            Bool(r)
        }
        "AIGetLightLevel" => Float(vm.ai_light_level(this, vecv(a, 0)?)),
        "AIVisibility" => Float(vm.ai_visibility(this, match a.first() { None | Some(Void) => true, Some(v) => v.as_bool().unwrap_or(true) })),
        "AICanSee" => {
            let other = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            // omitted arguments take the exec wrapper's defaults: visibility 1, check visibility/direction/LOS on, cylinder off
            let b = |i: usize, default: bool| match a.get(i) { None | Some(Void) => default, Some(v) => v.as_bool().unwrap_or(default) };
            let vis = match a.get(1) { None | Some(Void) => 1.0, Some(v) => v.as_f32().unwrap_or(1.0) };
            Float(vm.ai_can_see(this, other, vis, b(2, true), b(3, true), b(4, false), b(5, true))?)
        }
        "LineOfSightTo" => {
            let other = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            Bool(vm.line_of_sight_to(this, other, false, matches!(a.get(1), Some(Bool(true)))))
        }
        "CanSee" => {
            let other = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            Bool(vm.line_of_sight_to(this, other, true, false))
        }
        "FindStairRotation" => Int(vm.find_stair_rotation(this, flt(a, 0)?)?),
        "PickTarget" | "PickAnyTarget" => {
            let pick = if name == "PickTarget" { Vm::pick_target } else { Vm::pick_any_target };
            let (t, aim, dist) = pick(vm, this, flt(a, 0)?, flt(a, 1)?, vecv(a, 2)?, vecv(a, 3)?);
            a[0] = Float(aim);
            a[1] = Float(dist);
            Obj(t)
        }
        "RandomBiasedRotation" => {
            let g = |i: usize| a.get(i).and_then(Value::as_i32).unwrap_or(0);
            Rotator(vm.random_biased_rotation(g(0), flt(a, 1)?, g(2), flt(a, 3)?))
        }
        "pointReachable" => Bool(vm.point_reachable(this, vecv(a, 0)?)?),
        "actorReachable" => {
            let g = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            Bool(vm.actor_reachable(this, g)?)
        }
        // optional bSinglePath (default false), bClearPaths (default true)
        "FindPathTo" | "FindPathToward" => {
            let single = matches!(a.get(1), Some(Bool(true)));
            let clear = !matches!(a.get(2), Some(Bool(false)));
            if name == "FindPathTo" {
                Obj(vm.native_find_path_to(this, vecv(a, 0)?, single, clear, None)?)
            } else {
                let g = match a.first() { Some(Obj(r)) if vm.is_live(*r) => *r, _ => 0 };
                Obj(vm.native_find_path_to(this, [0.0; 3], single, clear, Some(g))?)
            }
        }
        "ClearPaths" => {
            vm.clear_paths(this)?;
            Void
        }
        // `APawn::findRandomDest` in this build returns nothing whenever the level has navigation points (its check is
        // inverted) and has nothing to pick otherwise: the native only clears the paths (optional bClearPaths, default
        // true) and returns None
        "FindRandomDest" => {
            if !matches!(a.first(), Some(Bool(false))) {
                vm.clear_paths(this)?;
            }
            Obj(0)
        }
        // do the two collision cylinders overlap? (brush actors and the level never do)
        "IsOverlapping" => {
            let other = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            let o = vm.offsets();
            if !vm.is_live(other) || other == vm.level || vm.is_brush_actor(this) || vm.is_brush_actor(other) {
                Bool(false)
            } else {
                let (l1, l2) = (vm.location(this), vm.location(other));
                let ((r1, h1), (r2, h2)) = (vm.cylinder(this), vm.cylinder(other));
                let _ = o;
                Bool((l1[0] - l2[0]).powi(2) + (l1[1] - l2[1]).powi(2) < (r1 + r2).powi(2) && (l1[2] - l2[2]).powi(2) < (h1 + h2).powi(2))
            }
        }
        // no viewport yet: nobody is looking (the original asks the player's camera)
        "PlayerCanSeeMe" => Bool(vm.player_can_see_me(this)),
        // `APawn::execEAdjustJump` (10302554): the pawn's velocity with the horizontal part `SuggestJumpVelocity`
        // picks for its `Destination`
        "EAdjustJump" => {
            let vel = { let off = vm.offsets().velocity; vm.vec_at(this, off) };
            let dest = vm.vp(this, "Destination");
            Vector(vm.suggest_jump_velocity(this, dest, vel))
        }
        "WaitForLanding" => {
            vm.set_fp(this, "LatentFloat", 2.5);
            if vm.u8p(this, "Physics") == crate::physics::PHYS_FALLING {
                vm.pending_latent = Some(crate::Latent::External("WaitForLanding".into()));
            }
            Void
        }
        // the state code waits for the mover to arrive (polled every frame; it always yields once, like the engine)
        "FinishInterpolation" => {
            vm.pending_latent = Some(crate::Latent::External("FinishInterpolation".into()));
            Void
        }
        "MakeNoise" => {
            vm.make_noise(this, flt(a, 0)?)?;
            Void
        }
        // time since the actor was last drawn; nothing is drawn yet, so LastRenderTime stays 0
        "LastRendered" => {
            let last = vm.fp(this, "LastRenderTime");
            Float((vm.time - last).max(0.0))
        }
        "Sleep" => {
            vm.pending_latent = Some(crate::Latent::Sleep(flt(a, 0)?));
            Void
        }
        // ends a Sleep the object's state code waits in (APawn::execStopWaiting)
        "StopWaiting" => {
            if let Some(i) = ref_instance(this) {
                if matches!(vm.objects[i].latent, Some(crate::Latent::Sleep(_))) {
                    vm.objects[i].latent = Some(crate::Latent::Sleep(-1.0));
                }
            }
            Void
        }
        // Vect reflected at the plane of Normal: V - 2 N (V . N), N normalised
        "MirrorVectorByNormal" => {
            // 101392a0: `A - 2 n (A . n)` with `n = SafeNormal(B)`
            let v = vecv(a, 0)?;
            let n = ue_bsp::safe_normal(vecv(a, 1)?);
            let d = v[0] * n[0] + v[1] * n[1] + v[2] * n[2];
            Vector([v[0] - 2.0 * n[0] * d, v[1] - 2.0 * n[1] * d, v[2] - 2.0 * n[2] * d])
        }
        // the carcasses a ScriptedPawn has seen: up to 4 names (AScriptedPawn::AddCarcass / HaveSeenCarcass)
        "HaveSeenCarcass" | "AddCarcass" => {
            let carcass = match a.first() { Some(Name(n)) => *n, _ => 0 };
            let count = vm.get_prop(this, "NumCarcasses", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0).clamp(0, 4);
            let seen = (0..count).any(|k| matches!(vm.get_prop(this, "Carcasses", k as u32), Ok(Name(n)) if n == carcass));
            if name == "AddCarcass" {
                if count < 4 && !seen {
                    vm.set_prop(this, "Carcasses", count as u32, Name(carcass))?;
                    vm.set_prop(this, "NumCarcasses", 0, Int(count + 1))?;
                }
                Void
            } else {
                Bool(seen)
            }
        }
        // 1013de80 / 1013dfb0: only the probe names (Core's 300..363) have a bit; others are warned about and left
        "Enable" | "Disable" => {
            let probe = match a.first() {
                Some(Name(n)) => crate::PROBE_NAMES.iter().any(|p| p.eq_ignore_ascii_case(vm.reg.names.str(*n))),
                _ => false,
            };
            if !probe {
                vm.log.push(format!("{name}: not a probe function"));
            } else if let (Some(Name(n)), Some(i)) = (a.first(), ref_instance(this)) {
                let o = &mut vm.objects[i];
                if name == "Disable" {
                    o.disabled.insert(*n);
                } else {
                    o.disabled.remove(n);
                }
            }
            Void
        }
        // 1013edb0: the name of `StateFrame.StateNode`, which is the class when the object is in no state
        "GetStateName" => match vm.state_name(this).map(str::to_string) {
            Some(n) => Name(vm.reg.names.intern(&n)),
            None => Name(vm.class_of_ref(this).map(|c| vm.reg.class(c).name).unwrap_or(0)),
        },
        "IsInState" => {
            let want = match a.first() { Some(Name(n)) => *n, _ => 0 };
            let mut found = false;
            if let Some(i) = ref_instance(this) {
                let mut cur = vm.objects[i].state;
                while let Some(st) = cur {
                    if vm.reg.state_def(st).name == want {
                        found = true;
                        break;
                    }
                    cur = vm.reg.state_def(st).super_state;
                }
                // 1013ece0 walks from `StateNode`: in no state that is the class, so a class name of its chain counts
                if vm.objects[i].state.is_none() {
                    let mut c = Some(vm.objects[i].class);
                    while let Some(k) = c {
                        if vm.reg.class(k).name == want {
                            found = true;
                            break;
                        }
                        c = vm.reg.class(k).super_id;
                    }
                }
            }
            Bool(found)
        }

        // ---- actor properties the engine maintains
        // `AActor::SetCollisionSize` (10369320): out of the hash, new size, back in
        "SetCollisionSize" => {
            vm.hash_remove_colliding(this);
            vm.set_prop(this, "CollisionRadius", 0, Float(flt(a, 0)?))?;
            vm.set_prop(this, "CollisionHeight", 0, Float(flt(a, 1)?))?;
            vm.hash_add_colliding(this);
            Bool(true)
        }
        "SetCollision" => {
            // `AActor::SetCollision` (10369200): out of the hash if colliding, new flags, back in if colliding.
            // Omitted arguments keep the current setting.
            vm.hash_remove_colliding(this);
            for (i, prop) in ["bCollideActors", "bBlockActors", "bBlockPlayers"].iter().enumerate() {
                if let Some(Bool(b)) = a.get(i) {
                    vm.set_prop(this, prop, 0, Bool(*b))?;
                }
            }
            vm.hash_add_colliding(this);
            Void
        }
        // (a viewport window has a native of the same name)
        "SetRotation" if !vm.is_class(this, "Window") => {
            let rot = rotv(a, 0)?;
            let mut hit = crate::collision::CheckResult::none();
            Bool(vm.move_actor(this, [0.0; 3], rot, &mut hit, false, false, false, false)?)
        }
        "Move" => {
            let delta = vecv(a, 0)?;
            let rot = [
                vm.get_prop(this, "Rotation", 0).ok().and_then(|v| if let Rotator(r) = v { Some(r) } else { None }).unwrap_or([0; 3]),
            ][0];
            let mut hit = crate::collision::CheckResult::none();
            Bool(vm.move_actor(this, delta, rot, &mut hit, false, false, false, false)?)
        }
        "MoveSmooth" => Bool(vm.move_smooth(this, vecv(a, 0)?)?),
        "SetLocation" => Bool(vm.far_move_actor(this, vecv(a, 0)?, false, false)?),
        "RotRand" => {
            // yaw from the first `rand()`, pitch from the second, roll from a third: `(rand() * 2) % 65535`
            let y = (vm.next_rand() as i32 * 2) % 0xffff;
            let p = (vm.next_rand() as i32 * 2) % 0xffff;
            let r = if matches!(a.first(), Some(Bool(true))) { (vm.next_rand() as i32 * 2) % 0xffff } else { 0 };
            Rotator([p, y, r])
        }
        "AddPawn" => {
            // push onto the level's pawn list
            let lvl = vm.level;
            if lvl != 0 {
                let head = vm.get_prop(lvl, "PawnList", 0)?;
                vm.set_prop(this, "nextPawn", 0, head)?;
                vm.set_prop(lvl, "PawnList", 0, Obj(this))?;
            }
            Void
        }
        "RemovePawn" => {
            let lvl = vm.level;
            if lvl != 0 {
                let next = vm.get_prop(this, "nextPawn", 0)?;
                let mut cur = vm.get_prop(lvl, "PawnList", 0)?;
                if cur == Obj(this) {
                    vm.set_prop(lvl, "PawnList", 0, next)?;
                } else {
                    while let Obj(p) = cur {
                        if p == 0 { break; }
                        let nx = vm.get_prop(p, "nextPawn", 0)?;
                        if nx == Obj(this) {
                            vm.set_prop(p, "nextPawn", 0, next)?;
                            break;
                        }
                        cur = nx;
                    }
                }
                vm.set_prop(this, "nextPawn", 0, Obj(0))?;
            }
            Void
        }

        // ---- world
        "Spawn" => {
            let class = match a.first() { Some(Obj(r)) => vm.class_of_ref(*r), _ => None };
            let class = class.ok_or_else(|| rt("Spawn: class is none"))?;
            let owner = match a.get(1) { Some(Obj(r)) => *r, _ => 0 };
            let tag = match a.get(2) { Some(Name(n)) if *n != 0 => Some(*n), _ => None };
            // without a location or rotation the new actor starts where the spawning actor is, turned like it
            let loc = match a.get(3) { Some(Vector(v)) => Some(*v), _ => vm.is_live(this).then(|| vm.location(this)) };
            let rot = match a.get(4) {
                Some(Rotator(v)) => Some(*v),
                _ => match vm.get_prop(this, "Rotation", 0) {
                    Ok(Rotator(r)) => Some(r),
                    _ => None,
                },
            };
            // `execSpawn`: `SpawnActor(Class, None, SpawnOwner, Instigator, Location, Rotation)`, then the new actor's
            // `Tag` becomes `SpawnTag` (None when omitted; during its spawn events it was the class name)
            let instigator = vm.op(this, "Instigator");
            let tag = tag.unwrap_or(0);
            match vm.spawn_actor(class, owner, instigator, loc, rot, false)? {
                Some(o) => {
                    vm.set_prop(o, "Tag", 0, Name(tag))?;
                    Obj(o)
                }
                None => Obj(0),
            }
        }
        // `UCanvas::execDrawActor` (10303d05): the actor is shown, drawn through the renderer in the canvas's frame
        // (`URender::DrawActor`: a sprite of its own, no leaf or fog lights; the host draws it after the world's depth
        // clear), then hidden again; `WireFrame` switches the viewer's `RendMap` to wireframe for it (not drawn so
        // here), `ClearZ` clears the depth first. The frame the sprite is set up in is the canvas's: nothing is drawn
        // (and nothing stamped) in an empty one.
        "DrawActor" if vm.is_class(this, "Canvas") => {
            let actor = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            if actor != 0 && vm.is_live(actor) {
                let visible = vm.fp(this, "ClipX") >= 1.0 && vm.fp(this, "ClipY") >= 1.0;
                if visible {
                    // `DrawActorSprite` stamps the draw
                    let now = if vm.level != 0 { vm.fp(vm.level, "TimeSeconds") } else { 0.0 };
                    vm.set_fp(actor, "LastRenderTime", now);
                    vm.canvas_draws.push(crate::CanvasDraw { actor, wireframe: matches!(a.get(1), Some(Bool(true))), clear_z: matches!(a.get(2), Some(Bool(true))) });
                }
                vm.set_bp(actor, "bHidden", true);
            }
            Void
        }
        "Destroy" if vm.is_class(this, "Window") => {
            vm.ui_safe_destroy(this);
            Void
        }
        "Destroy" => Bool(vm.destroy(this)?),
        "SetTimer" => {
            vm.set_prop(this, "TimerRate", 0, Float(flt(a, 0)?))?;
            vm.set_prop(this, "bTimerLoop", 0, Bool(boo(a, 1)?))?;
            vm.set_prop(this, "TimerCounter", 0, Float(0.0))?;
            Void
        }
        "SetOwner" => {
            let o = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            vm.set_owner(this, o)?;
            Void
        }
        "Error" => return Err(rt(format!("script error: {}", strv(a, 0)?))),
        // `property_text.rs`
        "GetPropertyText" => {
            let n = strv(a, 0)?;
            Str(vm.get_property_text(this, &n))
        }
        "SetPropertyText" => {
            let (n, text) = (strv(a, 0)?, strv(a, 1)?);
            vm.set_property_text(this, &n, &text);
            Void
        }

        // ---- AI events: see `event_manager.rs`
        "AISetEventCallback" => {
            let nm = |i: usize| match a.get(i) { Some(Name(n)) => *n, _ => 0 };
            // omitted flags take the exec wrapper's defaults: visibility, direction and line of sight on, cylinder off
            let fl = |i: usize, default: bool| match a.get(i) { None | Some(Void) => default, Some(v) => v.as_bool().unwrap_or(default) };
            let cb = crate::ai::AiCallback {
                callback: nm(1),
                score_callback: nm(2),
                check_visibility: fl(3, true),
                check_dir: fl(4, true),
                check_cylinder: fl(5, false),
                check_los: fl(6, true),
            };
            vm.ai.events.set_callback(this, nm(0), cb);
            Void
        }
        "AIClearEventCallback" => {
            if let Some(Name(n)) = a.first() {
                vm.ai.events.clear_callback(this, *n);
            }
            Void
        }
        "AIStartEvent" | "AISendEvent" => {
            let event = match a.first() { Some(Name(n)) => *n, _ => 0 };
            let kind = a.get(1).and_then(Value::as_i32).unwrap_or(0) as u8;
            // omitted value is 1.0 and omitted radius 800 in the exec wrappers
            let value = match a.get(2) { None | Some(Void) => 1.0, Some(v) => v.as_f32().unwrap_or(1.0) };
            let radius = match a.get(3) { None | Some(Void) => 800.0, Some(v) => v.as_f32().unwrap_or(800.0) };
            vm.ai_emit_event(this, event, kind, name == "AISendEvent", value, radius);
            Void
        }
        "AIEndEvent" => {
            let ev = match a.first() { Some(Name(n)) => *n, _ => 0 };
            let kind = a.get(1).and_then(Value::as_i32).unwrap_or(0) as u8;
            vm.ai_end_event(this, ev, &[kind]);
            Void
        }
        "AIClearEvent" => {
            let ev = match a.first() { Some(Name(n)) => *n, _ => 0 };
            vm.ai_end_event(this, ev, &[1, 0, 2]);
            Void
        }
        "AICanHear" => {
            let other = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            let vol = match a.get(1) { None | Some(Void) => 1.0, Some(v) => v.as_f32().unwrap_or(1.0) };
            let radius = match a.get(2) { None | Some(Void) => -1.0, Some(v) => v.as_f32().unwrap_or(-1.0) };
            Float(vm.ai_can_hear(this, other, vol, radius))
        }
        // `APawn::AICanSmell` is a stub in the original: it answers 0
        "AICanSmell" => Float(0.0),
        "InitEventManager" => {
            vm.ai.events.reset();
            Void
        }
        // ---- conversation binding (ConSys subsystem not present yet)
        "ConBindEvents" => {
            vm.ai.con_bound.insert(this);
            vm.con_bind_events(this)?;
            Void
        }
        "CreateFlagRef" | "CreateConCamera" | "BindEvents" | "BindActorEvents" | "ClearBindEvents" | "GetSpeechAudio" | "GetSpeechLength" | "GetSoundLength" | "GetLabelCount" | "GetLabel" | "GetRandomLabel" => vm.con_native(name, this, a)?,
        "InStasis" => Bool(vm.in_stasis(this)),
        // ---- decals: attaching needs a surface trace; without a world there is nothing to hit
        "AttachDecal" => {
            let dir = match a.get(1) { Some(Vector(v)) => *v, _ => [0.0; 3] };
            Bool(vm.attach_decal(this, flt(a, 0)?, dir))
        }
        "GreaterGreater_VectorRotator" => Vector(rotate_vector(vecv(a, 0)?, rotv(a, 1)?, false)),
        "LessLess_VectorRotator" => Vector(rotate_vector(vecv(a, 0)?, rotv(a, 1)?, true)),

        // ---- tracing
        "Trace" => {
            // Trace(out HitLocation, out HitNormal, TraceEnd, optional TraceStart, optional bTraceActors, optional Extent)
            let end = vecv(a, 2)?;
            let start = match a.get(3) {
                Some(Vector(v)) => *v,
                _ => vm.location(this),
            };
            // bTraceActors defaults to the tracer's own bCollideActors (exec wrapper)
            let trace_actors = match a.get(4) { None | Some(Void) => vm.bp(this, "bCollideActors"), Some(v) => v.as_bool().unwrap_or(false) };
            let extent = match a.get(5) {
                Some(Vector(v)) => *v,
                _ => [0.0; 3],
            };
            let flags = if trace_actors { crate::collision::TRACE_PROJ_TARGETS | 6 } else { 6 };
            let hit = vm.single_line_check(this, end, start, flags, extent, 0);
            a[0] = Vector(hit.location);
            a[1] = Vector(hit.normal);
            Obj(hit.actor)
        }
        // axes of a rotation (roll sign UNVERIFIED, like the vector/rotator operators); GetUnAxes uses the inverse
        "GetAxes" | "GetUnAxes" => {
            // `GMath.UnitCoords / R` (GetAxes 10139e90) or `* R` (GetUnAxes 1013a2d0) through Core's `FCoords`
            // operators with the sine table; X, Y, Z are the result's axes
            let r = rotv(a, 0)?;
            let mut c = ue_bsp::Coords::UNIT;
            if name == "GetUnAxes" {
                c.mul_rotator(r, sin_tab);
            } else {
                c.div_rotator(r, sin_tab);
            }
            a[1] = Vector(c.x);
            a[2] = Vector(c.y);
            a[3] = Vector(c.z);
            Void
        }
        // axis-aligned box of the actor (its brush for movers, else its collision cylinder) as if it stood at the given
        // location and rotation (defaults: where it is); `bExact` is ignored
        "GetBoundingBox" => {
            let loc = match a.get(3) {
                Some(Vector(v)) => *v,
                _ => vm.location(this),
            };
            let rot = match a.get(4) {
                Some(Rotator(r)) => *r,
                _ => vm.rot_of(this),
            };
            let pivot = match vm.get_prop(this, "PrePivot", 0) {
                Ok(Vector(v)) => v,
                _ => [0.0; 3],
            };
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            let corners: Vec<[f32; 3]> = match vm.brush_local_bounds(this) {
                Some((bl, bh)) => (0..8)
                    .map(|i| {
                        let c = [if i & 1 == 0 { bl[0] } else { bh[0] }, if i & 2 == 0 { bl[1] } else { bh[1] }, if i & 4 == 0 { bl[2] } else { bh[2] }];
                        let r = rotate_vector([c[0] - pivot[0], c[1] - pivot[1], c[2] - pivot[2]], rot, false);
                        [loc[0] + r[0], loc[1] + r[1], loc[2] + r[2]]
                    })
                    .collect(),
                None => {
                    let (r, h) = vm.cylinder(this);
                    (0..8).map(|i| [loc[0] + if i & 1 == 0 { -r } else { r }, loc[1] + if i & 2 == 0 { -r } else { r }, loc[2] + if i & 4 == 0 { -h } else { h }]).collect()
                }
            };
            for c in corners {
                for k in 0..3 {
                    lo[k] = lo[k].min(c[k]);
                    hi[k] = hi[k].max(c[k]);
                }
            }
            a[0] = Vector(lo);
            a[1] = Vector(hi);
            Bool(true)
        }
        "FastTrace" => {
            let end = vecv(a, 0)?;
            let start = match a.get(1) {
                Some(Vector(v)) => *v,
                _ => vm.location(this),
            };
            Bool(vm.bsp.as_ref().map(|b| b.fast_line_check(end, start)).unwrap_or(true))
        }

        "GetMeshTexture" => {
            let n = a.first().and_then(Value::as_i32).unwrap_or(0);
            Obj(vm.get_mesh_texture(this, n))
        }

        // `StaticLoadObject` (`load_object.rs`)
        "DynamicLoadObject" => {
            let name = strv(a, 0)?;
            let class = match a.get(1) {
                Some(Obj(c)) => *c,
                _ => 0,
            };
            let may_fail = matches!(a.get(2), Some(Bool(true)));
            Obj(vm.dynamic_load_object(&name, class, may_fail))
        }
        "GetAddressURL" => Str(String::new()),
        // networking is not implemented: binding "fails" by reporting port 0
        "BindPort" => Int(0),

        // ---- decals
        "DetachDecal" => {
            vm.detach_decal(this);
            Void
        }

        // ---- physics and relations
        "SetPhysics" => {
            let mode = a.first().and_then(Value::as_i32).unwrap_or(0) as u8;
            vm.set_physics(this, mode, 0)?;
            Void
        }
        "SetBase" => {
            let new_base = match a.first() { Some(Obj(r)) => *r, _ => 0 };
            vm.set_base(this, new_base, true)?;
            Void
        }

        // ---- animation: only the script-visible state is kept until meshes are loaded, so rates are the
        // requested ones (the engine converts them with the sequence's frame count) and the end of a
        // non-looping sequence is never reported by itself
        "PlayAnim" | "LoopAnim" => {
            let seq = match a.first() { Some(Name(n)) => *n, _ => 0 };
            let looping = name == "LoopAnim";
            let rate = a.get(1).and_then(Value::as_f32).filter(|_| !matches!(a.get(1), Some(Void))).unwrap_or(1.0);
            let tween = a.get(2).and_then(Value::as_f32).filter(|_| !matches!(a.get(2), Some(Void))).unwrap_or(-1.0);
            let min_rate = if looping { a.get(3).and_then(Value::as_f32).unwrap_or(0.0) } else { 0.0 };
            vm.native_play_anim(this, seq, rate, tween, min_rate, looping);
            Void
        }
        "TweenAnim" => {
            let seq = match a.first() { Some(Name(n)) => *n, _ => 0 };
            vm.native_tween_anim(this, seq, flt(a, 1)?);
            Void
        }
        "FinishAnim" => {
            vm.native_finish_anim(this);
            Void
        }
        "HasAnim" => {
            let seq = match a.first() { Some(Name(n)) => *n, _ => 0 };
            Bool(vm.native_has_anim(this, seq))
        }
        "GetAnimGroup" => {
            let seq = match a.first() { Some(Name(n)) => *n, _ => 0 };
            Name(vm.native_anim_group(this, seq))
        }

        // `execPlayBlendAnim` / `execTweenBlendAnim` (103e0d80, 103e1300): optional rate 1, tween -1, slot 0
        "PlayBlendAnim" => {
            let seq = match a.first() { Some(Name(n)) => *n, _ => 0 };
            let opt = |i: usize, d: f32| match a.get(i) { None | Some(Void) => d, Some(v) => v.as_f32().unwrap_or(d) };
            let (rate, tween) = (opt(1, 1.0), opt(2, -1.0));
            let slot = match a.get(3) { None | Some(Void) => 0, Some(v) => v.as_i32().unwrap_or(0) };
            vm.native_play_blend_anim(this, seq, rate, tween, slot);
            Void
        }
        "TweenBlendAnim" => {
            let seq = match a.first() { Some(Name(n)) => *n, _ => 0 };
            let time = flt(a, 1)?;
            let slot = match a.get(2) { None | Some(Void) => 0, Some(v) => v.as_i32().unwrap_or(0) };
            vm.native_tween_blend_anim(this, seq, time, slot);
            Void
        }
        // a sequence is playing when it has a rate (tweening before the first frame counts through TweenRate)
        "IsAnimating" => {
            let seq = vm.get_prop(this, "AnimSequence", 0)?;
            let frame = vm.get_prop(this, "AnimFrame", 0)?.as_f32().unwrap_or(0.0);
            let rate = vm.get_prop(this, if frame < 0.0 { "TweenRate" } else { "AnimRate" }, 0)?.as_f32().unwrap_or(0.0);
            Bool(seq != Name(0) && rate != 0.0)
        }

        // ---- sound and console (requests are recorded for the audio/console subsystems)
        // PlayOwnedSound only differs in network play (not sent to a remote owner)
        "PlaySound" | "PlayOwnedSound" if vm.class_of_ref(this).map(|c| vm.is_actor_class(c)).unwrap_or(false) => {
            // `sounds.rs` (engine-sounds.md): omitted optional arguments take the engine's defaults: slot
            // SLOT_Misc, the actor's TransientSoundVolume and TransientSoundRadius, pitch 1
            let given = |i: usize| a.get(i).filter(|v| !matches!(v, Void));
            let volume = match given(2).and_then(Value::as_f32) {
                Some(v) => v,
                None => vm.fp(this, "TransientSoundVolume"),
            };
            let radius = match given(4).and_then(Value::as_f32) {
                Some(r) => r,
                None => vm.fp(this, "TransientSoundRadius"),
            };
            let args = crate::sounds::PlayArgs {
                sound: match a.first() { Some(Obj(r)) => *r, _ => 0 },
                slot: given(1).and_then(Value::as_i32).unwrap_or(1) as u8,
                volume,
                no_override: given(3).and_then(Value::as_bool).unwrap_or(false),
                radius,
                pitch: given(5).and_then(Value::as_f32).unwrap_or(1.0),
            };
            let owned = name == "PlayOwnedSound";
            if args.sound == 0 {
                // the original returns without writing the result
                if owned { Void } else { Int(0) }
            } else {
                let id = vm.play_sound_native(this, args, owned);
                if owned { Void } else { Int(id) }
            }
        }
        // `Window.PlaySound` (same name) falls through to the window natives
        "StopSound" => {
            let id = int(a, 0)?;
            vm.stop_sound_native(id);
            Void
        }
        // three natives of one name: the class of `this` picks the route (`console.rs`)
        "ConsoleCommand" => {
            let cmd = strv(a, 0)?;
            let class = vm.class_of_ref(this);
            let is = |vm: &Vm, n: &str| class.zip(vm.reg.class_by_name(n)).is_some_and(|(c, k)| vm.is_a(c, k));
            if is(vm, "Console") {
                Bool(vm.console_command_console(this, &cmd)?)
            } else if is(vm, "PlayerPawn") {
                Str(vm.console_command_player(this, &cmd)?)
            } else {
                Str(vm.console_command_actor(&cmd)?)
            }
        }
        // configuration goes to the per-user layer, never to the game folder
        // `object_config.rs`
        "SaveConfig" | "StaticSaveConfig" => {
            vm.save_config(this, name == "StaticSaveConfig")?;
            Void
        }
        "ResetConfig" => Void,
        "ClientTravel" | "ServerTravel" => {
            let items = a.get(if name == "ClientTravel" { 2 } else { 1 }).and_then(Value::as_bool).unwrap_or(false);
            vm.travel_request = Some(crate::travel::TravelRequest { url: strv(a, 0)?, items });
            // `UGameEngine::SetClientTravel` (10303f2b): every actor of the level that has a `PreTravel` function
            // gets it now (the player sets `PlayerTraveling`, mission scripts save their state, AugLight ...)
            for actor in vm.live_actors() {
                vm.send_event(actor, "PreTravel", vec![])?;
            }
            Void
        }
        // `new_object.rs`: deleted at once; the result is never written
        "CriticalDelete" => {
            if let Some(Obj(o)) = a.first() {
                vm.critical_delete(*o);
            }
            Int(0)
        }
        // `clock(out int cycles)` / `unclock` (1013ef20 / 1013eff0): minus / plus the low 32 bits of the CPU's time
        // stamp counter (unclock also takes 34 off); `CyclesToSeconds` scales by `GSecondsPerCycle`. The port counts
        // nanoseconds since start-up (1 GHz).
        "clock" | "unclock" => {
            let now = vm.clock_start.elapsed().as_nanos() as u64 as u32 as i32;
            let cur = a.first().and_then(Value::as_i32).unwrap_or(0);
            let v = if name == "clock" { cur.wrapping_sub(now) } else { cur.wrapping_add(now).wrapping_sub(0x22) };
            if let Some(x) = a.first_mut() {
                *x = Int(v);
            }
            Void
        }
        "CyclesToSeconds" => Float((a.first().and_then(Value::as_i32).unwrap_or(0) as f64 * 1e-9) as f32),
        "SetInstantMusicVolume" | "SetInstantSoundVolume" | "SetInstantSpeechVolume" => {
            let slot = match name {
                "SetInstantMusicVolume" => 0,
                "SetInstantSoundVolume" => 1,
                _ => 2,
            };
            vm.ai.instant_volumes[slot] = Some(a.first().and_then(Value::as_i32).unwrap_or(0).clamp(0, 255) as u8);
            Void
        }
        "Localize" => Str(vm.localize(&strv(a, 0)?, &strv(a, 1)?, &strv(a, 2)?)),
        "GetConfig" => Str(vm.get_config(&strv(a, 0)?, &strv(a, 1)?)),

        // ---- diagnostics
        "Log" | "Warn" => {
            let s = strv(a, 0)?;
            vm.log.push(s);
            Void
        }
        other => match vm.native_ext(this, other, a) {
            Some(r) => r?,
            None if vm.strict_natives => return Err(VmError::Unsupported(format!("native function {other}"))),
            // a native the port lacks: the original engine has them all, so stopping the script here would freeze
            // the actor (an NPC's state code dies). Say so once, return nothing, carry on.
            None => {
                if vm.missing_natives.insert(other.to_string()) {
                    let msg = format!("MISSING NATIVE {other} (called on {}) returns nothing", vm.class_of_ref(this).map(|c| vm.reg.class(c).path.clone()).unwrap_or_default());
                    eprintln!("{msg}");
                    vm.log.push(msg);
                }
                Void
            }
        },
    })
}

/// `FGlobalMath::TrigFLOAT`: 16384 sines of the angles i * 2pi / 16384 (computed in double, stored as float).
fn trig_table() -> &'static [f32; 16384] {
    static T: std::sync::OnceLock<Box<[f32; 16384]>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = Box::new([0.0f32; 16384]);
        for (i, x) in t.iter_mut().enumerate() {
            *x = (i as f64 * 0.000_383_495_196_971_410_3).sin() as f32;
        }
        t
    })
}

/// `GMath.SinTab(angle)`: the table sine of a rotator angle (65536 units per turn), in steps of 4 units.
pub(crate) fn sin_tab(a: i32) -> f32 {
    trig_table()[((a >> 2) & 16383) as usize]
}

/// `GMath.CosTab(angle)`.
pub(crate) fn cos_tab(a: i32) -> f32 {
    trig_table()[((a.wrapping_add(16384) >> 2) & 16383) as usize]
}

/// Rotates `v` by `r` (`v >> r`), or by the inverse rotation (`v << r`). Rotator units: 65536 per turn.
/// Axes follow the engine's GetAxes convention: yaw about Z, pitch about Y, roll about X.
pub fn rotate_vector(v: [f32; 3], r: [i32; 3], inverse: bool) -> [f32; 3] {
    let (sp, cp) = (sin_tab(r[0]), cos_tab(r[0]));
    let (sy, cy) = (sin_tab(r[1]), cos_tab(r[1]));
    let (sr, cr) = (sin_tab(r[2]), cos_tab(r[2]));
    let x = [cp * cy, cp * sy, sp];
    let y = [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp];
    let z = [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp];
    if inverse {
        let d = |b: [f32; 3]| v[0] * b[0] + v[1] * b[1] + v[2] * b[2];
        [d(x), d(y), d(z)]
    } else {
        std::array::from_fn(|i| v[0] * x[i] + v[1] * y[i] + v[2] * z[i])
    }
}

/// Names of the native functions implemented in `call`.
pub static IMPLEMENTED: &[&str] = &[
    "AllObjects",
    "clock",
    "unclock",
    "CyclesToSeconds",
    "SetBoolFlagFromString",
    "DrawActor",
    "Normalize",
    "Invert",
    "UpdateURL",
    "GetDefaultURL",
    "GetURLMap",
    "FinishInterpolation",
    "GetBoundingBox",
    "BindPort",
    "DynamicLoadObject",
    "GetAddressURL",
    "GetMeshTexture",
    "Move",
    "MoveSmooth",
    "SetLocation",
    "FastTrace",
    "Trace",
    "ConsoleCommand",
    "DetachDecal",
    "LoopAnim",
    "PlayAnim",
    "PlaySound",
    "SaveConfig",
    "ClientTravel",
    "ServerTravel",
    "SetInstantMusicVolume",
    "SetInstantSoundVolume",
    "SetInstantSpeechVolume",
    "SetBase",
    "SetPhysics",
    "StaticSaveConfig",
    "StopSound",
    "TweenAnim",
    "AIClearEvent",
    "AIClearEventCallback",
    "AIEndEvent",
    "AISendEvent",
    "AISetEventCallback",
    "AIStartEvent",
    "UpdateParticles",
    "ParabolicTrace",
    "AICanHear",
    "AICanSmell",
    "AttachDecal",
    "InStasis",
    "ConBindEvents",
    "GreaterGreater_VectorRotator",
    "InitEventManager",
    "LessLess_VectorRotator",
    "RotRand",
    "Abs",
    "AddAdd_Byte",
    "AddAdd_Int",
    "AddAdd_PreByte",
    "AddAdd_PreInt",
    "AddEqual_ByteByte",
    "AddEqual_FloatFloat",
    "AddEqual_IntInt",
    "AddEqual_RotatorRotator",
    "AddEqual_VectorVector",
    "Add_FloatFloat",
    "Add_IntInt",
    "Add_RotatorRotator",
    "Add_VectorVector",
    "AndAnd_BoolBool",
    "And_IntInt",
    "Asc",
    "At_StrStr",
    "Atan",
    "Caps",
    "Chr",
    "Clamp",
    "ClassIsChildOf",
    "ComplementEqual_FloatFloat",
    "ComplementEqual_StrStr",
    "Complement_PreInt",
    "Concat_StrStr",
    "Cos",
    "Cross_VectorVector",
    "DivideEqual_ByteByte",
    "DivideEqual_FloatFloat",
    "DivideEqual_IntFloat",
    "DivideEqual_RotatorFloat",
    "DivideEqual_VectorFloat",
    "Divide_FloatFloat",
    "Divide_IntInt",
    "Divide_RotatorFloat",
    "Divide_VectorFloat",
    "Dot_VectorVector",
    "EqualEqual_BoolBool",
    "EqualEqual_FloatFloat",
    "EqualEqual_IntInt",
    "EqualEqual_NameName",
    "EqualEqual_ObjectObject",
    "EqualEqual_RotatorRotator",
    "EqualEqual_StrStr",
    "EqualEqual_VectorVector",
    "Exp",
    "FClamp",
    "FMax",
    "FMin",
    "FRand",
    "GetPlayerPawn",
    "GetAxes",
    "PlayerCanSeeMe",
    "EAdjustJump",
    "FinishAnim",
    "HasAnim",
    "GetAnimGroup",
    "IsOverlapping",
    "pointReachable",
    "actorReachable",
    "FindPathTo",
    "FindPathToward",
    "ClearPaths",
    "FindRandomDest",
    "RandomBiasedRotation",
    "AIGetLightLevel",
    "AIVisibility",
    "AICanSee",
    "LineOfSightTo",
    "CanSee",
    "PickTarget",
    "PickAnyTarget",
    "FindStairRotation",
    "IsValidEnemy",
    "GetAllianceType",
    "GetPawnAllianceType",
    "TurnTo",
    "TurnToward",
    "MoveTo",
    "StrafeTo",
    "StrafeFacing",
    "PlayOwnedSound",
    "StopWaiting",
    "MirrorVectorByNormal",
    "HaveSeenCarcass",
    "AddCarcass",
    "MoveToward",
    "AIDirectionReachable",
    "AIPickRandomDestination",
    "WaitForLanding",
    "MakeNoise",
    "LastRendered",
    "PlayBlendAnim",
    "TweenBlendAnim",
    "IsAnimating",
    "GetUnAxes",
    "GreaterEqual_FloatFloat",
    "GreaterEqual_IntInt",
    "GreaterEqual_StrStr",
    "GreaterGreaterGreater_IntInt",
    "GreaterGreater_IntInt",
    "Greater_FloatFloat",
    "Greater_IntInt",
    "Greater_StrStr",
    "InStr",
    "IsA",
    "Left",
    "Len",
    "Lerp",
    "LessEqual_FloatFloat",
    "LessEqual_IntInt",
    "LessEqual_StrStr",
    "LessLess_IntInt",
    "Less_FloatFloat",
    "Less_IntInt",
    "Less_StrStr",
    "Log",
    "Loge",
    "Max",
    "Mid",
    "Min",
    "MultiplyEqual_ByteByte",
    "MultiplyEqual_FloatFloat",
    "MultiplyEqual_IntFloat",
    "MultiplyEqual_RotatorFloat",
    "MultiplyEqual_VectorFloat",
    "MultiplyEqual_VectorVector",
    "MultiplyMultiply_FloatFloat",
    "Multiply_FloatFloat",
    "Multiply_FloatRotator",
    "Multiply_FloatVector",
    "Multiply_IntInt",
    "Multiply_RotatorFloat",
    "Multiply_VectorFloat",
    "Multiply_VectorVector",
    "Normal",
    "NotEqual_BoolBool",
    "NotEqual_FloatFloat",
    "NotEqual_IntInt",
    "NotEqual_NameName",
    "NotEqual_ObjectObject",
    "NotEqual_RotatorRotator",
    "NotEqual_StrStr",
    "NotEqual_VectorVector",
    "Not_PreBool",
    "OrOr_BoolBool",
    "Or_IntInt",
    "Percent_FloatFloat",
    "Rand",
    "Right",
    "Sin",
    "Smerp",
    "Sqrt",
    "Square",
    "SubtractEqual_ByteByte",
    "SubtractEqual_FloatFloat",
    "SubtractEqual_IntInt",
    "SubtractEqual_RotatorRotator",
    "SubtractEqual_VectorVector",
    "SubtractSubtract_Byte",
    "SubtractSubtract_Int",
    "SubtractSubtract_PreByte",
    "SubtractSubtract_PreInt",
    "Subtract_FloatFloat",
    "Subtract_IntInt",
    "Subtract_PreFloat",
    "Subtract_PreInt",
    "Subtract_PreVector",
    "Subtract_RotatorRotator",
    "Subtract_VectorVector",
    "Tan",
    "VRand",
    "VSize",
    "Warn",
    "XorXor_BoolBool",
    "Xor_IntInt",
];

pub fn is_implemented(name: &str) -> bool {
    IMPLEMENTED.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `op` (operators resolved at compile time) gives exactly what `call` gives for the native's name, on operands of
    /// every kind (wrong kinds included: the same error).
    #[test]
    fn operators_match_their_natives() {
        let Some(dir) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
        let mut vm = Vm::new(ue_registry::Registry::load_game(&dir).expect("load game"));
        let names = [
            "Not_PreBool", "EqualEqual_BoolBool", "NotEqual_BoolBool", "XorXor_BoolBool", "AndAnd_BoolBool", "OrOr_BoolBool",
            "Subtract_PreInt", "Complement_PreInt", "Add_IntInt", "Subtract_IntInt", "Multiply_IntInt", "Less_IntInt",
            "Greater_IntInt", "LessEqual_IntInt", "GreaterEqual_IntInt", "EqualEqual_IntInt", "NotEqual_IntInt", "And_IntInt",
            "Xor_IntInt", "Or_IntInt", "Subtract_PreFloat", "Add_FloatFloat", "Subtract_FloatFloat", "Multiply_FloatFloat",
            "Less_FloatFloat", "Greater_FloatFloat", "LessEqual_FloatFloat", "GreaterEqual_FloatFloat", "EqualEqual_FloatFloat",
            "NotEqual_FloatFloat", "EqualEqual_NameName", "NotEqual_NameName", "EqualEqual_ObjectObject", "NotEqual_ObjectObject",
        ];
        let vals = [
            Value::Void, Value::Bool(true), Value::Bool(false), Value::Int(0), Value::Int(-7), Value::Int(i32::MAX), Value::Int(i32::MIN),
            Value::Byte(200), Value::Float(1.5), Value::Float(-0.0), Value::Float(f32::NAN), Value::Name(3), Value::Name(0),
            Value::Obj(0), Value::Obj(0x8000_0005), Value::Str("x".into()),
        ];
        let show = |r: &Res<Value>| match r {
            Ok(v) => format!("ok {v:?}"),
            Err(e) => format!("err {e}"),
        };
        let mut checked = 0;
        for name in names {
            let (which, n) = crate::ir::Op::of(name).expect(name);
            for x in &vals {
                for y in &vals {
                    let mut args = if n == 1 { vec![x.clone()] } else { vec![x.clone(), y.clone()] };
                    let want = show(&call(&mut vm, 0, name, &mut args));
                    let got = show(&op(which, x.clone(), if n == 1 { Value::Void } else { y.clone() }));
                    // NaN compares unequal to itself in Debug only through its bits: compare the texts
                    assert_eq!(got, want, "{name}({x:?}, {y:?})");
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, names.len() * vals.len() * vals.len());
        eprintln!("{checked} operator results compared");
    }

    /// msvcrt `%f`: half up on the 17-digit string (1/128 = 0.0078125 -> 0.007813, Rust's own rounding gives
    /// 0.007812), 17 significant digits then zeros (1e30), the x87's special values, negative zero.
    #[test]
    fn floats_print_as_msvcrt() {
        assert_eq!(float_str(0.0078125), "0.007813");
        assert_eq!(float_str(-0.0078125), "-0.007813");
        assert_eq!(float_str(1.5), "1.500000");
        assert_eq!(float_str(0.0), "0.000000");
        assert_eq!(float_str(-0.0), "-0.000000");
        assert_eq!(float_str(1e30), "1000000015047466200000000000000.000000");
        assert_eq!(float_str(0.4999999), "0.500000");
        assert_eq!(float_str(9.9999995), "9.999999");
        assert_eq!(float_str(0.0000004), "0.000000");
        // 0.0000005 as a float is 4.99999987e-7: 0.000000; a little more rounds up
        assert_eq!(float_str(0.0000005), "0.000000");
        assert_eq!(float_str(0.00000051), "0.000001");
        assert_eq!(float_str(f32::INFINITY), "1.#INF00");
        assert_eq!(float_str(f32::NEG_INFINITY), "-1.#INF00");
        assert_eq!(float_str(f32::from_bits(0xFFC0_0000)), "-1.#IND00");
        assert_eq!(float_str(f32::from_bits(0x7FC0_0000)), "1.#QNAN0");
    }

    /// `atof` takes `d` for the exponent; the cast to a vector reads one number per comma-separated part.
    #[test]
    fn text_to_numbers() {
        assert_eq!(c_atof("1d2"), 100.0);
        assert_eq!(c_atof("2.5E1x"), 25.0);
    }

    /// The string casts to vector (0x4D) and rotator (0x4E): one number per comma-separated part, nothing else skipped.
    #[test]
    fn strings_cast_to_vectors_and_rotators() {
        let Some(dir) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
        let mut vm = Vm::new(ue_registry::Registry::load_game(&dir).expect("load game"));
        let s = |x: &str| Value::Str(x.into());
        assert_eq!(cast(&mut vm, 0x4D, s("1.5,-2,3e1")).unwrap(), Value::Vector([1.5, -2.0, 30.0]));
        assert_eq!(cast(&mut vm, 0x4D, s("7")).unwrap(), Value::Vector([7.0, 0.0, 0.0]));
        assert_eq!(cast(&mut vm, 0x4D, s("(X=1,Y=2,Z=3)")).unwrap(), Value::Vector([0.0; 3]));
        assert_eq!(cast(&mut vm, 0x4E, s("100, 200,-300")).unwrap(), Value::Rotator([100, 200, -300]));
    }
}

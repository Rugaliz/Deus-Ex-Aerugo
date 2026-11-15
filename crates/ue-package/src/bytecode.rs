//! Scanner for compiled UnrealScript (UE1 token stream).
//!
//! A script's declared size is measured in *memory* bytes, where every object or name operand occupies a
//! fixed width, while the serialized stream uses compact indices. Finding where the serialized code ends
//! therefore requires walking the token tree and counting both coordinate systems.

use crate::{Error, Reader, Result};

/// Width of an object-pointer or name operand in the in-memory script array.
const REF_MEM: usize = 4;

// Opcode values (UE1).
pub const EX_LOCAL_VARIABLE: u8 = 0x00;
pub const EX_INSTANCE_VARIABLE: u8 = 0x01;
pub const EX_DEFAULT_VARIABLE: u8 = 0x02;
pub const EX_RETURN: u8 = 0x04;
pub const EX_SWITCH: u8 = 0x05;
pub const EX_JUMP: u8 = 0x06;
pub const EX_JUMP_IF_NOT: u8 = 0x07;
pub const EX_STOP: u8 = 0x08;
pub const EX_ASSERT: u8 = 0x09;
pub const EX_CASE: u8 = 0x0A;
pub const EX_NOTHING: u8 = 0x0B;
pub const EX_LABEL_TABLE: u8 = 0x0C;
pub const EX_GOTO_LABEL: u8 = 0x0D;
pub const EX_EAT_STRING: u8 = 0x0E;
pub const EX_LET: u8 = 0x0F;
pub const EX_NEW: u8 = 0x11;
pub const EX_CLASS_CONTEXT: u8 = 0x12;
pub const EX_META_CAST: u8 = 0x13;
pub const EX_LET_BOOL: u8 = 0x14;
pub const EX_END_PARM_VALUE: u8 = 0x15;
pub const EX_END_FUNCTION_PARMS: u8 = 0x16;
pub const EX_SELF: u8 = 0x17;
pub const EX_SKIP: u8 = 0x18;
pub const EX_CONTEXT: u8 = 0x19;
pub const EX_ARRAY_ELEMENT: u8 = 0x1A;
pub const EX_VIRTUAL_FUNCTION: u8 = 0x1B;
pub const EX_FINAL_FUNCTION: u8 = 0x1C;
pub const EX_INT_CONST: u8 = 0x1D;
pub const EX_FLOAT_CONST: u8 = 0x1E;
pub const EX_STRING_CONST: u8 = 0x1F;
pub const EX_OBJECT_CONST: u8 = 0x20;
pub const EX_NAME_CONST: u8 = 0x21;
pub const EX_ROTATION_CONST: u8 = 0x22;
pub const EX_VECTOR_CONST: u8 = 0x23;
pub const EX_BYTE_CONST: u8 = 0x24;
pub const EX_INT_ZERO: u8 = 0x25;
pub const EX_INT_ONE: u8 = 0x26;
pub const EX_TRUE: u8 = 0x27;
pub const EX_FALSE: u8 = 0x28;
pub const EX_NATIVE_PARM: u8 = 0x29;
pub const EX_NO_OBJECT: u8 = 0x2A;
pub const EX_INT_CONST_BYTE: u8 = 0x2C;
pub const EX_BOOL_VARIABLE: u8 = 0x2D;
pub const EX_DYNAMIC_CAST: u8 = 0x2E;
pub const EX_ITERATOR: u8 = 0x2F;
pub const EX_ITERATOR_POP: u8 = 0x30;
pub const EX_ITERATOR_NEXT: u8 = 0x31;
pub const EX_STRUCT_CMP_EQ: u8 = 0x32;
pub const EX_STRUCT_CMP_NE: u8 = 0x33;
pub const EX_UNICODE_STRING_CONST: u8 = 0x34;
pub const EX_STRUCT_MEMBER: u8 = 0x36;
pub const EX_GLOBAL_FUNCTION: u8 = 0x38;
/// Opcodes in this range are primitive type conversions taking exactly one expression.
pub const EX_CAST_FIRST: u8 = 0x39;
pub const EX_CAST_LAST: u8 = 0x5F;
pub const EX_EXTENDED_NATIVE: u8 = 0x60;
pub const EX_FIRST_NATIVE: u8 = 0x70;

/// Result of scanning one script.
#[derive(Debug, Clone)]
pub struct Scan {
    /// Serialized byte range of the code, relative to the start of the object data.
    pub file: std::ops::Range<usize>,
    /// Number of top-level statements/expressions.
    pub top_level: usize,
}

struct Scanner<'a, 'b> {
    r: &'b mut Reader<'a>,
    mem: usize,
    none_name: i32,
    depth: usize,
}

impl Scanner<'_, '_> {
    fn expr(&mut self) -> Result<()> {
        let op = self.r.u8()?;
        self.mem += 1;
        self.expr_op(op)
    }

    fn params(&mut self) -> Result<()> {
        loop {
            let op = self.r.u8()?;
            self.mem += 1;
            if op == EX_END_FUNCTION_PARMS {
                return Ok(());
            }
            self.expr_op(op)?;
        }
    }

    fn skip_ref(&mut self) -> Result<()> {
        self.r.compact_index()?;
        self.mem += REF_MEM;
        Ok(())
    }

    fn skip(&mut self, file_bytes: usize, mem_bytes: usize) -> Result<()> {
        self.r.bytes(file_bytes)?;
        self.mem += mem_bytes;
        Ok(())
    }

    fn expr_op(&mut self, op: u8) -> Result<()> {
        self.depth += 1;
        if self.depth > 4096 {
            return Err(Error::Invalid("script expression nesting too deep".into()));
        }
        let res = self.expr_op_inner(op);
        self.depth -= 1;
        res
    }

    fn expr_op_inner(&mut self, op: u8) -> Result<()> {
        match op {
            EX_LOCAL_VARIABLE | EX_INSTANCE_VARIABLE | EX_DEFAULT_VARIABLE | EX_NATIVE_PARM | EX_OBJECT_CONST => self.skip_ref(),
            EX_NAME_CONST => self.skip_ref(),
            EX_RETURN | EX_GOTO_LABEL | EX_EAT_STRING | EX_BOOL_VARIABLE => self.expr(),
            EX_SWITCH => {
                self.skip(1, 1)?;
                self.expr()
            }
            EX_JUMP => self.skip(2, 2),
            EX_JUMP_IF_NOT | EX_SKIP => {
                self.skip(2, 2)?;
                self.expr()
            }
            EX_ASSERT => {
                self.skip(2, 2)?;
                self.expr()
            }
            EX_CASE => {
                let next = self.r.u16()?;
                self.mem += 2;
                if next != 0xFFFF {
                    self.expr()?;
                }
                Ok(())
            }
            EX_STOP | EX_NOTHING | EX_SELF | EX_INT_ZERO | EX_INT_ONE | EX_TRUE | EX_FALSE | EX_NO_OBJECT
            | EX_ITERATOR_POP | EX_ITERATOR_NEXT | EX_END_PARM_VALUE | EX_END_FUNCTION_PARMS => Ok(()),
            EX_LABEL_TABLE => loop {
                let name = self.r.compact_index()?;
                self.mem += REF_MEM;
                self.skip(4, 4)?;
                if name == self.none_name {
                    return Ok(());
                }
            },
            EX_LET | EX_LET_BOOL | EX_ARRAY_ELEMENT => {
                self.expr()?;
                self.expr()
            }
            EX_NEW => {
                for _ in 0..4 {
                    self.expr()?;
                }
                Ok(())
            }
            EX_CLASS_CONTEXT | EX_CONTEXT => {
                self.expr()?;
                self.skip(2, 2)?;
                self.skip(1, 1)?;
                self.expr()
            }
            EX_META_CAST | EX_DYNAMIC_CAST | EX_STRUCT_MEMBER => {
                self.skip_ref()?;
                self.expr()
            }
            EX_VIRTUAL_FUNCTION | EX_GLOBAL_FUNCTION | EX_FINAL_FUNCTION => {
                self.skip_ref()?;
                self.params()
            }
            EX_INT_CONST | EX_FLOAT_CONST => self.skip(4, 4),
            EX_STRING_CONST => {
                let mut n = 1;
                while self.r.u8()? != 0 {
                    n += 1;
                }
                self.mem += n;
                Ok(())
            }
            EX_UNICODE_STRING_CONST => {
                let mut n = 2;
                while self.r.u16()? != 0 {
                    n += 2;
                }
                self.mem += n;
                Ok(())
            }
            EX_ROTATION_CONST | EX_VECTOR_CONST => self.skip(12, 12),
            EX_BYTE_CONST | EX_INT_CONST_BYTE => self.skip(1, 1),
            EX_ITERATOR => {
                self.expr()?;
                self.skip(2, 2)
            }
            EX_STRUCT_CMP_EQ | EX_STRUCT_CMP_NE => {
                self.skip_ref()?;
                self.expr()?;
                self.expr()
            }
            EX_CAST_FIRST..=EX_CAST_LAST => self.expr(),
            EX_EXTENDED_NATIVE..=0x6F => {
                self.skip(1, 1)?;
                self.params()
            }
            EX_FIRST_NATIVE..=0xFF => self.params(),
            other => Err(Error::Invalid(format!("unknown script token 0x{other:02X} at file offset {}", self.r.pos() - 1))),
        }
    }
}

/// Scans statements until `mem_size` in-memory bytes have been consumed.
/// The reader must be positioned at the first token; on success it is left just past the last one.
pub fn scan(r: &mut Reader, mem_size: usize, none_name: i32) -> Result<Scan> {
    let start = r.pos();
    let mut s = Scanner { r, mem: 0, none_name, depth: 0 };
    let mut top_level = 0;
    while s.mem < mem_size {
        s.expr()?;
        top_level += 1;
    }
    if s.mem != mem_size {
        return Err(Error::Invalid(format!("script scan consumed {} memory bytes, expected {mem_size}", s.mem)));
    }
    Ok(Scan { file: start..s.r.pos(), top_level })
}

mod opcodes;

pub use opcodes::{OpCode, Operand};

use crate::error::{Error, Result};
use crate::metadata::{Metadata, Token};
use crate::reader::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseKind {
    Catch,
    Filter,
    Finally,
    Fault,
}

#[derive(Debug, Clone, Copy)]
pub struct ExceptionClause {
    pub kind: ClauseKind,
    pub try_offset: u32,
    pub try_length: u32,
    pub handler_offset: u32,
    pub handler_length: u32,
    pub class_or_filter: u32,
}

#[derive(Debug, Clone)]
pub struct MethodBody<'a> {
    pub max_stack: u16,
    pub local_signature: Token,
    pub init_locals: bool,
    pub code: &'a [u8],
    pub clauses: Vec<ExceptionClause>,
}

impl<'a> MethodBody<'a> {
    pub fn parse(data: &'a [u8], rva: u32) -> Result<Self> {
        let bad = |reason| Error::BadMethodBody { rva, reason };
        let mut reader = Reader::new(data, "method body");
        let first = reader.u8()?;
        match first & 0x3 {
            0x2 => {
                let code = reader
                    .bytes((first >> 2) as usize)
                    .map_err(|_| bad("tiny body runs past the section"))?;
                Ok(Self {
                    max_stack: 8,
                    local_signature: Token(0),
                    init_locals: false,
                    code,
                    clauses: Vec::new(),
                })
            }
            0x3 => {
                let second = reader.u8()?;
                let flags = u16::from_le_bytes([first, second]);
                let header_size = (flags >> 12) as usize * 4;
                if header_size < 12 {
                    return Err(bad("fat header is smaller than 12 bytes"));
                }
                let max_stack = reader.u16()?;
                let code_size = reader.u32()? as usize;
                let local_signature = Token(reader.u32()?);
                reader.seek(header_size)?;
                let code = reader
                    .bytes(code_size)
                    .map_err(|_| bad("code runs past the section"))?;
                let mut clauses = Vec::new();
                let mut more = flags & 0x8 != 0;
                while more {
                    reader.align(4)?;
                    let kind = reader.u8()?;
                    if kind & 0x3F != 0x1 {
                        return Err(bad("unknown extra data section"));
                    }
                    let fat = kind & 0x40 != 0;
                    more = kind & 0x80 != 0;
                    if fat {
                        let size = reader.bytes(3)?;
                        let size = u32::from_le_bytes([size[0], size[1], size[2], 0]) as usize;
                        for _ in 0..size.saturating_sub(4) / 24 {
                            clauses.push(ExceptionClause::read(&mut reader, true, rva)?);
                        }
                    } else {
                        let size = reader.u8()? as usize;
                        reader.skip(2)?;
                        for _ in 0..size.saturating_sub(4) / 12 {
                            clauses.push(ExceptionClause::read(&mut reader, false, rva)?);
                        }
                    }
                }
                Ok(Self {
                    max_stack,
                    local_signature,
                    init_locals: flags & 0x10 != 0,
                    code,
                    clauses,
                })
            }
            _ => Err(bad("unknown header format")),
        }
    }

    pub fn instructions(&self) -> Instructions<'a> {
        Instructions {
            code: self.code,
            position: 0,
        }
    }
}

impl ExceptionClause {
    fn read(reader: &mut Reader<'_>, fat: bool, rva: u32) -> Result<Self> {
        let (flags, try_offset, try_length, handler_offset, handler_length) = if fat {
            (
                reader.u32()?,
                reader.u32()?,
                reader.u32()?,
                reader.u32()?,
                reader.u32()?,
            )
        } else {
            (
                u32::from(reader.u16()?),
                u32::from(reader.u16()?),
                u32::from(reader.u8()?),
                u32::from(reader.u16()?),
                u32::from(reader.u8()?),
            )
        };
        let kind = match flags & 0x7 {
            0x0 => ClauseKind::Catch,
            0x1 => ClauseKind::Filter,
            0x2 => ClauseKind::Finally,
            0x4 => ClauseKind::Fault,
            _ => {
                return Err(Error::BadMethodBody {
                    rva,
                    reason: "unknown exception clause kind",
                });
            }
        };
        Ok(Self {
            kind,
            try_offset,
            try_length,
            handler_offset,
            handler_length,
            class_or_filter: reader.u32()?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SwitchTargets<'a> {
    raw: &'a [u8],
    base: u32,
}

impl<'a> SwitchTargets<'a> {
    pub fn len(&self) -> usize {
        self.raw.len() / 4
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = i64> + 'a {
        let base = i64::from(self.base);
        self.raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(move |c| base + i64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Argument<'a> {
    None,
    Int(i64),
    Float(f64),
    Variable(u16),
    Token(Token),
    Target(i64),
    Switch(SwitchTargets<'a>),
}

#[derive(Debug, Clone, Copy)]
pub struct Instruction<'a> {
    pub offset: u32,
    pub opcode: &'static OpCode,
    pub argument: Argument<'a>,
}

pub struct Instructions<'a> {
    code: &'a [u8],
    position: usize,
}

impl<'a> Iterator for Instructions<'a> {
    type Item = Result<Instruction<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.code.len() {
            return None;
        }
        let result = self.decode();
        if result.is_err() {
            self.position = self.code.len();
        }
        Some(result)
    }
}

impl<'a> Instructions<'a> {
    fn decode(&mut self) -> Result<Instruction<'a>> {
        let mut reader = Reader::at(self.code, self.position, "IL stream")?;
        let offset = self.position as u32;
        let first = reader.u8()?;
        let opcode = if first == 0xFE {
            let second = reader.u8()?;
            opcodes::TWO_BYTE
                .get(second as usize)
                .and_then(Option::as_ref)
                .ok_or(Error::BadOpcode {
                    offset: offset as usize,
                    byte: second,
                })?
        } else {
            opcodes::ONE_BYTE[first as usize]
                .as_ref()
                .ok_or(Error::BadOpcode {
                    offset: offset as usize,
                    byte: first,
                })?
        };
        let argument = match opcode.operand {
            Operand::None => Argument::None,
            Operand::I8 => Argument::Int(i64::from(reader.u8()? as i8)),
            Operand::U8 => Argument::Variable(u16::from(reader.u8()?)),
            Operand::U16 => Argument::Variable(reader.u16()?),
            Operand::I32 => Argument::Int(i64::from(reader.u32()? as i32)),
            Operand::I64 => Argument::Int(reader.u64()? as i64),
            Operand::F32 => Argument::Float(f64::from(f32::from_bits(reader.u32()?))),
            Operand::F64 => Argument::Float(f64::from_bits(reader.u64()?)),
            Operand::Token => Argument::Token(Token(reader.u32()?)),
            Operand::Branch8 => {
                let delta = i64::from(reader.u8()? as i8);
                Argument::Target(reader.position() as i64 + delta)
            }
            Operand::Branch32 => {
                let delta = i64::from(reader.u32()? as i32);
                Argument::Target(reader.position() as i64 + delta)
            }
            Operand::Switch => {
                let count = reader.u32()? as usize;
                let raw = reader.bytes(count.checked_mul(4).ok_or(Error::Truncated {
                    what: "switch table",
                    offset: offset as usize,
                })?)?;
                Argument::Switch(SwitchTargets {
                    raw,
                    base: reader.position() as u32,
                })
            }
        };
        self.position = reader.position();
        Ok(Instruction {
            offset,
            opcode,
            argument,
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct VerifyStats {
    pub instructions: u64,
    pub branches: u64,
    pub tokens: u64,
}

pub fn verify(metadata: &Metadata<'_>, body: &MethodBody<'_>, rva: u32) -> Result<VerifyStats> {
    let bad = |reason| Error::BadMethodBody { rva, reason };
    let length = body.code.len();
    let mut boundary = vec![false; length + 1];
    boundary[length] = true;
    let mut targets: Vec<i64> = Vec::new();
    let mut stats = VerifyStats::default();
    for instruction in body.instructions() {
        let instruction = instruction?;
        boundary[instruction.offset as usize] = true;
        stats.instructions += 1;
        match instruction.argument {
            Argument::Target(target) => targets.push(target),
            Argument::Switch(switch) => targets.extend(switch.iter()),
            Argument::Token(token) => {
                stats.tokens += 1;
                let standalone = instruction.opcode.value == 0x29;
                if !metadata.is_valid_token(token) || (standalone && token.kind() != 0x11) {
                    return Err(bad("instruction references a token that does not exist"));
                }
            }
            _ => {}
        }
    }
    stats.branches = targets.len() as u64;
    for target in targets {
        if target < 0 || target as usize > length || !boundary[target as usize] {
            return Err(bad("branch target is not an instruction boundary"));
        }
    }
    for clause in &body.clauses {
        for (start, size) in [
            (clause.try_offset, clause.try_length),
            (clause.handler_offset, clause.handler_length),
        ] {
            let end = start as usize + size as usize;
            if end > length || !boundary[start as usize] || !boundary[end] {
                return Err(bad("exception clause does not align with instructions"));
            }
        }
        if clause.kind == ClauseKind::Filter
            && (clause.class_or_filter as usize >= length
                || !boundary[clause.class_or_filter as usize])
        {
            return Err(bad("filter does not start on an instruction"));
        }
        if clause.kind == ClauseKind::Catch
            && !metadata.is_valid_token(Token(clause.class_or_filter))
        {
            return Err(bad("catch clause references a type that does not exist"));
        }
    }
    if !body.local_signature.is_null() && !metadata.is_valid_token(body.local_signature) {
        return Err(bad("local signature token does not exist"));
    }
    Ok(stats)
}

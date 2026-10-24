use crate::error::{Error, Result};
use crate::metadata::{TableId, Token};
use crate::reader::Reader;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeSig {
    Void,
    Boolean,
    Char,
    I1,
    U1,
    I2,
    U2,
    I4,
    U4,
    I8,
    U8,
    R4,
    R8,
    String,
    Object,
    TypedReference,
    IntPtr,
    UIntPtr,
    Pointer(Box<TypeSig>),
    ByRef(Box<TypeSig>),
    ValueType(Token),
    Class(Token),
    TypeVar(u32),
    MethodVar(u32),
    Array(Box<ArrayShape>),
    SzArray(Box<TypeSig>),
    GenericInst {
        value_type: bool,
        definition: Token,
        arguments: Vec<TypeSig>,
    },
    FunctionPointer(Box<MethodSig>),
    Modified {
        required: bool,
        modifier: Token,
        inner: Box<TypeSig>,
    },
    Pinned(Box<TypeSig>),
    Sentinel,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArrayShape {
    pub element: TypeSig,
    pub rank: u32,
    pub sizes: Vec<u32>,
    pub lower_bounds: Vec<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallingConvention {
    Default,
    C,
    StdCall,
    ThisCall,
    FastCall,
    VarArg,
    Unmanaged,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodSig {
    pub convention: CallingConvention,
    pub has_this: bool,
    pub explicit_this: bool,
    pub generic_parameters: u32,
    pub return_type: TypeSig,
    pub parameters: Vec<TypeSig>,
    pub vararg_start: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Signature {
    Method(MethodSig),
    Field(TypeSig),
    Property {
        has_this: bool,
        property_type: TypeSig,
        parameters: Vec<TypeSig>,
    },
    Locals(Vec<TypeSig>),
    MethodSpec(Vec<TypeSig>),
}

const MAX_DEPTH: u32 = 64;

struct Decoder<'a> {
    reader: Reader<'a>,
}

fn bad() -> Error {
    Error::BadIndex {
        table: "signature",
        index: 0,
    }
}

impl<'a> Decoder<'a> {
    fn new(blob: &'a [u8]) -> Self {
        Self {
            reader: Reader::new(blob, "signature"),
        }
    }

    fn finish<T>(self, value: T) -> Result<T> {
        if self.reader.is_empty() {
            Ok(value)
        } else {
            Err(bad())
        }
    }

    fn type_def_or_ref(&mut self) -> Result<Token> {
        let raw = self.reader.compressed_u32()?;
        let table = match raw & 0x3 {
            0 => TableId::TypeDef,
            1 => TableId::TypeRef,
            2 => TableId::TypeSpec,
            _ => return Err(bad()),
        };
        Ok(Token::new(table, raw >> 2))
    }

    fn compressed_i32(&mut self) -> Result<i32> {
        let start = self.reader.position();
        let raw = self.reader.compressed_u32()?;
        let width = self.reader.position() - start;
        let bits = match width {
            1 => 7,
            2 => 14,
            _ => 29,
        };
        let value = raw >> 1;
        Ok(if raw & 1 == 0 {
            value as i32
        } else {
            (value as i32) - (1 << (bits - 1))
        })
    }

    fn peek(&self) -> Result<u8> {
        let mut probe = self.reader;
        probe.u8()
    }

    fn type_sig(&mut self, depth: u32) -> Result<TypeSig> {
        if depth > MAX_DEPTH {
            return Err(bad());
        }
        let next = depth + 1;
        let element = self.reader.u8()?;
        Ok(match element {
            0x01 => TypeSig::Void,
            0x02 => TypeSig::Boolean,
            0x03 => TypeSig::Char,
            0x04 => TypeSig::I1,
            0x05 => TypeSig::U1,
            0x06 => TypeSig::I2,
            0x07 => TypeSig::U2,
            0x08 => TypeSig::I4,
            0x09 => TypeSig::U4,
            0x0A => TypeSig::I8,
            0x0B => TypeSig::U8,
            0x0C => TypeSig::R4,
            0x0D => TypeSig::R8,
            0x0E => TypeSig::String,
            0x0F => TypeSig::Pointer(Box::new(self.type_sig(next)?)),
            0x10 => TypeSig::ByRef(Box::new(self.type_sig(next)?)),
            0x11 => TypeSig::ValueType(self.type_def_or_ref()?),
            0x12 => TypeSig::Class(self.type_def_or_ref()?),
            0x13 => TypeSig::TypeVar(self.reader.compressed_u32()?),
            0x14 => {
                let element = self.type_sig(next)?;
                let rank = self.reader.compressed_u32()?;
                let size_count = self.reader.compressed_u32()?;
                let sizes = (0..size_count)
                    .map(|_| self.reader.compressed_u32())
                    .collect::<Result<_>>()?;
                let bound_count = self.reader.compressed_u32()?;
                let lower_bounds = (0..bound_count)
                    .map(|_| self.compressed_i32())
                    .collect::<Result<_>>()?;
                TypeSig::Array(Box::new(ArrayShape {
                    element,
                    rank,
                    sizes,
                    lower_bounds,
                }))
            }
            0x15 => {
                let value_type = match self.reader.u8()? {
                    0x11 => true,
                    0x12 => false,
                    _ => return Err(bad()),
                };
                let definition = self.type_def_or_ref()?;
                let count = self.reader.compressed_u32()?;
                let arguments = (0..count)
                    .map(|_| self.type_sig(next))
                    .collect::<Result<_>>()?;
                TypeSig::GenericInst {
                    value_type,
                    definition,
                    arguments,
                }
            }
            0x16 => TypeSig::TypedReference,
            0x18 => TypeSig::IntPtr,
            0x19 => TypeSig::UIntPtr,
            0x1B => {
                let header = self.reader.u8()?;
                TypeSig::FunctionPointer(Box::new(self.method_sig(header, next)?))
            }
            0x1C => TypeSig::Object,
            0x1D => TypeSig::SzArray(Box::new(self.type_sig(next)?)),
            0x1E => TypeSig::MethodVar(self.reader.compressed_u32()?),
            0x1F | 0x20 => {
                let modifier = self.type_def_or_ref()?;
                TypeSig::Modified {
                    required: element == 0x1F,
                    modifier,
                    inner: Box::new(self.type_sig(next)?),
                }
            }
            0x41 => TypeSig::Sentinel,
            0x45 => TypeSig::Pinned(Box::new(self.type_sig(next)?)),
            _ => return Err(bad()),
        })
    }

    fn method_sig(&mut self, header: u8, depth: u32) -> Result<MethodSig> {
        let convention = match header & 0x0F {
            0x0 => CallingConvention::Default,
            0x1 => CallingConvention::C,
            0x2 => CallingConvention::StdCall,
            0x3 => CallingConvention::ThisCall,
            0x4 => CallingConvention::FastCall,
            0x5 => CallingConvention::VarArg,
            0x9 => CallingConvention::Unmanaged,
            _ => return Err(bad()),
        };
        let generic_parameters = if header & 0x10 != 0 {
            self.reader.compressed_u32()?
        } else {
            0
        };
        let count = self.reader.compressed_u32()?;
        let return_type = self.type_sig(depth)?;
        let mut parameters = Vec::with_capacity(count.min(256) as usize);
        let mut vararg_start = None;
        while parameters.len() < count as usize {
            if self.peek()? == 0x41 {
                self.reader.u8()?;
                if vararg_start.replace(parameters.len()).is_some() {
                    return Err(bad());
                }
                continue;
            }
            parameters.push(self.type_sig(depth)?);
        }
        Ok(MethodSig {
            convention,
            has_this: header & 0x20 != 0,
            explicit_this: header & 0x40 != 0,
            generic_parameters,
            return_type,
            parameters,
            vararg_start,
        })
    }
}

impl Signature {
    pub fn parse(blob: &[u8]) -> Result<Signature> {
        let mut decoder = Decoder::new(blob);
        let header = decoder.reader.u8()?;
        let signature = match header & 0x0F {
            0x06 => Signature::Field(decoder.type_sig(0)?),
            0x07 => {
                let count = decoder.reader.compressed_u32()?;
                Signature::Locals(
                    (0..count)
                        .map(|_| decoder.type_sig(0))
                        .collect::<Result<_>>()?,
                )
            }
            0x08 => {
                let count = decoder.reader.compressed_u32()?;
                let property_type = decoder.type_sig(0)?;
                let parameters = (0..count)
                    .map(|_| decoder.type_sig(0))
                    .collect::<Result<_>>()?;
                Signature::Property {
                    has_this: header & 0x20 != 0,
                    property_type,
                    parameters,
                }
            }
            0x0A => {
                let count = decoder.reader.compressed_u32()?;
                Signature::MethodSpec(
                    (0..count)
                        .map(|_| decoder.type_sig(0))
                        .collect::<Result<_>>()?,
                )
            }
            _ => Signature::Method(decoder.method_sig(header, 0)?),
        };
        decoder.finish(signature)
    }

    pub fn parse_type(blob: &[u8]) -> Result<TypeSig> {
        let mut decoder = Decoder::new(blob);
        let sig = decoder.type_sig(0)?;
        decoder.finish(sig)
    }
}

impl TypeSig {
    pub fn tokens(&self, out: &mut Vec<Token>) {
        match self {
            TypeSig::ValueType(token) | TypeSig::Class(token) => out.push(*token),
            TypeSig::Pointer(inner)
            | TypeSig::ByRef(inner)
            | TypeSig::SzArray(inner)
            | TypeSig::Pinned(inner) => inner.tokens(out),
            TypeSig::Array(shape) => shape.element.tokens(out),
            TypeSig::GenericInst {
                definition,
                arguments,
                ..
            } => {
                out.push(*definition);
                arguments.iter().for_each(|a| a.tokens(out));
            }
            TypeSig::FunctionPointer(method) => method.tokens(out),
            TypeSig::Modified {
                modifier, inner, ..
            } => {
                out.push(*modifier);
                inner.tokens(out);
            }
            _ => {}
        }
    }
}

impl MethodSig {
    pub fn tokens(&self, out: &mut Vec<Token>) {
        self.return_type.tokens(out);
        self.parameters.iter().for_each(|p| p.tokens(out));
    }
}

impl Signature {
    pub fn tokens(&self, out: &mut Vec<Token>) {
        match self {
            Signature::Method(method) => method.tokens(out),
            Signature::Field(field) => field.tokens(out),
            Signature::Property {
                property_type,
                parameters,
                ..
            } => {
                property_type.tokens(out);
                parameters.iter().for_each(|p| p.tokens(out));
            }
            Signature::Locals(types) | Signature::MethodSpec(types) => {
                types.iter().for_each(|t| t.tokens(out))
            }
        }
    }
}

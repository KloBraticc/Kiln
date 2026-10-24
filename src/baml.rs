use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::il::{Argument, MethodBody};
use crate::loader::{AssemblySet, TypeHandle};
use crate::metadata::{TableId, Token};
use crate::reader::Reader;
use crate::resources::seven_bit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeId {
    Known(u16),
    Local(u16),
    Named(&'static str),
}

const TYPE_EXTENSION: TypeId = TypeId::Named("System.Windows.Markup.TypeExtension");
const STATIC_EXTENSION: TypeId = TypeId::Named("System.Windows.Markup.StaticExtension");
const STATIC_RESOURCE: TypeId = TypeId::Named("System.Windows.StaticResourceExtension");

impl TypeId {
    fn from_raw(raw: u16) -> Self {
        if raw & 0x8000 != 0 {
            TypeId::Known((raw as i16).unsigned_abs())
        } else {
            TypeId::Local(raw)
        }
    }
}

#[derive(Debug, Default)]
pub struct Document {
    pub assemblies: HashMap<u16, String>,
    pub types: HashMap<u16, (u16, String)>,
    pub attributes: HashMap<u16, (TypeId, String)>,
    pub elements: Vec<TypeId>,
    pub referenced: Vec<TypeId>,
    pub properties: Vec<u16>,
    pub records: usize,
}

fn fixed_size(record: u8) -> Option<usize> {
    Some(match record {
        0x01 => 6,
        0x02 | 0x04 | 0x08 | 0x0A | 0x0C | 0x0E | 0x1A | 0x29 | 0x2A | 0x2B | 0x31 => 0,
        0x03 | 0x30 | 0x37 => 3,
        0x07 | 0x09 | 0x0B | 0x0D | 0x2C | 0x2E | 0x32 => 2,
        0x21 | 0x22 | 0x25 | 0x2D | 0x36 | 0x38 => 4,
        0x23 => 6,
        0x27 | 0x28 => 9,
        0x35 => 8,
        _ => return None,
    })
}

fn string<'a>(reader: &mut Reader<'a>) -> Result<String> {
    let length = seven_bit(reader)? as usize;
    Ok(String::from_utf8_lossy(reader.bytes(length)?).into_owned())
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

pub fn is_baml(data: &[u8]) -> bool {
    let signature: Vec<u8> = "MSBAML".encode_utf16().flat_map(u16::to_le_bytes).collect();
    data.len() > 16 && data[4..16] == signature[..]
}

pub fn parse(data: &[u8]) -> Result<Document> {
    if !is_baml(data) {
        return Err(Error::BadMagic("BAML"));
    }
    let mut reader = Reader::new(data, "BAML");
    let length = reader.u32()? as usize;
    reader.skip(length.next_multiple_of(4))?;
    reader.skip(12)?;
    let mut document = Document::default();
    while !reader.is_empty() {
        let record = reader.u8()?;
        document.records += 1;
        if let Some(size) = fixed_size(record) {
            let body = reader.bytes(size)?;
            match record {
                0x03 if body[2] & 0x02 != 0 => {
                    document.referenced.push(TypeId::from_raw(u16_at(body, 0)));
                }
                0x27 => {
                    document.elements.push(TYPE_EXTENSION);
                    document.referenced.push(TypeId::from_raw(u16_at(body, 0)));
                }
                0x03 | 0x30 | 0x28 => {
                    document.elements.push(TypeId::from_raw(u16_at(body, 0)));
                }
                0x22 => {
                    document.properties.push(u16_at(body, 0));
                    document.elements.push(TYPE_EXTENSION);
                    document.referenced.push(TypeId::from_raw(u16_at(body, 2)));
                }
                0x37 => {
                    let value = u16_at(body, 1);
                    if body[0] & 0x01 != 0 {
                        document.elements.push(TYPE_EXTENSION);
                        document.referenced.push(TypeId::from_raw(value));
                    } else if body[0] & 0x02 != 0 {
                        document.elements.push(STATIC_EXTENSION);
                        document.properties.push(value);
                    }
                }
                0x38 => {
                    document.properties.push(u16_at(body, 0));
                    document.elements.push(STATIC_RESOURCE);
                }
                0x2C => document.referenced.push(TypeId::from_raw(u16_at(body, 0))),
                0x23 => {
                    document.properties.push(u16_at(body, 0));
                    let flags = u16_at(body, 2);
                    let value = u16_at(body, 4);
                    document.elements.push(TypeId::Known(flags & 0x0FFF));
                    if flags & 0x4000 != 0 {
                        document.referenced.push(TypeId::from_raw(value));
                    } else if flags & 0x2000 != 0 {
                        document.properties.push(value);
                    }
                }
                0x07 | 0x09 | 0x0B | 0x0D | 0x21 | 0x2E => {
                    document.properties.push(u16_at(body, 0));
                }
                _ => {}
            }
            if record == 0x02 {
                break;
            }
            continue;
        }
        if record == 0x2F {
            document.elements.push(TypeId::from_raw(reader.u16()?));
            string(&mut reader)?;
            continue;
        }
        let start = reader.position();
        let size = seven_bit(&mut reader)? as usize;
        let header = reader.position() - start;
        let payload = size
            .checked_sub(header)
            .ok_or(Error::BadMagic("BAML record size"))?;
        let mut body = Reader::new(reader.bytes(payload)?, "BAML record");
        match record {
            0x1C => {
                let id = body.u16()?;
                document.assemblies.insert(id, string(&mut body)?);
            }
            0x1D | 0x1E => {
                let id = body.u16()?;
                let assembly = body.u16()? & 0x0FFF;
                let name = string(&mut body)?;
                document.types.insert(id, (assembly, name));
                if record == 0x1E {
                    document.referenced.push(TypeId::from_raw(body.u16()?));
                }
            }
            0x1F => {
                let id = body.u16()?;
                let owner = TypeId::from_raw(body.u16()?);
                body.u8()?;
                document.attributes.insert(id, (owner, string(&mut body)?));
            }
            0x05 | 0x12 => document.properties.push(body.u16()?),
            0x06 => {
                document.properties.push(body.u16()?);
                let serializer = body.u16()? & !0x4000;
                document.referenced.push(if serializer & 0x8000 != 0 {
                    TypeId::from_raw(serializer)
                } else {
                    TypeId::Known(serializer)
                });
            }
            0x24 => {
                document.properties.push(body.u16()?);
                string(&mut body)?;
                document.referenced.push(TypeId::from_raw(body.u16()?));
            }
            0x11 => {
                string(&mut body)?;
                document.referenced.push(TypeId::from_raw(body.u16()?));
            }
            _ => {}
        }
    }
    Ok(document)
}

pub struct KnownTypes {
    types: HashMap<u16, TypeHandle>,
}

impl KnownTypes {
    pub fn load(set: &AssemblySet) -> Self {
        let mut types = HashMap::new();
        if let Some(found) = Self::scan(set) {
            types = found;
        }
        Self { types }
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn get(&self, id: u16) -> Option<TypeHandle> {
        self.types.get(&id).copied()
    }

    fn scan(set: &AssemblySet) -> Option<HashMap<u16, TypeHandle>> {
        let module = set.find_assembly("PresentationFramework")?;
        let indexer = set.find_type_by_name(module, "System.Windows.Markup", "TypeIndexer")?;
        let metadata = &set.modules[module].metadata;
        let row = metadata.method_range(indexer.row).ok()?.find(|&row| {
            metadata.string_cell(TableId::MethodDef, row, 3).ok() == Some("InitializeOneType")
        })?;
        let rva = metadata.cell(TableId::MethodDef, row, 0).ok()?;
        let body =
            MethodBody::parse(set.modules[module].image.tail_at_rva(rva).ok()?, rva).ok()?;
        let instructions: Vec<_> = body.instructions().collect::<Result<Vec<_>>>().ok()?;
        let tokens: Vec<(u32, Token)> = instructions
            .iter()
            .filter_map(|i| match (i.opcode.value, i.argument) {
                (0xD0, Argument::Token(token)) => Some((i.offset, token)),
                _ => None,
            })
            .collect();
        let mut types = HashMap::new();
        for (index, instruction) in instructions.iter().enumerate() {
            let Argument::Switch(targets) = instruction.argument else {
                continue;
            };
            let base = match (index.checked_sub(2).map(|i| &instructions[i]), instructions.get(index.wrapping_sub(1))) {
                (Some(constant), Some(subtract)) if subtract.opcode.value == 0x59 => {
                    match constant.argument {
                        Argument::Int(value) => value,
                        _ => match constant.opcode.value {
                            0x16..=0x1E => i64::from(constant.opcode.value) - 0x16,
                            _ => 0,
                        },
                    }
                }
                _ => 0,
            };
            for (case, target) in targets.iter().enumerate() {
                let Some(&(_, token)) = tokens.iter().find(|(offset, _)| i64::from(*offset) >= target)
                else {
                    continue;
                };
                let Ok(handle) = set.resolve_type_token(module, token) else {
                    continue;
                };
                types.entry((base + case as i64) as u16).or_insert(handle);
            }
        }
        Some(types)
    }
}

pub struct Resolved {
    pub handle: Option<TypeHandle>,
    pub name: String,
}

pub fn resolve_type(
    set: &AssemblySet,
    known: &KnownTypes,
    document: &Document,
    id: TypeId,
) -> Resolved {
    match id {
        TypeId::Known(value) => match known.get(value) {
            Some(handle) => Resolved {
                handle: Some(handle),
                name: set.full_type_name(handle).unwrap_or_default(),
            },
            None => Resolved {
                handle: None,
                name: format!("known type {value}"),
            },
        },
        TypeId::Named(name) => Resolved {
            handle: find_type(set, None, name),
            name: name.to_owned(),
        },
        TypeId::Local(value) => {
            let Some((assembly, name)) = document.types.get(&value) else {
                return Resolved {
                    handle: None,
                    name: format!("type {value}"),
                };
            };
            let assembly = document
                .assemblies
                .get(assembly)
                .and_then(|full| full.split(',').next())
                .and_then(|simple| set.find_assembly(simple.trim()));
            Resolved {
                handle: find_type(set, assembly, name),
                name: name.clone(),
            }
        }
    }
}

pub fn find_type(set: &AssemblySet, preferred: Option<usize>, full: &str) -> Option<TypeHandle> {
    let mut parts = full.split('+');
    let outer = parts.next()?;
    let (namespace, name) = outer.rsplit_once('.').unwrap_or(("", outer));
    let modules = preferred.into_iter().chain(0..set.modules.len());
    for module in modules {
        let Some(mut handle) = set.find_named(module, namespace, name) else {
            continue;
        };
        let mut ok = true;
        for inner in parts.clone() {
            match set.find_nested(handle, inner) {
                Some(found) => handle = found,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Some(handle);
        }
    }
    None
}

use std::collections::HashMap;

use crate::codegen::asm::{Absolute, AbsoluteTarget};
use crate::codegen::{CompiledMethod, Helpers, MethodKey, VarType};
use crate::loader::TypeHandle;
use crate::objects::{ElementKind, RuntimeType};

const MAGIC: &[u8; 8] = b"KILNPAK1";

pub struct Payload {
    pub application: usize,
    pub modules: Vec<(String, Vec<u8>)>,
    pub properties: Vec<(String, String)>,
    pub methods: HashMap<MethodKey, CompiledMethod>,
}

fn helper_addresses(helpers: &Helpers) -> Vec<u64> {
    let mut addresses = helpers.float_to_int.to_vec();
    addresses.extend([
        helpers.uint64_to_double,
        helpers.remainder32,
        helpers.remainder64,
        helpers.allocate,
        helpers.is_instance,
        helpers.initialize_array,
        helpers.throw,
        helpers.initialize,
    ]);
    addresses
}

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.u64(value.len() as u64);
        self.0.extend_from_slice(value);
    }
    fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
    fn handle(&mut self, handle: TypeHandle) {
        self.u32(handle.module as u32);
        self.u32(handle.row);
    }
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(count).filter(|end| *end <= self.data.len()).ok_or("payload is truncated")?;
        let slice = &self.data[self.at..end];
        self.at = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| "bad u32")?))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| "bad u64")?))
    }
    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let length = self.u64()? as usize;
        self.take(length)
    }
    fn text(&mut self) -> Result<String, String> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| "payload text is not UTF-8".into())
    }
    fn handle(&mut self) -> Result<TypeHandle, String> {
        Ok(TypeHandle {
            module: self.u32()? as usize,
            row: self.u32()?,
        })
    }
}

fn var_code(var: VarType) -> u8 {
    match var {
        VarType::I8 => 0,
        VarType::U8 => 1,
        VarType::I16 => 2,
        VarType::U16 => 3,
        VarType::I32 => 4,
        VarType::I64 => 5,
        VarType::F32 => 6,
        VarType::F64 => 7,
        VarType::Ref => 8,
        VarType::Pointer => 9,
    }
}

fn var_from(code: u8) -> Result<VarType, String> {
    Ok(match code {
        0 => VarType::I8,
        1 => VarType::U8,
        2 => VarType::I16,
        3 => VarType::U16,
        4 => VarType::I32,
        5 => VarType::I64,
        6 => VarType::F32,
        7 => VarType::F64,
        8 => VarType::Ref,
        9 => VarType::Pointer,
        _ => return Err(format!("unknown value type {code}")),
    })
}

fn element_code(kind: ElementKind) -> u8 {
    match kind {
        ElementKind::I8 => 0,
        ElementKind::U8 => 1,
        ElementKind::I16 => 2,
        ElementKind::U16 => 3,
        ElementKind::I32 => 4,
        ElementKind::I64 => 5,
        ElementKind::F32 => 6,
        ElementKind::F64 => 7,
        ElementKind::Reference => 8,
    }
}

fn element_from(code: u8) -> Result<ElementKind, String> {
    Ok(match code {
        0 => ElementKind::I8,
        1 => ElementKind::U8,
        2 => ElementKind::I16,
        3 => ElementKind::U16,
        4 => ElementKind::I32,
        5 => ElementKind::I64,
        6 => ElementKind::F32,
        7 => ElementKind::F64,
        8 => ElementKind::Reference,
        _ => return Err(format!("unknown element kind {code}")),
    })
}

fn write_type(writer: &mut Writer, kind: RuntimeType) -> Result<(), String> {
    match kind {
        RuntimeType::Class(handle) => {
            writer.u8(0);
            writer.handle(handle);
        }
        RuntimeType::Array(element) => {
            writer.u8(1);
            writer.u8(element_code(element));
        }
        RuntimeType::ValueArray(handle) => {
            writer.u8(2);
            writer.handle(handle);
        }
        RuntimeType::String => writer.u8(3),
        RuntimeType::Exact(_) | RuntimeType::ExactArray(_) => {
            return Err("compiled code refers to a fallback type".into());
        }
    }
    Ok(())
}

fn read_type(reader: &mut Reader<'_>) -> Result<RuntimeType, String> {
    Ok(match reader.u8()? {
        0 => RuntimeType::Class(reader.handle()?),
        1 => RuntimeType::Array(element_from(reader.u8()?)?),
        2 => RuntimeType::ValueArray(reader.handle()?),
        3 => RuntimeType::String,
        other => return Err(format!("unknown runtime type {other}")),
    })
}

fn write_method(writer: &mut Writer, key: &MethodKey, method: &CompiledMethod, helpers: &[u64]) -> Result<(), String> {
    writer.u32(key.module as u32);
    writer.u32(key.row);
    writer.bytes(&method.code);
    writer.u32(method.absolutes.len() as u32);
    for absolute in &method.absolutes {
        writer.u64(absolute.at as u64);
        match absolute.target {
            AbsoluteTarget::Method(index) => {
                writer.u8(0);
                writer.u64(index as u64);
            }
            AbsoluteTarget::Address(address) => {
                let index = helpers
                    .iter()
                    .position(|helper| *helper == address)
                    .ok_or("compiled code refers to an address that is not a runtime helper")?;
                writer.u8(1);
                writer.u32(index as u32);
            }
            AbsoluteTarget::Type(kind) => {
                writer.u8(2);
                write_type(writer, kind)?;
            }
            AbsoluteTarget::Static(module, row, reference) => {
                writer.u8(3);
                writer.u32(module as u32);
                writer.u32(row);
                writer.u8(u8::from(reference));
            }
            AbsoluteTarget::InitFlag(handle) => {
                writer.u8(4);
                writer.handle(handle);
            }
            AbsoluteTarget::Literal(module, offset) => {
                writer.u8(5);
                writer.u32(module as u32);
                writer.u32(offset);
            }
            AbsoluteTarget::Rva(module, row) => {
                writer.u8(6);
                writer.u32(module as u32);
                writer.u32(row);
            }
            AbsoluteTarget::Array(kind, module, token) => {
                writer.u8(7);
                writer.u8(element_code(kind));
                writer.u32(module as u32);
                writer.u32(token);
            }
        }
    }
    writer.u32(method.callees.len() as u32);
    for callee in &method.callees {
        writer.u32(callee.module as u32);
        writer.u32(callee.row);
    }
    writer.u32(method.parameters.len() as u32);
    for parameter in &method.parameters {
        writer.u8(var_code(*parameter));
    }
    writer.u8(method.returns.map_or(0xFF, var_code));
    writer.u8(u8::from(method.uses_objects));
    writer.u32(method.references.len() as u32);
    for reference in &method.references {
        writer.u32(*reference as u32);
    }
    Ok(())
}

fn read_method(reader: &mut Reader<'_>, helpers: &[u64]) -> Result<(MethodKey, CompiledMethod), String> {
    let key = MethodKey {
        module: reader.u32()? as usize,
        row: reader.u32()?,
    };
    let code = reader.bytes()?.to_vec();
    let mut absolutes = Vec::new();
    for _ in 0..reader.u32()? {
        let at = reader.u64()? as usize;
        let target = match reader.u8()? {
            0 => AbsoluteTarget::Method(reader.u64()? as usize),
            1 => AbsoluteTarget::Address(
                *helpers
                    .get(reader.u32()? as usize)
                    .ok_or("payload refers to an unknown runtime helper")?,
            ),
            2 => AbsoluteTarget::Type(read_type(reader)?),
            3 => AbsoluteTarget::Static(reader.u32()? as usize, reader.u32()?, reader.u8()? != 0),
            4 => AbsoluteTarget::InitFlag(reader.handle()?),
            5 => AbsoluteTarget::Literal(reader.u32()? as usize, reader.u32()?),
            6 => AbsoluteTarget::Rva(reader.u32()? as usize, reader.u32()?),
            7 => AbsoluteTarget::Array(element_from(reader.u8()?)?, reader.u32()? as usize, reader.u32()?),
            other => return Err(format!("unknown relocation {other}")),
        };
        absolutes.push(Absolute { at, target });
    }
    let mut callees = Vec::new();
    for _ in 0..reader.u32()? {
        callees.push(MethodKey {
            module: reader.u32()? as usize,
            row: reader.u32()?,
        });
    }
    let mut parameters = Vec::new();
    for _ in 0..reader.u32()? {
        parameters.push(var_from(reader.u8()?)?);
    }
    let returns = match reader.u8()? {
        0xFF => None,
        code => Some(var_from(code)?),
    };
    let uses_objects = reader.u8()? != 0;
    let mut references = Vec::new();
    for _ in 0..reader.u32()? {
        references.push(reader.u32()? as i32);
    }
    Ok((
        key,
        CompiledMethod {
            code,
            absolutes,
            callees,
            parameters,
            returns,
            uses_objects,
            references,
        },
    ))
}

pub fn encode(payload: &Payload, helpers: &Helpers) -> Result<Vec<u8>, String> {
    let helpers = helper_addresses(helpers);
    let mut writer = Writer::default();
    writer.u32(payload.application as u32);
    writer.u32(payload.modules.len() as u32);
    for (name, bytes) in &payload.modules {
        writer.text(name);
        writer.bytes(bytes);
    }
    writer.u32(payload.properties.len() as u32);
    for (name, value) in &payload.properties {
        writer.text(name);
        writer.text(value);
    }
    let mut keys: Vec<&MethodKey> = payload.methods.keys().collect();
    keys.sort_by_key(|key| (key.module, key.row));
    writer.u32(keys.len() as u32);
    for key in keys {
        write_method(&mut writer, key, &payload.methods[key], &helpers)?;
    }
    let length = writer.0.len() as u64;
    writer.u64(length);
    writer.0.extend_from_slice(MAGIC);
    Ok(writer.0)
}

pub fn decode(image: &'static [u8], helpers: &Helpers) -> Result<Option<(Payload, Vec<&'static [u8]>)>, String> {
    if image.len() < 16 || &image[image.len() - 8..] != MAGIC {
        return Ok(None);
    }
    let length = u64::from_le_bytes(image[image.len() - 16..image.len() - 8].try_into().map_err(|_| "bad trailer")?) as usize;
    let start = (image.len() - 16).checked_sub(length).ok_or("payload length is wrong")?;
    let helpers = helper_addresses(helpers);
    let mut reader = Reader {
        data: &image[start..image.len() - 16],
        at: 0,
    };
    let application = reader.u32()? as usize;
    let mut modules = Vec::new();
    let mut slices = Vec::new();
    for _ in 0..reader.u32()? {
        let name = reader.text()?;
        let bytes = reader.bytes()?;
        slices.push(bytes);
        modules.push((name, Vec::new()));
    }
    let mut properties = Vec::new();
    for _ in 0..reader.u32()? {
        properties.push((reader.text()?, reader.text()?));
    }
    let mut methods = HashMap::new();
    for _ in 0..reader.u32()? {
        let (key, method) = read_method(&mut reader, &helpers)?;
        methods.insert(key, method);
    }
    Ok(Some((
        Payload {
            application,
            modules,
            properties,
            methods,
        },
        slices,
    )))
}

pub fn set_subsystem(executable: &mut [u8], subsystem: u16) -> Result<(), String> {
    let header = u32::from_le_bytes(executable.get(0x3C..0x40).ok_or("not a PE file")?.try_into().map_err(|_| "bad header")?) as usize;
    let field = header + 24 + 68;
    let slot = executable.get_mut(field..field + 2).ok_or("PE header is truncated")?;
    slot.copy_from_slice(&subsystem.to_le_bytes());
    Ok(())
}

pub fn subsystem_of(executable: &[u8]) -> Option<u16> {
    let header = u32::from_le_bytes(executable.get(0x3C..0x40)?.try_into().ok()?) as usize;
    let field = header + 24 + 68;
    Some(u16::from_le_bytes(executable.get(field..field + 2)?.try_into().ok()?))
}

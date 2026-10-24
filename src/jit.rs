use std::collections::HashMap;

use crate::codegen::asm::{
    ARGUMENT_REGISTERS, AbsoluteTarget, Assembler, R8, R9, R10, RAX, RBP, RCX, RDX, RSP,
};
use crate::codegen::{CompiledMethod, Helpers, MethodKey, VarType};
use crate::signature::TypeSig;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut u8, size: usize, allocation: u32, protection: u32) -> *mut u8;
    fn VirtualProtect(address: *mut u8, size: usize, protection: u32, old: *mut u32) -> i32;
    fn VirtualFree(address: *mut u8, size: usize, kind: u32) -> i32;
    fn FlushInstructionCache(process: isize, address: *const u8, size: usize) -> i32;
    fn GetCurrentProcess() -> isize;
}

const MEM_COMMIT_RESERVE: u32 = 0x3000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_EXECUTE_READ: u32 = 0x20;

extern "win64" fn to_i8(value: f64) -> i64 {
    value as i32 as i8 as i64
}

extern "win64" fn to_u8(value: f64) -> i64 {
    value as i32 as u8 as i64
}

extern "win64" fn to_i16(value: f64) -> i64 {
    value as i32 as i16 as i64
}

extern "win64" fn to_u16(value: f64) -> i64 {
    value as i32 as u16 as i64
}

extern "win64" fn to_i32(value: f64) -> i64 {
    value as i32 as i64
}

extern "win64" fn to_u32(value: f64) -> i64 {
    value as u32 as i64
}

extern "win64" fn to_i64(value: f64) -> i64 {
    value as i64
}

extern "win64" fn to_u64(value: f64) -> i64 {
    value as u64 as i64
}

extern "win64" fn uint64_to_double(value: u64) -> f64 {
    value as f64
}

extern "win64" fn remainder32(a: f32, b: f32) -> f32 {
    a % b
}

extern "win64" fn remainder64(a: f64, b: f64) -> f64 {
    a % b
}

pub fn helpers() -> Helpers {
    Helpers {
        float_to_int: [
            to_i8 as *const () as u64,
            to_u8 as *const () as u64,
            to_i16 as *const () as u64,
            to_u16 as *const () as u64,
            to_i32 as *const () as u64,
            to_u32 as *const () as u64,
            to_i64 as *const () as u64,
            to_u64 as *const () as u64,
        ],
        uint64_to_double: uint64_to_double as *const () as u64,
        remainder32: remainder32 as *const () as u64,
        remainder64: remainder64 as *const () as u64,
        allocate: crate::runtime::allocate as *const () as u64,
        is_instance: crate::runtime::is_instance as *const () as u64,
        initialize_array: crate::runtime::initialize_array as *const () as u64,
        throw: crate::interpreter::compiled_throw as *const () as u64,
        initialize: crate::interpreter::compiled_initialize as *const () as u64,
    }
}

pub struct ExecutableMemory {
    base: *mut u8,
    size: usize,
}

impl ExecutableMemory {
    pub fn address(&self, offset: usize) -> u64 {
        self.base as u64 + offset as u64
    }
}

impl Drop for ExecutableMemory {
    fn drop(&mut self) {
        unsafe {
            VirtualFree(self.base, 0, MEM_RELEASE);
        }
        let _ = self.size;
    }
}

pub struct Image {
    memory: ExecutableMemory,
    thunks: HashMap<MethodKey, usize>,
}

type Thunk = unsafe extern "win64" fn(*const u64, *mut [u64; 2]);

impl Image {
    pub fn link(
        methods: &HashMap<MethodKey, CompiledMethod>,
        model: &crate::objects::ObjectModel<'_>,
    ) -> Result<Image, String> {
        Self::link_with_fallback(methods, model, &|_| None, &|_, _| None)
    }

    pub fn link_with_fallback(
        methods: &HashMap<MethodKey, CompiledMethod>,
        model: &crate::objects::ObjectModel<'_>,
        fallback: &dyn Fn(&MethodKey) -> Option<u64>,
        arrays: &dyn Fn(usize, u32) -> Option<u64>,
    ) -> Result<Image, String> {
        let state = crate::runtime::install();
        let set = model.set;
        let mut keys: Vec<&MethodKey> = methods.keys().collect();
        keys.sort_by_key(|k| (k.module, k.row));
        let mut code = Vec::new();
        let mut offsets: HashMap<MethodKey, usize> = HashMap::new();
        for key in &keys {
            code.resize(code.len().next_multiple_of(16), 0xCC);
            offsets.insert((*key).clone(), code.len());
            code.extend_from_slice(&methods[*key].code);
        }
        let mut thunk_offsets = HashMap::new();
        let mut pending = Vec::new();
        for key in &keys {
            code.resize(code.len().next_multiple_of(16), 0xCC);
            let thunk = build_thunk(&methods[*key].parameters);
            thunk_offsets.insert((*key).clone(), code.len());
            pending.push((code.len() + thunk.1, offsets[*key]));
            code.extend_from_slice(&thunk.0);
        }

        code.resize(code.len().next_multiple_of(16), 0xCC);
        let trap_offset = code.len();
        code.extend_from_slice(&[0x48, 0x83, 0xEC, 0x28, 0x48, 0xB8]);
        code.extend_from_slice(&(crate::runtime::trap as *const () as u64).to_le_bytes());
        code.extend_from_slice(&[0xFF, 0xD0, 0x48, 0x83, 0xC4, 0x28, 0xC3]);

        let mut relocations: Vec<(usize, Relocation)> = Vec::new();
        for key in &keys {
            let method = &methods[*key];
            let start = offsets[*key];
            for absolute in &method.absolutes {
                let relocation = match absolute.target {
                    AbsoluteTarget::Address(address) => Relocation::Address(address),
                    AbsoluteTarget::Method(index) => {
                        let callee = &method.callees[index];
                        match offsets.get(callee) {
                            Some(offset) => Relocation::Offset(*offset),
                            None => Relocation::Address(
                                fallback(callee).ok_or("a callee was not compiled")?,
                            ),
                        }
                    }
                    AbsoluteTarget::Type(kind) => {
                        Relocation::Address(state.type_record(model, kind)?)
                    }
                    AbsoluteTarget::Static(module, row, reference) => {
                        Relocation::Address(state.static_slot(model, module, row, reference))
                    }
                    AbsoluteTarget::InitFlag(handle) => {
                        Relocation::Address(state.init_flag(handle))
                    }
                    AbsoluteTarget::Literal(module, offset) => {
                        Relocation::Address(state.string_literal(set, model, module, offset)?)
                    }
                    AbsoluteTarget::Array(kind, module, token) => match arrays(module, token) {
                        Some(record) => Relocation::Address(record),
                        None => Relocation::Address(state.type_record(model, crate::objects::RuntimeType::Array(kind))?),
                    },
                    AbsoluteTarget::Rva(module, row) => {
                        Relocation::Address(crate::runtime::rva_data(set, module, row)?)
                    }
                };
                relocations.push((start + absolute.at, relocation));
            }
        }
        for (at, target) in pending {
            relocations.push((at, Relocation::Offset(target)));
        }

        let memory = ExecutableMemory::new_at_any(&code, &relocations)?;
        let trap = memory.address(trap_offset);
        for key in &keys {
            let start = memory.address(offsets[*key]) as usize;
            let method = &methods[*key];
            state.register_method(start, start + method.code.len(), &method.references);
            let metadata = &set.modules[key.module].metadata;
            let name = format!(
                "{}::{}::{}",
                set.modules[key.module].name,
                set.modules[key.module]
                    .method_owner(key.row)
                    .ok()
                    .flatten()
                    .map(|owner| set.type_name(crate::loader::TypeHandle { module: key.module, row: owner }))
                    .unwrap_or_default(),
                metadata
                    .string_cell(crate::metadata::TableId::MethodDef, key.row, 3)
                    .unwrap_or("?")
            );
            state.method_names.insert(start, name);
            state
                .code_addresses
                .insert((*key).clone(), memory.address(offsets[*key]));
        }
        state.trap_address = trap;
        let handles: Vec<crate::loader::TypeHandle> = state
            .types
            .keys()
            .filter_map(|kind| match kind {
                crate::objects::RuntimeType::Class(handle) => Some(*handle),
                _ => None,
            })
            .collect();
        for handle in handles {
            let info = model.class(handle)?;
            let addresses: Vec<u64> = info
                .vtable
                .iter()
                .map(|method| match method {
                    Some(key) => state
                        .code_addresses
                        .get(key)
                        .copied()
                        .or_else(|| fallback(key))
                        .unwrap_or(trap),
                    None => trap,
                })
                .collect();
            if let Some(record) = state.types.get_mut(&crate::objects::RuntimeType::Class(handle)) {
                record.words[7..7 + addresses.len()].copy_from_slice(&addresses);
            }
        }
        Ok(Image {
            memory,
            thunks: thunk_offsets,
        })
    }

    pub fn call(&self, key: &MethodKey, arguments: &[u64]) -> Option<[u64; 2]> {
        let offset = *self.thunks.get(key)?;
        let thunk: Thunk = unsafe { std::mem::transmute(self.memory.address(offset) as usize) };
        let mut result = [0u64; 2];
        unsafe { thunk(arguments.as_ptr(), &mut result) };
        Some(result)
    }
}

enum Relocation {
    Address(u64),
    Offset(usize),
}

impl ExecutableMemory {
    fn new_at_any(code: &[u8], relocations: &[(usize, Relocation)]) -> Result<Self, String> {
        let size = code.len().max(1).next_multiple_of(4096);
        unsafe {
            let base = VirtualAlloc(
                std::ptr::null_mut(),
                size,
                MEM_COMMIT_RESERVE,
                PAGE_READWRITE,
            );
            if base.is_null() {
                return Err("executable memory could not be allocated".into());
            }
            std::ptr::copy_nonoverlapping(code.as_ptr(), base, code.len());
            for (at, relocation) in relocations {
                let value = match relocation {
                    Relocation::Address(address) => *address,
                    Relocation::Offset(offset) => base as u64 + *offset as u64,
                };
                std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), base.add(*at), 8);
            }
            let mut old = 0u32;
            if VirtualProtect(base, size, PAGE_EXECUTE_READ, &mut old) == 0 {
                VirtualFree(base, 0, MEM_RELEASE);
                return Err("executable memory could not be protected".into());
            }
            FlushInstructionCache(GetCurrentProcess(), base, size);
            Ok(Self { base, size })
        }
    }
}

static STUBS: std::sync::Mutex<Vec<(usize, usize, usize)>> = std::sync::Mutex::new(Vec::new());

pub fn emit_fallback_stub(parameters: &[VarType], index: u64) -> Option<u64> {
    let entry = crate::interpreter::fallback_entry as *const () as u64;
    emit_code(&build_fallback_stub(parameters, index, entry))
}

fn emit_code(code: &[u8]) -> Option<u64> {
    let mut pages = STUBS.lock().ok()?;
    let needs_page = pages
        .last()
        .is_none_or(|&(_, size, used)| used.next_multiple_of(16) + code.len() > size);
    if needs_page {
        let size = 64 * 1024;
        let base = unsafe { VirtualAlloc(std::ptr::null_mut(), size, MEM_COMMIT_RESERVE, PAGE_READWRITE) };
        if base.is_null() {
            return None;
        }
        pages.push((base as usize, size, 0));
    }
    let page = pages.last_mut()?;
    let at = page.2.next_multiple_of(16);
    let address = page.0 + at;
    unsafe {
        let mut old = 0u32;
        VirtualProtect(page.0 as *mut u8, page.1, PAGE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), address as *mut u8, code.len());
        VirtualProtect(page.0 as *mut u8, page.1, PAGE_EXECUTE_READ, &mut old);
        FlushInstructionCache(GetCurrentProcess(), page.0 as *const u8, page.1);
    }
    page.2 = at + code.len();
    Some(address as u64)
}

fn build_fallback_stub(parameters: &[VarType], index: u64, entry: u64) -> Vec<u8> {
    let mut asm = Assembler::default();
    let count = parameters.len().max(1) as i32;
    asm.prologue((8 * count as u32 + 32).next_multiple_of(16));
    for (position, var) in parameters.iter().enumerate() {
        let slot = -8 * (count - position as i32);
        if position < 4 {
            match var {
                VarType::F32 => asm.sse_store(false, RBP, slot, position as u8),
                VarType::F64 => asm.sse_store(true, RBP, slot, position as u8),
                _ => asm.store(true, RBP, slot, ARGUMENT_REGISTERS[position]),
            }
        } else {
            asm.load(true, RAX, RBP, 48 + 8 * (position as i32 - 4));
            asm.store(true, RBP, slot, RAX);
        }
    }
    asm.move_immediate(RCX, index);
    asm.load_address(RDX, RBP, -8 * count);
    asm.load(true, R8, RBP, 0);
    asm.load_address(R9, RBP, 16);
    asm.move_immediate(RAX, entry);
    asm.call_register(RAX);
    asm.xmm_from_gpr(true, 0, RAX);
    asm.epilogue();
    let (code, _) = asm.finish().expect("fallback stub has no labels");
    code
}

fn build_thunk(parameters: &[VarType]) -> (Vec<u8>, usize) {
    let mut asm = Assembler::default();
    let stack_arguments = parameters.len().saturating_sub(4) as u32;
    let frame = (16 + SAVED_AREA + 32 + 8 * stack_arguments).next_multiple_of(16);
    asm.prologue(frame);
    asm.store(true, RBP, -8, RCX);
    asm.store(true, RBP, -16, RDX);
    for (index, register) in NONVOLATILE.iter().enumerate() {
        asm.store(true, RBP, -24 - 8 * index as i32, *register);
    }
    for xmm in 6..16u8 {
        asm.vector_store(RBP, vector_slot(xmm), xmm);
    }
    asm.move_register(true, R10, RCX);
    for (index, _) in parameters.iter().enumerate().skip(4) {
        asm.load(true, RAX, R10, 8 * index as i32);
        asm.store(true, RSP, 32 + 8 * (index as i32 - 4), RAX);
    }
    for (index, var) in parameters.iter().enumerate().take(4) {
        match var {
            VarType::F32 => asm.sse_load(false, index as u8, R10, 8 * index as i32),
            VarType::F64 => asm.sse_load(true, index as u8, R10, 8 * index as i32),
            _ => asm.load(true, ARGUMENT_REGISTERS[index], R10, 8 * index as i32),
        }
    }
    let call_at = asm.code.len() + 2;
    asm.move_immediate(RAX, 0);
    asm.call_register(RAX);
    for (index, register) in NONVOLATILE.iter().enumerate() {
        asm.load(true, *register, RBP, -24 - 8 * index as i32);
    }
    for xmm in 6..16u8 {
        asm.vector_load(xmm, RBP, vector_slot(xmm));
    }
    asm.load(true, RCX, RBP, -16);
    asm.store(true, RCX, 0, RAX);
    asm.sse_store(true, RCX, 8, 0);
    asm.epilogue();
    let (code, _) = asm.finish().expect("thunk has no labels");
    (code, call_at)
}

const NONVOLATILE: [u8; 7] = [3, 6, 7, 12, 13, 14, 15];
const SAVED_AREA: u32 = 8 * 7 + 16 * 10 + 8;

fn vector_slot(xmm: u8) -> i32 {
    -24 - 8 * 7 - 16 * (i32::from(xmm) - 5)
}

pub fn abort_routine() -> Option<u64> {
    static ROUTINE: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *ROUTINE.get_or_init(|| {
        emit_code(&[0x48, 0x89, 0xCC, 0x5D, 0x31, 0xC0, 0x0F, 0x57, 0xC0, 0xC3])
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Boolean,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    String,
}

impl ValueKind {
    pub fn from_sig(sig: &TypeSig) -> Option<ValueKind> {
        use TypeSig as T;
        Some(match sig {
            T::Boolean => ValueKind::Boolean,
            T::I1 => ValueKind::I8,
            T::U1 => ValueKind::U8,
            T::I2 => ValueKind::I16,
            T::U2 | T::Char => ValueKind::U16,
            T::I4 => ValueKind::I32,
            T::U4 => ValueKind::U32,
            T::I8 | T::IntPtr => ValueKind::I64,
            T::U8 | T::UIntPtr => ValueKind::U64,
            T::R4 => ValueKind::F32,
            T::R8 => ValueKind::F64,
            T::String => ValueKind::String,
            T::Modified { inner, .. } => return ValueKind::from_sig(inner),
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            ValueKind::Boolean => "bool",
            ValueKind::I8 => "i8",
            ValueKind::U8 => "u8",
            ValueKind::I16 => "i16",
            ValueKind::U16 => "u16",
            ValueKind::I32 => "i32",
            ValueKind::U32 => "u32",
            ValueKind::I64 => "i64",
            ValueKind::U64 => "u64",
            ValueKind::F32 => "f32",
            ValueKind::F64 => "f64",
            ValueKind::String => "string",
        }
    }

    pub fn parse(name: &str) -> Option<ValueKind> {
        Some(match name {
            "bool" => ValueKind::Boolean,
            "i8" => ValueKind::I8,
            "u8" => ValueKind::U8,
            "i16" => ValueKind::I16,
            "u16" => ValueKind::U16,
            "i32" => ValueKind::I32,
            "u32" => ValueKind::U32,
            "i64" => ValueKind::I64,
            "u64" => ValueKind::U64,
            "f32" => ValueKind::F32,
            "f64" => ValueKind::F64,
            "string" => ValueKind::String,
            _ => return None,
        })
    }

    pub fn normalize_integer(self, raw: i64) -> u64 {
        match self {
            ValueKind::Boolean => u64::from(raw as u8 != 0),
            ValueKind::I8 => raw as i8 as i64 as u64,
            ValueKind::U8 => raw as u8 as u64,
            ValueKind::I16 => raw as i16 as i64 as u64,
            ValueKind::U16 => raw as u16 as u64,
            ValueKind::I32 => raw as i32 as i64 as u64,
            ValueKind::U32 => raw as u32 as u64,
            ValueKind::I64 | ValueKind::U64 | ValueKind::String => raw as u64,
            ValueKind::F32 => {
                let value = f32::from_bits(raw as u32);
                if value.is_nan() {
                    0x7FC0_0000
                } else {
                    u64::from(value.to_bits())
                }
            }
            ValueKind::F64 => {
                let value = f64::from_bits(raw as u64);
                if value.is_nan() {
                    0x7FF8_0000_0000_0000
                } else {
                    value.to_bits()
                }
            }
        }
    }

    pub fn normalize_result(self, result: [u64; 2]) -> u64 {
        match self {
            ValueKind::F32 | ValueKind::F64 => self.normalize_integer(result[1] as i64),
            _ => self.normalize_integer(result[0] as i64),
        }
    }
}

const INTEGER_POOL: [i64; 34] = [
    0,
    1,
    -1,
    2,
    3,
    7,
    8,
    10,
    15,
    16,
    31,
    32,
    63,
    64,
    100,
    127,
    128,
    255,
    256,
    1000,
    65535,
    65536,
    -2,
    -100,
    -128,
    i32::MAX as i64,
    i32::MIN as i64,
    i64::MAX,
    i64::MIN,
    0x1234_5678,
    0x7FFF_FFFF_FFFF,
    -987_654_321,
    1_000_000_007,
    0x0F0F_0F0F_0F0F_0F0F,
];

const FLOAT_POOL: [f64; 24] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    -0.5,
    1.5,
    2.5,
    std::f64::consts::PI,
    -2.75,
    100.25,
    1e10,
    -1e10,
    1e-10,
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::MAX,
    f64::MIN_POSITIVE,
    2_147_483_648.0,
    -2_147_483_649.0,
    i64::MAX as f64,
    4_294_967_296.0,
    0.1,
];

pub fn generate_argument(kind: ValueKind, seed: u64) -> u64 {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    state ^= state >> 29;
    state = state.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    state ^= state >> 32;
    match kind {
        ValueKind::F32 => {
            let value = FLOAT_POOL[(state % FLOAT_POOL.len() as u64) as usize] as f32;
            u64::from(value.to_bits())
        }
        ValueKind::F64 => FLOAT_POOL[(state % FLOAT_POOL.len() as u64) as usize].to_bits(),
        _ => {
            let raw = if state % 5 == 4 {
                state as i64
            } else {
                INTEGER_POOL[(state % INTEGER_POOL.len() as u64) as usize]
            };
            kind.normalize_integer(raw)
        }
    }
}

const STRING_POOL: [&str; 16] = [
    "",
    "a",
    "Hello",
    "hello world",
    "  padded  ",
    "123",
    "-45",
    "MixedCase",
    "\u{c4}\u{d6}\u{dc}\u{df}",
    "\u{65e5}\u{672c}\u{8a9e}",
    "a,b,,c",
    "C:\\Windows\\System32\\file.txt",
    "https://example.com/path?x=1&y=2",
    "tab\tnew\nline",
    "0x1F",
    "voidstrap",
];

pub fn encode_units(units: Option<&[u16]>) -> String {
    match units {
        None => "z".into(),
        Some(units) => {
            let mut text = String::with_capacity(1 + units.len() * 4);
            text.push('s');
            for unit in units {
                text.push_str(&format!("{unit:04x}"));
            }
            text
        }
    }
}

pub fn decode_units(text: &str) -> Option<Vec<u16>> {
    let body = text.strip_prefix('s')?;
    (0..body.len() / 4)
        .map(|i| u16::from_str_radix(&body[i * 4..i * 4 + 4], 16).ok())
        .collect()
}

pub fn generate_text(kind: ValueKind, seed: u64) -> String {
    if kind == ValueKind::String {
        let pick = generate_argument(ValueKind::U64, seed);
        if pick % 17 == 16 {
            return encode_units(None);
        }
        let units: Vec<u16> = STRING_POOL[(pick % STRING_POOL.len() as u64) as usize]
            .encode_utf16()
            .collect();
        return encode_units(Some(&units));
    }
    format!("{:x}", generate_argument(kind, seed))
}

pub fn result_text(kind: ValueKind, raw: [u64; 2]) -> String {
    if kind == ValueKind::String {
        if raw[0] == 0 {
            return encode_units(None);
        }
        let units = unsafe {
            let length = *((raw[0] + 8) as *const u32) as usize;
            std::slice::from_raw_parts(
                (raw[0] + u64::from(crate::objects::STRING_DATA)) as *const u16,
                length,
            )
            .to_vec()
        };
        return encode_units(Some(&units));
    }
    format!("{:x}", kind.normalize_result(raw))
}

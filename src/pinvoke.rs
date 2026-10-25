use std::arch::asm;
use std::cell::Cell;
use std::path::Path;
use std::sync::Mutex;

use crate::metadata::{Metadata, TableId, Token};

pub const EXACT_SPELLING: u32 = 0x1;
pub const CHARSET_MASK: u32 = 0x6;
pub const CHARSET_UNICODE: u32 = 0x4;
pub const CHARSET_AUTO: u32 = 0x6;
pub const SUPPORTS_LAST_ERROR: u32 = 0x40;
pub const PRESERVE_SIG: u32 = 0x80;
const HAS_FIELD_MARSHAL: u32 = 0x2000;

pub struct Import<'a> {
    pub flags: u32,
    pub entry: &'a str,
    pub library: &'a str,
}

impl Import<'_> {
    pub fn unicode(&self) -> bool {
        matches!(self.flags & CHARSET_MASK, CHARSET_UNICODE | CHARSET_AUTO)
    }
}

pub fn find_import<'a>(metadata: &Metadata<'a>, method_row: u32) -> Option<Import<'a>> {
    let wanted = Token::new(TableId::MethodDef, method_row);
    (1..=metadata.rows(TableId::ImplMap)).find_map(|row| {
        let forwarded = metadata.coded(TableId::ImplMap, row, 1).ok().flatten()?;
        if forwarded.table() != wanted.table() || forwarded.row() != wanted.row() {
            return None;
        }
        let flags = metadata.cell(TableId::ImplMap, row, 0).ok()?;
        let entry = metadata.string_cell(TableId::ImplMap, row, 2).ok()?;
        let scope = metadata.cell(TableId::ImplMap, row, 3).ok()?;
        let library = metadata.string_cell(TableId::ModuleRef, scope, 0).ok()?;
        Some(Import { flags, entry, library })
    })
}

pub fn parameter_flags(metadata: &Metadata<'_>, method_row: u32, sequence: u32) -> u32 {
    let first = metadata.cell(TableId::MethodDef, method_row, 5).unwrap_or(0);
    let end = if method_row < metadata.rows(TableId::MethodDef) {
        metadata.cell(TableId::MethodDef, method_row + 1, 5).unwrap_or(first)
    } else {
        metadata.rows(TableId::Param) + 1
    };
    (first..end)
        .find(|&row| metadata.cell(TableId::Param, row, 1).ok() == Some(sequence))
        .and_then(|row| metadata.cell(TableId::Param, row, 0).ok())
        .unwrap_or(0)
}

pub fn marshals(metadata: &Metadata<'_>, method_row: u32) -> Vec<(u32, u8)> {
    let first = metadata.cell(TableId::MethodDef, method_row, 5).unwrap_or(0);
    let end = if method_row < metadata.rows(TableId::MethodDef) {
        metadata.cell(TableId::MethodDef, method_row + 1, 5).unwrap_or(first)
    } else {
        metadata.rows(TableId::Param) + 1
    };
    let marked: Vec<u32> = (first..end)
        .filter(|&row| metadata.cell(TableId::Param, row, 0).unwrap_or(0) & HAS_FIELD_MARSHAL != 0)
        .collect();
    if marked.is_empty() {
        return Vec::new();
    }
    (1..=metadata.rows(TableId::FieldMarshal))
        .filter_map(|row| {
            let parent = metadata.coded(TableId::FieldMarshal, row, 0).ok().flatten()?;
            if parent.table() != Some(TableId::Param) || !marked.contains(&parent.row()) {
                return None;
            }
            let blob = metadata.cell(TableId::FieldMarshal, row, 1).ok()?;
            let native = *metadata.blob(blob).ok()?.first()?;
            let sequence = metadata.cell(TableId::Param, parent.row(), 1).ok()?;
            Some((sequence, native))
        })
        .collect()
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *const u8, model: u32) -> i32;
    fn CoGetApartmentType(kind: *mut i32, qualifier: *mut i32) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> usize;
    fn FreeLibrary(module: usize) -> i32;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
    fn GetLastError() -> u32;
    fn GetModuleHandleW(name: *const u16) -> usize;
    fn GetCurrentThreadId() -> u32;
    fn GetTickCount64() -> u64;
    fn WaitForSingleObjectEx(handle: usize, milliseconds: u32, alertable: i32) -> u32;
    fn WaitForMultipleObjectsEx(count: u32, handles: *const usize, all: i32, milliseconds: u32, alertable: i32) -> u32;
    fn SignalObjectAndWait(signal: usize, wait: usize, milliseconds: u32, alertable: i32) -> u32;
    fn SetLastError(code: u32);
    fn WideCharToMultiByte(
        page: u32,
        flags: u32,
        wide: *const u16,
        wide_count: i32,
        bytes: *mut u8,
        byte_count: i32,
        default: *const u8,
        used_default: *mut i32,
    ) -> i32;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn load_library(directory: &Path, library: &str) -> Option<usize> {
    static LIBRARIES: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());
    let cache_key = format!("{}|{library}", directory.display());
    let mut libraries = LIBRARIES.lock().ok()?;
    if let Some((_, handle)) = libraries.iter().find(|(name, _)| *name == cache_key) {
        return Some(*handle);
    }
    let has_dll = Path::new(library)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dll") || e.eq_ignore_ascii_case("exe"));
    let mut candidates = Vec::new();
    for name in [library.to_owned(), format!("{library}.dll")] {
        if name != library && has_dll {
            continue;
        }
        candidates.push(directory.join(&name).display().to_string());
        candidates.push(name);
    }
    let handle = candidates.iter().find_map(|candidate| {
        if candidate.contains(['\\', '/']) && !Path::new(candidate).exists() {
            return None;
        }
        match unsafe { LoadLibraryW(wide(candidate).as_ptr()) } {
            0 => None,
            handle => Some(handle),
        }
    })?;
    libraries.push((cache_key, handle));
    Some(handle)
}

fn export(library: usize, name: &str) -> usize {
    if let Some(ordinal) = name.strip_prefix('#').and_then(|n| n.parse::<u16>().ok()) {
        return unsafe { GetProcAddress(library, usize::from(ordinal) as *const u8) };
    }
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    unsafe { GetProcAddress(library, bytes.as_ptr()) }
}

pub fn load_named(directory: &Path, library: &str) -> usize {
    load_library(directory, library).unwrap_or(0)
}

pub fn load_path(path: &str) -> usize {
    unsafe { LoadLibraryW(wide(path).as_ptr()) }
}

pub fn symbol(library: usize, name: &str) -> usize {
    export(library, name)
}

pub fn free_library(library: usize) {
    unsafe { FreeLibrary(library) };
}

pub fn resolve(directory: &Path, import: &Import<'_>, method_name: &str) -> Result<usize, String> {
    let library = load_library(directory, import.library)
        .ok_or_else(|| format!("platform invoke library {} is not available", import.library))?;
    let entry = if import.entry.is_empty() { method_name } else { import.entry };
    let mut names = vec![entry.to_owned()];
    if import.flags & EXACT_SPELLING == 0 {
        if import.unicode() {
            names.insert(0, format!("{entry}W"));
        } else {
            names.push(format!("{entry}A"));
        }
    }
    names
        .iter()
        .map(|name| export(library, name))
        .find(|&function| function != 0)
        .ok_or_else(|| format!("platform invoke entry {entry} is not exported by {}", import.library))
}

thread_local! {
    static LAST_ERROR: Cell<u32> = const { Cell::new(0) };
    static SYSTEM_ERROR: Cell<u32> = const { Cell::new(0) };
}

pub fn apartment_state() -> i32 {
    let (mut kind, mut qualifier) = (0i32, 0i32);
    if unsafe { CoGetApartmentType(&raw mut kind, &raw mut qualifier) } < 0 {
        return 2;
    }
    match kind {
        0 | 3 => 0,
        1 => 1,
        _ => 2,
    }
}

pub fn enter_apartment(state: i32) -> i32 {
    let model = if state == 0 { 2 } else { 0 };
    unsafe { CoInitializeEx(std::ptr::null(), model) };
    apartment_state()
}

pub fn tick_count() -> u64 {
    unsafe { GetTickCount64() }
}

pub fn current_thread_id() -> u32 {
    unsafe { GetCurrentThreadId() }
}

pub fn wait_one(handle: u64, milliseconds: i32) -> i32 {
    match crate::threads::blocking(|| unsafe { WaitForSingleObjectEx(handle as usize, milliseconds as u32, 1) }) {
        0 => 0,
        0x80 => 0x80,
        0x102 => 0x102,
        0xC0 => -1,
        _ => -1,
    }
}

pub fn wait_many(handles: u64, count: i32, all: bool, milliseconds: i32) -> i32 {
    let count = count.max(0) as u32;
    let result = crate::threads::blocking(|| unsafe {
        WaitForMultipleObjectsEx(count, handles as *const usize, i32::from(all), milliseconds as u32, 1)
    });
    match result {
        index if index < count => index as i32,
        index if (0x80..0x80 + count).contains(&index) => index as i32,
        0x102 => 0x102,
        _ => -1,
    }
}

pub fn signal_and_wait(signal: u64, wait: u64, milliseconds: i32) -> i32 {
    match crate::threads::blocking(|| unsafe { SignalObjectAndWait(signal as usize, wait as usize, milliseconds as u32, 1) }) {
        0 => 0,
        0x80 => 0x80,
        0x102 => 0x102,
        _ => -1,
    }
}

pub fn process_module() -> u64 {
    unsafe { GetModuleHandleW(std::ptr::null()) as u64 }
}

pub fn last_error() -> u32 {
    LAST_ERROR.with(Cell::get)
}

pub fn set_last_error(code: u32) {
    LAST_ERROR.with(|cell| cell.set(code));
}

pub fn system_error() -> u32 {
    SYSTEM_ERROR.with(Cell::get)
}

pub fn set_system_error(code: u32) {
    SYSTEM_ERROR.with(|cell| cell.set(code));
}

pub fn ansi(units: &[u16]) -> Vec<u8> {
    if units.is_empty() {
        return vec![0];
    }
    let count = units.len() as i32;
    let size = unsafe {
        WideCharToMultiByte(0, 0, units.as_ptr(), count, std::ptr::null_mut(), 0, std::ptr::null(), std::ptr::null_mut())
    };
    let mut bytes = vec![0u8; size.max(0) as usize + 1];
    unsafe {
        WideCharToMultiByte(0, 0, units.as_ptr(), count, bytes.as_mut_ptr(), size, std::ptr::null(), std::ptr::null_mut());
    }
    bytes
}

pub unsafe fn call(function: usize, arguments: &[u64], capture_error: bool) -> (u64, u64) {
    let mut slots = arguments.to_vec();
    while slots.len() < 4 || slots.len() % 2 == 1 {
        slots.push(0);
    }
    let bytes = slots.len() * 8;
    let system_error = if capture_error { 0 } else { SYSTEM_ERROR.with(Cell::get) };
    let (integer, float, error) = crate::threads::blocking(|| unsafe {
        let integer: u64;
        let float: f64;
        SetLastError(system_error);
        asm!(
            "mov r12, rsp",
            "sub rsp, r13",
            "and rsp, -16",
            "xor r10, r10",
            "2:",
            "cmp r10, r14",
            "jae 3f",
            "mov rax, [r15 + r10*8]",
            "mov [rsp + r10*8], rax",
            "inc r10",
            "jmp 2b",
            "3:",
            "mov rcx, [rsp]",
            "mov rdx, [rsp + 8]",
            "mov r8, [rsp + 16]",
            "mov r9, [rsp + 24]",
            "movq xmm0, rcx",
            "movq xmm1, rdx",
            "movq xmm2, r8",
            "movq xmm3, r9",
            "call r11",
            "mov rsp, r12",
            in("r11") function,
            in("r13") bytes,
            in("r14") slots.len(),
            in("r15") slots.as_ptr(),
            out("r12") _,
            lateout("rax") integer,
            lateout("xmm0") float,
            clobber_abi("win64"),
        );
        (integer, float, GetLastError())
    });
    SYSTEM_ERROR.with(|cell| cell.set(error));
    if capture_error {
        set_last_error(error);
    }
    (integer, float.to_bits())
}

pub type Handler = unsafe fn(context: usize, slot: usize, integers: *const u64, floats: *const u64) -> u64;

#[derive(Clone, Copy)]
struct Entry {
    handler: Handler,
    context: usize,
    slot: usize,
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

fn entries() -> std::sync::MutexGuard<'static, Vec<Entry>> {
    ENTRIES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

const STUB_SIZE: usize = 96;
const CHUNK: usize = 64 * 1024;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut u8, size: usize, allocation: u32, protection: u32) -> *mut u8;
}

unsafe extern "win64" fn reverse_entry(index: u64, integers: *const u64, floats: *const u64) -> u64 {
    let entry = entries().get(index as usize).copied();
    match entry {
        Some(entry) => {
            let acquired = !crate::threads::holding();
            if acquired {
                crate::threads::acquire();
            }
            let result = unsafe { (entry.handler)(entry.context, entry.slot, integers, floats) };
            if acquired {
                crate::threads::release();
            }
            result
        }
        None => {
            eprintln!("native callback {index} has no entry");
            std::process::abort();
        }
    }
}

fn executable(size: usize) -> Option<usize> {
    static MEMORY: Mutex<(usize, usize)> = Mutex::new((0, 0));
    let mut memory = MEMORY.lock().ok()?;
    if memory.0 == 0 || memory.1 + size > CHUNK {
        let base = unsafe { VirtualAlloc(std::ptr::null_mut(), CHUNK, 0x3000, 0x40) };
        if base.is_null() {
            return None;
        }
        *memory = (base as usize, 0);
    }
    let address = memory.0 + memory.1;
    memory.1 += size;
    Some(address)
}

pub fn make_stub(handler: Handler, context: usize, slot: usize) -> Option<usize> {
    let index = {
        let mut entries = entries();
        entries.push(Entry { handler, context, slot });
        entries.len() - 1
    } as u64;
    let target = reverse_entry as *const () as usize as u64;
    let mut code: Vec<u8> = vec![
        0x48, 0x89, 0x4C, 0x24, 0x08, 0x48, 0x89, 0x54, 0x24, 0x10, 0x4C, 0x89, 0x44, 0x24, 0x18, 0x4C, 0x89,
        0x4C, 0x24, 0x20, 0x48, 0x83, 0xEC, 0x48, 0x66, 0x0F, 0xD6, 0x44, 0x24, 0x20, 0x66, 0x0F, 0xD6, 0x4C,
        0x24, 0x28, 0x66, 0x0F, 0xD6, 0x54, 0x24, 0x30, 0x66, 0x0F, 0xD6, 0x5C, 0x24, 0x38, 0x48, 0xB9,
    ];
    code.extend_from_slice(&index.to_le_bytes());
    code.extend_from_slice(&[0x48, 0x8D, 0x54, 0x24, 0x50, 0x4C, 0x8D, 0x44, 0x24, 0x20, 0x48, 0xB8]);
    code.extend_from_slice(&target.to_le_bytes());
    code.extend_from_slice(&[0xFF, 0xD0, 0x66, 0x48, 0x0F, 0x6E, 0xC0, 0x48, 0x83, 0xC4, 0x48, 0xC3]);
    let address = executable(STUB_SIZE)?;
    unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), address as *mut u8, code.len()) };
    Some(address)
}

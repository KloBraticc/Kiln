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

pub fn has_field_marshal(metadata: &Metadata<'_>, method_row: u32) -> bool {
    let first = metadata.cell(TableId::MethodDef, method_row, 5).unwrap_or(0);
    let end = if method_row < metadata.rows(TableId::MethodDef) {
        metadata.cell(TableId::MethodDef, method_row + 1, 5).unwrap_or(first)
    } else {
        metadata.rows(TableId::Param) + 1
    };
    (first..end).any(|row| metadata.cell(TableId::Param, row, 0).unwrap_or(0) & HAS_FIELD_MARSHAL != 0)
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
    fn GetLastError() -> u32;
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
    let has_extension = Path::new(library).extension().is_some();
    let mut candidates = Vec::new();
    for name in [library.to_owned(), format!("{library}.dll")] {
        if name != library && has_extension {
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
}

pub fn last_error() -> u32 {
    LAST_ERROR.with(Cell::get)
}

pub fn set_last_error(code: u32) {
    LAST_ERROR.with(|cell| cell.set(code));
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
    let integer: u64;
    let float: f64;
    if capture_error {
        unsafe { SetLastError(0) };
    }
    unsafe {
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
    }
    if capture_error {
        set_last_error(unsafe { GetLastError() });
    }
    (integer, float.to_bits())
}

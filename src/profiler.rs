use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

#[repr(C, align(16))]
struct Context {
    bytes: [u8; 1232],
}

#[repr(C)]
struct Symbol {
    size_of_struct: u32,
    type_index: u32,
    reserved: [u64; 2],
    index: u32,
    size: u32,
    module_base: u64,
    flags: u32,
    value: u64,
    address: u64,
    register: u32,
    scope: u32,
    tag: u32,
    name_length: u32,
    max_name_length: u32,
    name: [u8; 512],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThread() -> usize;
    fn GetCurrentProcess() -> isize;
    fn DuplicateHandle(source: usize, handle: usize, target: usize, out: *mut usize, access: u32, inherit: i32, options: u32) -> i32;
    fn SuspendThread(thread: usize) -> u32;
    fn ResumeThread(thread: usize) -> u32;
    fn GetThreadContext(thread: usize, context: *mut Context) -> i32;
    fn GetModuleHandleW(name: *const u16) -> usize;
}

#[link(name = "dbghelp")]
unsafe extern "system" {
    fn SymInitialize(process: usize, path: *const u8, invade: i32) -> i32;
    fn SymSetOptions(options: u32) -> u32;
    fn SymFromAddr(process: usize, address: u64, displacement: *mut u64, symbol: *mut Symbol) -> i32;
}

#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
}

static SAMPLES: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn start() {
    if std::env::var_os("KILN_PROFILE").is_none() || RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut target = 0usize;
    unsafe {
        DuplicateHandle(GetCurrentProcess() as usize, GetCurrentThread(), GetCurrentProcess() as usize, &raw mut target, 0, 0, 2);
        timeBeginPeriod(1);
    }
    std::thread::spawn(move || {
        let mut context = Box::new(Context { bytes: [0; 1232] });
        while RUNNING.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(1));
            unsafe {
                if SuspendThread(target) == u32::MAX {
                    break;
                }
                context.bytes[0x30..0x34].copy_from_slice(&0x0010_0001u32.to_le_bytes());
                if GetThreadContext(target, &mut *context) != 0 {
                    let rip = u64::from_le_bytes(context.bytes[0xF8..0x100].try_into().unwrap_or([0; 8]));
                    SAMPLES.lock().unwrap_or_else(PoisonError::into_inner).push(rip);
                }
                ResumeThread(target);
            }
        }
    });
}

pub fn report() {
    if !RUNNING.swap(false, Ordering::SeqCst) {
        return;
    }
    let samples = std::mem::take(&mut *SAMPLES.lock().unwrap_or_else(PoisonError::into_inner));
    let process = unsafe { GetCurrentProcess() } as usize;
    let search = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.display().to_string()))
        .unwrap_or_default();
    let mut search = search.into_bytes();
    search.push(0);
    unsafe {
        SymSetOptions(0x2 | 0x4 | 0x10 | 0x200);
        SymInitialize(process, search.as_ptr(), 1);
    }
    let state = crate::runtime::state();
    let mut cache: HashMap<u64, String> = HashMap::new();
    let mut totals: HashMap<String, usize> = HashMap::new();
    for rip in &samples {
        let name = cache
            .entry(*rip)
            .or_insert_with(|| {
                if let Some((_, method)) = state.method_containing(*rip as usize) {
                    return format!("compiled {method}");
                }
                let mut symbol = Box::new(Symbol {
                    size_of_struct: 88,
                    type_index: 0,
                    reserved: [0; 2],
                    index: 0,
                    size: 0,
                    module_base: 0,
                    flags: 0,
                    value: 0,
                    address: 0,
                    register: 0,
                    scope: 0,
                    tag: 0,
                    name_length: 0,
                    max_name_length: 511,
                    name: [0; 512],
                });
                let mut displacement = 0u64;
                if unsafe { SymFromAddr(process, *rip, &raw mut displacement, &mut *symbol) } != 0 {
                    let length = (symbol.name_length as usize).min(511);
                    String::from_utf8_lossy(&symbol.name[..length]).into_owned()
                } else {
                    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as u64;
                    if *rip >= base && *rip < base + 0x1000_0000 {
                        format!("exe+{:#x}", rip - base)
                    } else {
                        format!("unknown {:#x}", rip & !0xFFF)
                    }
                }
            })
            .clone();
        *totals.entry(name).or_default() += 1;
    }
    let mut sorted: Vec<_> = totals.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    eprintln!("kiln profile: {} samples", samples.len());
    for (name, count) in sorted.iter().take(3000) {
        eprintln!("{:>6.1}%  {count:>7}  {name}", 100.0 * *count as f64 / samples.len().max(1) as f64);
    }
}

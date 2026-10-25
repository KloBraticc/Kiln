use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::codegen::{MethodKey, VarType};
use crate::generics::{Ctx, Gen, Types};
use crate::il::{Argument, ClauseKind, ExceptionClause, MethodBody};
use crate::jit::Image;
use crate::loader::TypeHandle;
use crate::members::Member;
use crate::metadata::{TableId, Token};
use crate::pinvoke;
use crate::objects::{
    ARRAY_DATA, ElementKind, ObjectModel, RuntimeType, STRING_DATA, StructLayout,
};
use crate::runtime;
use crate::signature::{CallingConvention, Signature, TypeSig};

const MAX_DEPTH: usize = 400;
const FUNCTION_TAG: u64 = 0x4B00_0000_0000_0000;
const INTERNAL_CALL: u32 = 0x1000;
const RUNTIME_IMPL: u32 = 0x3;
const PINVOKE: u32 = 0x2000;
const ABSTRACT: u32 = 0x400;
const VIRTUAL: u32 = 0x40;
const STATIC: u32 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum V {
    I4(i32),
    I8(i64),
    F4(f32),
    F8(f64),
    O(u64),
    P(u64),
    S(u64),
}

impl V {
    fn bits(self) -> u64 {
        match self {
            V::I4(v) => v as i64 as u64,
            V::I8(v) => v as u64,
            V::F4(v) => u64::from(v.to_bits()),
            V::F8(v) => v.to_bits(),
            V::O(v) | V::P(v) | V::S(v) => v,
        }
    }

    pub fn int(self) -> i64 {
        match self {
            V::I4(v) => i64::from(v),
            V::I8(v) => v,
            V::O(v) | V::P(v) | V::S(v) => v as i64,
            V::F4(v) => v as i64,
            V::F8(v) => v as i64,
        }
    }

    fn float(self) -> f64 {
        match self {
            V::F4(v) => f64::from(v),
            V::F8(v) => v,
            other => other.int() as f64,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, V::F4(_) | V::F8(_))
    }

    fn address(self) -> u64 {
        match self {
            V::O(v) | V::P(v) | V::S(v) => v,
            V::I8(v) => v as u64,
            V::I4(v) => v as u32 as u64,
            _ => 0,
        }
    }
}

#[derive(Debug)]
pub enum Fault {
    Exception(u64),
    Unsupported(String),
}

pub type Run<T> = Result<T, Fault>;

fn unsupported<T>(what: impl Into<String>) -> Run<T> {
    let reason = what.into();
    if crate::diagnostics::enabled() {
        crate::diagnostics::write_once("unsupported", &reason);
    }
    Err(Fault::Unsupported(reason))
}

fn map<T>(result: Result<T, String>) -> Run<T> {
    result.map_err(Fault::Unsupported)
}

#[derive(Clone, Copy)]
enum Native {
    Intrinsic(Intrinsic),
    DelegateConstruct,
    DelegateInvoke,
    StringConstructor,
    PlatformInvoke(usize),
    Accessor(usize),
}

struct Platform {
    function: usize,
    passes: Vec<Pass>,
    back: Back,
    last_error: bool,
    hresult: bool,
}

enum Pass {
    Raw(VarType),
    Bool,
    Wide,
    Ansi,
    Utf8,
    Array,
    Value(u32),
    Copy(u32),
    Field(u32),
    SafeHandle(u32),
    SafeHandleOut(Gen, u32),
    StringOut(u8),
    BoolOut(u8),
    Delegate,
    Formatted(Rc<marshal::NativeLayout>, bool),
    FormattedRef(Rc<marshal::NativeLayout>),
    FormattedValue(Rc<marshal::NativeLayout>),
    AsAny(bool),
    Builder(bool),
    Com(Gen),
    ComOut,
    Unknown,
}

enum Back {
    Nothing,
    Raw(VarType),
    Bool(u64),
    Value(Rc<StructLayout>),
    Buffer(Rc<StructLayout>),
    SafeHandle(Gen, u32),
}

struct Callback {
    key: MethodKey,
    ctx: Ctx,
    receiver: Option<u64>,
    parameters: Vec<VarType>,
    structs: Vec<Option<Rc<StructLayout>>>,
    returns: Option<VarType>,
    struct_return: Option<Rc<StructLayout>>,
}

unsafe fn callback_handler(context: usize, slot: usize, integers: *const u64, floats: *const u64) -> u64 {
    let interpreter = unsafe { &*(context as *const Interpreter<'static>) };
    interpreter.reverse(slot, integers, floats)
}

#[derive(Clone, Copy)]
enum Intrinsic {
    False,
    Intern(bool),
    HasComponentSize,
    TrackerVtable,
    UntrackedReference,
    CountedHandle,
    TaggedImpl,
    UnknownImpl,
    TypeMemory,
    Collect,
    SetExitCode,
    ExitCode,
    ArrayRank,
    Thread(threading::ThreadOp),
    LoadLibrary(bool),
    LibrarySymbol,
    FreeLibrary,
    Math1(fn(f64) -> f64),
    MathF1(fn(f32) -> f32),
    Math2(fn(f64, f64) -> f64),
    MathF2(fn(f32, f32) -> f32),
    StringLength,
    StringChar,
    StringAllocate,
    StringEquals,
    StringConcat,
    ObjectClone,
    InitializeArray,
    Memmove,
    ResourceString(usize),
    Exchange(VarType),
    CompareExchange(VarType),
    ExchangeAdd(VarType),
    Fused,
    FusedF,
    MultiplyAdd,
    MultiplyAddF,
    TypeFromHandle,
    ContainsReferences,
    BitwiseEquatable,
    DefaultEqualityComparer,
    DefaultComparer,
    CreateInstance,
    GetType,
    TypeIsValueType,
    TypeIsEnum,
    Unsafe(UnsafeOp),
    CreateSpan,
    MemmoveElements,
    ArrayData,
    ArrayCopy,
    ArrayClear,
    BlockCopy,
    RandomBytes,
    ThreadId,
    LastError,
    SetLastError,
    ClearMemory,
    Reflect(reflection::Reflect),
    EnumHash,
    EnumEquals,
    CurrentThread,
    CurrentThreadOnStack,
    SystemError,
    SetSystemError,
    AllocateRecord(bool),
    Collections,
    DefaultConstruct,
    NewHandle,
    DependentFree,
    DependentClear,
    DependentSetSecond,
    DependentBoth,
    DependentSecond,
    DependentTarget,
    DependentAlloc,
    IdentityHash,
    HandleAlloc,
    HandleFree,
    HandleGet,
    HandleSet,
    HandleExchange,
    DelegatePointer,
    Fence,
    Nothing,
    True,
    CanCastTo,
    AnotherGenericParameter,
    ProcessorCount,
    SpinWait,
}

#[derive(Clone, Copy)]
enum UnsafeOp {
    Same,
    Add,
    AddBytes,
    Subtract,
    SubtractBytes,
    SizeOf,
    Read,
    Write,
    Copy,
    CopyBlock,
    InitBlock,
    AreSame,
    Greater,
    Less,
    ByteOffset,
    IsNull,
    Null,
    Nothing,
    Unbox,
    BitCast,
}

type IntrinsicRows = (HashSet<u32>, HashSet<u32>);
type UnaryPair = (fn(f64) -> f64, fn(f32) -> f32);
type BinaryPair = (fn(f64, f64) -> f64, fn(f32, f32) -> f32);

#[derive(Clone)]
enum Ty {
    Prim(VarType),
    Struct(Rc<StructLayout>),
}

impl Ty {
    fn prim(&self) -> Option<VarType> {
        match self {
            Ty::Prim(var) => Some(*var),
            Ty::Struct(_) => None,
        }
    }

    fn words(&self) -> usize {
        match self {
            Ty::Prim(_) => 1,
            Ty::Struct(layout) => (layout.size as usize).div_ceil(8).max(1),
        }
    }

    fn size(&self) -> u32 {
        match self {
            Ty::Prim(var) => var_size(*var),
            Ty::Struct(layout) => layout.size,
        }
    }
}

fn prims(types: &[Ty]) -> Option<Vec<VarType>> {
    types.iter().map(Ty::prim).collect()
}

struct FieldRef {
    owner: Gen,
    module: usize,
    row: u32,
    is_static: bool,
    has_rva: bool,
    thread_static: bool,
    owner_is_value: bool,
    ty: Ty,
    offset: u64,
}

struct Loaded {
    key: MethodKey,
    ctx: Ctx,
    owner: TypeHandle,
    owner_gen: Gen,
    code: &'static [u8],
    clauses: Vec<ExceptionClause>,
    parameters: Vec<Ty>,
    returns: Option<Ty>,
    locals: Vec<Ty>,
    native: Option<Native>,
    is_static: bool,
    is_virtual: bool,
    is_abstract: bool,
}

struct Frame {
    memory: Box<[u64]>,
    offsets: Vec<usize>,
    kinds: Vec<Ty>,
    stack: Vec<V>,
    caught: Vec<u64>,
    scratch: Vec<Box<[u8]>>,
}

fn trace_indent(depth: usize) -> String {
    format!("{:>6} {}", pinvoke::current_thread_id(), " ".repeat(depth.min(60)))
}

fn frames() -> &'static mut Vec<usize> {
    &mut runtime::roots().frames
}

fn pinned() -> &'static mut Vec<u64> {
    &mut runtime::roots().pinned
}

static TYPE_ROOTS: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

fn type_roots() -> std::sync::MutexGuard<'static, Vec<u64>> {
    TYPE_ROOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

const EXACT_TAG: u64 = 0x8000_0000_0000_0000;

thread_local! {
    static LAST_NATIVE: Cell<(usize, u32)> = const { Cell::new((0, 0)) };
    static LAST_METHOD: Cell<(usize, u32)> = const { Cell::new((0, 0)) };
}

pub static FALLBACK_ENTRIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

unsafe extern "system" fn wrapper_query_interface(_this: u64, _iid: u64, result: *mut u64) -> i32 {
    if !result.is_null() {
        unsafe { *result = 0 };
    }
    0x8000_4002_u32 as i32
}

unsafe extern "system" fn wrapper_not_implemented(_this: u64, _version: u64) -> i32 {
    0x8000_4001_u32 as i32
}

unsafe extern "system" fn wrapper_reference(_this: u64) -> u32 {
    1
}

pub static EXIT_CODE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

type InstKey = (MethodKey, Vec<u32>, Vec<u32>);

struct VTable {
    slots: Vec<Option<(MethodKey, Gen)>>,
    slot_of: HashMap<MethodKey, usize>,
}

fn scan_roots(out: &mut Vec<u64>) {
    for roots in runtime::threads() {
        for &frame in &roots.frames {
            let frame = unsafe { &*(frame as *const Frame) };
            for (index, kind) in frame.kinds.iter().enumerate() {
                let base = frame.offsets[index];
                match kind {
                    Ty::Prim(VarType::Ref) => out.push(frame.memory[base]),
                    Ty::Struct(layout) => {
                        let address = frame.memory.as_ptr() as u64 + 8 * base as u64;
                        for offset in &layout.references {
                            out.push(unsafe { *((address + u64::from(*offset)) as *const u64) });
                        }
                    }
                    Ty::Prim(_) => {}
                }
            }
            for value in &frame.stack {
                if let V::O(object) | V::S(object) = value {
                    out.push(*object);
                }
            }
            out.extend_from_slice(&frame.caught);
        }
        out.extend_from_slice(&roots.pinned);
    }
    out.extend_from_slice(&type_roots());
}

struct Pin(usize);

impl Pin {
    fn new(values: &[V]) -> Self {
        let pinned = pinned();
        let mark = pinned.len();
        pinned.extend(values.iter().filter_map(|v| match v {
            V::O(o) | V::S(o) => Some(*o),
            _ => None,
        }));
        Pin(mark)
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        pinned().truncate(self.0);
    }
}

struct Registered;

impl Registered {
    fn new(frame: &Frame) -> Self {
        frames().push(frame as *const Frame as usize);
        Registered
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        frames().pop();
    }
}

impl Frame {
    fn slot(&self, index: usize) -> Run<(u64, Ty)> {
        let kind = self
            .kinds
            .get(index)
            .ok_or_else(|| Fault::Unsupported("variable index".into()))?;
        Ok((
            self.memory.as_ptr() as u64 + 8 * self.offsets[index] as u64,
            kind.clone(),
        ))
    }
}

enum Exit {
    Return(Option<V>),
    EndFinally,
    EndFilter(bool),
    Leave(u32, u32),
}

pub struct Interpreter<'s> {
    pub model: &'s ObjectModel<'s>,
    image: Cell<*const Image>,
    loaded: RefCell<HashMap<InstKey, Rc<Loaded>>>,
    types: Types<'s>,
    exact_layouts: RefCell<HashMap<u32, Rc<StructLayout>>>,
    generic_init: RefCell<HashMap<u32, Option<Option<String>>>>,
    vtables: RefCell<HashMap<u32, Rc<VTable>>>,
    functions: RefCell<Vec<(MethodKey, Ctx)>>,
    type_objects: RefCell<HashMap<Gen, u64>>,
    object_types: RefCell<HashMap<u64, Gen>>,
    rva_sizes: RefCell<HashMap<u64, u32>>,
    implementations: RefCell<HashMap<(u64, InstKey), (MethodKey, Ctx)>>,
    names: RefCell<HashMap<TypeHandle, String>>,
    resources: RefCell<HashMap<usize, HashMap<String, String>>>,
    failed_types: RefCell<HashMap<TypeHandle, Option<String>>>,
    fields: RefCell<HashMap<(usize, u32, Vec<u32>, Vec<u32>), Rc<FieldRef>>>,
    intrinsic_rows: RefCell<HashMap<usize, IntrinsicRows>>,
    pub interpreted: Cell<u64>,
    pub compiled_calls: Cell<u64>,
    pending: RefCell<Option<Pending>>,
    pub budget: Cell<u64>,
    trace_calls: bool,
    platforms: RefCell<Vec<Rc<Platform>>>,
    callbacks: RefCell<Vec<Callback>>,
    unmanaged_rows: RefCell<HashMap<usize, Rc<HashSet<u32>>>>,
    unmanaged_stubs: RefCell<HashMap<MethodKey, u64>>,
    delegate_stubs: RefCell<HashMap<u64, u64>>,
    assemblies: RefCell<HashMap<usize, u64>>,
    entry_module: Cell<usize>,
    module_objects: RefCell<HashMap<usize, u64>>,
    method_tables: RefCell<HashMap<u32, u64>>,
    method_table_types: RefCell<HashMap<u64, Gen>>,
    native_layouts: RefCell<HashMap<u32, Rc<marshal::NativeLayout>>>,
    handles: RefCell<members::Handles>,
    native_delegates: RefCell<HashMap<u64, (usize, u64)>>,
    indirect_platforms: RefCell<HashMap<(usize, u32, u64), usize>>,
    wrappers: RefCell<com::Wrappers>,
    accessors: RefCell<Vec<Rc<accessors::Accessor>>>,
    used_modules: RefCell<HashSet<usize>>,
    method_insts: RefCell<HashMap<(usize, u32, Ctx), (MethodKey, Ctx)>>,
    assignable: RefCell<HashMap<(Gen, Gen), bool>>,
}

enum Pending {
    Fault(Fault),
    Throw(&'static str),
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: extern "system" fn(*mut [usize; 2]) -> i32) -> usize;
}

const CONTINUE_EXECUTION: i32 = -1;
const CONTINUE_SEARCH: i32 = 0;

extern "system" fn hardware_fault(pointers: *mut [usize; 2]) -> i32 {
    let bridge = BRIDGE.load(std::sync::atomic::Ordering::Acquire);
    if bridge.is_null() {
        return CONTINUE_SEARCH;
    }
    let (record, context) = unsafe { ((*pointers)[0], (*pointers)[1]) };
    let code = unsafe { *(record as *const u32) };
    let name = match code {
        0xC000_0005 => "NullReferenceException",
        0xC000_0094 => "DivideByZeroException",
        0xC000_0095 => "OverflowException",
        _ => return CONTINUE_SEARCH,
    };
    let rip = unsafe { *((context + 0xF8) as *const u64) } as usize;
    let state = runtime::state();
    if !state.is_compiled(rip) {
        return CONTINUE_SEARCH;
    }
    let Some(routine) = crate::jit::abort_routine() else {
        return CONTINUE_SEARCH;
    };
    let interpreter = unsafe { &*(*bridge).interpreter };
    if interpreter.trace_calls {
        let fault = unsafe { *((record + 0x28) as *const u64) };
        let method = state.method_containing(rip).map(|(offset, name)| format!("{name}+{offset:#x}")).unwrap_or_default();
        eprintln!("hardware fault {name} in compiled {method} reading {fault:#x}");
    }
    interpreter
        .pending
        .borrow_mut()
        .get_or_insert(Pending::Throw(name));
    let frame = unsafe { *((context + 0xA0) as *const u64) };
    let outermost = outermost_compiled(frame);
    unsafe {
        *((context + 0x80) as *mut u64) = outermost;
        *((context + 0xF8) as *mut u64) = routine;
    }
    CONTINUE_EXECUTION
}

pub extern "win64" fn compiled_throw(kind: u64, frame: u64) -> u64 {
    let name = match kind {
        1 => "IndexOutOfRangeException",
        2 => "InvalidCastException",
        3 => "OverflowException",
        _ => "NullReferenceException",
    };
    let bridge = BRIDGE.load(std::sync::atomic::Ordering::Acquire);
    if bridge.is_null() {
        runtime::trap();
    } else {
        let interpreter = unsafe { &*(*bridge).interpreter };
        if interpreter.trace_calls {
            eprintln!("compiled code raised {name}");
        }
        interpreter.pending.borrow_mut().get_or_insert(Pending::Throw(name));
    }
    let Some(routine) = crate::jit::abort_routine() else {
        std::process::abort();
    };
    let routine: extern "win64" fn(u64) -> ! = unsafe { std::mem::transmute(routine as usize) };
    routine(outermost_compiled(frame))
}

extern "system" fn report_fault(pointers: *mut [usize; 2]) -> i32 {
    let (record, context) = unsafe { ((*pointers)[0], (*pointers)[1]) };
    let code = unsafe { *(record as *const u32) };
    if code < 0xC000_0000 {
        return CONTINUE_SEARCH;
    }
    let rip = unsafe { *((context + 0xF8) as *const u64) } as usize;
    let address = unsafe { *((record + 0x28) as *const u64) };
    match runtime::state().method_containing(rip) {
        Some((offset, name)) => eprintln!(
            "fault {code:#x} at {address:#x} in compiled method {name} at offset {offset:#x}"
        ),
        None => {
            let (module, row) = LAST_NATIVE.with(Cell::get);
            let bridge = BRIDGE.load(std::sync::atomic::Ordering::Acquire);
            let name = if bridge.is_null() {
                String::new()
            } else {
                let interpreter = unsafe { &*(*bridge).interpreter };
                let key = MethodKey { module, row };
                let owner = interpreter.model.method_owner(&key).map(|o| interpreter.type_name(o)).unwrap_or_default();
                let method = interpreter.model.set.modules[module].metadata.string_cell(TableId::MethodDef, row, 3).unwrap_or("");
                format!("{owner}::{method}")
            };
            let message = format!("{code:#x} reading {address:#x} outside compiled code at {rip:#x}, last native {name}");
            crate::diagnostics::write("fault", &message);
            eprintln!(
                "fault {message}{}
{}",
                crate::diagnostics::managed_stack(40),
                std::backtrace::Backtrace::force_capture()
            )
        }
    }
    CONTINUE_SEARCH
}

pub fn install_fault_report() {
    unsafe { AddVectoredExceptionHandler(0, report_fault) };
}

pub extern "win64" fn compiled_initialize(handle: u64, cctor: u64, frame: u64, stack: u64) {
    let owner = TypeHandle {
        module: (handle >> 32) as usize,
        row: handle as u32,
    };
    let bridge = BRIDGE.load(std::sync::atomic::Ordering::Acquire);
    if bridge.is_null() {
        let flag = runtime::state().init_flag(owner) as *mut u8;
        if unsafe { *flag } != 0 {
            return;
        }
        unsafe { *flag = 1 };
        let function: extern "win64" fn() = unsafe { std::mem::transmute(cctor as usize) };
        function();
        return;
    }
    let interpreter = unsafe { &*(*bridge).interpreter };
    runtime::roots().transitions.push((frame, stack));
    let outcome = interpreter.ensure_initialized(owner);
    runtime::roots().transitions.pop();
    if let Err(fault) = outcome {
        abort_compiled(interpreter, frame, fault);
    }
}

fn outermost_compiled(frame: u64) -> u64 {
    let state = runtime::state();
    let mut outermost = frame as usize;
    loop {
        let returned = unsafe { *((outermost + 8) as *const u64) } as usize;
        if !state.is_compiled(returned) {
            break;
        }
        outermost = unsafe { *(outermost as *const u64) } as usize;
    }
    outermost as u64
}

struct Bridge {
    interpreter: *const Interpreter<'static>,
    keys: RefCell<Vec<(MethodKey, Ctx, bool)>>,
    stubs: RefCell<HashMap<(InstKey, bool), u64>>,
}

fn record_vtable(record: u64) -> Vec<u64> {
    let words = record as *const u64;
    let length = unsafe { *words.add(6) } as usize;
    (0..length).map(|slot| unsafe { *words.add(7 + slot) }).collect()
}

fn resolve_stub(key: &MethodKey) -> Option<u64> {
    let bridge = BRIDGE.load(std::sync::atomic::Ordering::Acquire);
    if bridge.is_null() {
        return None;
    }
    unsafe { &*(*bridge).interpreter }.stub_for(key)
}

fn resolve_boxed_stub(key: &MethodKey) -> Option<u64> {
    let bridge = BRIDGE.load(std::sync::atomic::Ordering::Acquire);
    if bridge.is_null() {
        return None;
    }
    unsafe { &*(*bridge).interpreter }.stub_entry(key, &Ctx::default(), true)
}

static BRIDGE: std::sync::atomic::AtomicPtr<Bridge> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

pub(crate) unsafe extern "win64" fn fallback_entry(index: u64, arguments: *const u64, frame: u64, stack: u64) -> u64 {
    let bridge = BRIDGE.load(std::sync::atomic::Ordering::Acquire);
    if bridge.is_null() {
        runtime::trap();
        return 0;
    }
    let bridge = unsafe { &*bridge };
    let interpreter = unsafe { &*bridge.interpreter };
    FALLBACK_ENTRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let Some((key, ctx, boxed)) = bridge.keys.borrow().get(index as usize).cloned() else {
        runtime::trap();
        return 0;
    };
    let key = &key;
    let loaded = match interpreter.load_ctx(key, &ctx) {
        Ok(loaded) => loaded,
        Err(fault) => abort_compiled(interpreter, frame, fault),
    };
    let Some(parameters) = prims(&loaded.parameters) else {
        abort_compiled(interpreter, frame, Fault::Unsupported("struct parameters from compiled code".into()))
    };
    let values: Vec<V> = parameters
        .iter()
        .enumerate()
        .map(|(position, var)| {
            let raw = unsafe { *arguments.add(position) };
            if boxed && position == 0 { V::P(raw + 8) } else { load_raw(*var, raw) }
        })
        .collect();
    runtime::roots().transitions.push((frame, stack));
    let outcome = interpreter.invoke_ctx(key, &ctx, &values);
    runtime::roots().transitions.pop();
    match outcome {
        Ok(value) => {
            let returns = loaded.returns.as_ref().and_then(Ty::prim);
            let raw = result_raw(returns, value);
            if matches!(returns, Some(VarType::F32 | VarType::F64)) {
                raw[1]
            } else {
                raw[0]
            }
        }
        Err(fault) => {
            let metadata = &interpreter.model.set.modules[key.module].metadata;
            if metadata.string_cell(TableId::MethodDef, key.row, 3).ok() == Some(".cctor") {
                let flag = runtime::state().init_flag(loaded.owner) as *mut u8;
                unsafe { *flag = 0 };
            }
            drop(values);
            drop(parameters);
            drop(loaded);
            abort_compiled(interpreter, frame, fault)
        }
    }
}

fn abort_compiled(interpreter: &Interpreter<'_>, frame: u64, fault: Fault) -> ! {
    interpreter
        .pending
        .borrow_mut()
        .get_or_insert(Pending::Fault(fault));
    let outermost = outermost_compiled(frame);
    let Some(routine) = crate::jit::abort_routine() else {
        std::process::abort();
    };
    let routine: extern "win64" fn(u64) -> ! = unsafe { std::mem::transmute(routine as usize) };
    routine(outermost)
}

impl<'s> Interpreter<'s> {
    pub fn new(model: &'s ObjectModel<'s>, image: Option<&'s Image>) -> Self {
        crate::threads::acquire();
        crate::diagnostics::attach(model);
        let state = runtime::install();
        state.root_hook = Some(scan_roots);
        Self {
            model,
            image: Cell::new(image.map_or(std::ptr::null(), |i| i as *const Image)),
            loaded: RefCell::new(HashMap::new()),
            types: Types::new(model.set),
            exact_layouts: RefCell::new(HashMap::new()),
            generic_init: RefCell::new(HashMap::new()),
            vtables: RefCell::new(HashMap::new()),
            functions: RefCell::new(Vec::new()),
            type_objects: RefCell::new(HashMap::new()),
            object_types: RefCell::new(HashMap::new()),
            rva_sizes: RefCell::new(HashMap::new()),
            implementations: RefCell::new(HashMap::new()),
            names: RefCell::new(HashMap::new()),
            resources: RefCell::new(HashMap::new()),
            failed_types: RefCell::new(HashMap::new()),
            fields: RefCell::new(HashMap::new()),
            intrinsic_rows: RefCell::new(HashMap::new()),
            interpreted: Cell::new(0),
            compiled_calls: Cell::new(0),
            pending: RefCell::new(None),
            budget: Cell::new(u64::MAX),
            trace_calls: std::env::var_os("KILN_CALLS").is_some(),
            platforms: RefCell::new(Vec::new()),
            callbacks: RefCell::new(Vec::new()),
            unmanaged_rows: RefCell::new(HashMap::new()),
            unmanaged_stubs: RefCell::new(HashMap::new()),
            delegate_stubs: RefCell::new(HashMap::new()),
            assemblies: RefCell::new(HashMap::new()),
            entry_module: Cell::new(0),
            module_objects: RefCell::new(HashMap::new()),
            method_tables: RefCell::new(HashMap::new()),
            method_table_types: RefCell::new(HashMap::new()),
            native_layouts: RefCell::new(HashMap::new()),
            handles: RefCell::new(members::Handles::default()),
            native_delegates: RefCell::new(HashMap::new()),
            indirect_platforms: RefCell::new(HashMap::new()),
            wrappers: RefCell::new(com::Wrappers::default()),
            accessors: RefCell::new(Vec::new()),
            used_modules: RefCell::new(HashSet::new()),
            method_insts: RefCell::new(HashMap::new()),
            assignable: RefCell::new(HashMap::new()),
        }
    }

    pub fn array_record_for(&self, module: usize, token: u32) -> Option<u64> {
        let empty = Ctx {
            types: Rc::from(Vec::new()),
            methods: Rc::from(Vec::new()),
        };
        self.array_type(module, Token(token), &empty).ok().map(|(record, _, _)| record)
    }

    pub fn install_bridge(&self) {
        let bridge = Box::new(Bridge {
            interpreter: (self as *const Interpreter<'_>).cast(),
            keys: RefCell::new(Vec::new()),
            stubs: RefCell::new(HashMap::new()),
        });
        BRIDGE.store(Box::into_raw(bridge), std::sync::atomic::Ordering::Release);
        runtime::state().late_resolver = Some(resolve_stub);
        runtime::state().boxed_resolver = Some(resolve_boxed_stub);
        unsafe { AddVectoredExceptionHandler(1, hardware_fault) };
    }

    pub fn attach(&self, image: &Image) {
        self.image.set(image as *const Image);
    }

    pub fn stub_for(&self, key: &MethodKey) -> Option<u64> {
        self.stub_for_ctx(key, &Ctx::default())
    }

    fn stub_for_ctx(&self, key: &MethodKey, ctx: &Ctx) -> Option<u64> {
        self.stub_entry(key, ctx, false)
    }

    fn stub_entry(&self, key: &MethodKey, ctx: &Ctx, boxed: bool) -> Option<u64> {
        let bridge = BRIDGE.load(std::sync::atomic::Ordering::Acquire);
        if bridge.is_null() {
            return None;
        }
        let bridge = unsafe { &*bridge };
        let inst = (self.inst_key(key, ctx), boxed);
        if let Some(address) = bridge.stubs.borrow().get(&inst) {
            return Some(*address);
        }
        let loaded = self.load_ctx(key, ctx).ok()?;
        let index = bridge.keys.borrow().len() as u64;
        let address = crate::jit::emit_fallback_stub(&prims(&loaded.parameters)?, index)?;
        bridge.keys.borrow_mut().push((key.clone(), ctx.clone(), boxed));
        bridge.stubs.borrow_mut().insert(inst, address);
        Some(address)
    }

    pub fn type_name(&self, handle: TypeHandle) -> String {
        if let Some(name) = self.names.borrow().get(&handle) {
            return name.clone();
        }
        let name = self.model.set.full_type_name(handle).unwrap_or_default();
        self.names.borrow_mut().insert(handle, name.clone());
        name
    }

    fn corelib_type(&self, namespace: &str, name: &str) -> Run<TypeHandle> {
        let core = self
            .model
            .set
            .find_assembly("System.Private.CoreLib")
            .ok_or_else(|| Fault::Unsupported("System.Private.CoreLib is not loaded".into()))?;
        self.model
            .set
            .find_type_by_name(core, namespace, name)
            .ok_or_else(|| Fault::Unsupported(format!("{namespace}.{name} was not found")))
    }

    pub fn exception_type(&self, object: u64) -> String {
        match self.object_class(object) {
            Ok(handle) => {
                let name = self.type_name(handle);
                name.rsplit('.').next().unwrap_or(&name).to_owned()
            }
            Err(_) => "?".into(),
        }
    }

    fn throw<T>(&self, name: &str) -> Run<T> {
        if self.trace_calls {
            eprintln!("{}throw {name}", trace_indent(runtime::roots().depth));
        }
        crate::diagnostics::write("runtime throw", name);
        let handle = match self
            .corelib_type("System", name)
            .or_else(|_| self.corelib_type("System.IO", name))
        {
            Ok(handle) => handle,
            Err(_) => self.corelib_type("System.Threading", name)?,
        };
        let record = self.record(RuntimeType::Class(handle))?;
        let object = runtime::allocate_managed(record, 0);
        if object == 0 {
            return unsupported("out of memory");
        }
        Err(Fault::Exception(object))
    }

    fn record(&self, kind: RuntimeType) -> Run<u64> {
        map(runtime::state().type_record(self.model, kind))
    }

    fn object_class(&self, object: u64) -> Run<TypeHandle> {
        match self.object_gen(object)? {
            Gen::Named(handle, _) => Ok(handle),
            _ => self.corelib_type("System", "Array"),
        }
    }

    fn object_gen(&self, object: u64) -> Run<Gen> {
        let record = unsafe { *(object as *const u64) } & !1;
        let identity = unsafe { *((record + 40) as *const u64) };
        if identity & EXACT_TAG != 0 && identity != u64::MAX {
            return self
                .types
                .get((identity & !EXACT_TAG) as u32)
                .ok_or_else(|| Fault::Unsupported("unknown exact type".into()));
        }
        if identity != u64::MAX {
            return Ok(Gen::Named(
                TypeHandle {
                    module: (identity >> 32) as usize,
                    row: identity as u32,
                },
                Rc::from(Vec::new()),
            ));
        }
        let kind = runtime::state().kinds.get(&record).copied();
        let element = match kind {
            Some(RuntimeType::String) => return map(self.types.core("String")),
            Some(RuntimeType::Array(kind)) => map(self.types.core(element_core_name(kind)))?,
            Some(RuntimeType::ValueArray(handle)) => Gen::Named(handle, Rc::from(Vec::new())),
            Some(RuntimeType::ExactArray(id)) => self
                .types
                .get(id)
                .ok_or_else(|| Fault::Unsupported("unknown exact type".into()))?,
            _ => {
                let flags = unsafe { *((record + 8) as *const u64) };
                if flags & runtime::FLAG_STRING != 0 {
                    return map(self.types.core("String"));
                }
                return unsupported("object of an unknown type");
            }
        };
        Ok(Gen::SzArray(Rc::new(element)))
    }

    fn load(&self, key: &MethodKey) -> Run<Rc<Loaded>> {
        self.load_ctx(key, &Ctx::default())
    }

    fn inst_key(&self, key: &MethodKey, ctx: &Ctx) -> InstKey {
        (
            key.clone(),
            ctx.types.iter().map(|g| self.types.id(g)).collect(),
            ctx.methods.iter().map(|g| self.types.id(g)).collect(),
        )
    }

    fn load_ctx(&self, key: &MethodKey, ctx: &Ctx) -> Run<Rc<Loaded>> {
        let inst = self.inst_key(key, ctx);
        if let Some(loaded) = self.loaded.borrow().get(&inst) {
            return Ok(loaded.clone());
        }
        let loaded = Rc::new(self.load_uncached(key, ctx)?);
        self.loaded.borrow_mut().insert(inst, loaded.clone());
        Ok(loaded)
    }

    fn ty_of(&self, module: usize, sig: &TypeSig, ctx: &Ctx) -> Run<Ty> {
        let exact = map(self.types.resolve(module, sig, ctx))?;
        self.gen_ty(&exact)
    }

    fn gen_ty(&self, exact: &Gen) -> Run<Ty> {
        match exact {
            Gen::Named(handle, _) => {
                let set = self.model.set;
                let name = self.type_name(*handle);
                let primitive = match name.as_str() {
                    "System.Boolean" | "System.Byte" => Some(VarType::U8),
                    "System.SByte" => Some(VarType::I8),
                    "System.Int16" => Some(VarType::I16),
                    "System.UInt16" | "System.Char" => Some(VarType::U16),
                    "System.Int32" | "System.UInt32" => Some(VarType::I32),
                    "System.Int64" | "System.UInt64" | "System.IntPtr" | "System.UIntPtr" => {
                        Some(VarType::I64)
                    }
                    "System.Single" => Some(VarType::F32),
                    "System.Double" => Some(VarType::F64),
                    "System.Void" => return unsupported("a value of type void"),
                    "System.TypedReference" => return unsupported("typed references"),
                    _ => None,
                };
                if let Some(var) = primitive {
                    return Ok(Ty::Prim(var));
                }
                if !set.is_value_type(*handle).unwrap_or(false) {
                    return Ok(Ty::Prim(VarType::Ref));
                }
                if set.is_enum(*handle).unwrap_or(false) {
                    let size = set
                        .value_type_layout(*handle)
                        .map_err(|e| Fault::Unsupported(format!("{e:?}")))?
                        .size;
                    return Ok(Ty::Prim(match size {
                        1 => VarType::U8,
                        2 => VarType::U16,
                        4 => VarType::I32,
                        _ => VarType::I64,
                    }));
                }
                Ok(Ty::Struct(self.layout_of(exact)?))
            }
            Gen::SzArray(_) | Gen::Array(_, _) | Gen::Var(..) | Gen::MethodVar(..) => Ok(Ty::Prim(VarType::Ref)),
            Gen::ByRef(_) => Ok(Ty::Prim(VarType::Pointer)),
            Gen::Pointer(_) | Gen::FunctionPointer => Ok(Ty::Prim(VarType::I64)),
        }
    }

    fn layout_of(&self, exact: &Gen) -> Run<Rc<StructLayout>> {
        let Gen::Named(handle, arguments) = exact else {
            return unsupported("layout of a non struct");
        };
        if arguments.is_empty()
            && let Ok(layout) = self.model.struct_layout(*handle)
        {
            return Ok(layout);
        }
        let id = self.types.id(exact);
        if let Some(found) = self.exact_layouts.borrow().get(&id) {
            return Ok(found.clone());
        }
        let layout_arguments = self.types.layout_arguments(arguments).map_err(Fault::Unsupported)?;
        let (layout, placements) = self
            .model
            .set
            .value_type_fields_with(*handle, &layout_arguments)
            .map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
        let ctx = exact.context();
        let mut references = Vec::new();
        let mut fields = HashMap::new();
        for placement in &placements {
            fields.insert(placement.field, placement.offset);
            if !placement.layout.has_references {
                continue;
            }
            let sig = map(self.model.field_signature(handle.module, placement.field))?;
            let field = map(self.types.resolve(handle.module, &sig, &ctx))?;
            match self.gen_ty(&field)? {
                Ty::Prim(VarType::Ref) => references.push(placement.offset),
                Ty::Struct(inner) => {
                    references.extend(inner.references.iter().map(|r| placement.offset + r))
                }
                Ty::Prim(_) => {}
            }
        }
        let built = Rc::new(StructLayout {
            handle: *handle,
            instance: Some(id),
            size: layout.size,
            references,
            fields,
        });
        self.exact_layouts.borrow_mut().insert(id, built.clone());
        Ok(built)
    }

    fn layout_gen(&self, layout: &StructLayout) -> Gen {
        layout
            .instance
            .and_then(|id| self.types.get(id))
            .unwrap_or_else(|| Gen::Named(layout.handle, Rc::from(Vec::new())))
    }

    fn record_of(&self, exact: &Gen) -> Run<u64> {
        match exact {
            Gen::Named(handle, arguments) => {
                if arguments.is_empty() {
                    if self.type_name(*handle) == "System.String" {
                        return self.record(RuntimeType::String);
                    }
                    if let Ok(record) = self.record(RuntimeType::Class(*handle)) {
                        return Ok(record);
                    }
                }
                let id = self.types.id(exact);
                if let Some(found) = runtime::state().types.get(&RuntimeType::Exact(id)) {
                    return Ok(found.words.as_ptr() as u64);
                }
                let set = self.model.set;
                if set.is_value_type(*handle).unwrap_or(false) {
                    let layout = self.layout_of(exact)?;
                    let offsets: Vec<u32> = layout.references.iter().map(|r| r + 8).collect();
                    let vtable = self.exact_vtable(exact);
                    return Ok(runtime::state().custom_record(
                        RuntimeType::Exact(id),
                        u64::from((layout.size + 8).next_multiple_of(8).max(16)),
                        0,
                        0,
                        0,
                        offsets,
                        EXACT_TAG | u64::from(id),
                        vtable,
                    ));
                }
                let layout_arguments = self.types.layout_arguments(arguments).map_err(Fault::Unsupported)?;
                let (end, placements) = set
                    .class_fields_with(*handle, &layout_arguments)
                    .map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
                let parent = match map(self.types.base(exact))? {
                    Some(base) => self.record_of(&base)?,
                    None => 0,
                };
                let mut offsets = if parent != 0 {
                    runtime::state().record_offsets(parent)
                } else {
                    Vec::new()
                };
                let ctx = exact.context();
                for placement in &placements {
                    if !placement.layout.has_references {
                        continue;
                    }
                    let sig = map(self.model.field_signature(handle.module, placement.field))?;
                    let field = map(self.types.resolve(handle.module, &sig, &ctx))?;
                    match self.gen_ty(&field)? {
                        Ty::Prim(VarType::Ref) => offsets.push(placement.offset + 8),
                        Ty::Struct(inner) => offsets
                            .extend(inner.references.iter().map(|r| placement.offset + 8 + r)),
                        Ty::Prim(_) => {}
                    }
                }
                let vtable = self.exact_vtable(exact);
                Ok(runtime::state().custom_record(
                    RuntimeType::Exact(id),
                    u64::from((end + 8).next_multiple_of(8).max(16)),
                    0,
                    0,
                    parent,
                    offsets,
                    EXACT_TAG | u64::from(id),
                    vtable,
                ))
            }
            Gen::SzArray(element) => self.array_record(element),
            _ => unsupported(format!("objects of type {}", self.types.full_name(exact))),
        }
    }

    fn exact_vtable(&self, exact: &Gen) -> Vec<u64> {
        let state = runtime::state();
        let trap = state.trap_address;
        let Ok(table) = self.vtable_of(exact) else {
            return Vec::new();
        };
        let boxed = self.types.is_value_type(exact);
        table
            .slots
            .iter()
            .map(|slot| {
                let Some((key, declaring)) = slot else {
                    return trap;
                };
                let Ok(owner) = self.model.method_owner(key) else {
                    return trap;
                };
                let ctx = self.owner_context(declaring, owner, &Rc::from(Vec::new()));
                if boxed && self.model.set.is_value_type(owner).unwrap_or(false) {
                    return self.stub_entry(key, &ctx, true).unwrap_or(trap);
                }
                let compiled = if ctx.is_empty() { runtime::state().code_addresses.get(key).copied() } else { None };
                compiled.or_else(|| self.stub_for_ctx(key, &ctx)).unwrap_or(trap)
            })
            .collect()
    }

    fn array_record(&self, element: &Gen) -> Run<u64> {
        match self.gen_ty(element)? {
            Ty::Prim(var) => {
                let kind = ElementKind::from_var(var).unwrap_or(ElementKind::I64);
                let shared = self.record(RuntimeType::Array(kind))?;
                if map(self.types.core(element_core_name(kind)))? == *element {
                    return Ok(shared);
                }
                let references = if kind == ElementKind::Reference {
                    runtime::FLAG_REFERENCE_ELEMENTS
                } else {
                    0
                };
                let vtable = record_vtable(shared);
                Ok(runtime::state().custom_record(
                    RuntimeType::ExactArray(self.types.id(element)),
                    u64::from(ARRAY_DATA),
                    u64::from(kind.size()),
                    runtime::FLAG_ARRAY | references,
                    shared,
                    Vec::new(),
                    u64::MAX,
                    vtable,
                ))
            }
            Ty::Struct(layout) => match layout.instance {
                None => self.record(RuntimeType::ValueArray(layout.handle)),
                Some(id) => {
                    let vtable = record_vtable(self.record(RuntimeType::Array(ElementKind::Reference))?);
                    Ok(runtime::state().custom_record(
                        RuntimeType::ExactArray(id),
                        u64::from(ARRAY_DATA),
                        u64::from(layout.size),
                        runtime::FLAG_ARRAY | runtime::FLAG_STRUCT_ELEMENTS,
                        0,
                        layout.references.clone(),
                        u64::MAX,
                        vtable,
                    ))
                }
            },
        }
    }

    fn element_size(&self, element: &Gen) -> Run<(Ty, u32)> {
        let ty = self.gen_ty(element)?;
        let size = match &ty {
            Ty::Prim(var) => ElementKind::from_var(*var).map_or(8, ElementKind::size),
            Ty::Struct(layout) => layout.size,
        };
        Ok((ty, size))
    }

    fn allocate_box(&self, exact: &Gen) -> Run<u64> {
        let record = self.record_of(exact)?;
        match runtime::allocate_managed(record, 0) {
            0 => self.throw("OutOfMemoryException"),
            object => Ok(object),
        }
    }

    fn read_value(&self, address: u64, ty: &Ty, keep: &[V]) -> Run<V> {
        match ty {
            Ty::Prim(var) => Ok(read(address, *var)),
            Ty::Struct(layout) => {
                let _pin = Pin::new(keep);
                let boxed = self.allocate_box(&self.layout_gen(layout))?;
                unsafe {
                    std::ptr::copy(address as *const u8, (boxed + 8) as *mut u8, layout.size as usize);
                }
                Ok(V::S(boxed))
            }
        }
    }

    fn write_value(&self, address: u64, ty: &Ty, value: V) -> Run<()> {
        match ty {
            Ty::Prim(var) => {
                write(address, *var, value);
                Ok(())
            }
            Ty::Struct(layout) => match value {
                V::S(boxed) | V::O(boxed) if boxed != 0 => {
                    unsafe {
                        std::ptr::copy(
                            (boxed + 8) as *const u8,
                            address as *mut u8,
                            layout.size as usize,
                        );
                    }
                    Ok(())
                }
                other => unsupported(format!(
                    "a value of struct {} was expected, got {other:?}",
                    self.type_name(layout.handle)
                )),
            },
        }
    }

    fn load_uncached(&self, key: &MethodKey, ctx: &Ctx) -> Run<Loaded> {
        self.mark_used(key.module);
        let set = self.model.set;
        let module = &set.modules[key.module];
        let metadata = &module.metadata;
        let owner = map(self.model.method_owner(key))?;
        let sig = map(self.model.method_sig(key))?;
        let owner_parameters = self.types.parameter_count(TableId::TypeDef, owner.row, owner.module);
        if ctx.types.len() != owner_parameters || ctx.methods.len() != sig.generic_parameters as usize {
            return unsupported("open generic method");
        }
        let owner_gen = Gen::Named(owner, ctx.types.clone());
        if sig.convention != CallingConvention::Default {
            return unsupported("non default calling convention");
        }
        let flags = metadata.cell(TableId::MethodDef, key.row, 2).unwrap_or(0);
        let implementation = metadata.cell(TableId::MethodDef, key.row, 1).unwrap_or(0);
        let rva = metadata.cell(TableId::MethodDef, key.row, 0).unwrap_or(0);
        let owner_is_value = set.is_value_type(owner).unwrap_or(false);
        let mut parameters = Vec::new();
        if sig.has_this {
            parameters.push(Ty::Prim(if owner_is_value {
                VarType::Pointer
            } else {
                VarType::Ref
            }));
        }
        for parameter in &sig.parameters {
            parameters.push(self.ty_of(key.module, parameter, ctx)?);
        }
        let returns = match &sig.return_type {
            TypeSig::Void => None,
            TypeSig::Modified { inner, .. } if **inner == TypeSig::Void => None,
            other => Some(self.ty_of(key.module, other, ctx)?),
        };
        let name = metadata.string_cell(TableId::MethodDef, key.row, 3).unwrap_or("");
        let primitive_returns = match &returns {
            None => Some(None),
            Some(Ty::Prim(var)) => Some(Some(*var)),
            Some(Ty::Struct(_)) => None,
        };
        let native = match (prims(&parameters), primitive_returns) {
            _ if name == ".ctor" && self.type_name(owner) == "System.String" => {
                Some(Native::StringConstructor)
            }
            _ if !sig.has_this
                && name == "GetRuntimeType"
                && parameters.len() == 1
                && self.type_name(owner) == "System.RuntimeTypeHandle" =>
            {
                Some(Native::Intrinsic(Intrinsic::Reflect(reflection::Reflect::Member(members::Member::TypeFromMethodTable))))
            }
            _ if reflection::internal(&self.type_name(owner), name, parameters.len()).is_some() => {
                reflection::internal(&self.type_name(owner), name, parameters.len()).map(|r| Native::Intrinsic(Intrinsic::Reflect(r)))
            }
            _ if self.generic_native(owner, name, ctx).is_some() => {
                self.generic_native(owner, name, ctx)
            }
            (Some(parameters), Some(returns)) => self.native(owner, name, &parameters, returns),
            _ if name == "GetTypeFromHandle" && self.type_name(owner) == "System.Type" => {
                Some(Native::Intrinsic(Intrinsic::TypeFromHandle))
            }
            _ if name == "InitializeArray"
                && self.type_name(owner) == "System.Runtime.CompilerServices.RuntimeHelpers"
                && parameters.len() == 2 =>
            {
                Some(Native::Intrinsic(Intrinsic::InitializeArray))
            }
            _ => self.delegate_native(owner, name),
        };
        let native = match native {
            None if flags & PINVOKE != 0 => {
                Some(self.platform_invoke(key, name, &sig, &parameters, &returns, implementation, ctx)?)
            }
            None if rva == 0 && flags & ABSTRACT == 0 => self.unsafe_accessor(key, ctx)?.map(|accessor| {
                let mut accessors = self.accessors.borrow_mut();
                accessors.push(accessor);
                Native::Accessor(accessors.len() - 1)
            }),
            other => other,
        };
        let mut code: &'static [u8] = &[];
        let mut clauses = Vec::new();
        let mut locals = Vec::new();
        if native.is_none() {
            if implementation & INTERNAL_CALL != 0 || implementation & RUNTIME_IMPL != 0 {
                return unsupported(format!(
                    "internal call {}::{name}",
                    self.type_name(owner)
                ));
            }
            if rva == 0 && flags & ABSTRACT != 0 {
                return Ok(Loaded {
                    key: key.clone(),
                    ctx: ctx.clone(),
                    owner,
                    owner_gen,
                    code: &[],
                    clauses: Vec::new(),
                    parameters,
                    returns,
                    locals,
                    native,
                    is_static: flags & STATIC != 0,
                    is_virtual: flags & VIRTUAL != 0,
                    is_abstract: true,
                });
            }
            if rva == 0 {
                return unsupported(format!("method without a body {}::{name}", self.type_name(owner)));
            }
            let body = module
                .image
                .tail_at_rva(rva)
                .and_then(|tail| MethodBody::parse(tail, rva))
                .map_err(|e| Fault::Unsupported(e.to_string()))?;
            if !body.local_signature.is_null() {
                let blob = metadata
                    .cell(TableId::StandAloneSig, body.local_signature.row(), 0)
                    .and_then(|b| metadata.blob(b))
                    .map_err(|e| Fault::Unsupported(e.to_string()))?;
                let Ok(Signature::Locals(types)) = Signature::parse(blob) else {
                    return unsupported("local signature does not parse");
                };
                {
                    for local in &types {
                        locals.push(match local {
                            TypeSig::Pinned(inner) => self.ty_of(key.module, inner, ctx)?,
                            other => self.ty_of(key.module, other, ctx)?,
                        });
                    }
                }
            }
            if self.is_intrinsic(key, owner) && self.calls_itself(key, body.code) {
                return unsupported(format!("JIT intrinsic {}::{name}", self.type_name(owner)));
            }
            code = body.code;
            clauses = body.clauses;
        }
        Ok(Loaded {
            key: key.clone(),
            ctx: ctx.clone(),
            owner,
            owner_gen,
            code,
            clauses,
            parameters,
            returns,
            locals,
            native,
            is_static: flags & STATIC != 0,
            is_virtual: flags & VIRTUAL != 0,
            is_abstract: false,
        })
    }

    fn is_intrinsic(&self, key: &MethodKey, owner: TypeHandle) -> bool {
        let mut cache = self.intrinsic_rows.borrow_mut();
        let (methods, types) = cache.entry(key.module).or_insert_with(|| {
            let scan = |table| {
                self.model
                    .set
                    .rows_with_attribute(
                        key.module,
                        table,
                        "System.Runtime.CompilerServices",
                        "IntrinsicAttribute",
                    )
                    .unwrap_or_default()
            };
            (scan(TableId::MethodDef), scan(TableId::TypeDef))
        });
        methods.contains(&key.row) || types.contains(&owner.row)
    }

    fn calls_itself(&self, key: &MethodKey, code: &[u8]) -> bool {
        let body = MethodBody {
            max_stack: 0,
            local_signature: Token(0),
            init_locals: false,
            code,
            clauses: Vec::new(),
        };
        body.instructions().flatten().any(|instruction| {
            instruction.opcode.value == 0x28
                && matches!(instruction.argument, Argument::Token(token)
                    if self.method_inst(key.module, token, &Ctx::default()).ok().map(|m| m.0).as_ref() == Some(key)
                        || self.method_token(key.module, token).ok().as_ref() == Some(key))
        })
    }

    fn native(
        &self,
        owner: TypeHandle,
        name: &str,
        parameters: &[VarType],
        returns: Option<VarType>,
    ) -> Option<Native> {
        let type_name = self.type_name(owner);
        if let Some(operation) = threading::thread_internal(&type_name, name, parameters.len()) {
            return Some(Native::Intrinsic(Intrinsic::Thread(operation)));
        }
        if let Some(operation) = reflection::internal(&type_name, name, parameters.len()) {
            return Some(Native::Intrinsic(Intrinsic::Reflect(operation)));
        }
        let float = parameters.first() == Some(&VarType::F32);
        if let Some(native) = self.delegate_native(owner, name) {
            return Some(native);
        }
        if self.is_delegate(owner) && type_name != "System.Delegate" && type_name != "System.MulticastDelegate" {
            return None;
        }
        let intrinsic = match (type_name.as_str(), name) {
            (t, "get_IsSupported")
                if t.starts_with("System.Runtime.Intrinsics.X86.")
                    || t.starts_with("System.Runtime.Intrinsics.Arm.")
                    || t.starts_with("System.Runtime.Intrinsics.Wasm.") =>
            {
                Intrinsic::False
            }
            (t, "get_IsHardwareAccelerated")
                if t.starts_with("System.Runtime.Intrinsics.") || t == "System.Numerics.Vector" =>
            {
                Intrinsic::False
            }
            (t, "UsingResourceKeys") if t == "System.SR" || t.ends_with(".SR") => Intrinsic::False,
            ("System.Double" | "System.Single", "MultiplyAddEstimate") if parameters.len() == 3 => {
                match (float, std::arch::is_x86_feature_detected!("fma")) {
                    (true, true) => Intrinsic::FusedF,
                    (false, true) => Intrinsic::Fused,
                    (true, false) => Intrinsic::MultiplyAddF,
                    (false, false) => Intrinsic::MultiplyAdd,
                }
            }
            ("System.Math" | "System.MathF", "FusedMultiplyAdd") if parameters.len() == 3 => {
                if float { Intrinsic::FusedF } else { Intrinsic::Fused }
            }
            ("System.Math" | "System.MathF", _) => return self.math(name, parameters, float),
            ("System.String", "get_Length") => Intrinsic::StringLength,
            ("System.String", "get_Chars") => Intrinsic::StringChar,
            ("System.String", "FastAllocateString") => Intrinsic::StringAllocate,
            ("System.String", "Equals" | "op_Equality")
                if parameters.len() == 2 && returns == Some(VarType::U8) =>
            {
                Intrinsic::StringEquals
            }
            ("System.String", "Concat")
                if (2..=4).contains(&parameters.len())
                    && parameters.iter().all(|p| *p == VarType::Ref)
                    && returns == Some(VarType::Ref) =>
            {
                Intrinsic::StringConcat
            }
            ("System.Object", "MemberwiseClone") => Intrinsic::ObjectClone,
            ("System.Array", "Copy" | "ConstrainedCopy")
                if (parameters.len() == 3 || parameters.len() == 5) && parameters[0] == VarType::Ref =>
            {
                Intrinsic::ArrayCopy
            }
            ("System.Array", "Clear") if parameters.len() == 3 || parameters.len() == 1 => Intrinsic::ArrayClear,
            ("System.Buffer", "BlockCopy") if parameters.len() == 5 => Intrinsic::BlockCopy,
            ("Interop+BCrypt", "BCryptGenRandom") if parameters.len() == 4 => Intrinsic::RandomBytes,
            ("System.Environment", "get_CurrentManagedThreadId") => Intrinsic::ThreadId,
            ("System.Environment", "set_ExitCode") if parameters.len() == 1 => Intrinsic::SetExitCode,
            ("System.Environment", "get_ExitCode") if parameters.is_empty() => Intrinsic::ExitCode,
            ("System.RuntimeTypeHandle", "InternalAllocNoChecks_FastPath") if parameters.len() == 1 => Intrinsic::AllocateRecord(false),
            ("System.Enum", "GetHashCode") if parameters.len() == 1 => Intrinsic::EnumHash,
            ("System.Enum", "Equals") if parameters.len() == 2 => Intrinsic::EnumEquals,
            ("System.Threading.Thread", "GetCurrentThreadNative") if parameters.is_empty() => Intrinsic::CurrentThread,
            ("System.GC", "_CollectionCount") => Intrinsic::Collections,
            ("System.GC", "GetMemoryInfo") if parameters.len() == 2 => Intrinsic::Nothing,
            ("System.GC", "GetMaxGeneration" | "GetGeneration" | "GetGenerationInternal") => Intrinsic::False,
            ("System.Diagnostics.Debugger", "get_IsAttached" | "IsLogging") => Intrinsic::False,
            ("System.Diagnostics.Debugger", "Log" | "NotifyOfCrossThreadDependencySlow") => Intrinsic::Nothing,
            ("System.RuntimeType", "CreateInstanceDefaultCtor") if parameters.len() == 3 => Intrinsic::DefaultConstruct,
            ("System.Runtime.DependentHandle", "InternalFree") if parameters.len() == 1 => Intrinsic::DependentFree,
            ("System.Runtime.DependentHandle", "InternalSetTargetToNull") if parameters.len() == 1 => Intrinsic::DependentClear,
            ("System.Runtime.DependentHandle", "InternalSetDependent") if parameters.len() == 2 => Intrinsic::DependentSetSecond,
            ("System.Runtime.DependentHandle", "InternalGetTargetAndDependent") if parameters.len() == 2 => Intrinsic::DependentBoth,
            ("System.Runtime.DependentHandle", "InternalGetDependent") if parameters.len() == 1 => Intrinsic::DependentSecond,
            ("System.Runtime.DependentHandle", "InternalGetTarget") if parameters.len() == 1 => Intrinsic::DependentTarget,
            ("System.Runtime.DependentHandle", "InternalAlloc") if parameters.len() == 2 => Intrinsic::DependentAlloc,
            ("System.Runtime.CompilerServices.RuntimeHelpers", "TryEnsureSufficientExecutionStack") => Intrinsic::True,
            ("System.Runtime.CompilerServices.RuntimeHelpers", "EnsureSufficientExecutionStack") => Intrinsic::Nothing,
            ("System.Runtime.CompilerServices.RuntimeHelpers", "PrepareDelegate" | "PrepareMethod" | "PrepareContractedDelegate" | "ProbeForSufficientStack") => Intrinsic::Nothing,
            ("System.Runtime.CompilerServices.RuntimeHelpers", "GetHashCode" | "TryGetHashCode" | "InternalGetHashCode") if parameters.len() == 1 => Intrinsic::IdentityHash,
            ("System.Runtime.InteropServices.GCHandle", "_InternalAlloc" | "InternalAlloc") if parameters.len() == 2 => {
                Intrinsic::HandleAlloc
            }
            ("System.Runtime.InteropServices.GCHandle", "_InternalFree" | "InternalFree") if parameters.len() == 1 => {
                Intrinsic::HandleFree
            }
            ("System.Runtime.InteropServices.GCHandle", "InternalGet") if parameters.len() == 1 => Intrinsic::HandleGet,
            ("System.Runtime.InteropServices.GCHandle", "InternalSet") if parameters.len() == 2 => Intrinsic::HandleSet,
            ("System.Runtime.InteropServices.GCHandle", "InternalCompareExchange") if parameters.len() == 3 => {
                Intrinsic::HandleExchange
            }
            ("System.GC", "_SuppressFinalize" | "_ReRegisterForFinalize") if parameters.len() == 1 => {
                Intrinsic::Nothing
            }
            ("System.Runtime.InteropServices.Marshal", "GetFunctionPointerForDelegateInternal")
                if parameters.len() == 1 =>
            {
                Intrinsic::DelegatePointer
            }
            ("System.Threading.Interlocked", "MemoryBarrier" | "ReadMemoryBarrier") if parameters.is_empty() => {
                Intrinsic::Fence
            }
            ("System.RuntimeTypeHandle", "CanCastTo") if parameters.len() == 2 => Intrinsic::CanCastTo,
            ("System.RuntimeTypeHandle", "CreateInstanceForAnotherGenericParameter")
                if (2..=3).contains(&parameters.len())
                    && parameters.iter().all(|p| *p == VarType::Ref)
                    && returns == Some(VarType::Ref) =>
            {
                Intrinsic::AnotherGenericParameter
            }
            ("System.Runtime.InteropServices.Marshal", "GetLastPInvokeError" | "GetLastWin32Error")
                if parameters.is_empty() =>
            {
                Intrinsic::LastError
            }
            ("System.Runtime.InteropServices.Marshal", "SetLastPInvokeError") if parameters.len() == 1 => {
                Intrinsic::SetLastError
            }
            ("System.Runtime.InteropServices.Marshal", "GetLastSystemError") if parameters.is_empty() => Intrinsic::SystemError,
            ("System.Runtime.InteropServices.Marshal", "SetLastSystemError") if parameters.len() == 1 => Intrinsic::SetSystemError,
            ("System.Threading.Interlocked", "Exchange")
                if parameters.len() == 2 && parameters[0] == VarType::Pointer =>
            {
                Intrinsic::Exchange(parameters[1])
            }
            ("System.Threading.Interlocked", "CompareExchange")
                if parameters.len() == 3 && parameters[0] == VarType::Pointer =>
            {
                Intrinsic::CompareExchange(parameters[1])
            }
            ("System.Threading.Interlocked", "ExchangeAdd")
                if parameters.len() == 2 && parameters[0] == VarType::Pointer =>
            {
                Intrinsic::ExchangeAdd(parameters[1])
            }
            ("System.Runtime.CompilerServices.RuntimeHelpers", "InitializeArray") => {
                Intrinsic::InitializeArray
            }
            ("System.Buffer", "Memmove") if parameters.len() == 3 => Intrinsic::Memmove,
            ("System.Buffer", "__Memmove") if parameters.len() == 3 => Intrinsic::Memmove,
            ("System.Buffer", "BulkMoveWithWriteBarrierInternal" | "__BulkMoveWithWriteBarrier") if parameters.len() == 3 => Intrinsic::Memmove,
            _ => {
                if (type_name == "System.SR" || type_name.ends_with(".SR"))
                    && name == "GetResourceString"
                    && (parameters.len() == 1
                        || (parameters.len() == 2 && !self.forwards_resource_lookup(owner)))
                {
                    Intrinsic::ResourceString(owner.module)
                } else {
                    return None;
                }
            }
        };
        Some(Native::Intrinsic(intrinsic))
    }

    fn platform_invoke(
        &self,
        key: &MethodKey,
        name: &str,
        sig: &crate::signature::MethodSig,
        parameters: &[Ty],
        returns: &Option<Ty>,
        implementation: u32,
        ctx: &Ctx,
    ) -> Run<Native> {
        let module = &self.model.set.modules[key.module];
        let metadata = &module.metadata;
        let import = pinvoke::find_import(metadata, key.row)
            .ok_or_else(|| Fault::Unsupported("platform invoke without an import".into()))?;
        let hresult = implementation & pinvoke::PRESERVE_SIG == 0;
        let marshals = pinvoke::marshals(metadata, key.row);
        let native_of = |sequence: u32| marshals.iter().find(|(s, _)| *s == sequence).map(|(_, n)| *n);
        if import.library.eq_ignore_ascii_case("QCall") {
            let entry = if import.entry.is_empty() { name } else { import.entry };
            let intrinsic = match (entry, parameters.len()) {
                (entry, count) if threading::thread_qcall(entry, count).is_some() => {
                    Intrinsic::Thread(threading::thread_qcall(entry, count).unwrap_or(threading::ThreadOp::Ignore))
                }
                ("Buffer_Clear", 2) => Intrinsic::ClearMemory,
                ("Buffer_MemMove", 3) => Intrinsic::Memmove,
                ("String_Intern", 1) => Intrinsic::Intern(false),
                ("NativeLibrary_LoadByName", 5) => Intrinsic::LoadLibrary(false),
                ("NativeLibrary_LoadFromPath", 2) => Intrinsic::LoadLibrary(true),
                ("NativeLibrary_GetSymbol", 3) => Intrinsic::LibrarySymbol,
                ("NativeLibrary_FreeLib", 1) => Intrinsic::FreeLibrary,
                ("String_IsInterned", 1) => Intrinsic::Intern(true),
                ("Environment_GetProcessorCount", 0) => Intrinsic::ProcessorCount,
                ("ThreadNative_SpinWait", 1) => Intrinsic::SpinWait,
                ("MarshalNative_IsBuiltInComSupported", 0) => Intrinsic::True,
                ("GCInterface_AddMemoryPressure" | "GCInterface_RemoveMemoryPressure", 1) => Intrinsic::Nothing,
                ("QCall_GetGCHandleForTypeHandle", 2) => Intrinsic::NewHandle,
                ("Monitor_GetLockContentionCount", 0) => Intrinsic::False,
                ("AssemblyNative_IsApplyUpdateSupported", 0) => Intrinsic::False,
                ("GCInterface_Collect", _) => Intrinsic::Collect,
                ("RuntimeTypeHandle_AllocateTypeAssociatedMemory", _) => Intrinsic::TypeMemory,
                ("ComWrappers_GetIUnknownImpl", 3) => Intrinsic::UnknownImpl,
                ("ComWrappers_GetTaggedImpl", 0) => Intrinsic::TaggedImpl,
                ("ComWrappers_GetIReferenceTrackerTargetVftbl", 0) => Intrinsic::TrackerVtable,
                ("ComWrappers_GetUntrackedAddRefRelease", 0) => Intrinsic::UntrackedReference,
                ("ComWrappers_AllocateRefCountedHandle", 1) => Intrinsic::CountedHandle,
                ("ComWrappers_RegisterIsRootedCallback", _) => Intrinsic::Nothing,
                ("GCInterface_WaitForPendingFinalizers", 0) => Intrinsic::Nothing,
                ("MultiCoreJIT_InternalSetProfileRoot" | "MultiCoreJIT_InternalStartProfile", _) => Intrinsic::Nothing,
                ("ThreadNative_GetCurrentThread", 1) => Intrinsic::CurrentThreadOnStack,
                ("RuntimeTypeHandle_InternalAllocNoChecks" | "RuntimeTypeHandle_InternalAlloc", 2) => Intrinsic::AllocateRecord(true),
                (entry, count) if reflection::qcall(entry, count).is_some() => {
                    Intrinsic::Reflect(reflection::qcall(entry, count).unwrap_or(reflection::Reflect::EntryAssembly))
                }
                _ => return unsupported(format!("runtime QCall {entry}")),
            };
            return Ok(Native::Intrinsic(intrinsic));
        }
        if import.library.trim_start_matches("lib") == "System.Globalization.Native" {
            let entry = if import.entry.is_empty() { name } else { import.entry };
            return match entry {
                "GlobalizationNative_LoadICU" => Ok(Native::Intrinsic(Intrinsic::False)),
                _ => unsupported(format!("globalization entry {entry} without ICU")),
            };
        }
        let unicode = import.unicode();
        let passes = sig
            .parameters
            .iter()
            .zip(parameters)
            .enumerate()
            .map(|(index, (parameter, ty))| {
                let outward = pinvoke::parameter_flags(metadata, key.row, index as u32 + 1) & 0x2 != 0;
                self.platform_pass(key.module, unmodified(parameter), ty, unicode, native_of(index as u32 + 1), ctx, outward)
            })
            .collect::<Run<Vec<_>>>()?;
        let back = self.platform_back(key.module, sig, returns, native_of(0), ctx)?;
        let directory = module.path.parent().unwrap_or(std::path::Path::new("."));
        let function = map(pinvoke::resolve(directory, &import, name))?;
        let mut platforms = self.platforms.borrow_mut();
        platforms.push(Rc::new(Platform {
            function,
            passes,
            back,
            last_error: import.flags & pinvoke::SUPPORTS_LAST_ERROR != 0,
            hresult,
        }));
        Ok(Native::PlatformInvoke(platforms.len() - 1))
    }

    fn platform_back(
        &self,
        module: usize,
        sig: &crate::signature::MethodSig,
        returns: &Option<Ty>,
        native: Option<u8>,
        ctx: &Ctx,
    ) -> Run<Back> {
        let declared = unmodified(&sig.return_type);
        Ok(match (returns, declared) {
            (None, _) => Back::Nothing,
            (Some(_), TypeSig::Boolean) => match native {
                None | Some(0x02) => Back::Bool(0xFFFF_FFFF),
                Some(0x03 | 0x04) => Back::Bool(0xFF),
                Some(other) => return unsupported(format!("platform invoke MarshalAs return type 0x{other:02x}")),
            },
            (Some(Ty::Prim(VarType::Ref)), TypeSig::Class(token)) => {
                let exact = map(self.types.resolve_token(module, *token, ctx))?;
                if !self.is_safe_handle(&exact)? {
                    return unsupported("platform invoke returning an object");
                }
                Back::SafeHandle(exact, self.safe_handle_offset()?)
            }
            (Some(Ty::Prim(VarType::Ref)), _) => return unsupported("platform invoke returning an object"),
            (Some(Ty::Prim(var)), _) => Back::Raw(*var),
            (Some(Ty::Struct(layout)), _) if !layout.references.is_empty() => {
                return unsupported("platform invoke returning a struct with references");
            }
            (Some(Ty::Struct(layout)), _) if matches!(layout.size, 1 | 2 | 4 | 8) => Back::Value(layout.clone()),
            (Some(Ty::Struct(layout)), _) => Back::Buffer(layout.clone()),
        })
    }

    fn indirect_platform(&self, module: usize, row: u32, function: u64, sig: &crate::signature::MethodSig, ctx: &Ctx) -> Run<usize> {
        if let Some(index) = self.indirect_platforms.borrow().get(&(module, row, function)) {
            return Ok(*index);
        }
        if sig.convention == CallingConvention::Default || sig.has_this {
            return unsupported("calli on a managed signature without a managed target");
        }
        let passes = sig
            .parameters
            .iter()
            .map(|parameter| {
                let ty = self.ty_of(module, unmodified(parameter), ctx)?;
                self.platform_pass(module, unmodified(parameter), &ty, true, None, ctx, false)
            })
            .collect::<Run<Vec<_>>>()?;
        let returns = match unmodified(&sig.return_type) {
            TypeSig::Void => None,
            other => Some(self.ty_of(module, other, ctx)?),
        };
        let back = self.platform_back(module, sig, &returns, None, ctx)?;
        let index = {
            let mut platforms = self.platforms.borrow_mut();
            platforms.push(Rc::new(Platform {
                function: function as usize,
                passes,
                back,
                last_error: false,
                hresult: false,
            }));
            platforms.len() - 1
        };
        self.indirect_platforms.borrow_mut().insert((module, row, function), index);
        Ok(index)
    }

    fn native_delegate(&self, function: u64, exact: &Gen) -> Run<u64> {
        let handle = exact.handle().ok_or_else(|| Fault::Unsupported("delegate without a type".into()))?;
        let metadata = &self.model.set.modules[handle.module].metadata;
        let row = metadata
            .method_range(handle.row)
            .ok()
            .and_then(|mut range| range.find(|&row| metadata.string_cell(TableId::MethodDef, row, 3).ok() == Some("Invoke")))
            .ok_or_else(|| Fault::Unsupported("delegate without Invoke".into()))?;
        let key = MethodKey { module: handle.module, row };
        let ctx = exact.context();
        let loaded = self.load_ctx(&key, &ctx)?;
        let sig = map(self.model.method_sig(&key))?;
        let passes = sig
            .parameters
            .iter()
            .zip(&loaded.parameters[1..])
            .map(|(parameter, ty)| self.platform_pass(key.module, unmodified(parameter), ty, false, None, &ctx, false))
            .collect::<Run<Vec<_>>>()?;
        let back = self.platform_back(key.module, &sig, &loaded.returns, None, &ctx)?;
        let index = {
            let mut platforms = self.platforms.borrow_mut();
            platforms.push(Rc::new(Platform {
                function: function as usize,
                passes,
                back,
                last_error: false,
                hresult: false,
            }));
            platforms.len() - 1
        };
        let object = runtime::allocate_managed(self.record_of(exact)?, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        type_roots().push(object);
        self.native_delegates.borrow_mut().insert(object, (index, function));
        Ok(object)
    }

    fn platform_pass(
        &self,
        module: usize,
        sig: &TypeSig,
        ty: &Ty,
        unicode: bool,
        native: Option<u8>,
        ctx: &Ctx,
        outward: bool,
    ) -> Run<Pass> {
        let declared = match sig {
            TypeSig::Class(token) | TypeSig::ValueType(token) => Some(map(self.types.resolve_token(module, *token, ctx))?),
            TypeSig::GenericInst { definition, .. } => Some(map(self.types.resolve_token(module, *definition, ctx))?),
            _ => None,
        };
        let is_delegate = declared
            .as_ref()
            .and_then(Gen::handle)
            .is_some_and(|handle| self.is_delegate(handle));
        if let Some(native) = native {
            return Ok(match (sig, native) {
                (TypeSig::Boolean, 0x02..=0x04) => Pass::Bool,
                (TypeSig::String, 0x14) => Pass::Ansi,
                (TypeSig::String, 0x15 | 0x16) => Pass::Wide,
                (TypeSig::String, 0x30) => Pass::Utf8,
                (TypeSig::SzArray(element), 0x2A) if blittable(element, unicode) => Pass::Array,
                (_, 0x26) if is_delegate => Pass::Delegate,
                (_, 0x28) => Pass::AsAny(unicode),
                (_, 0x2B) if declared.as_ref().is_some_and(|exact| self.has_layout(exact)) => {
                    let exact = declared.clone().unwrap_or(Gen::FunctionPointer);
                    Pass::Formatted(self.native_layout(&exact)?, outward)
                }
                (_, 0x14 | 0x15 | 0x16)
                    if declared
                        .as_ref()
                        .and_then(Gen::handle)
                        .is_some_and(|h| self.type_name(h) == "System.Text.StringBuilder") =>
                {
                    Pass::Builder(native != 0x14)
                }
                (TypeSig::ByRef(inner), 0x02..=0x04) if matches!(unmodified(inner), TypeSig::Boolean) => Pass::BoolOut(native),
                (TypeSig::ByRef(inner), 0x14) if matches!(unmodified(inner), TypeSig::String) => Pass::StringOut(1),
                (TypeSig::ByRef(inner), 0x15 | 0x16) if matches!(unmodified(inner), TypeSig::String) => Pass::StringOut(0),
                (TypeSig::ByRef(inner), 0x30) if matches!(unmodified(inner), TypeSig::String) => Pass::StringOut(2),
                (TypeSig::ByRef(_), 0x19 | 0x1C) => Pass::ComOut,
                (_, 0x19 | 0x1C) if declared.as_ref().is_some_and(|g| self.is_com_interface(g)) => {
                    Pass::Com(declared.clone().unwrap_or(Gen::FunctionPointer))
                }
                (_, 0x19 | 0x1C) => Pass::Unknown,
                (
                    TypeSig::I1
                    | TypeSig::U1
                    | TypeSig::I2
                    | TypeSig::U2
                    | TypeSig::I4
                    | TypeSig::U4
                    | TypeSig::I8
                    | TypeSig::U8
                    | TypeSig::R4
                    | TypeSig::R8
                    | TypeSig::IntPtr
                    | TypeSig::UIntPtr
                    | TypeSig::Char,
                    0x03..=0x0D | 0x1F | 0x20,
                ) => match ty {
                    Ty::Prim(var) => Pass::Raw(*var),
                    Ty::Struct(_) => return unsupported("platform invoke MarshalAs on a struct"),
                },
                _ => return unsupported(format!("platform invoke MarshalAs native type 0x{native:02x}")),
            });
        }
        Ok(match (sig, ty) {
            (TypeSig::Boolean, _) => Pass::Bool,
            (TypeSig::String, _) if unicode => Pass::Wide,
            (TypeSig::String, _) => Pass::Ansi,
            (TypeSig::SzArray(element), _) if blittable(element, unicode) => Pass::Array,
            (TypeSig::SzArray(element), _) if self.blittable_struct(module, unmodified(element), ctx)? => Pass::Array,
            (_, Ty::Prim(VarType::Ref)) if is_delegate => Pass::Delegate,
            (_, Ty::Prim(VarType::Ref)) => match &declared {
                Some(exact) if exact.handle().is_some_and(|h| self.type_name(h) == "System.Text.StringBuilder") => {
                    Pass::Builder(unicode)
                }
                Some(exact) if self.is_safe_handle(exact)? => Pass::SafeHandle(self.safe_handle_offset()?),
                Some(exact) if self.is_com_interface(exact) => Pass::Com(exact.clone()),
                Some(exact) if self.has_layout(exact) => Pass::Formatted(self.native_layout(exact)?, outward),
                _ => {
                    return unsupported(format!(
                        "platform invoke marshaling of {}",
                        declared.as_ref().map(|g| self.types.full_name(g)).unwrap_or_else(|| format!("{sig:?}"))
                    ));
                }
            },
            (TypeSig::ByRef(inner), Ty::Prim(VarType::Pointer)) if matches!(unmodified(inner), TypeSig::Boolean) => {
                Pass::BoolOut(0x02)
            }
            (TypeSig::ByRef(inner), Ty::Prim(VarType::Pointer)) if matches!(unmodified(inner), TypeSig::String) => {
                Pass::StringOut(if unicode { 0 } else { 1 })
            }
            (TypeSig::ByRef(inner), Ty::Prim(VarType::Pointer))
                if matches!(unmodified(inner), TypeSig::Class(_) | TypeSig::GenericInst { .. })
                    && self.is_com_interface(&map(self.types.resolve(module, unmodified(inner), ctx))?) =>
            {
                Pass::ComOut
            }
            (TypeSig::ByRef(inner), Ty::Prim(VarType::Pointer))
                if matches!(unmodified(inner), TypeSig::Class(_) | TypeSig::GenericInst { .. })
                    && self.is_safe_handle(&map(self.types.resolve(module, unmodified(inner), ctx))?)? =>
            {
                Pass::SafeHandleOut(map(self.types.resolve(module, unmodified(inner), ctx))?, self.safe_handle_offset()?)
            }
            (TypeSig::ByRef(inner), Ty::Prim(VarType::Pointer)) if matches!(unmodified(inner), TypeSig::ValueType(_) | TypeSig::GenericInst { .. }) => {
                let exact = map(self.types.resolve(module, unmodified(inner), ctx))?;
                match self.gen_ty(&exact)? {
                    Ty::Struct(_) => {
                        let layout = self.native_layout(&exact)?;
                        if layout.blittable { Pass::Raw(VarType::Pointer) } else { Pass::FormattedRef(layout) }
                    }
                    _ => Pass::Raw(VarType::Pointer),
                }
            }
            (_, Ty::Prim(var)) => Pass::Raw(*var),
            (_, Ty::Struct(layout)) if self.type_name(layout.handle) == "System.Runtime.InteropServices.HandleRef" => {
                Pass::Field(self.struct_field_offset(layout, "_handle")?)
            }
            (_, Ty::Struct(layout)) if !layout.references.is_empty() => {
                let native = self.native_layout(&self.layout_gen(layout))?;
                if matches!(native.size, 1 | 2 | 4 | 8) { Pass::FormattedValue(native) } else { Pass::Formatted(native, false) }
            }
            (_, Ty::Struct(layout)) if matches!(layout.size, 1 | 2 | 4 | 8) => Pass::Value(layout.size),
            (_, Ty::Struct(layout)) => Pass::Copy(layout.size),
        })
    }

    fn builder_call(&self, builder: u64, name: &str, argument: Option<V>) -> Run<V> {
        let owner = self.object_gen(builder)?.handle().ok_or_else(|| Fault::Unsupported("builder without a type".into()))?;
        let metadata = &self.model.set.modules[owner.module].metadata;
        let key = metadata
            .method_range(owner.row)
            .map_err(|e| Fault::Unsupported(e.to_string()))?
            .map(|row| MethodKey { module: owner.module, row })
            .find(|key| {
                metadata.string_cell(TableId::MethodDef, key.row, 3).ok() == Some(name)
                    && self.model.method_sig(key).is_ok_and(|sig| {
                        sig.has_this
                            && match argument {
                                None => sig.parameters.is_empty(),
                                Some(_) => sig.parameters.len() == 1 && sig.parameters[0] == TypeSig::String,
                            }
                    })
            })
            .ok_or_else(|| Fault::Unsupported(format!("StringBuilder has no {name}")))?;
        let mut arguments = vec![V::O(builder)];
        arguments.extend(argument);
        Ok(self.invoke(&key, &arguments)?.unwrap_or(V::O(0)))
    }

    fn blittable_struct(&self, module: usize, sig: &TypeSig, ctx: &Ctx) -> Run<bool> {
        if !matches!(sig, TypeSig::ValueType(_) | TypeSig::GenericInst { value_type: true, .. }) {
            return Ok(false);
        }
        let exact = map(self.types.resolve(module, sig, ctx))?;
        match self.gen_ty(&exact)? {
            Ty::Struct(_) => Ok(self.native_layout(&exact)?.blittable),
            Ty::Prim(var) => Ok(var != VarType::Ref),
        }
    }

    fn has_layout(&self, exact: &Gen) -> bool {
        exact.handle().is_some_and(|handle| {
            self.model.set.modules[handle.module]
                .metadata
                .cell(TableId::TypeDef, handle.row, 0)
                .unwrap_or(0)
                & 0x18
                != 0
        })
    }

    fn is_safe_handle(&self, exact: &Gen) -> Run<bool> {
        let safe = Gen::Named(
            self.corelib_type("System.Runtime.InteropServices", "SafeHandle")?,
            Rc::from(Vec::new()),
        );
        Ok(self.gen_assignable(exact, &safe))
    }

    fn safe_handle_offset(&self) -> Run<u32> {
        let handle = self.corelib_type("System.Runtime.InteropServices", "SafeHandle")?;
        let (_, placements) = self
            .model
            .set
            .class_fields(handle)
            .map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
        let metadata = &self.model.set.modules[handle.module].metadata;
        placements
            .iter()
            .find(|p| metadata.string_cell(TableId::Field, p.field, 1).ok() == Some("handle"))
            .map(|p| p.offset + 8)
            .ok_or_else(|| Fault::Unsupported("SafeHandle has no handle field".into()))
    }

    fn struct_field_offset(&self, layout: &StructLayout, name: &str) -> Run<u32> {
        let metadata = &self.model.set.modules[layout.handle.module].metadata;
        layout
            .fields
            .iter()
            .find(|(row, _)| metadata.string_cell(TableId::Field, **row, 1).ok() == Some(name))
            .map(|(_, offset)| *offset)
            .ok_or_else(|| Fault::Unsupported(format!("struct field {name} is missing")))
    }

    fn platform_call(&self, index: usize, arguments: &[V]) -> Run<Option<V>> {
        let platform = self.platforms.borrow()[index].clone();
        let mut buffers: Vec<Vec<u64>> = Vec::new();
        let mut raw = Vec::with_capacity(arguments.len() + 1);
        let buffer = match &platform.back {
            Back::Buffer(layout) => {
                let boxed = self.allocate_box(&self.layout_gen(layout))?;
                raw.push(boxed + 8);
                vec![V::S(boxed)]
            }
            _ => Vec::new(),
        };
        let _pin = Pin::new(&buffer);
        let mut copy_back: Vec<(Rc<marshal::NativeLayout>, u64, u64)> = Vec::new();
        let mut builders: Vec<(u64, u64, bool)> = Vec::new();
        let mut com_outs: Vec<(u64, u64)> = Vec::new();
        let mut handle_outs: Vec<(u64, u64, Gen, u32)> = Vec::new();
        let mut string_outs: Vec<(u64, u64, u8)> = Vec::new();
        let mut bool_outs: Vec<(u64, u64, u8)> = Vec::new();
        for (pass, value) in platform.passes.iter().zip(arguments) {
            let data = match value {
                V::S(boxed) | V::O(boxed) if *boxed != 0 => boxed + 8,
                other => other.address(),
            };
            raw.push(match pass {
                Pass::Raw(var) => store_raw(*var, *value),
                Pass::Bool => u64::from(value.int() != 0),
                Pass::Array => match value.address() {
                    0 => 0,
                    array => array + u64::from(ARRAY_DATA),
                },
                Pass::Wide | Pass::Ansi | Pass::Utf8 => match value.address() {
                    0 => 0,
                    string => {
                        let units = string_units(string);
                        let bytes: Vec<u8> = match pass {
                            Pass::Wide => units.iter().chain(&[0]).flat_map(|u| u.to_le_bytes()).collect(),
                            Pass::Utf8 => String::from_utf16_lossy(units).bytes().chain([0]).collect(),
                            _ => pinvoke::ansi(units),
                        };
                        buffers.push(words(&bytes));
                        buffers[buffers.len() - 1].as_ptr() as u64
                    }
                },
                Pass::Value(size) => {
                    let mut bits = 0u64;
                    unsafe {
                        std::ptr::copy_nonoverlapping(data as *const u8, (&raw mut bits).cast(), *size as usize);
                    }
                    bits
                }
                Pass::Copy(size) => {
                    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, *size as usize) };
                    buffers.push(words(bytes));
                    buffers[buffers.len() - 1].as_ptr() as u64
                }
                Pass::Field(offset) => unsafe { *((data + u64::from(*offset)) as *const u64) },
                Pass::SafeHandle(offset) => match value.address() {
                    0 => return self.throw("ArgumentNullException"),
                    object => unsafe { *((object + u64::from(*offset)) as *const u64) },
                },
                Pass::Delegate => match value.address() {
                    0 => 0,
                    object => self.delegate_stub(object)?,
                },
                Pass::Formatted(layout, outward) => match value {
                    V::O(0) | V::S(0) => 0,
                    V::O(object) => {
                        buffers.push(vec![0u64; (layout.size as usize).div_ceil(8)]);
                        let native = buffers[buffers.len() - 1].as_ptr() as u64;
                        self.marshal_out(layout, *object, native, &mut buffers)?;
                        if *outward {
                            copy_back.push((layout.clone(), native, *object));
                        }
                        native
                    }
                    other => {
                        buffers.push(vec![0u64; (layout.size as usize).div_ceil(8)]);
                        let native = buffers[buffers.len() - 1].as_ptr() as u64;
                        self.marshal_out(layout, data, native, &mut buffers)?;
                        let _ = other;
                        native
                    }
                },
                Pass::FormattedRef(layout) => match value.address() {
                    0 => 0,
                    managed => {
                        buffers.push(vec![0u64; (layout.size as usize).div_ceil(8)]);
                        let native = buffers[buffers.len() - 1].as_ptr() as u64;
                        self.marshal_out(layout, managed, native, &mut buffers)?;
                        copy_back.push((layout.clone(), native, managed));
                        native
                    }
                },
                Pass::Com(interface) => match value.address() {
                    0 => 0,
                    object => match self.rcw_interface(object, interface)? {
                        Some(pointer) => pointer,
                        None => self.com_wrapper(object, interface)?,
                    },
                },
                Pass::Unknown => match value.address() {
                    0 => 0,
                    object => match self.rcw_pointer(object) {
                        Some(pointer) => pointer,
                        None => {
                            let exact = self.object_gen(object)?;
                            let interface = self
                                .types
                                .all_interfaces(&exact)
                                .into_iter()
                                .find(|candidate| self.is_com_interface(candidate))
                                .ok_or_else(|| Fault::Unsupported("passing a managed object without a COM interface as IUnknown".into()))?;
                            self.com_wrapper(object, &interface)?
                        }
                    },
                },
                Pass::BoolOut(native) => {
                    let target = value.address();
                    let current = target != 0 && unsafe { *(target as *const u8) } != 0;
                    let initial = match (native, current) {
                        (0x04, true) => 0xFFFF,
                        (_, true) => 1,
                        (_, false) => 0,
                    };
                    buffers.push(vec![initial; 1]);
                    let slot = buffers[buffers.len() - 1].as_ptr() as u64;
                    bool_outs.push((slot, target, *native));
                    slot
                }
                Pass::StringOut(kind) => {
                    let target = value.address();
                    let current = if target == 0 { 0 } else { unsafe { *(target as *const u64) } };
                    let initial = if current == 0 {
                        0
                    } else {
                        let units = string_units(current);
                        let bytes: Vec<u8> = match kind {
                            0 => units.iter().chain(&[0]).flat_map(|u| u.to_le_bytes()).collect(),
                            2 => String::from_utf16_lossy(units).bytes().chain([0]).collect(),
                            _ => pinvoke::ansi(units),
                        };
                        pinvoke::task_alloc(&bytes)
                    };
                    buffers.push(vec![initial; 1]);
                    let slot = buffers[buffers.len() - 1].as_ptr() as u64;
                    string_outs.push((slot, target, *kind));
                    slot
                }
                Pass::SafeHandleOut(exact, offset) => {
                    let target = value.address();
                    let current = match target {
                        0 => 0,
                        _ => match unsafe { *(target as *const u64) } {
                            0 => 0,
                            object => unsafe { *((object + u64::from(*offset)) as *const u64) },
                        },
                    };
                    buffers.push(vec![current; 1]);
                    let slot = buffers[buffers.len() - 1].as_ptr() as u64;
                    handle_outs.push((slot, target, exact.clone(), *offset));
                    slot
                }
                Pass::ComOut => {
                    buffers.push(vec![0u64; 1]);
                    let slot = buffers[buffers.len() - 1].as_ptr() as u64;
                    com_outs.push((slot, value.address()));
                    slot
                }
                Pass::Builder(unicode) => match value.address() {
                    0 => 0,
                    builder => {
                        let capacity = self.builder_call(builder, "get_Capacity", None)?.int().max(0) as usize;
                        let text = self.builder_call(builder, "ToString", None)?.address();
                        let units: Vec<u16> = if text == 0 { Vec::new() } else { string_units(text).to_vec() };
                        let mut bytes: Vec<u8> = if *unicode {
                            units.iter().flat_map(|u| u.to_le_bytes()).collect()
                        } else {
                            pinvoke::ansi(&units)
                        };
                        let unit = if *unicode { 2 } else { 1 };
                        bytes.resize((capacity + 2) * unit, 0);
                        buffers.push(words(&bytes));
                        let native = buffers[buffers.len() - 1].as_ptr() as u64;
                        builders.push((builder, native, *unicode));
                        native
                    }
                },
                Pass::AsAny(unicode) => match value.address() {
                    0 => 0,
                    object => {
                        let exact = self.object_gen(object)?;
                        let is_string = exact.handle().is_some_and(|h| self.type_name(h) == "System.String");
                        if is_string {
                            let units = string_units(object);
                            let bytes: Vec<u8> = if *unicode {
                                units.iter().chain(&[0]).flat_map(|u| u.to_le_bytes()).collect()
                            } else {
                                pinvoke::ansi(units)
                            };
                            buffers.push(words(&bytes));
                            buffers[buffers.len() - 1].as_ptr() as u64
                        } else if matches!(exact, Gen::SzArray(_)) {
                            object + u64::from(ARRAY_DATA)
                        } else {
                            let layout = self.native_layout(&exact)?;
                            buffers.push(vec![0u64; (layout.size as usize).div_ceil(8)]);
                            let native = buffers[buffers.len() - 1].as_ptr() as u64;
                            let managed = if self.types.is_value_type(&exact) { object + 8 } else { object };
                            self.marshal_out(&layout, managed, native, &mut buffers)?;
                            copy_back.push((layout, native, managed));
                            native
                        }
                    }
                },
                Pass::FormattedValue(layout) => {
                    let mut scratch = vec![0u64; 1];
                    let native = scratch.as_mut_ptr() as u64;
                    self.marshal_out(layout, data, native, &mut buffers)?;
                    scratch[0]
                }
            });
        }
        let returned = if platform.hresult && !matches!(platform.back, Back::Nothing) {
            buffers.push(vec![0u64; 4]);
            let slot = buffers[buffers.len() - 1].as_ptr() as u64;
            raw.push(slot);
            Some(slot)
        } else {
            None
        };
        let (mut integer, mut float) = unsafe { pinvoke::call(platform.function, &raw, platform.last_error) };
        if platform.hresult {
            if (integer as i32) < 0 {
                return self.throw_hresult(integer as i32);
            }
            if let Some(slot) = returned {
                integer = unsafe { *(slot as *const u64) };
                float = integer;
            }
        }
        for (layout, native, managed) in copy_back {
            self.marshal_in(&layout, native, managed)?;
        }
        for (slot, target, native) in bool_outs {
            let raw = unsafe { *(slot as *const u64) };
            let truth = match native {
                0x03 => raw & 0xFF != 0,
                0x04 => raw & 0xFFFF != 0,
                _ => raw & 0xFFFF_FFFF != 0,
            };
            if target != 0 {
                unsafe { *(target as *mut u8) = u8::from(truth) };
            }
        }
        for (slot, target, kind) in string_outs {
            let native = unsafe { *(slot as *const u64) };
            let object = if native == 0 {
                0
            } else {
                let units = match kind {
                    0 => reflection::wide_units(native),
                    2 => {
                        let text = unsafe { std::ffi::CStr::from_ptr(native as *const std::ffi::c_char) }.to_string_lossy().into_owned();
                        text.encode_utf16().collect()
                    }
                    _ => reflection::ansi_units(native),
                };
                pinvoke::task_free(native);
                runtime::new_string(&units)
            };
            if target != 0 {
                unsafe { *(target as *mut u64) = object };
            }
        }
        for (slot, target, exact, offset) in handle_outs {
            let object = self.construct_default(&exact)?;
            unsafe { *((object + u64::from(offset)) as *mut u64) = *(slot as *const u64) };
            if target != 0 {
                unsafe { *(target as *mut u64) = object };
            }
        }
        for (slot, target) in com_outs {
            let object = self.rcw_for(unsafe { *(slot as *const u64) })?;
            if target != 0 {
                unsafe { *(target as *mut u64) = object };
            }
        }
        for (builder, native, unicode) in builders {
            let units = if unicode { reflection::wide_units(native) } else { reflection::ansi_units(native) };
            let text = runtime::new_string(&units);
            self.builder_call(builder, "Clear", None)?;
            self.builder_call(builder, "Append", Some(V::O(text)))?;
        }
        Ok(match &platform.back {
            Back::Nothing => None,
            Back::Bool(mask) => Some(V::I4(i32::from(integer & mask != 0))),
            Back::Raw(VarType::F32) => Some(V::F4(f32::from_bits(float as u32))),
            Back::Raw(VarType::F64) => Some(V::F8(f64::from_bits(float))),
            Back::Raw(var) => Some(load_raw(*var, integer)),
            Back::Value(layout) => {
                let boxed = self.allocate_box(&self.layout_gen(layout))?;
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (&raw const integer).cast::<u8>(),
                        (boxed + 8) as *mut u8,
                        layout.size as usize,
                    );
                }
                Some(V::S(boxed))
            }
            Back::Buffer(_) => buffer.first().copied(),
            Back::SafeHandle(exact, offset) => {
                let object = self.construct_default(exact)?;
                unsafe { *((object + u64::from(*offset)) as *mut u64) = integer };
                Some(V::O(object))
            }
        })
    }

    fn is_unmanaged_entry(&self, key: &MethodKey) -> bool {
        let rows = self
            .unmanaged_rows
            .borrow_mut()
            .entry(key.module)
            .or_insert_with(|| Rc::new(self.scan_unmanaged_entries(key.module)))
            .clone();
        rows.contains(&key.row)
    }

    fn scan_unmanaged_entries(&self, module: usize) -> HashSet<u32> {
        let set = self.model.set;
        let metadata = &set.modules[module].metadata;
        let mut rows = HashSet::new();
        for row in 1..=metadata.rows(TableId::CustomAttribute) {
            let (Ok(Some(owner)), Ok(Some(constructor))) = (
                metadata.coded(TableId::CustomAttribute, row, 0),
                metadata.coded(TableId::CustomAttribute, row, 1),
            ) else {
                continue;
            };
            if owner.table() != Some(TableId::MethodDef) {
                continue;
            }
            let name = match constructor.table() {
                Some(TableId::MemberRef) => metadata
                    .coded(TableId::MemberRef, constructor.row(), 0)
                    .ok()
                    .flatten()
                    .filter(|parent| parent.table() == Some(TableId::TypeRef))
                    .and_then(|parent| metadata.string_cell(TableId::TypeRef, parent.row(), 1).ok()),
                Some(TableId::MethodDef) => set.modules[module]
                    .method_owner(constructor.row())
                    .ok()
                    .flatten()
                    .and_then(|owner| metadata.string_cell(TableId::TypeDef, owner, 1).ok()),
                _ => None,
            };
            if name == Some("UnmanagedCallersOnlyAttribute") {
                rows.insert(owner.row());
            }
        }
        rows
    }

    fn unmanaged_stub(&self, key: &MethodKey, ctx: &Ctx) -> Run<u64> {
        if let Some(stub) = self.unmanaged_stubs.borrow().get(key) {
            return Ok(*stub);
        }
        let stub = self.callback_stub(key, ctx, None)?;
        self.unmanaged_stubs.borrow_mut().insert(key.clone(), stub);
        Ok(stub)
    }

    fn delegate_stub(&self, object: u64) -> Run<u64> {
        if let Some((_, function)) = self.native_delegates.borrow().get(&object) {
            return Ok(*function);
        }
        if let Some(stub) = self.delegate_stubs.borrow().get(&object) {
            return Ok(*stub);
        }
        let exact = self.object_gen(object)?;
        let handle = exact
            .handle()
            .ok_or_else(|| Fault::Unsupported("delegate without a type".into()))?;
        let metadata = &self.model.set.modules[handle.module].metadata;
        let row = metadata
            .method_range(handle.row)
            .ok()
            .and_then(|mut range| range.find(|&row| metadata.string_cell(TableId::MethodDef, row, 3).ok() == Some("Invoke")))
            .ok_or_else(|| Fault::Unsupported("delegate without Invoke".into()))?;
        let key = MethodKey { module: handle.module, row };
        type_roots().push(object);
        let stub = self.callback_stub(&key, &exact.context(), Some(object))?;
        self.delegate_stubs.borrow_mut().insert(object, stub);
        Ok(stub)
    }

    fn callback_stub(&self, key: &MethodKey, ctx: &Ctx, receiver: Option<u64>) -> Run<u64> {
        let loaded = self.load_ctx(key, ctx)?;
        let skip = usize::from(receiver.is_some());
        let mut parameters = Vec::new();
        let mut structs = Vec::new();
        for ty in &loaded.parameters[skip.min(loaded.parameters.len())..] {
            match ty {
                Ty::Prim(var) => {
                    parameters.push(*var);
                    structs.push(None);
                }
                Ty::Struct(layout) if layout.references.is_empty() => {
                    parameters.push(VarType::I64);
                    structs.push(Some(layout.clone()));
                }
                Ty::Struct(_) => return unsupported("native callback with struct arguments that hold references"),
            }
        }
        let (returns, struct_return) = match &loaded.returns {
            None => (None, None),
            Some(Ty::Prim(var)) => (Some(*var), None),
            Some(Ty::Struct(layout)) if layout.references.is_empty() => (Some(VarType::I64), Some(layout.clone())),
            Some(Ty::Struct(_)) => return unsupported("native callback returning a struct that holds references"),
        };
        let slot = {
            let mut callbacks = self.callbacks.borrow_mut();
            callbacks.push(Callback {
                key: key.clone(),
                ctx: ctx.clone(),
                receiver,
                parameters,
                structs,
                returns,
                struct_return,
            });
            callbacks.len() - 1
        };
        pinvoke::make_stub(callback_handler, self as *const Self as usize, slot)
            .map(|stub| stub as u64)
            .ok_or_else(|| Fault::Unsupported("no executable memory for a native callback".into()))
    }

    fn reverse(&self, slot: usize, integers: *const u64, floats: *const u64) -> u64 {
        let (key, ctx, receiver, parameters, structs, returns, struct_return) = {
            let callbacks = self.callbacks.borrow();
            let callback = &callbacks[slot];
            (
                callback.key.clone(),
                callback.ctx.clone(),
                callback.receiver,
                callback.parameters.clone(),
                callback.structs.clone(),
                callback.returns,
                callback.struct_return.clone(),
            )
        };
        let hidden = struct_return.as_ref().filter(|layout| !matches!(layout.size, 1 | 2 | 4 | 8)).map(|_| unsafe { *integers });
        let shift = usize::from(hidden.is_some());
        let mut pins = Vec::new();
        let mut arguments = Vec::with_capacity(parameters.len() + 1);
        if let Some(receiver) = receiver {
            arguments.push(V::O(receiver));
        }
        for (index, var) in parameters.iter().enumerate() {
            let position = index + shift;
            let raw = unsafe {
                if position < 4 && matches!(var, VarType::F32 | VarType::F64) {
                    *floats.add(position)
                } else {
                    *integers.add(position)
                }
            };
            match &structs[index] {
                Some(layout) => {
                    let boxed = match self.allocate_box(&self.layout_gen(layout)) {
                        Ok(boxed) => boxed,
                        Err(_) => {
                            eprintln!("native callback could not box a struct argument");
                            std::process::abort();
                        }
                    };
                    let size = layout.size as usize;
                    let source = if matches!(size, 1 | 2 | 4 | 8) { (&raw const raw).cast::<u8>() } else { raw as *const u8 };
                    unsafe { std::ptr::copy_nonoverlapping(source, (boxed + 8) as *mut u8, size) };
                    pins.push(Pin::new(&[V::S(boxed)]));
                    arguments.push(V::S(boxed));
                }
                None => arguments.push(load_raw(*var, raw)),
            }
        }
        let _pins = pins;
        match self.invoke_ctx(&key, &ctx, &arguments) {
            Ok(Some(value)) if struct_return.is_some() => {
                let layout = struct_return.as_ref().map_or(0, |layout| layout.size as usize);
                let source = match value {
                    V::S(boxed) | V::O(boxed) if boxed != 0 => boxed + 8,
                    _ => 0,
                };
                match hidden {
                    Some(buffer) => {
                        if source != 0 {
                            unsafe { std::ptr::copy_nonoverlapping(source as *const u8, buffer as *mut u8, layout) };
                        }
                        buffer
                    }
                    None => {
                        let mut bits = 0u64;
                        if source != 0 {
                            unsafe { std::ptr::copy_nonoverlapping(source as *const u8, (&raw mut bits).cast::<u8>(), layout) };
                        }
                        bits
                    }
                }
            }
            Ok(Some(value)) => returns.map_or(0, |var| store_raw(var, value)),
            Ok(None) => 0,
            Err(Fault::Exception(object)) => {
                eprintln!("unhandled {} in a native callback", self.exception_type(object));
                std::process::abort();
            }
            Err(Fault::Unsupported(reason)) => {
                eprintln!("native callback could not run: {reason}");
                std::process::abort();
            }
        }
    }

    fn forwards_resource_lookup(&self, owner: TypeHandle) -> bool {
        let set = self.model.set;
        let metadata = &set.modules[owner.module].metadata;
        let Ok(range) = metadata.method_range(owner.row) else {
            return false;
        };
        let single: Vec<u32> = range
            .clone()
            .filter(|&row| {
                metadata.string_cell(TableId::MethodDef, row, 3).ok() == Some("GetResourceString")
                    && self
                        .model
                        .method_sig(&MethodKey { module: owner.module, row })
                        .is_ok_and(|sig| sig.parameters.len() == 1)
            })
            .collect();
        for row in range {
            if metadata.string_cell(TableId::MethodDef, row, 3).ok() != Some("GetResourceString") {
                continue;
            }
            let key = MethodKey { module: owner.module, row };
            if self.model.method_sig(&key).is_ok_and(|sig| sig.parameters.len() != 2) {
                continue;
            }
            let rva = metadata.cell(TableId::MethodDef, row, 0).unwrap_or(0);
            let Ok(body) = set.modules[owner.module]
                .image
                .tail_at_rva(rva)
                .and_then(|tail| MethodBody::parse(tail, rva))
            else {
                return false;
            };
            return body.instructions().flatten().any(|instruction| {
                instruction.opcode.value == 0x28
                    && matches!(instruction.argument, Argument::Token(token)
                        if self.method_token(owner.module, token).is_ok_and(|target| target.module == owner.module && single.contains(&target.row)))
            });
        }
        false
    }

    fn generic_native(&self, owner: TypeHandle, name: &str, ctx: &Ctx) -> Option<Native> {
        let type_name = self.type_name(owner);
        let intrinsic = match (type_name.as_str(), name) {
            ("System.Runtime.CompilerServices.RuntimeHelpers", "IsReferenceOrContainsReferences")
                if ctx.methods.len() == 1 =>
            {
                Intrinsic::ContainsReferences
            }
            ("System.Runtime.CompilerServices.RuntimeHelpers", "IsBitwiseEquatable")
                if ctx.methods.len() == 1 =>
            {
                Intrinsic::BitwiseEquatable
            }
            ("System.Collections.Generic.EqualityComparer`1", "get_Default") => {
                Intrinsic::DefaultEqualityComparer
            }
            ("System.Collections.Generic.Comparer`1", "get_Default") => Intrinsic::DefaultComparer,
            ("System.Activator", "CreateInstance") if ctx.methods.len() == 1 => {
                Intrinsic::CreateInstance
            }
            ("System.Object", "GetType") => Intrinsic::GetType,
            ("System.Buffer", "Memmove") if ctx.methods.len() == 1 => Intrinsic::MemmoveElements,
            ("System.Runtime.InteropServices.MemoryMarshal", "GetArrayDataReference") => Intrinsic::ArrayData,
            ("System.Runtime.CompilerServices.RuntimeHelpers", "GetMultiDimensionalArrayRank") => Intrinsic::ArrayRank,
            ("System.Runtime.CompilerServices.RuntimeHelpers", "ObjectHasComponentSize") => Intrinsic::HasComponentSize,
            ("System.Runtime.CompilerServices.Unsafe", _) => Intrinsic::Unsafe(self.unsafe_operation(name)?),
            ("System.Runtime.CompilerServices.RuntimeHelpers", "CreateSpan") if ctx.methods.len() == 1 => {
                Intrinsic::CreateSpan
            }
            ("System.Type" | "System.RuntimeType", "get_IsValueType" | "IsValueTypeImpl") => {
                Intrinsic::TypeIsValueType
            }
            ("System.Type" | "System.RuntimeType", "get_IsEnum") => Intrinsic::TypeIsEnum,
            _ => return None,
        };
        Some(Native::Intrinsic(intrinsic))
    }

    fn unsafe_operation(&self, name: &str) -> Option<UnsafeOp> {
        Some(match name {
            "As" | "AsRef" | "AsPointer" => UnsafeOp::Same,
            "Add" => UnsafeOp::Add,
            "AddByteOffset" => UnsafeOp::AddBytes,
            "Subtract" => UnsafeOp::Subtract,
            "SubtractByteOffset" => UnsafeOp::SubtractBytes,
            "SizeOf" => UnsafeOp::SizeOf,
            "Read" | "ReadUnaligned" => UnsafeOp::Read,
            "Write" | "WriteUnaligned" => UnsafeOp::Write,
            "Copy" => UnsafeOp::Copy,
            "CopyBlock" | "CopyBlockUnaligned" => UnsafeOp::CopyBlock,
            "InitBlock" | "InitBlockUnaligned" => UnsafeOp::InitBlock,
            "AreSame" => UnsafeOp::AreSame,
            "IsAddressGreaterThan" => UnsafeOp::Greater,
            "IsAddressLessThan" => UnsafeOp::Less,
            "ByteOffset" => UnsafeOp::ByteOffset,
            "IsNullRef" => UnsafeOp::IsNull,
            "NullRef" => UnsafeOp::Null,
            "SkipInit" => UnsafeOp::Nothing,
            "Unbox" => UnsafeOp::Unbox,
            "BitCast" => UnsafeOp::BitCast,
            _ => return None,
        })
    }

    fn unsafe_element(&self, ctx: &Ctx, index: usize) -> Run<(Ty, u32)> {
        let exact = ctx
            .methods
            .get(index)
            .ok_or_else(|| Fault::Unsupported("unsafe operation without a type".into()))?;
        let ty = self.gen_ty(exact)?;
        let size = ty.size();
        Ok((ty, size))
    }

    fn run_unsafe(&self, operation: UnsafeOp, a: &[V], ctx: &Ctx) -> Run<Option<V>> {
        let base = |value: V| value.address() as i64;
        Ok(Some(match operation {
            UnsafeOp::Same => a[0],
            UnsafeOp::Add => {
                let size = i64::from(self.unsafe_element(ctx, 0)?.1);
                V::P(base(a[0]).wrapping_add(a[1].int().wrapping_mul(size)) as u64)
            }
            UnsafeOp::AddBytes => V::P(base(a[0]).wrapping_add(a[1].int()) as u64),
            UnsafeOp::Subtract => {
                let size = i64::from(self.unsafe_element(ctx, 0)?.1);
                V::P(base(a[0]).wrapping_sub(a[1].int().wrapping_mul(size)) as u64)
            }
            UnsafeOp::SubtractBytes => V::P(base(a[0]).wrapping_sub(a[1].int()) as u64),
            UnsafeOp::SizeOf => V::I4(self.unsafe_element(ctx, 0)?.1 as i32),
            UnsafeOp::Read => {
                let (ty, _) = self.unsafe_element(ctx, 0)?;
                self.read_value(a[0].address(), &ty, &[])?
            }
            UnsafeOp::Write => {
                let (ty, _) = self.unsafe_element(ctx, 0)?;
                self.write_value(a[0].address(), &ty, a[1])?;
                return Ok(None);
            }
            UnsafeOp::Copy => {
                let (_, size) = self.unsafe_element(ctx, 0)?;
                unsafe {
                    std::ptr::copy(a[1].address() as *const u8, a[0].address() as *mut u8, size as usize);
                }
                return Ok(None);
            }
            UnsafeOp::CopyBlock => {
                unsafe {
                    std::ptr::copy(
                        a[1].address() as *const u8,
                        a[0].address() as *mut u8,
                        a[2].int() as u32 as usize,
                    );
                }
                return Ok(None);
            }
            UnsafeOp::InitBlock => {
                unsafe {
                    std::ptr::write_bytes(a[0].address() as *mut u8, a[1].int() as u8, a[2].int() as u32 as usize);
                }
                return Ok(None);
            }
            UnsafeOp::AreSame => V::I4(i32::from(a[0].address() == a[1].address())),
            UnsafeOp::Greater => V::I4(i32::from(a[0].address() > a[1].address())),
            UnsafeOp::Less => V::I4(i32::from(a[0].address() < a[1].address())),
            UnsafeOp::ByteOffset => V::I8(base(a[1]).wrapping_sub(base(a[0]))),
            UnsafeOp::IsNull => V::I4(i32::from(a[0].address() == 0)),
            UnsafeOp::Null => V::P(0),
            UnsafeOp::Nothing => return Ok(None),
            UnsafeOp::Unbox => V::P(self.non_null(a[0])? + 8),
            UnsafeOp::BitCast => {
                let (from, _) = self.unsafe_element(ctx, 0)?;
                let (to, _) = self.unsafe_element(ctx, 1)?;
                match (&from, &to) {
                    (Ty::Prim(from), Ty::Prim(to)) => load_raw(*to, store_raw(*from, a[0])),
                    _ => {
                        let mut scratch = [0u64; 64];
                        let address = scratch.as_mut_ptr() as u64;
                        self.write_value(address, &from, a[0])?;
                        self.read_value(address, &to, &[a[0]])?
                    }
                }
            }
        }))
    }

    fn delegate_native(&self, owner: TypeHandle, name: &str) -> Option<Native> {
        let type_name = self.type_name(owner);
        if !self.is_delegate(owner) || type_name == "System.Delegate" || type_name == "System.MulticastDelegate" {
            return None;
        }
        match name {
            ".ctor" => Some(Native::DelegateConstruct),
            "Invoke" => Some(Native::DelegateInvoke),
            _ => None,
        }
    }

    fn math(&self, name: &str, parameters: &[VarType], float: bool) -> Option<Native> {
        let one: Option<UnaryPair> = match name {
            "Sqrt" => Some((f64::sqrt, f32::sqrt)),
            "Sin" => Some((crt::sin, crt::sinf)),
            "Cos" => Some((crt::cos, crt::cosf)),
            "Tan" => Some((crt::tan, crt::tanf)),
            "Asin" => Some((crt::asin, crt::asinf)),
            "Acos" => Some((crt::acos, crt::acosf)),
            "Atan" => Some((crt::atan, crt::atanf)),
            "Sinh" => Some((crt::sinh, crt::sinhf)),
            "Cosh" => Some((crt::cosh, crt::coshf)),
            "Tanh" => Some((crt::tanh, crt::tanhf)),
            "Asinh" => Some((crt::asinh, crt::asinhf)),
            "Acosh" => Some((crt::acosh, crt::acoshf)),
            "Atanh" => Some((crt::atanh, crt::atanhf)),
            "Exp" => Some((crt::exp, crt::expf)),
            "Log" => Some((crt::log, crt::logf)),
            "Log10" => Some((crt::log10, crt::log10f)),
            "Log2" => Some((crt::log2, crt::log2f)),
            "Cbrt" => Some((crt::cbrt, crt::cbrtf)),
            "Floor" => Some((f64::floor, f32::floor)),
            "Ceiling" => Some((f64::ceil, f32::ceil)),
            "Truncate" => Some((f64::trunc, f32::trunc)),
            "ReciprocalEstimate" => Some((reciprocal, reciprocal_single)),
            "ReciprocalSqrtEstimate" => Some((reciprocal_sqrt, reciprocal_sqrt_single)),
            _ => None,
        };
        let two: Option<BinaryPair> = match name {
            "Pow" => Some((crt::pow, crt::powf)),
            "Atan2" => Some((crt::atan2, crt::atan2f)),
            "FMod" => Some((crt::fmod, crt::fmodf)),
            _ => None,
        };
        let is_float = parameters.iter().all(|p| matches!(p, VarType::F32 | VarType::F64));
        if !is_float || parameters.is_empty() {
            return None;
        }
        match (parameters.len(), one, two) {
            (1, Some((d, f)), _) => Some(Native::Intrinsic(if float {
                Intrinsic::MathF1(f)
            } else {
                Intrinsic::Math1(d)
            })),
            (2, _, Some((d, f))) => Some(Native::Intrinsic(if float {
                Intrinsic::MathF2(f)
            } else {
                Intrinsic::Math2(d)
            })),
            _ => None,
        }
    }

    fn is_delegate(&self, handle: TypeHandle) -> bool {
        let mut current = Some(handle);
        let mut depth = 0;
        while let Some(handle) = current {
            if self.type_name(handle) == "System.Delegate" {
                return true;
            }
            depth += 1;
            if depth > 32 {
                return false;
            }
            current = self.model.set.base_type(handle).ok().flatten();
        }
        false
    }

    pub fn invoke(&self, key: &MethodKey, arguments: &[V]) -> Run<Option<V>> {
        self.invoke_ctx(key, &Ctx::default(), arguments)
    }

    fn invoke_ctx(&self, key: &MethodKey, ctx: &Ctx, arguments: &[V]) -> Run<Option<V>> {
        let _pin = Pin::new(arguments);
        let loaded = self.load_ctx(key, ctx)?;
        if arguments.len() != loaded.parameters.len() {
            return unsupported("argument count mismatch");
        }
        let receiver;
        let arguments = match arguments.first() {
            Some(V::O(boxed) | V::S(boxed))
                if *boxed != 0
                    && !loaded.is_static
                    && matches!(loaded.parameters.first(), Some(Ty::Prim(VarType::Pointer))) =>
            {
                let mut adjusted = arguments.to_vec();
                adjusted[0] = V::P(boxed + 8);
                receiver = adjusted;
                &receiver[..]
            }
            _ => arguments,
        };
        if let Some(native) = loaded.native {
            if self.trace_calls {
                let name = self.model.set.modules[key.module]
                    .metadata
                    .string_cell(TableId::MethodDef, key.row, 3)
                    .unwrap_or("?");
                eprintln!(
                    "{}native {}::{name} {arguments:?}",
                    trace_indent(runtime::roots().depth),
                    self.types.full_name(&loaded.owner_gen)
                );
            }
            let result = self.run_native(&loaded, native, arguments)?;
            return match (&loaded.returns, result) {
                (Some(Ty::Struct(layout)), Some(value @ (V::P(_) | V::I8(_) | V::I4(_)))) => {
                    let boxed = self.allocate_box(&self.layout_gen(layout))?;
                    let bits = value.bits();
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            (&raw const bits).cast::<u8>(),
                            (boxed + 8) as *mut u8,
                            (layout.size as usize).min(8),
                        )
                    };
                    Ok(Some(V::S(boxed)))
                }
                (_, result) => Ok(result),
            };
        }
        let image = self.image.get();
        if !image.is_null()
            && ctx.is_empty()
            && prims(&loaded.parameters).is_some()
            && !matches!(loaded.returns, Some(Ty::Struct(_)))
            && let Some(outcome) = self.call_compiled(unsafe { &*image }, &loaded, arguments)
        {
            return outcome;
        }
        self.interpret(&loaded, arguments)
    }

    fn call_compiled(&self, image: &Image, loaded: &Loaded, arguments: &[V]) -> Option<Run<Option<V>>> {
        let raw: Vec<u64> = arguments.iter().map(|v| v.bits()).collect();
        let result = image.call(&loaded.key, &raw)?;
        self.compiled_calls.set(self.compiled_calls.get() + 1);
        let pending = self.pending.borrow_mut().take();
        match pending {
            Some(Pending::Fault(fault)) => return Some(Err(fault)),
            Some(Pending::Throw(name)) => return Some(self.throw(name)),
            None => {}
        }
        Some(Ok(loaded.returns.as_ref().and_then(Ty::prim).map(|var| match var {
            VarType::F32 => V::F4(f32::from_bits(result[1] as u32)),
            VarType::F64 => V::F8(f64::from_bits(result[1])),
            other => load_raw(other, result[0]),
        })))
    }

    fn run_native(&self, loaded: &Loaded, native: Native, arguments: &[V]) -> Run<Option<V>> {
        LAST_NATIVE.with(|last| last.set((loaded.key.module, loaded.key.row)));
        match native {
            Native::Intrinsic(intrinsic) => self.intrinsic(intrinsic, arguments, &loaded.ctx),
            Native::PlatformInvoke(index) => self.platform_call(index, arguments),
            Native::Accessor(index) => {
                let accessor = self.accessors.borrow()[index].clone();
                self.run_accessor(&accessor, arguments)
            }
            Native::StringConstructor => unsupported("string constructors are only reached through newobj"),
            Native::DelegateConstruct => {
                let this = arguments[0].address();
                let (target, function) = self.delegate_fields()?;
                unsafe {
                    *((this + target) as *mut u64) = arguments[1].address();
                    *((this + function) as *mut u64) = arguments[2].bits();
                }
                Ok(None)
            }
            Native::DelegateInvoke => {
                let this = arguments[0].address();
                if this == 0 {
                    return self.throw("NullReferenceException");
                }
                if let (Ok(list), Ok(count)) = (
                    self.class_field_offset("System", "MulticastDelegate", "_invocationList"),
                    self.class_field_offset("System", "MulticastDelegate", "_invocationCount"),
                ) {
                    let invocations = unsafe { *((this + u64::from(list)) as *const u64) };
                    let total = unsafe { *((this + u64::from(count)) as *const i64) };
                    if invocations != 0 && total > 0 && matches!(self.object_gen(invocations), Ok(Gen::SzArray(_))) {
                        let mut last = None;
                        for index in 0..total as u64 {
                            let item = unsafe { *((invocations + u64::from(ARRAY_DATA) + 8 * index) as *const u64) };
                            let mut forwarded = arguments.to_vec();
                            forwarded[0] = V::O(item);
                            last = self.run_native(loaded, Native::DelegateInvoke, &forwarded)?;
                        }
                        return Ok(last);
                    }
                }
                if let Some((index, _)) = self.native_delegates.borrow().get(&this).copied() {
                    return self.platform_call(index, &arguments[1..]);
                }
                let (target, function) = self.delegate_fields()?;
                let (target, function) = unsafe {
                    (
                        *((this + target) as *const u64),
                        *((this + function) as *const u64),
                    )
                };
                let (key, ctx) = self
                    .decode_function(function)
                    .ok_or_else(|| Fault::Unsupported("delegate to native code".into()))?;
                let callee = self.load_ctx(&key, &ctx)?;
                let mut forwarded = Vec::with_capacity(arguments.len());
                if target != 0 || !callee.is_static {
                    if target != 0 || callee.parameters.len() == arguments.len() {
                        forwarded.push(V::O(target));
                    }
                }
                forwarded.extend_from_slice(&arguments[1..]);
                let result = self.invoke_ctx(&key, &ctx, &forwarded)?;
                Ok(result.map(|v| match &loaded.returns {
                    Some(Ty::Prim(var)) => coerce(v, *var),
                    _ => v,
                }))
            }
        }
    }

    fn delegate_fields(&self) -> Run<(u64, u64)> {
        let handle = self.corelib_type("System", "Delegate")?;
        let (_, placements) = self
            .model
            .set
            .class_fields(handle)
            .map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
        let metadata = &self.model.set.modules[handle.module].metadata;
        let find = |name: &str| {
            placements
                .iter()
                .find(|p| metadata.string_cell(TableId::Field, p.field, 1).ok() == Some(name))
                .map(|p| u64::from(p.offset) + 8)
        };
        match (find("_target"), find("_methodPtr")) {
            (Some(target), Some(function)) => Ok((target, function)),
            _ => unsupported("delegate layout"),
        }
    }

    fn intrinsic(&self, intrinsic: Intrinsic, a: &[V], ctx: &Ctx) -> Run<Option<V>> {
        Ok(Some(match intrinsic {
            Intrinsic::Unsafe(operation) => return self.run_unsafe(operation, a, ctx),
            Intrinsic::ArrayData => V::P(Self::array_data(self.non_null(a[0])?)),
            Intrinsic::TrackerVtable => {
                static TABLE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
                let table = *TABLE.get_or_init(|| {
                    let entries: Box<[u64]> = vec![
                        wrapper_query_interface as *const () as u64,
                        wrapper_reference as *const () as u64,
                        wrapper_reference as *const () as u64,
                        wrapper_reference as *const () as u64,
                        wrapper_reference as *const () as u64,
                        wrapper_reference as *const () as u64,
                        wrapper_reference as *const () as u64,
                    ]
                    .into_boxed_slice();
                    Box::leak(entries).as_ptr() as usize
                });
                V::P(table as u64)
            }
            Intrinsic::UntrackedReference => V::P(wrapper_reference as *const () as u64),
            Intrinsic::CountedHandle => {
                let slot = unsafe { *(Self::struct_data(a[0]) as *const u64) };
                let object = if slot == 0 { 0 } else { unsafe { *(slot as *const u64) } };
                V::P(runtime::state().allocate_handle(object))
            }
            Intrinsic::TaggedImpl => V::P(wrapper_not_implemented as *const () as u64),
            Intrinsic::UnknownImpl => {
                unsafe {
                    *(a[0].address() as *mut u64) = wrapper_query_interface as *const () as u64;
                    *(a[1].address() as *mut u64) = wrapper_reference as *const () as u64;
                    *(a[2].address() as *mut u64) = wrapper_reference as *const () as u64;
                }
                return Ok(None);
            }
            Intrinsic::TypeMemory => {
                let size = a.get(1).map_or(0, |v| v.int() as u32 as usize);
                let alignment = a.get(2).map_or(8, |v| (v.int() as u32 as usize).max(8));
                let block: &'static mut [u8] = Box::leak(vec![0u8; size + alignment].into_boxed_slice());
                let address = block.as_ptr() as usize;
                V::P(address.next_multiple_of(alignment) as u64)
            }
            Intrinsic::Collect => {
                runtime::state().collect(0, 0);
                return Ok(None);
            }
            Intrinsic::SetExitCode => {
                EXIT_CODE.store(a[0].int() as i32, std::sync::atomic::Ordering::SeqCst);
                return Ok(None);
            }
            Intrinsic::ExitCode => V::I4(EXIT_CODE.load(std::sync::atomic::Ordering::SeqCst)),
            Intrinsic::HasComponentSize => {
                let object = self.non_null(a[0])?;
                let record = unsafe { *(object as *const u64) } & !1;
                let flags = unsafe { *((record + 8) as *const u64) };
                V::I4(i32::from(flags & (runtime::FLAG_ARRAY | runtime::FLAG_STRING) != 0))
            }
            Intrinsic::ArrayRank => V::I4(Self::array_rank(self.non_null(a[0])?) as i32),
            Intrinsic::ThreadId => V::I4(self.current_managed_id()?),
            Intrinsic::Nothing => return Ok(None),
            Intrinsic::Reflect(operation) => return self.reflect(operation, a, ctx),
            Intrinsic::Thread(operation) => return self.thread_call(operation, a),
            Intrinsic::EnumHash => {
                let (var, bits) = self.enum_value(a[0])?;
                V::I4(match var {
                    VarType::I8 => {
                        let v = bits as i8 as i32;
                        v ^ (v << 8)
                    }
                    VarType::I16 => {
                        let v = bits as i16;
                        (v as u16 as u32 | ((v as i32 as u32) << 16)) as i32
                    }
                    VarType::I64 => (bits as i32) ^ ((bits >> 32) as i32),
                    _ => bits as i32,
                })
            }
            Intrinsic::EnumEquals => {
                let other = a[1].address();
                let this = self.boxed_address(a[0]);
                let same = other != 0
                    && this != 0
                    && self.object_gen(other).ok() == self.object_gen(this).ok()
                    && self.enum_value(a[0])? == self.enum_value(V::O(other))?;
                V::I4(i32::from(same))
            }
            Intrinsic::CurrentThread => V::O(self.current_thread()?),
            Intrinsic::AllocateRecord(on_stack) => {
                let exact = self.method_table_type(a[0].bits())?;
                let object = runtime::allocate_managed(self.record_of(&exact)?, 0);
                if object == 0 {
                    return self.throw("OutOfMemoryException");
                }
                if on_stack {
                    self.store_handle(a[1], object);
                    return Ok(None);
                }
                V::O(object)
            }
            Intrinsic::CurrentThreadOnStack => {
                let thread = self.current_thread()?;
                self.store_handle(a[0], thread);
                return Ok(None);
            }
            Intrinsic::Collections => V::I4(runtime::COLLECTIONS.load(std::sync::atomic::Ordering::Relaxed) as i32),
            Intrinsic::DefaultConstruct => {
                let exact = self
                    .object_types
                    .borrow()
                    .get(&self.non_null(a[0])?)
                    .cloned()
                    .ok_or_else(|| Fault::Unsupported("type object from outside the fallback".into()))?;
                if self.nullable_argument(&exact).is_some() {
                    return Ok(Some(V::O(0)));
                }
                match self.gen_ty(&exact)? {
                    Ty::Prim(VarType::Ref) => V::O(self.construct_default(&exact)?),
                    Ty::Prim(var) => {
                        let boxed = self.allocate_box(&exact)?;
                        write(boxed + 8, var, load_raw(var, 0));
                        V::O(boxed)
                    }
                    Ty::Struct(_) => V::O(self.allocate_box(&exact)?),
                }
            }
            Intrinsic::NewHandle => V::P(runtime::state().allocate_handle(0)),
            Intrinsic::DependentFree => { unsafe { *(a[0].bits() as *mut [u64; 2]) = [0, 0] }; return Ok(None) },
            Intrinsic::DependentClear => { unsafe { *(a[0].bits() as *mut u64) = 0 }; return Ok(None) },
            Intrinsic::DependentSetSecond => { unsafe { *((a[0].bits() + 8) as *mut u64) = a[1].address() }; return Ok(None) },
            Intrinsic::DependentBoth => { let target = unsafe { *(a[0].bits() as *const u64) }; unsafe { *(a[1].address() as *mut u64) = if target == 0 { 0 } else { *((a[0].bits() + 8) as *const u64) } }; V::O(target) },
            Intrinsic::DependentSecond => V::O(unsafe { *((a[0].bits() + 8) as *const u64) }),
            Intrinsic::DependentTarget => V::O(unsafe { *(a[0].bits() as *const u64) }),
            Intrinsic::DependentAlloc => V::P(runtime::state().allocate_pair(a[0].address(), a[1].address())),
            Intrinsic::IdentityHash => V::I4(identity_hash(a[0].address())),
            Intrinsic::HandleAlloc => V::P(runtime::state().allocate_handle(a[0].address())),
            Intrinsic::HandleFree => {
                runtime::state().free_handle(a[0].bits());
                return Ok(None);
            }
            Intrinsic::HandleGet => V::O(unsafe { *((a[0].bits() & !1) as *const u64) }),
            Intrinsic::HandleSet => {
                unsafe { *((a[0].bits() & !1) as *mut u64) = a[1].address() };
                return Ok(None);
            }
            Intrinsic::HandleExchange => {
                let slot = (a[0].bits() & !1) as *mut u64;
                let previous = unsafe { *slot };
                if previous == a[2].address() {
                    unsafe { *slot = a[1].address() };
                }
                V::O(previous)
            }
            Intrinsic::DelegatePointer => V::P(self.delegate_stub(self.non_null(a[0])?)?),
            Intrinsic::Fence => {
                std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
                return Ok(None);
            }
            Intrinsic::True => V::I4(1),
            Intrinsic::CanCastTo => {
                let types = self.object_types.borrow();
                match (types.get(&a[0].address()), types.get(&a[1].address())) {
                    (Some(source), Some(target)) => V::I4(i32::from(self.gen_assignable(source, target))),
                    _ => return unsupported("CanCastTo on a type the fallback did not create"),
                }
            }
            Intrinsic::Intern(lookup) => {
                let slot = unsafe { *(Self::struct_data(a[0]) as *const u64) };
                let string = if slot == 0 { 0 } else { unsafe { *(slot as *const u64) } };
                if string != 0 {
                    let units = string_units(string);
                    let state = runtime::state();
                    let result = if lookup {
                        if units.is_empty() { state.empty_string } else { state.interned.get(units).copied().unwrap_or(0) }
                    } else {
                        state.intern(units)
                    };
                    unsafe { *(slot as *mut u64) = result };
                }
                return Ok(None);
            }
            Intrinsic::LoadLibrary(from_path) => {
                let name = String::from_utf16_lossy(&reflection::wide_units(a[0].address()));
                let handle = if from_path {
                    pinvoke::load_path(&name)
                } else {
                    let module = self.qcall_assembly(a[1])?;
                    let path = &self.model.set.modules[module].path;
                    pinvoke::load_named(path.parent().unwrap_or(std::path::Path::new(".")), &name)
                };
                let throw_on_error = a[if from_path { 1 } else { 4 }].int() != 0;
                if handle == 0 && throw_on_error {
                    return self.throw("DllNotFoundException");
                }
                V::I8(handle as i64)
            }
            Intrinsic::LibrarySymbol => {
                let name = String::from_utf16_lossy(&reflection::wide_units(a[1].address()));
                let function = pinvoke::symbol(a[0].bits() as usize, &name);
                if function == 0 && a[2].int() != 0 {
                    return self.throw("EntryPointNotFoundException");
                }
                V::I8(function as i64)
            }
            Intrinsic::FreeLibrary => {
                pinvoke::free_library(a[0].bits() as usize);
                return Ok(None);
            }
            Intrinsic::ClearMemory => {
                unsafe { std::ptr::write_bytes(a[0].address() as *mut u8, 0, a[1].bits() as usize) };
                return Ok(None);
            }
            Intrinsic::ProcessorCount => V::I4(processor_count()),
            Intrinsic::SpinWait => {
                if crate::threads::others_waiting() {
                    crate::threads::yield_now();
                } else {
                    for _ in 0..a[0].int().max(0) {
                        std::hint::spin_loop();
                    }
                }
                return Ok(None);
            }
            Intrinsic::LastError => V::I4(pinvoke::last_error() as i32),
            Intrinsic::SystemError => V::I4(pinvoke::system_error() as i32),
            Intrinsic::SetSystemError => {
                pinvoke::set_system_error(a[0].int() as u32);
                return Ok(None);
            }
            Intrinsic::SetLastError => {
                pinvoke::set_last_error(a[0].int() as u32);
                return Ok(None);
            }
            Intrinsic::RandomBytes => {
                use std::hash::{BuildHasher, Hasher};
                let buffer = a[1].address() as *mut u8;
                let length = a[2].int().max(0) as usize;
                let state = std::collections::hash_map::RandomState::new();
                for chunk in 0..length.div_ceil(8) {
                    let mut hasher = state.build_hasher();
                    hasher.write_usize(chunk);
                    let bytes = hasher.finish().to_le_bytes();
                    for (index, byte) in bytes.iter().enumerate() {
                        let at = chunk * 8 + index;
                        if at < length {
                            unsafe { *buffer.add(at) = *byte };
                        }
                    }
                }
                V::I4(0)
            }
            Intrinsic::ArrayCopy => {
                let (source, source_index, destination, destination_index, length) = if a.len() == 3 {
                    (a[0].address(), 0, a[1].address(), 0, a[2].int())
                } else {
                    (a[0].address(), a[1].int(), a[2].address(), a[3].int(), a[4].int())
                };
                self.copy_array(source, source_index, destination, destination_index, length)?;
                return Ok(None);
            }
            Intrinsic::ArrayClear => {
                let array = a[0].address();
                if array == 0 {
                    return self.throw("ArgumentNullException");
                }
                let count = i64::from(unsafe { *((array + 8) as *const u32) });
                let (index, length) = if a.len() == 3 { (a[1].int(), a[2].int()) } else { (0, count) };
                if index < 0 || length < 0 || index + length > count {
                    return self.throw("IndexOutOfRangeException");
                }
                let component = array_component(array);
                unsafe {
                    std::ptr::write_bytes(
                        (array + u64::from(ARRAY_DATA) + index as u64 * component) as *mut u8,
                        0,
                        (length as u64 * component) as usize,
                    );
                }
                return Ok(None);
            }
            Intrinsic::BlockCopy => {
                let (source, destination) = (a[0].address(), a[2].address());
                if source == 0 || destination == 0 {
                    return self.throw("ArgumentNullException");
                }
                let bytes = |array: u64| -> i64 {
                    i64::from(unsafe { *((array + 8) as *const u32) }) * array_component(array) as i64
                };
                let (source_offset, destination_offset, count) = (a[1].int(), a[3].int(), a[4].int());
                if source_offset < 0 || destination_offset < 0 || count < 0 {
                    return self.throw("ArgumentOutOfRangeException");
                }
                if source_offset + count > bytes(source) || destination_offset + count > bytes(destination) {
                    return self.throw("ArgumentException");
                }
                unsafe {
                    std::ptr::copy(
                        (source + u64::from(ARRAY_DATA) + source_offset as u64) as *const u8,
                        (destination + u64::from(ARRAY_DATA) + destination_offset as u64) as *mut u8,
                        count as usize,
                    );
                }
                return Ok(None);
            }
            Intrinsic::MemmoveElements => {
                let size = self.gen_ty(&ctx.methods[0])?.size() as usize;
                unsafe {
                    std::ptr::copy(
                        a[1].address() as *const u8,
                        a[0].address() as *mut u8,
                        a[2].int() as usize * size,
                    );
                }
                return Ok(None);
            }
            Intrinsic::CreateSpan => {
                let address = a[0].address();
                let size = *self
                    .rva_sizes
                    .borrow()
                    .get(&address)
                    .ok_or_else(|| Fault::Unsupported("span data from an unknown field".into()))?;
                let element = self.gen_ty(&ctx.methods[0])?.size();
                let span = Gen::Named(
                    map(self.types.core_handle("System.ReadOnlySpan`1"))?,
                    Rc::from(vec![ctx.methods[0].clone()]),
                );
                let layout = self.layout_of(&span)?;
                let metadata = &self.model.set.modules[layout.handle.module].metadata;
                let boxed = self.allocate_box(&span)?;
                for (row, offset) in &layout.fields {
                    let target = boxed + 8 + u64::from(*offset);
                    match metadata.string_cell(TableId::Field, *row, 1).ok() {
                        Some("_reference") => unsafe { *(target as *mut u64) = address },
                        Some("_length") => unsafe { *(target as *mut i32) = (size / element.max(1)) as i32 },
                        _ => {}
                    }
                }
                V::S(boxed)
            }
            Intrinsic::TypeFromHandle => match a[0] {
                V::S(boxed) | V::O(boxed) if boxed != 0 => V::O(unsafe { *((boxed + 8) as *const u64) }),
                _ => V::O(0),
            },
            Intrinsic::ContainsReferences => {
                let argument = &ctx.methods[0];
                V::I4(i32::from(match self.gen_ty(argument)? {
                    Ty::Prim(VarType::Ref) => true,
                    Ty::Struct(layout) => !layout.references.is_empty(),
                    Ty::Prim(_) => false,
                }))
            }
            Intrinsic::BitwiseEquatable => {
                let argument = &ctx.methods[0];
                let name = argument.handle().map(|h| self.type_name(h)).unwrap_or_default();
                let enumeration = argument
                    .handle()
                    .is_some_and(|h| self.model.set.is_enum(h).unwrap_or(false));
                V::I4(i32::from(
                    enumeration
                        || matches!(
                            name.as_str(),
                            "System.Boolean"
                                | "System.Byte"
                                | "System.SByte"
                                | "System.Char"
                                | "System.Int16"
                                | "System.UInt16"
                                | "System.Int32"
                                | "System.UInt32"
                                | "System.Int64"
                                | "System.UInt64"
                                | "System.IntPtr"
                                | "System.UIntPtr"
                        ),
                ))
            }
            Intrinsic::DefaultEqualityComparer => V::O(self.default_comparer(&ctx.types[0], true)?),
            Intrinsic::DefaultComparer => V::O(self.default_comparer(&ctx.types[0], false)?),
            Intrinsic::CreateInstance => {
                let target = ctx.methods[0].clone();
                match self.gen_ty(&target)? {
                    Ty::Prim(VarType::Ref) => V::O(self.construct_default(&target)?),
                    Ty::Prim(var) => load_raw(var, 0),
                    Ty::Struct(_) => V::S(self.allocate_box(&target)?),
                }
            }
            Intrinsic::AnotherGenericParameter => {
                let exact = {
                    let types = self.object_types.borrow();
                    let handle = types
                        .get(&a[0].address())
                        .and_then(Gen::handle)
                        .ok_or_else(|| Fault::Unsupported("generic type object from outside the fallback".into()))?;
                    let arguments = a[1..]
                        .iter()
                        .map(|t| types.get(&t.address()).cloned())
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| Fault::Unsupported("type argument from outside the fallback".into()))?;
                    Gen::Named(handle, Rc::from(arguments))
                };
                V::O(self.construct_default(&exact)?)
            }
            Intrinsic::GetType => {
                let object = self.non_null(a[0])?;
                let exact = self.object_gen(object)?;
                V::O(self.type_object(&exact)?)
            }
            Intrinsic::TypeIsValueType | Intrinsic::TypeIsEnum => {
                let object = self.non_null(a[0])?;
                let exact = self
                    .object_types
                    .borrow()
                    .get(&object)
                    .cloned()
                    .ok_or_else(|| Fault::Unsupported("type object from outside the fallback".into()))?;
                let result = match intrinsic {
                    Intrinsic::TypeIsValueType => self.types.is_value_type(&exact),
                    _ => exact
                        .handle()
                        .is_some_and(|h| self.model.set.is_enum(h).unwrap_or(false)),
                };
                V::I4(i32::from(result))
            }
            Intrinsic::False => V::I4(0),
            Intrinsic::Math1(f) => V::F8(f(a[0].float())),
            Intrinsic::MathF1(f) => V::F4(f(a[0].float() as f32)),
            Intrinsic::Math2(f) => V::F8(f(a[0].float(), a[1].float())),
            Intrinsic::MathF2(f) => V::F4(f(a[0].float() as f32, a[1].float() as f32)),
            Intrinsic::StringLength => {
                let s = self.non_null(a[0])?;
                V::I4(unsafe { *((s + 8) as *const i32) })
            }
            Intrinsic::StringChar => {
                let s = self.non_null(a[0])?;
                if s & 7 != 0 || unsafe { *(s as *const u64) } == 0 {
                    let (module, row) = LAST_METHOD.with(Cell::get);
                    let key = MethodKey { module, row };
                    let owner = self.model.method_owner(&key).map(|o| self.type_name(o)).unwrap_or_default();
                    let method = self.model.set.modules[module].metadata.string_cell(TableId::MethodDef, row, 3).unwrap_or("");
                    return unsupported(format!(
                        "get_Chars on a freed or invalid string {:?} called from {owner}::{method}{}",
                        a[0],
                        crate::diagnostics::managed_stack(40)
                    ));
                }
                let index = a[1].int();
                let length = i64::from(unsafe { *((s + 8) as *const i32) });
                if index < 0 || index >= length {
                    return self.throw("IndexOutOfRangeException");
                }
                V::I4(i32::from(unsafe {
                    *((s + u64::from(STRING_DATA) + 2 * index as u64) as *const u16)
                }))
            }
            Intrinsic::StringAllocate => {
                let length = a[0].int();
                if length < 0 {
                    return self.throw("OverflowException");
                }
                V::O(self.allocate_string(length as usize)?)
            }
            Intrinsic::StringEquals => {
                let (x, y) = (a[0].address(), a[1].address());
                V::I4(i32::from(x == y || (x != 0 && y != 0 && string_units(x) == string_units(y))))
            }
            Intrinsic::StringConcat => {
                let mut units = Vec::new();
                for value in a {
                    if value.address() != 0 {
                        units.extend_from_slice(string_units(value.address()));
                    }
                }
                let result = self.allocate_string(units.len())?;
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        units.as_ptr(),
                        (result + u64::from(STRING_DATA)) as *mut u16,
                        units.len(),
                    );
                }
                V::O(result)
            }
            Intrinsic::ObjectClone => {
                let source = self.non_null(a[0])?;
                let record = unsafe { *(source as *const u64) } & !1;
                let (base, component, flags) = unsafe {
                    let r = record as *const u64;
                    (*r & 0xFFFF_FFFF, *r >> 32, *r.add(1))
                };
                let count = if flags & (runtime::FLAG_ARRAY | runtime::FLAG_STRING) != 0 {
                    u64::from(unsafe { *((source + 8) as *const u32) })
                } else {
                    0
                };
                let copy = runtime::allocate_managed(record, count);
                if copy == 0 {
                    return unsupported("out of memory");
                }
                let size = base + component * count;
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (source + 8) as *const u8,
                        (copy + 8) as *mut u8,
                        size as usize - 8,
                    );
                }
                V::O(copy)
            }
            Intrinsic::InitializeArray => {
                let array = self.non_null(a[0])?;
                runtime::initialize_array(array, a[1].address());
                return Ok(None);
            }
            Intrinsic::Memmove => {
                unsafe {
                    std::ptr::copy(
                        a[1].address() as *const u8,
                        a[0].address() as *mut u8,
                        a[2].int() as usize,
                    );
                }
                return Ok(None);
            }
            Intrinsic::Fused => V::F8(crt::fma(a[0].float(), a[1].float(), a[2].float())),
            Intrinsic::FusedF => V::F4(crt::fmaf(
                a[0].float() as f32,
                a[1].float() as f32,
                a[2].float() as f32,
            )),
            Intrinsic::MultiplyAdd => V::F8(a[0].float() * a[1].float() + a[2].float()),
            Intrinsic::MultiplyAddF => {
                V::F4(a[0].float() as f32 * a[1].float() as f32 + a[2].float() as f32)
            }
            Intrinsic::Exchange(var) => {
                let address = self.non_null(a[0])?;
                let old = read(address, var);
                write(address, var, a[1]);
                old
            }
            Intrinsic::CompareExchange(var) => {
                let address = self.non_null(a[0])?;
                let old = read(address, var);
                if store_raw(var, old) == store_raw(var, a[2]) {
                    write(address, var, a[1]);
                }
                old
            }
            Intrinsic::ExchangeAdd(var) => {
                let address = self.non_null(a[0])?;
                let old = read(address, var);
                let sum = match var {
                    VarType::I32 => V::I4((old.int() as i32).wrapping_add(a[1].int() as i32)),
                    _ => V::I8(old.int().wrapping_add(a[1].int())),
                };
                write(address, var, sum);
                old
            }
            Intrinsic::ResourceString(module) => {
                let key = a[0].address();
                if key == 0 {
                    return self.throw(if self.model.set.modules[module].name == "System.Private.CoreLib" {
                        "NullReferenceException"
                    } else {
                        "ArgumentNullException"
                    });
                }
                let key = String::from_utf16_lossy(string_units(key));
                let found = self.resource_string(module, &key);
                match found {
                    Some(text) => {
                        let units: Vec<u16> = text.encode_utf16().collect();
                        let string = self.allocate_string(units.len())?;
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                units.as_ptr(),
                                (string + u64::from(STRING_DATA)) as *mut u16,
                                units.len(),
                            );
                        }
                        V::O(string)
                    }
                    None if a.len() == 2 => V::O(0),
                    None if self.model.set.modules[module].name == "System.Private.CoreLib" => a[0],
                    None => V::O(0),
                }
            }
        }))
    }

    fn resource_string(&self, module: usize, key: &str) -> Option<String> {
        let mut cache = self.resources.borrow_mut();
        let table = cache.entry(module).or_insert_with(|| {
            let mut table = HashMap::new();
            let Ok(manifest) = crate::resources::manifest(&self.model.set.modules[module]) else {
                return table;
            };
            for (name, data) in manifest {
                if !name.ends_with(".resources") || name.ends_with(".g.resources") {
                    continue;
                }
                if let Ok(entries) = crate::resources::entries(data) {
                    for (entry, value) in entries {
                        if let crate::resources::Value::Text(text) = value {
                            table.entry(entry).or_insert(text);
                        }
                    }
                }
            }
            table
        });
        table.get(key).cloned()
    }

    fn copy_array(
        &self,
        source: u64,
        source_index: i64,
        destination: u64,
        destination_index: i64,
        length: i64,
    ) -> Run<()> {
        if source == 0 || destination == 0 {
            return self.throw("ArgumentNullException");
        }
        let source_length = i64::from(unsafe { *((source + 8) as *const u32) });
        let destination_length = i64::from(unsafe { *((destination + 8) as *const u32) });
        if length < 0 || source_index < 0 || destination_index < 0 {
            return self.throw("ArgumentOutOfRangeException");
        }
        if source_index + length > source_length || destination_index + length > destination_length {
            return self.throw("ArgumentException");
        }
        let source_record = unsafe { *(source as *const u64) } & !1;
        let destination_record = unsafe { *(destination as *const u64) } & !1;
        let component = array_component(source);
        let source_data = source + u64::from(ARRAY_DATA) + source_index as u64 * component;
        let destination_data = destination + u64::from(ARRAY_DATA) + destination_index as u64 * component;
        if source_record == destination_record {
            unsafe {
                std::ptr::copy(source_data as *const u8, destination_data as *mut u8, (length as u64 * component) as usize);
            }
            return Ok(());
        }
        let (Gen::SzArray(from), Gen::SzArray(to)) = (self.object_gen(source)?, self.object_gen(destination)?) else {
            return unsupported("copies between multi dimensional arrays");
        };
        if self.types.is_value_type(&from) || self.types.is_value_type(&to) {
            let compatible = self.types.is_value_type(&from)
                && self.types.is_value_type(&to)
                && matches!((self.gen_ty(&from)?, self.gen_ty(&to)?), (Ty::Prim(_), Ty::Prim(_)))
                && self.primitive_element_kind(&from) == self.primitive_element_kind(&to);
            if !compatible {
                if self.trace_calls {
                    eprintln!("array copy from {} to {} is not compatible", self.types.full_name(&from), self.types.full_name(&to));
                }
                return self.throw("ArrayTypeMismatchException");
            }
            unsafe {
                std::ptr::copy(source_data as *const u8, destination_data as *mut u8, (length as u64 * component) as usize);
            }
            return Ok(());
        }
        let items: Vec<u64> = (0..length as u64)
            .map(|i| unsafe { *((source_data + 8 * i) as *const u64) })
            .collect();
        for item in &items {
            if *item != 0 && !self.is_assignable(*item, &to) {
                return self.throw("InvalidCastException");
            }
        }
        unsafe {
            std::ptr::copy(items.as_ptr(), destination_data as *mut u64, items.len());
        }
        Ok(())
    }

    fn allocate_string(&self, length: usize) -> Run<u64> {
        let record = runtime::state().string_type;
        if record == 0 {
            return unsupported("System.String has no type record");
        }
        if length == 0 && runtime::state().empty_string != 0 {
            return Ok(runtime::state().empty_string);
        }
        let object = runtime::allocate_managed(record, length as u64);
        if object == 0 {
            return unsupported("out of memory");
        }
        Ok(object)
    }

    fn non_null(&self, value: V) -> Run<u64> {
        match value.address() {
            0 => self.throw("NullReferenceException"),
            address => Ok(address),
        }
    }

    fn ensure_initialized_gen(&self, owner: &Gen) -> Run<()> {
        let Gen::Named(handle, arguments) = owner else {
            return Ok(());
        };
        if arguments.is_empty() {
            return self.ensure_initialized(*handle);
        }
        let id = self.types.id(owner);
        match self.generic_init.borrow().get(&id) {
            Some(None) => return Ok(()),
            Some(Some(Some(reason))) => return unsupported(reason.clone()),
            Some(Some(None)) => return self.throw("TypeInitializationException"),
            None => {}
        }
        let gate = (1u64 << 63) | u64::from(id);
        match crate::threads::begin_initialization(gate) {
            crate::threads::Initialization::Recursive => return Ok(()),
            crate::threads::Initialization::Waited => return self.ensure_initialized_gen(owner),
            crate::threads::Initialization::Run => {}
        }
        let outcome = self.run_generic_initializer(*handle, owner, id);
        if outcome.is_ok() {
            self.generic_init.borrow_mut().insert(id, None);
        }
        crate::threads::end_initialization(gate);
        outcome
    }

    fn run_generic_initializer(&self, handle: TypeHandle, owner: &Gen, id: u32) -> Run<()> {
        let metadata = &self.model.set.modules[handle.module].metadata;
        let Ok(range) = metadata.method_range(handle.row) else {
            return Ok(());
        };
        for row in range {
            if metadata.string_cell(TableId::MethodDef, row, 3).ok() != Some(".cctor") {
                continue;
            }
            let key = MethodKey { module: handle.module, row };
            match self.invoke_ctx(&key, &owner.context(), &[]) {
                Ok(_) => {}
                Err(Fault::Unsupported(reason)) => {
                    self.generic_init.borrow_mut().insert(id, Some(Some(reason.clone())));
                    return unsupported(reason);
                }
                Err(Fault::Exception(_)) => {
                    self.generic_init.borrow_mut().insert(id, Some(None));
                    return self.throw("TypeInitializationException");
                }
            }
        }
        Ok(())
    }

    fn ensure_initialized(&self, owner: TypeHandle) -> Run<()> {
        if owner.row != 1 {
            self.ensure_initialized(TypeHandle { module: owner.module, row: 1 })?;
        }
        if let Some(reason) = self.failed_types.borrow().get(&owner) {
            return match reason {
                Some(reason) => unsupported(reason.clone()),
                None => self.throw("TypeInitializationException"),
            };
        }
        let flag = runtime::state().init_flag(owner) as *mut u8;
        if unsafe { *flag } != 0 {
            return Ok(());
        }
        let gate = ((owner.module as u64) << 32) | u64::from(owner.row);
        match crate::threads::begin_initialization(gate) {
            crate::threads::Initialization::Recursive => return Ok(()),
            crate::threads::Initialization::Waited => return self.ensure_initialized(owner),
            crate::threads::Initialization::Run => {}
        }
        let outcome = self.run_type_initializer(owner);
        if outcome.is_ok() {
            unsafe { *flag = 1 };
        }
        crate::threads::end_initialization(gate);
        outcome
    }

    fn run_type_initializer(&self, owner: TypeHandle) -> Run<()> {
        let metadata = &self.model.set.modules[owner.module].metadata;
        let Ok(range) = metadata.method_range(owner.row) else {
            return Ok(());
        };
        for row in range {
            if metadata.string_cell(TableId::MethodDef, row, 3).ok() != Some(".cctor") {
                continue;
            }
            match self.invoke(&MethodKey { module: owner.module, row }, &[]) {
                Ok(_) => {}
                Err(Fault::Unsupported(reason)) => {
                    self.failed_types.borrow_mut().insert(owner, Some(reason.clone()));
                    return unsupported(reason);
                }
                Err(Fault::Exception(object)) => {
                    if self.trace_calls {
                        eprintln!(
                            "static constructor of {} threw {}: {}",
                            self.type_name(owner),
                            self.exception_type(object),
                            self.exception_message(object).unwrap_or_default()
                        );
                    }
                    self.failed_types.borrow_mut().insert(owner, None);
                    return self.throw("TypeInitializationException");
                }
            }
        }
        Ok(())
    }

    fn interpret(&self, loaded: &Loaded, arguments: &[V]) -> Run<Option<V>> {
        if loaded.is_abstract {
            return unsupported("abstract method");
        }
        let depth = runtime::roots().depth;
        if depth >= MAX_DEPTH {
            return unsupported("interpreter call depth");
        }
        runtime::roots().depth = depth + 1;
        LAST_METHOD.with(|last| last.set((loaded.key.module, loaded.key.row)));
        self.interpreted.set(self.interpreted.get() + 1);
        if self.trace_calls {
            let name = self.model.set.modules[loaded.key.module]
                .metadata
                .string_cell(TableId::MethodDef, loaded.key.row, 3)
                .unwrap_or("?");
            eprintln!(
                "{}{}::{name}",
                trace_indent(depth),
                self.types.full_name(&loaded.owner_gen)
            );
        }
        let mut kinds = loaded.parameters.clone();
        kinds.extend_from_slice(&loaded.locals);
        let mut offsets = Vec::with_capacity(kinds.len());
        let mut words = 0usize;
        for kind in &kinds {
            offsets.push(words);
            words += kind.words();
        }
        let mut frame = Box::new(Frame {
            memory: vec![0u64; words.max(1)].into_boxed_slice(),
            offsets,
            kinds,
            stack: Vec::new(),
            caught: Vec::new(),
            scratch: Vec::new(),
        });
        for (index, value) in arguments.iter().enumerate() {
            let (address, kind) = frame.slot(index)?;
            match kind {
                Ty::Prim(var) => frame.memory[frame.offsets[index]] = store_raw(var, *value),
                kind => self.write_value(address, &kind, *value)?,
            }
        }
        let registered = Registered::new(&frame);
        runtime::roots().modules.push(loaded.key.module);
        runtime::roots().calls.push((loaded.key.module, loaded.key.row));
        let result = self.execute(loaded, &mut frame, 0);
        runtime::roots().modules.pop();
        runtime::roots().calls.pop();
        drop(registered);
        runtime::roots().depth = depth;
        match result? {
            Exit::Return(value) => Ok(value),
            _ => unsupported("method ended outside a return"),
        }
    }

    fn region_end(&self, method: &Loaded, start: u32) -> Option<u32> {
        if start == 0 {
            return None;
        }
        method.clauses.iter().find_map(|c| {
            if c.handler_offset == start {
                Some(c.handler_offset + c.handler_length)
            } else if c.kind == ClauseKind::Filter && c.class_or_filter == start {
                Some(c.handler_offset)
            } else {
                None
            }
        })
    }

    fn execute(&self, method: &Loaded, frame: &mut Frame, start: u32) -> Run<Exit> {
        let end = self.region_end(method, start);
        let inside = |target: u32| match end {
            Some(end) => target >= start && target < end,
            None => true,
        };
        let mut pc = start as usize;
        let mut current = pc;
        loop {
            let exit = match self.step_block(method, frame, &mut pc, &mut current) {
                Ok(exit) => exit,
                Err(Fault::Exception(object)) => {
                    let Some(handler) = self.dispatch(method, frame, current as u32, object)? else {
                        return Err(Fault::Exception(object));
                    };
                    frame.stack.clear();
                    frame.stack.push(V::O(object));
                    frame.caught.push(object);
                    let outcome = self.execute(method, frame, handler);
                    frame.caught.pop();
                    outcome?
                }
                Err(other) => return Err(other),
            };
            match exit {
                Exit::Leave(from, target) => {
                    if !inside(target) {
                        return Ok(Exit::Leave(from, target));
                    }
                    self.run_finallies(method, frame, from, target)?;
                    frame.stack.clear();
                    pc = target as usize;
                }
                other => return Ok(other),
            }
        }
    }

    fn run_finallies(&self, method: &Loaded, frame: &mut Frame, from: u32, target: u32) -> Run<()> {
        let mut clauses: Vec<&ExceptionClause> = method
            .clauses
            .iter()
            .filter(|c| c.kind == ClauseKind::Finally)
            .filter(|c| contains_try(c, from) && !contains_try(c, target))
            .collect();
        clauses.sort_by_key(|c| c.try_length);
        for clause in clauses {
            self.run_handler_region(method, frame, clause.handler_offset)?;
        }
        Ok(())
    }

    fn run_handler_region(&self, method: &Loaded, frame: &mut Frame, offset: u32) -> Run<()> {
        let saved = std::mem::take(&mut frame.stack);
        let result = self.execute(method, frame, offset);
        frame.stack = saved;
        match result? {
            Exit::EndFinally => Ok(()),
            Exit::Return(_) => unsupported("return inside a finally block"),
            _ => unsupported("finally block ended unexpectedly"),
        }
    }

    fn dispatch(&self, method: &Loaded, frame: &mut Frame, at: u32, object: u64) -> Run<Option<u32>> {
        let mut clauses: Vec<&ExceptionClause> = method
            .clauses
            .iter()
            .filter(|c| contains_try(c, at))
            .collect();
        clauses.sort_by_key(|c| c.try_length);
        let mut passed: Vec<u32> = Vec::new();
        for clause in clauses {
            match clause.kind {
                ClauseKind::Catch => {
                    if self.catches(method, clause.class_or_filter, object)? {
                        self.unwind(method, frame, &passed, object)?;
                        return Ok(Some(clause.handler_offset));
                    }
                }
                ClauseKind::Filter => {
                    let saved = std::mem::take(&mut frame.stack);
                    frame.stack.push(V::O(object));
                    frame.caught.push(object);
                    let outcome = self.execute(method, frame, clause.class_or_filter);
                    frame.caught.pop();
                    frame.stack = saved;
                    let accepted = matches!(outcome, Ok(Exit::EndFilter(true)));
                    if accepted {
                        self.unwind(method, frame, &passed, object)?;
                        return Ok(Some(clause.handler_offset));
                    }
                }
                ClauseKind::Finally | ClauseKind::Fault => passed.push(clause.handler_offset),
            }
        }
        self.unwind(method, frame, &passed, object)?;
        Ok(None)
    }

    fn unwind(&self, method: &Loaded, frame: &mut Frame, handlers: &[u32], object: u64) -> Run<()> {
        frame.caught.push(object);
        let result = handlers
            .iter()
            .try_for_each(|&handler| self.run_handler_region(method, frame, handler));
        frame.caught.pop();
        result
    }

    fn catches(&self, method: &Loaded, token: u32, object: u64) -> Run<bool> {
        let target = map(self.types.resolve_token(method.key.module, Token(token), &method.ctx))?;
        Ok(self.is_assignable(object, &target))
    }

    fn is_assignable(&self, object: u64, target: &Gen) -> bool {
        if self.rcw_pointer(object).is_some() && self.is_com_interface(target) {
            return self.rcw_supports(object, target);
        }
        let Ok(exact) = self.object_gen(object) else {
            return false;
        };
        self.gen_assignable(&exact, target)
    }

    fn gen_assignable(&self, exact: &Gen, target: &Gen) -> bool {
        if exact == target {
            return true;
        }
        let key = (exact.clone(), target.clone());
        if let Some(found) = self.assignable.borrow().get(&key) {
            return *found;
        }
        let result = self.gen_assignable_uncached(exact, target);
        self.assignable.borrow_mut().insert(key, result);
        result
    }

    fn gen_assignable_uncached(&self, exact: &Gen, target: &Gen) -> bool {
        if let Some(handle) = target.handle()
            && self.type_name(handle) == "System.Object"
        {
            return true;
        }
        if let (Gen::SzArray(element), Gen::SzArray(wanted)) = (exact, target) {
            return element == wanted
                || (!self.types.is_value_type(element)
                    && !self.types.is_value_type(wanted)
                    && self.gen_assignable(element, wanted));
        }
        if let Gen::SzArray(element) = exact
            && let Gen::Named(handle, arguments) = target
        {
            let name = self.type_name(*handle);
            if arguments.is_empty() {
                return matches!(
                    name.as_str(),
                    "System.Array"
                        | "System.ICloneable"
                        | "System.Collections.IList"
                        | "System.Collections.ICollection"
                        | "System.Collections.IEnumerable"
                        | "System.Collections.IStructuralComparable"
                        | "System.Collections.IStructuralEquatable"
                );
            }
            return arguments.len() == 1
                && name.starts_with("System.Collections.Generic.I")
                && (&arguments[0] == element.as_ref()
                    || (!self.types.is_value_type(element) && self.gen_assignable(element, &arguments[0])));
        }
        self.types.ancestry(exact).iter().any(|g| g == target)
            || self.types.all_interfaces(exact).iter().any(|g| g == target)
    }





    fn method_token(&self, module: usize, token: Token) -> Run<MethodKey> {
        match token.table() {
            Some(TableId::MethodDef) => Ok(MethodKey {
                module,
                row: token.row(),
            }),
            Some(TableId::MemberRef) => match self.model.set.resolve_member_ref(module, token.row()) {
                Ok(Member::Method(handle)) if handle.token.table() == Some(TableId::MethodDef) => {
                    Ok(MethodKey {
                        module: handle.module,
                        row: handle.token.row(),
                    })
                }
                Ok(Member::ArrayMethod) => unsupported("multi dimensional arrays"),
                Ok(_) => unsupported("call to a non method member"),
                Err(error) => unsupported(format!("call target does not resolve: {error:?}")),
            },
            Some(TableId::MethodSpec) => unsupported("generic method instantiation"),
            _ => unsupported("method token"),
        }
    }

    fn method_inst(&self, module: usize, token: Token, ctx: &Ctx) -> Run<(MethodKey, Ctx)> {
        let cache_key = (module, token.0, ctx.clone());
        if let Some(found) = self.method_insts.borrow().get(&cache_key) {
            return Ok(found.clone());
        }
        let resolved = self.method_inst_uncached(module, token, ctx)?;
        self.method_insts.borrow_mut().insert(cache_key, resolved.clone());
        Ok(resolved)
    }

    fn method_inst_uncached(&self, module: usize, token: Token, ctx: &Ctx) -> Run<(MethodKey, Ctx)> {
        let set = self.model.set;
        match token.table() {
            Some(TableId::MethodDef) => {
                let key = MethodKey { module, row: token.row() };
                let owner = map(self.model.method_owner(&key))?;
                let count = self.types.parameter_count(TableId::TypeDef, owner.row, owner.module);
                let types = if count > 0 && ctx.types.len() == count {
                    ctx.types.clone()
                } else {
                    Rc::from(Vec::new())
                };
                Ok((key, Ctx { types, methods: Rc::from(Vec::new()) }))
            }
            Some(TableId::MemberRef) => {
                let key = self.method_token(module, token)?;
                let metadata = &set.modules[module].metadata;
                let parent = metadata
                    .coded(TableId::MemberRef, token.row(), 0)
                    .map_err(|e| Fault::Unsupported(e.to_string()))?;
                let mut types: Rc<[Gen]> = Rc::from(Vec::new());
                if let Some(parent) = parent
                    && parent.table() == Some(TableId::TypeSpec)
                {
                    let parent = map(self.types.resolve_token(module, parent, ctx))?;
                    let owner = map(self.model.method_owner(&key))?;
                    types = match self.types.instance_of(&parent, owner) {
                        Some(found) => found.arguments(),
                        None => parent.arguments(),
                    };
                }
                Ok((key, Ctx { types, methods: Rc::from(Vec::new()) }))
            }
            Some(TableId::MethodSpec) => {
                let metadata = &set.modules[module].metadata;
                let method = metadata
                    .coded(TableId::MethodSpec, token.row(), 0)
                    .map_err(|e| Fault::Unsupported(e.to_string()))?
                    .ok_or_else(|| Fault::Unsupported("method instantiation has no method".into()))?;
                let (key, inner) = self.method_inst(module, method, ctx)?;
                let blob = metadata
                    .cell(TableId::MethodSpec, token.row(), 1)
                    .and_then(|b| metadata.blob(b))
                    .map_err(|e| Fault::Unsupported(e.to_string()))?;
                let Ok(Signature::MethodSpec(arguments)) = Signature::parse(blob) else {
                    return unsupported("method instantiation does not parse");
                };
                let methods = arguments
                    .iter()
                    .map(|a| self.types.resolve(module, a, ctx))
                    .collect::<Result<Vec<_>, String>>()
                    .map_err(Fault::Unsupported)?;
                Ok((key, Ctx { types: inner.types, methods: Rc::from(methods) }))
            }
            _ => unsupported("method token"),
        }
    }

    fn parameter_gens(&self, key: &MethodKey, ctx: &Ctx) -> Option<Vec<Gen>> {
        let sig = self.model.method_sig(key).ok()?;
        let mut methods = ctx.methods.to_vec();
        while methods.len() < sig.generic_parameters as usize {
            methods.push(Gen::Array(Rc::new(Gen::FunctionPointer), 1000 + methods.len() as u32));
        }
        let ctx = Ctx { types: ctx.types.clone(), methods: Rc::from(methods) };
        let mut gens = Vec::with_capacity(sig.parameters.len() + 1);
        gens.push(self.types.resolve(key.module, &sig.return_type, &ctx).ok()?);
        for parameter in &sig.parameters {
            gens.push(self.types.resolve(key.module, parameter, &ctx).ok()?);
        }
        Some(gens)
    }

    fn signatures_match(&self, a: &MethodKey, a_ctx: &Ctx, b: &MethodKey, b_ctx: &Ctx) -> bool {
        let set = self.model.set;
        let (Ok(a_sig), Ok(b_sig)) = (self.model.method_sig(a), self.model.method_sig(b)) else {
            return false;
        };
        if a_sig.parameters.len() != b_sig.parameters.len()
            || a_sig.generic_parameters != b_sig.generic_parameters
        {
            return false;
        }
        match (self.parameter_gens(a, a_ctx), self.parameter_gens(b, b_ctx)) {
            (Some(x), Some(y)) => x == y,
            _ => set.methods_equal(a.module, &a_sig, b.module, &b_sig).unwrap_or(false),
        }
    }

    fn vtable_of(&self, exact: &Gen) -> Run<Rc<VTable>> {
        let id = self.types.id(exact);
        if let Some(found) = self.vtables.borrow().get(&id) {
            return Ok(found.clone());
        }
        let Gen::Named(handle, _) = exact else {
            return unsupported("virtual table of a non class");
        };
        let mut table = match map(self.types.base(exact))? {
            Some(base) => {
                let base = self.vtable_of(&base)?;
                VTable {
                    slots: base.slots.clone(),
                    slot_of: base.slot_of.clone(),
                }
            }
            _ => VTable {
                slots: Vec::new(),
                slot_of: HashMap::new(),
            },
        };
        let metadata = &self.model.set.modules[handle.module].metadata;
        let ctx = exact.context();
        let range = metadata
            .method_range(handle.row)
            .map_err(|e| Fault::Unsupported(e.to_string()))?;
        for row in range {
            let key = MethodKey { module: handle.module, row };
            let flags = metadata.cell(TableId::MethodDef, row, 2).unwrap_or(0);
            if flags & VIRTUAL == 0 {
                continue;
            }
            let name = metadata.string_cell(TableId::MethodDef, row, 3).unwrap_or("");
            let mut assigned = None;
            if flags & 0x100 == 0 {
                for (slot, existing) in table.slots.iter().enumerate().rev() {
                    let Some((existing, owner)) = existing else { continue };
                    if self.model.set.modules[existing.module]
                        .metadata
                        .string_cell(TableId::MethodDef, existing.row, 3)
                        .ok()
                        != Some(name)
                    {
                        continue;
                    }
                    if self.signatures_match(&key, &ctx, existing, &owner.context()) {
                        assigned = Some(slot);
                        break;
                    }
                }
            }
            let slot = match assigned {
                Some(slot) => {
                    table.slots[slot] = Some((key.clone(), exact.clone()));
                    slot
                }
                None => {
                    table.slots.push(Some((key.clone(), exact.clone())));
                    table.slots.len() - 1
                }
            };
            table.slot_of.insert(key, slot);
        }
        for row in 1..=metadata.rows(TableId::MethodImpl) {
            if metadata.cell(TableId::MethodImpl, row, 0).ok() != Some(handle.row) {
                continue;
            }
            let (Ok(Some(body)), Ok(Some(declaration))) = (
                metadata.coded(TableId::MethodImpl, row, 1),
                metadata.coded(TableId::MethodImpl, row, 2),
            ) else {
                continue;
            };
            let (Ok((body, _)), Ok((declaration, _))) = (
                self.method_inst(handle.module, body, &ctx),
                self.method_inst(handle.module, declaration, &ctx),
            ) else {
                continue;
            };
            if let Some(&slot) = table.slot_of.get(&declaration) {
                table.slots[slot] = Some((body.clone(), exact.clone()));
                table.slot_of.insert(body, slot);
            }
        }
        let table = Rc::new(table);
        self.vtables.borrow_mut().insert(id, table.clone());
        Ok(table)
    }

    fn owner_context(&self, exact: &Gen, owner: TypeHandle, methods: &Rc<[Gen]>) -> Ctx {
        let types = self
            .types
            .instance_of(exact, owner)
            .map(|g| g.arguments())
            .unwrap_or_else(|| Rc::from(Vec::new()));
        Ctx { types, methods: methods.clone() }
    }

    fn resolve_virtual(&self, key: &MethodKey, ctx: &Ctx, this: u64) -> Run<(MethodKey, Ctx)> {
        let record = unsafe { *(this as *const u64) } & !1;
        let inst = self.inst_key(key, ctx);
        if let Some(found) = self.implementations.borrow().get(&(record, inst.clone())) {
            return Ok(found.clone());
        }
        let exact = self.object_gen(this)?;
        let owner = map(self.model.method_owner(key))?;
        if let Gen::SzArray(element) = &exact
            && matches!(
                self.type_name(owner).as_str(),
                "System.Collections.Generic.IList`1"
                    | "System.Collections.Generic.ICollection`1"
                    | "System.Collections.Generic.IEnumerable`1"
                    | "System.Collections.Generic.IReadOnlyList`1"
                    | "System.Collections.Generic.IReadOnlyCollection`1"
            )
            && let Some(found) = self.array_helper(key, element)?
        {
            self.implementations.borrow_mut().insert((record, inst), found.clone());
            return Ok(found);
        }
        let owner_flags = self.model.set.modules[owner.module]
            .metadata
            .cell(TableId::TypeDef, owner.row, 0)
            .unwrap_or(0);
        let class = match &exact {
            Gen::Named(_, _) => exact.clone(),
            _ => map(self.types.core("Array"))?,
        };
        let found = if owner_flags & 0x20 != 0 {
            self.find_interface_implementation(&class, key, ctx)?
        } else {
            let table = self.vtable_of(&class)?;
            let slot = *table
                .slot_of
                .get(key)
                .ok_or_else(|| {
                    let name = self.model.set.modules[key.module]
                        .metadata
                        .string_cell(TableId::MethodDef, key.row, 3)
                        .unwrap_or("?");
                    Fault::Unsupported(format!(
                        "method {name} has no virtual slot on {}",
                        self.types.full_name(&class)
                    ))
                })?;
            let (target, declaring) = table.slots[slot]
                .clone()
                .ok_or_else(|| Fault::Unsupported("virtual slot is empty".into()))?;
            let target_owner = map(self.model.method_owner(&target))?;
            let mut context = self.owner_context(&declaring, target_owner, &ctx.methods);
            if context.types.is_empty() {
                context = self.owner_context(&class, target_owner, &ctx.methods);
            }
            (target, context)
        };
        self.implementations
            .borrow_mut()
            .insert((record, inst), found.clone());
        Ok(found)
    }

    fn variant_interface(&self, class: &Gen, key: &MethodKey, ctx: &Ctx) -> Run<Option<Ctx>> {
        let owner = map(self.model.method_owner(key))?;
        let metadata = &self.model.set.modules[owner.module].metadata;
        let variance: Vec<u32> = (0..ctx.types.len() as u32)
            .map(|number| {
                (1..=metadata.rows(TableId::GenericParam))
                    .find(|&g| {
                        metadata.cell(TableId::GenericParam, g, 0).ok() == Some(number)
                            && metadata
                                .coded(TableId::GenericParam, g, 2)
                                .ok()
                                .flatten()
                                .is_some_and(|t| t.table() == Some(TableId::TypeDef) && t.row() == owner.row)
                    })
                    .and_then(|g| metadata.cell(TableId::GenericParam, g, 1).ok())
                    .unwrap_or(0)
                    & 3
            })
            .collect();
        if variance.iter().all(|v| *v == 0) {
            return Ok(None);
        }
        for interface in self.types.all_interfaces(class) {
            let Gen::Named(handle, arguments) = &interface else { continue };
            if *handle != owner || arguments.len() != ctx.types.len() || **arguments == *ctx.types {
                continue;
            }
            let compatible = arguments.iter().zip(ctx.types.iter()).zip(&variance).all(|((implemented, requested), v)| match v {
                0 => implemented == requested,
                1 => !self.types.is_value_type(implemented) && self.gen_assignable(implemented, requested),
                _ => !self.types.is_value_type(requested) && self.gen_assignable(requested, implemented),
            });
            if compatible {
                return Ok(Some(Ctx {
                    types: arguments.clone(),
                    methods: ctx.methods.clone(),
                }));
            }
        }
        Ok(None)
    }

    fn array_helper(&self, key: &MethodKey, element: &Gen) -> Run<Option<(MethodKey, Ctx)>> {
        let helper = self.corelib_type("System", "SZArrayHelper")?;
        let metadata = &self.model.set.modules[key.module].metadata;
        let name = metadata.string_cell(TableId::MethodDef, key.row, 3).unwrap_or("");
        let short = name.rsplit('.').next().unwrap_or(name);
        let parameters = map(self.model.method_sig(key))?.parameters.len();
        let helper_metadata = &self.model.set.modules[helper.module].metadata;
        let found = helper_metadata
            .method_range(helper.row)
            .map_err(|e| Fault::Unsupported(e.to_string()))?
            .map(|row| MethodKey { module: helper.module, row })
            .find(|candidate| {
                helper_metadata.string_cell(TableId::MethodDef, candidate.row, 3).ok() == Some(short)
                    && self.model.method_sig(candidate).is_ok_and(|sig| sig.parameters.len() == parameters && sig.generic_parameters == 1)
            });
        Ok(found.map(|key| {
            (
                key,
                Ctx {
                    types: Rc::from(Vec::new()),
                    methods: Rc::from(vec![element.clone()]),
                },
            )
        }))
    }

    fn find_interface_implementation(&self, class: &Gen, key: &MethodKey, ctx: &Ctx) -> Run<(MethodKey, Ctx)> {
        let set = self.model.set;
        let name = set.modules[key.module]
            .metadata
            .string_cell(TableId::MethodDef, key.row, 3)
            .unwrap_or("");
        let chain = self.types.ancestry(class);
        for exact in &chain {
            let Some(handle) = exact.handle() else { continue };
            let metadata = &set.modules[handle.module].metadata;
            let type_ctx = exact.context();
            for row in 1..=metadata.rows(TableId::MethodImpl) {
                if metadata.cell(TableId::MethodImpl, row, 0).ok() != Some(handle.row) {
                    continue;
                }
                let (Ok(Some(body)), Ok(Some(declaration))) = (
                    metadata.coded(TableId::MethodImpl, row, 1),
                    metadata.coded(TableId::MethodImpl, row, 2),
                ) else {
                    continue;
                };
                let Ok((declared, declared_ctx)) = self.method_inst(handle.module, declaration, &type_ctx) else {
                    continue;
                };
                if &declared != key || (!ctx.types.is_empty() && declared_ctx.types != ctx.types) {
                    continue;
                }
                if let Ok((body, _)) = self.method_inst(handle.module, body, &type_ctx) {
                    let owner = map(self.model.method_owner(&body))?;
                    return Ok((body, self.owner_context(exact, owner, &ctx.methods)));
                }
            }
        }
        for exact in &chain {
            let Some(handle) = exact.handle() else { continue };
            let metadata = &set.modules[handle.module].metadata;
            let Ok(range) = metadata.method_range(handle.row) else {
                continue;
            };
            let type_ctx = Ctx { types: exact.arguments(), methods: ctx.methods.clone() };
            for row in range {
                if metadata.string_cell(TableId::MethodDef, row, 3).ok() != Some(name) {
                    continue;
                }
                let flags = metadata.cell(TableId::MethodDef, row, 2).unwrap_or(0);
                if flags & VIRTUAL == 0 {
                    continue;
                }
                let candidate = MethodKey { module: handle.module, row };
                if self.signatures_match(&candidate, &type_ctx, key, ctx) {
                    return Ok((candidate, type_ctx));
                }
            }
        }
        if let Some(variant) = self.variant_interface(class, key, ctx)? {
            return self.find_interface_implementation(class, key, &variant);
        }
        let flags = set.modules[key.module]
            .metadata
            .cell(TableId::MethodDef, key.row, 2)
            .unwrap_or(0);
        if flags & ABSTRACT == 0 {
            return Ok((key.clone(), ctx.clone()));
        }
        let name = set.modules[key.module]
            .metadata
            .string_cell(TableId::MethodDef, key.row, 3)
            .unwrap_or("?");
        unsupported(format!(
            "interface method {}::{name} has no implementation on {}",
            self.type_name(map(self.model.method_owner(key))?),
            self.types.full_name(class)
        ))
    }

    fn field_ref(&self, module: usize, token: Token, ctx: &Ctx) -> Run<Rc<FieldRef>> {
        let cache_key = (
            module,
            token.0,
            ctx.types.iter().map(|g| self.types.id(g)).collect::<Vec<_>>(),
            ctx.methods.iter().map(|g| self.types.id(g)).collect::<Vec<_>>(),
        );
        if let Some(found) = self.fields.borrow().get(&cache_key) {
            return Ok(found.clone());
        }
        let set = self.model.set;
        let (field_module, row, parent) = match token.table() {
            Some(TableId::Field) => (module, token.row(), None),
            Some(TableId::MemberRef) => {
                let parent = set.modules[module]
                    .metadata
                    .coded(TableId::MemberRef, token.row(), 0)
                    .ok()
                    .flatten()
                    .filter(|p| p.table() == Some(TableId::TypeSpec));
                match set.resolve_member_ref(module, token.row()) {
                    Ok(Member::Field(handle)) => (handle.module, handle.token.row(), parent),
                    Ok(_) => return unsupported("member reference is not a field"),
                    Err(error) => return unsupported(format!("field does not resolve: {error:?}")),
                }
            }
            _ => return unsupported("token is not a field"),
        };
        let handle = set.modules[field_module]
            .field_owner(row)
            .ok()
            .flatten()
            .map(|owner| TypeHandle { module: field_module, row: owner })
            .ok_or_else(|| Fault::Unsupported("field has no owner".into()))?;
        let owner = match parent {
            Some(parent) => {
                let parent = map(self.types.resolve_token(module, parent, ctx))?;
                self.types
                    .instance_of(&parent, handle)
                    .unwrap_or(parent)
            }
            None => {
                let count = self.types.parameter_count(TableId::TypeDef, handle.row, handle.module);
                if count > 0 && ctx.types.len() == count {
                    Gen::Named(handle, ctx.types.clone())
                } else {
                    Gen::Named(handle, Rc::from(Vec::new()))
                }
            }
        };
        let metadata = &set.modules[field_module].metadata;
        let flags = metadata.cell(TableId::Field, row, 0).unwrap_or(0);
        let sig = map(self.model.field_signature(field_module, row))?;
        let ty = self.ty_of(field_module, &sig, &owner.context())?;
        let is_static = flags & 0x10 != 0;
        let owner_is_value = set.is_value_type(handle).unwrap_or(false);
        let offset = if is_static {
            0
        } else if owner_is_value {
            u64::from(
                *self
                    .layout_of(&owner)?
                    .fields
                    .get(&row)
                    .ok_or_else(|| Fault::Unsupported("value type field".into()))?,
            )
        } else {
            let arguments = self.types.layout_arguments(&owner.arguments()).map_err(Fault::Unsupported)?;
            let (_, placements) = set
                .class_fields_with(handle, &arguments)
                .map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
            u64::from(
                placements
                    .iter()
                    .find(|p| p.field == row)
                    .map(|p| p.offset + 8)
                    .ok_or_else(|| Fault::Unsupported("field is not an instance field of its owner".into()))?,
            )
        };
        let found = Rc::new(FieldRef {
            owner,
            module: field_module,
            row,
            is_static,
            has_rva: flags & 0x100 != 0,
            thread_static: is_static && runtime::state().is_thread_static(self.model.set, field_module, row),
            owner_is_value,
            ty,
            offset,
        });
        self.fields.borrow_mut().insert(cache_key, found.clone());
        Ok(found)
    }

    fn field_address(&self, method: &Loaded, token: Token, target: V) -> Run<(u64, Ty)> {
        self.field_address_in(method.key.module, token, &method.ctx, target)
    }

    fn field_address_in(&self, module: usize, token: Token, ctx: &Ctx, target: V) -> Run<(u64, Ty)> {
        let field = self.field_ref(module, token, ctx)?;
        if field.is_static {
            self.ensure_initialized_gen(&field.owner)?;
            let state = runtime::state();
            let generic = !field.owner.arguments().is_empty();
            let address = if field.has_rva {
                map(runtime::rva_data(self.model.set, field.module, field.row))?
            } else if field.thread_static {
                let owner = if generic { self.types.id(&field.owner) } else { u32::MAX };
                let (size, references): (u32, Vec<u32>) = match &field.ty {
                    Ty::Prim(var) => (8, if *var == VarType::Ref { vec![0] } else { Vec::new() }),
                    Ty::Struct(layout) => (layout.size, layout.references.clone()),
                };
                runtime::roots().thread_static(owner, field.module, field.row, size, &references)
            } else if generic {
                let id = self.types.id(&field.owner);
                match &field.ty {
                    Ty::Prim(var) => state.exact_static(
                        id,
                        field.module,
                        field.row,
                        8,
                        if *var == VarType::Ref { &[0] } else { &[] },
                    ),
                    Ty::Struct(layout) => {
                        state.exact_static(id, field.module, field.row, layout.size, &layout.references)
                    }
                }
            } else {
                match &field.ty {
                    Ty::Prim(var) => state.static_slot(self.model, field.module, field.row, *var == VarType::Ref),
                    Ty::Struct(layout) => {
                        state.static_block(field.module, field.row, layout.size, &layout.references)
                    }
                }
            };
            return Ok((address, field.ty.clone()));
        }
        let base = match target {
            V::O(object) => {
                if object == 0 {
                    return self.throw("NullReferenceException");
                }
                if field.owner_is_value { object + 8 } else { object }
            }
            V::S(boxed) => boxed + 8,
            other => match other.address() {
                0 => return self.throw("NullReferenceException"),
                pointer => pointer,
            },
        };
        Ok((base + field.offset, field.ty.clone()))
    }

    fn type_ty(&self, module: usize, token: Token, ctx: &Ctx) -> Run<(Ty, Gen)> {
        let exact = map(self.types.resolve_token(module, token, ctx))?;
        Ok((self.gen_ty(&exact)?, exact))
    }

    fn array_type(&self, module: usize, token: Token, ctx: &Ctx) -> Run<(u64, Ty, u32)> {
        let element = map(self.types.resolve_token(module, token, ctx))?;
        let (ty, size) = self.element_size(&element)?;
        Ok((self.array_record(&element)?, ty, size))
    }





    fn step_block(
        &self,
        method: &Loaded,
        frame: &mut Frame,
        pc: &mut usize,
        current: &mut usize,
    ) -> Run<Exit> {
        let code = method.code;
        let module = method.key.module;
        let mut constrained: Option<Token> = None;
        loop {
            let at = *pc;
            *current = at;
            let remaining = self.budget.get();
            if remaining == 0 {
                return unsupported("instruction budget exhausted");
            }
            self.budget.set(remaining - 1);
            if remaining & 0x3FFF == 0 && crate::threads::others_waiting() {
                crate::threads::yield_now();
            }
            let instruction = decode_at(code, at)?;
            let next = instruction.1;
            let opcode = instruction.0.opcode.value;
            let argument = instruction.0.argument;
            *pc = next;
            macro_rules! pop {
                () => {
                    frame
                        .stack
                        .pop()
                        .ok_or_else(|| Fault::Unsupported("evaluation stack underflow".into()))?
                };
            }
            match opcode {
                0x00 | 0x01 | 0xFE12 | 0xFE13 | 0xFE14 | 0xFE19 | 0xFE1E => {}
                0x02..=0x09 | 0x0E | 0xFE09 | 0x11 | 0xFE0C => {
                    let index = match opcode {
                        0x02..=0x05 => (opcode - 0x02) as usize,
                        0x06..=0x09 => method.parameters.len() + (opcode - 0x06) as usize,
                        0x11 | 0xFE0C => method.parameters.len() + variable(argument),
                        _ => variable(argument),
                    };
                    let (address, kind) = frame.slot(index)?;
                    let value = self.read_value(address, &kind, &[])?;
                    frame.stack.push(value);
                }
                0x0A..=0x0D | 0x10 | 0xFE0B | 0x13 | 0xFE0E => {
                    let index = match opcode {
                        0x0A..=0x0D => method.parameters.len() + (opcode - 0x0A) as usize,
                        0x13 | 0xFE0E => method.parameters.len() + variable(argument),
                        _ => variable(argument),
                    };
                    let value = pop!();
                    let (address, kind) = frame.slot(index)?;
                    self.write_value(address, &kind, value)?;
                }
                0x0F | 0xFE0A | 0x12 | 0xFE0D => {
                    let index = variable(argument) + if matches!(opcode, 0x12 | 0xFE0D) { method.parameters.len() } else { 0 };
                    let (address, _) = frame.slot(index)?;
                    frame.stack.push(V::P(address));
                }
                0x14 => frame.stack.push(V::O(0)),
                0x15..=0x1E => frame.stack.push(V::I4(i32::from(opcode as i16 - 0x16))),
                0x1F | 0x20 => frame.stack.push(V::I4(int(argument) as i32)),
                0x21 => frame.stack.push(V::I8(int(argument))),
                0x22 => frame.stack.push(V::F4(float(argument) as f32)),
                0x23 => frame.stack.push(V::F8(float(argument))),
                0x25 => {
                    let top = *frame.stack.last().ok_or_else(|| Fault::Unsupported("evaluation stack underflow".into()))?;
                    frame.stack.push(top);
                }
                0x26 => {
                    pop!();
                }
                0x2A => {
                    let value = match &method.returns {
                        Some(Ty::Prim(var)) => Some(coerce(pop!(), *var)),
                        Some(Ty::Struct(_)) => Some(pop!()),
                        None => None,
                    };
                    return Ok(Exit::Return(value));
                }
                0x2B | 0x38 => *pc = target(argument)?,
                0x2C | 0x39 => {
                    if !truthy(pop!()) {
                        *pc = target(argument)?;
                    }
                }
                0x2D | 0x3A => {
                    if truthy(pop!()) {
                        *pc = target(argument)?;
                    }
                }
                0x2E..=0x37 | 0x3B..=0x44 => {
                    let b = pop!();
                    let a = pop!();
                    let relation = if opcode <= 0x37 { opcode - 0x2E } else { opcode - 0x3B };
                    if compare(relation, a, b) {
                        *pc = target(argument)?;
                    }
                }
                0x45 => {
                    let value = pop!().int() as u32;
                    let Argument::Switch(targets) = argument else {
                        return unsupported("switch operand");
                    };
                    if let Some(found) = targets.iter().nth(value as usize) {
                        *pc = found as usize + at;
                    }
                }
                0x46..=0x50 => {
                    let address = pop!().address();
                    if address == 0 {
                        return self.throw("NullReferenceException");
                    }
                    let var = match opcode {
                        0x46 => VarType::I8,
                        0x47 => VarType::U8,
                        0x48 => VarType::I16,
                        0x49 => VarType::U16,
                        0x4A | 0x4B => VarType::I32,
                        0x4C | 0x4D => VarType::I64,
                        0x4E => VarType::F32,
                        0x4F => VarType::F64,
                        _ => VarType::Ref,
                    };
                    let value = read(address, var);
                    frame.stack.push(value);
                }
                0x51..=0x57 | 0xDF => {
                    let value = pop!();
                    let address = pop!().address();
                    if address == 0 {
                        return self.throw("NullReferenceException");
                    }
                    let var = match opcode {
                        0x51 => VarType::Ref,
                        0x52 => VarType::I8,
                        0x53 => VarType::I16,
                        0x54 => VarType::I32,
                        0x55 | 0xDF => VarType::I64,
                        0x56 => VarType::F32,
                        _ => VarType::F64,
                    };
                    write(address, var, value);
                }
                0x58..=0x5E | 0x5F..=0x61 | 0xD6..=0xDB => {
                    let b = pop!();
                    let a = pop!();
                    let value = match arithmetic(opcode, a, b) {
                        Ok(value) => value,
                        Err(name) => return self.throw(name),
                    };
                    frame.stack.push(value);
                }
                0x62..=0x64 => {
                    let amount = pop!().int() as u32;
                    let value = pop!();
                    frame.stack.push(shift(opcode, value, amount));
                }
                0x65 => {
                    let value = pop!();
                    frame.stack.push(match value {
                        V::I4(v) => V::I4(v.wrapping_neg()),
                        V::I8(v) => V::I8(v.wrapping_neg()),
                        V::F4(v) => V::F4(-v),
                        V::F8(v) => V::F8(-v),
                        other => V::I8((other.int()).wrapping_neg()),
                    });
                }
                0x66 => {
                    let value = pop!();
                    frame.stack.push(match value {
                        V::I4(v) => V::I4(!v),
                        other => V::I8(!other.int()),
                    });
                }
                0x67..=0x6E | 0x76 | 0xD1..=0xD3 | 0xE0 => {
                    let value = pop!();
                    frame.stack.push(convert(opcode, value));
                }
                0x82..=0x8B | 0xB3..=0xBA | 0xD4 | 0xD5 => {
                    let value = pop!();
                    match convert_checked(opcode, value) {
                        Some(result) => frame.stack.push(result),
                        None => return self.throw("OverflowException"),
                    }
                }
                0xC3 => {
                    let value = pop!();
                    if !value.float().is_finite() {
                        return self.throw("ArithmeticException");
                    }
                    frame.stack.push(value);
                }
                0xFE01..=0xFE05 => {
                    let b = pop!();
                    let a = pop!();
                    let result = match opcode {
                        0xFE01 => compare(0, a, b),
                        0xFE02 => compare(2, a, b),
                        0xFE03 => compare(7, a, b),
                        0xFE04 => compare(4, a, b),
                        _ => compare(9, a, b),
                    };
                    frame.stack.push(V::I4(i32::from(result)));
                }
                0x72 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("string token");
                    };
                    let state = runtime::state();
                    let address = map(state.string_literal(self.model.set, self.model, module, token.row()))?;
                    frame.stack.push(V::O(address));
                }
                0x28 | 0x6F | 0x73 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("call token");
                    };
                    if let Some(array) = self.array_method(module, token, &method.ctx)? {
                        let count = if opcode == 0x73 { array.parameters.saturating_sub(1) } else { array.parameters };
                        if frame.stack.len() < count {
                            return unsupported("evaluation stack underflow");
                        }
                        let arguments: Vec<V> = frame.stack.split_off(frame.stack.len() - count);
                        let _pin = Pin::new(&arguments);
                        if let Some(value) = self.array_call(&array, &arguments)? {
                            frame.stack.push(value);
                        }
                        continue;
                    }
                    let (key, call_ctx) = self.method_inst(module, token, &method.ctx)?;
                    let callee = self.load_ctx(&key, &call_ctx)?;
                    if opcode == 0x73 {
                        self.construct(frame, &callee)?;
                    } else {
                        if callee.is_static && callee.native.is_none() {
                            self.ensure_initialized_gen(&callee.owner_gen)?;
                        }
                        let count = callee.parameters.len();
                        if frame.stack.len() < count {
                            return unsupported("evaluation stack underflow");
                        }
                        let mut arguments: Vec<V> = frame.stack.split_off(frame.stack.len() - count);
                        let _pin = Pin::new(&arguments);
                        let mut target = (key.clone(), call_ctx.clone());
                        let mut resolved = false;
                        if let Some(prefix) = constrained.take() {
                            let (ty, exact) = self.type_ty(module, prefix, &method.ctx)?;
                            if callee.is_static {
                                target = match self.find_value_override(&exact, &key, &call_ctx, false)? {
                                    Some(found) => found,
                                    None if !callee.is_abstract => (key.clone(), call_ctx.clone()),
                                    None => {
                                        let name = self.model.set.modules[key.module]
                                            .metadata
                                            .string_cell(TableId::MethodDef, key.row, 3)
                                            .unwrap_or("?");
                                        return unsupported(format!(
                                            "static virtual {}::{name} has no implementation on {}",
                                            self.types.full_name(&callee.owner_gen),
                                            self.types.full_name(&exact)
                                        ));
                                    }
                                };
                                resolved = true;
                            } else if matches!(ty, Ty::Prim(VarType::Ref)) {
                                arguments[0] = V::O(read(arguments[0].address(), VarType::Ref).address());
                            } else {
                                match self.find_value_override(&exact, &key, &call_ctx, true)? {
                                    Some(found) => {
                                        target = found;
                                        resolved = true;
                                    }
                                    None => {
                                        let value = self.read_value(arguments[0].address(), &ty, &[])?;
                                        let boxed = self.box_any(&exact, &ty, value)?;
                                        arguments[0] = V::O(boxed);
                                    }
                                }
                            }
                        }
                        if opcode == 0x6F
                            && !callee.is_static
                            && !resolved
                            && let V::O(this) = arguments[0]
                        {
                            if this == 0 {
                                return self.throw("NullReferenceException");
                            }
                            if callee.is_virtual && self.rcw_pointer(this).is_some() && callee.is_abstract {
                                if let Some(value) = self.rcw_call(&key, &call_ctx, &arguments)? {
                                    frame.stack.push(value);
                                }
                                continue;
                            }
                            if callee.is_virtual {
                                target = self.resolve_virtual(&key, &call_ctx, this)?;
                            }
                        }
                        let result = self.invoke_ctx(&target.0, &target.1, &arguments)?;
                        if let Some(value) = result {
                            frame.stack.push(value);
                        }
                    }
                }
                0x29 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("calli token");
                    };
                    let metadata = &self.model.set.modules[module].metadata;
                    let blob = metadata
                        .cell(TableId::StandAloneSig, token.row(), 0)
                        .and_then(|b| metadata.blob(b))
                        .map_err(|e| Fault::Unsupported(e.to_string()))?;
                    let Ok(Signature::Method(sig)) = Signature::parse(blob) else {
                        return unsupported("calli signature does not parse");
                    };
                    let function = pop!().bits();
                    let count = sig.parameters.len() + usize::from(sig.has_this && !sig.explicit_this);
                    if frame.stack.len() < count {
                        return unsupported("evaluation stack underflow");
                    }
                    let arguments: Vec<V> = frame.stack.split_off(frame.stack.len() - count);
                    let _pin = Pin::new(&arguments);
                    let result = if let Some((key, call_ctx)) = self.decode_function(function) {
                        match (self.invoke_ctx(&key, &call_ctx, &arguments)?, unmodified(&sig.return_type)) {
                            (None, TypeSig::Void) => None,
                            (None, TypeSig::R4) => Some(V::F4(0.0)),
                            (None, TypeSig::R8) => Some(V::F8(0.0)),
                            (None, TypeSig::I8 | TypeSig::U8) => Some(V::I8(0)),
                            (None, TypeSig::Pointer(_) | TypeSig::FunctionPointer(_) | TypeSig::IntPtr | TypeSig::UIntPtr) => Some(V::P(0)),
                            (None, _) => Some(V::I4(0)),
                            (result, _) => result,
                        }
                    } else {
                        if function == 0 {
                            return self.throw("NullReferenceException");
                        }
                        let index = self.indirect_platform(module, token.row(), function, &sig, &method.ctx)?;
                        self.platform_call(index, &arguments)?
                    };
                    if let Some(value) = result {
                        frame.stack.push(value);
                    }
                }
                0x27 => return unsupported("jmp"),
                0x74 | 0x75 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("cast token");
                    };
                    let value = pop!();
                    let object = value.address();
                    let target = map(self.types.resolve_token(module, token, &method.ctx))?;
                    let ok = object == 0 || self.is_assignable(object, &target);
                    if ok {
                        frame.stack.push(V::O(object));
                    } else if opcode == 0x75 {
                        frame.stack.push(V::O(0));
                    } else {
                        return self.throw("InvalidCastException");
                    }
                }
                0x7A => {
                    let object = pop!().address();
                    if object == 0 {
                        return self.throw("NullReferenceException");
                    }
                    if crate::diagnostics::enabled() {
                        crate::diagnostics::write(
                            "throw",
                            &format!("{}: {}", self.exception_type(object), self.exception_message(object).unwrap_or_default()),
                        );
                    }
                    if self.trace_calls {
                        eprintln!(
                            "{}throw {} {}",
                            trace_indent(runtime::roots().depth),
                            self.exception_type(object),
                            self.exception_message(object).unwrap_or_default()
                        );
                    }
                    return Err(Fault::Exception(object));
                }
                0xFE1A => {
                    let object = *frame
                        .caught
                        .last()
                        .ok_or_else(|| Fault::Unsupported("rethrow outside a handler".into()))?;
                    return Err(Fault::Exception(object));
                }
                0x7B | 0x7C | 0x7E | 0x7F => {
                    let Argument::Token(token) = argument else {
                        return unsupported("field token");
                    };
                    let target = if matches!(opcode, 0x7B | 0x7C) { pop!() } else { V::O(0) };
                    let _pin = Pin::new(&[target]);
                    let (address, ty) = self.field_address(method, token, target)?;
                    let value = if matches!(opcode, 0x7C | 0x7F) {
                        V::P(address)
                    } else {
                        self.read_value(address, &ty, &[target])?
                    };
                    frame.stack.push(value);
                }
                0x7D | 0x80 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("field token");
                    };
                    let value = pop!();
                    let target = if opcode == 0x7D { pop!() } else { V::O(0) };
                    let _pin = Pin::new(&[value, target]);
                    let (address, ty) = self.field_address(method, token, target)?;
                    self.write_value(address, &ty, value)?;
                }
                0x71 | 0x81 | 0x70 | 0xA5 | 0x79 | 0x8C | 0xFE15 | 0xFE1C => {
                    let Argument::Token(token) = argument else {
                        return unsupported("type token");
                    };
                    let (ty, exact) = self.type_ty(module, token, &method.ctx)?;
                    let is_reference = matches!(ty, Ty::Prim(VarType::Ref));
                    let nullable = self.nullable_argument(&exact);
                    match opcode {
                        0x71 => {
                            let address = pop!().address();
                            if address == 0 {
                                return self.throw("NullReferenceException");
                            }
                            let value = self.read_value(address, &ty, &[])?;
                            frame.stack.push(value);
                        }
                        0x81 => {
                            let value = pop!();
                            let address = pop!().address();
                            if address == 0 {
                                return self.throw("NullReferenceException");
                            }
                            self.write_value(address, &ty, value)?;
                        }
                        0x70 => {
                            let source = pop!().address();
                            let destination = pop!().address();
                            unsafe {
                                std::ptr::copy(source as *const u8, destination as *mut u8, ty.size() as usize);
                            }
                        }
                        0xFE15 => {
                            let address = pop!().address();
                            if address == 0 {
                                return self.throw("NullReferenceException");
                            }
                            unsafe { std::ptr::write_bytes(address as *mut u8, 0, ty.size() as usize) };
                        }
                        0xFE1C => frame.stack.push(V::I4(ty.size() as i32)),
                        0x8C => {
                            let value = pop!();
                            if is_reference {
                                frame.stack.push(value);
                            } else if let Some(inner) = &nullable {
                                let boxed = self.box_nullable(&exact, inner, value)?;
                                frame.stack.push(V::O(boxed));
                            } else {
                                let boxed = self.box_any(&exact, &ty, value)?;
                                frame.stack.push(V::O(boxed));
                            }
                        }
                        _ => {
                            let object = pop!().address();
                            if is_reference {
                                if opcode == 0x79 {
                                    return unsupported("unbox of a reference type");
                                }
                                if object != 0 && !self.is_assignable(object, &exact) {
                                    return self.throw("InvalidCastException");
                                }
                                frame.stack.push(V::O(object));
                            } else if let Some(inner) = &nullable
                                && opcode == 0xA5
                            {
                                let value = self.unbox_nullable(&exact, inner, object)?;
                                frame.stack.push(value);
                            } else {
                                if object == 0 {
                                    return self.throw("NullReferenceException");
                                }
                                if !self.unbox_compatible(object, &exact)? {
                                    return self.throw("InvalidCastException");
                                }
                                let value = if opcode == 0x79 {
                                    V::P(object + 8)
                                } else {
                                    self.read_value(object + 8, &ty, &[V::O(object)])?
                                };
                                frame.stack.push(value);
                            }
                        }
                    }
                }
                0x8D => {
                    let Argument::Token(token) = argument else {
                        return unsupported("array token");
                    };
                    let (record, _, _) = self.array_type(module, token, &method.ctx)?;
                    let count = pop!().int();
                    if count < 0 {
                        return self.throw("OverflowException");
                    }
                    let array = runtime::allocate_managed(record, count as u64);
                    if array == 0 {
                        return self.throw("OutOfMemoryException");
                    }
                    frame.stack.push(V::O(array));
                }
                0x8E => {
                    let array = pop!().address();
                    if array == 0 {
                        return self.throw("NullReferenceException");
                    }
                    frame.stack.push(V::I8(i64::from(unsafe { *((array + 8) as *const u32) })));
                }
                0x8F | 0x90..=0x9A | 0xA3 => {
                    let fixed = match opcode {
                        0x90 => Some(ElementKind::I8),
                        0x91 => Some(ElementKind::U8),
                        0x92 => Some(ElementKind::I16),
                        0x93 => Some(ElementKind::U16),
                        0x94 | 0x95 => Some(ElementKind::I32),
                        0x96 | 0x97 => Some(ElementKind::I64),
                        0x98 => Some(ElementKind::F32),
                        0x99 => Some(ElementKind::F64),
                        0x9A => Some(ElementKind::Reference),
                        _ => None,
                    };
                    let (ty, size) = match fixed {
                        Some(kind) => (Ty::Prim(kind.var()), kind.size()),
                        None => {
                            let Argument::Token(token) = argument else {
                                return unsupported("element token");
                            };
                            let (_, ty, size) = self.array_type(module, token, &method.ctx)?;
                            (ty, size)
                        }
                    };
                    let index = pop!().int();
                    let array = pop!();
                    let address = self.element(array.address(), index, size)?;
                    let value = if opcode == 0x8F {
                        V::P(address)
                    } else {
                        self.read_value(address, &ty, &[array])?
                    };
                    frame.stack.push(value);
                }
                0x9B..=0xA2 | 0xA4 => {
                    let fixed = match opcode {
                        0x9B | 0x9F => Some(ElementKind::I64),
                        0x9C => Some(ElementKind::I8),
                        0x9D => Some(ElementKind::I16),
                        0x9E => Some(ElementKind::I32),
                        0xA0 => Some(ElementKind::F32),
                        0xA1 => Some(ElementKind::F64),
                        0xA2 => Some(ElementKind::Reference),
                        _ => None,
                    };
                    let (ty, size) = match fixed {
                        Some(kind) => (Ty::Prim(kind.var()), kind.size()),
                        None => {
                            let Argument::Token(token) = argument else {
                                return unsupported("element token");
                            };
                            let (_, ty, size) = self.array_type(module, token, &method.ctx)?;
                            (ty, size)
                        }
                    };
                    let value = pop!();
                    let index = pop!().int();
                    let array = pop!().address();
                    let address = self.element(array, index, size)?;
                    self.write_value(address, &ty, value)?;
                }
                0xD0 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("token");
                    };
                    match token.table() {
                        Some(TableId::TypeDef) | Some(TableId::TypeRef) | Some(TableId::TypeSpec) => {
                            let exact = if token.table() == Some(TableId::TypeDef) {
                                Gen::Named(TypeHandle { module, row: token.row() }, Rc::from(Vec::new()))
                            } else {
                                map(self.types.resolve_token(module, token, &method.ctx))?
                            };
                            let handle = self.type_handle_value(&exact)?;
                            frame.stack.push(handle);
                        }
                        Some(TableId::Field) | Some(TableId::MemberRef)
                            if self.field_ref(module, token, &method.ctx).is_ok() =>
                        {
                            let field = self.field_ref(module, token, &method.ctx)?;
                            if !field.has_rva {
                                return unsupported("field handles");
                            }
                            let address = map(runtime::rva_data(self.model.set, field.module, field.row))?;
                            self.rva_sizes.borrow_mut().insert(address, field.ty.size());
                            frame.stack.push(V::P(map(runtime::rva_data(self.model.set, field.module, field.row))?));
                        }
                        _ => return unsupported("type and method handles"),
                    }
                }
                0xFE06 | 0xFE07 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("function token");
                    };
                    let (mut key, mut call_ctx) = self.method_inst(module, token, &method.ctx)?;
                    if opcode == 0xFE07 {
                        let this = pop!().address();
                        if this == 0 {
                            return self.throw("NullReferenceException");
                        }
                        let callee = self.load_ctx(&key, &call_ctx)?;
                        if callee.is_virtual {
                            (key, call_ctx) = self.resolve_virtual(&key, &call_ctx, this)?;
                        }
                    }
                    let encoded = if opcode == 0xFE06 && self.is_unmanaged_entry(&key) {
                        self.unmanaged_stub(&key, &call_ctx)?
                    } else {
                        self.encode_function(&key, &call_ctx)
                    };
                    frame.stack.push(V::I8(encoded as i64));
                }
                0xDC => return Ok(Exit::EndFinally),
                0xFE11 => {
                    let accepted = truthy(pop!());
                    return Ok(Exit::EndFilter(accepted));
                }
                0xDD | 0xDE => {
                    frame.stack.clear();
                    return Ok(Exit::Leave(at as u32, target(argument)? as u32));
                }
                0xFE0F => {
                    let size = pop!().int().max(0) as usize;
                    let mut block = vec![0u8; size.max(1)].into_boxed_slice();
                    let address = block.as_mut_ptr() as u64;
                    frame.scratch.push(block);
                    frame.stack.push(V::P(address));
                }
                0xFE16 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("constrained token");
                    };
                    constrained = Some(token);
                    continue;
                }
                0xFE17 => {
                    let size = pop!().int() as usize;
                    let source = pop!().address();
                    let destination = pop!().address();
                    unsafe { std::ptr::copy(source as *const u8, destination as *mut u8, size) };
                }
                0xFE18 => {
                    let size = pop!().int() as usize;
                    let value = pop!().int() as u8;
                    let destination = pop!().address();
                    unsafe { std::ptr::write_bytes(destination as *mut u8, value, size) };
                }
                other => return unsupported(format!("opcode {other:#x}")),
            }
            constrained = None;
        }
    }

    fn construct(&self, frame: &mut Frame, callee: &Loaded) -> Run<()> {
        let owner = callee.owner_gen.clone();
        let set = self.model.set;
        let count = callee.parameters.len() - 1;
        if frame.stack.len() < count {
            return unsupported("evaluation stack underflow");
        }
        self.ensure_initialized_gen(&owner)?;
        if set.is_value_type(callee.owner).unwrap_or(false) {
            let ty = self.gen_ty(&owner)?;
            let (storage, value) = match &ty {
                Ty::Struct(_) => {
                    let boxed = self.allocate_box(&owner)?;
                    (boxed + 8, Some(V::S(boxed)))
                }
                Ty::Prim(_) => {
                    let mut block = vec![0u8; 8].into_boxed_slice();
                    let address = block.as_mut_ptr() as u64;
                    frame.scratch.push(block);
                    (address, None)
                }
            };
            let mut arguments = vec![V::P(storage)];
            arguments.extend(frame.stack.split_off(frame.stack.len() - count));
            if let Some(value) = value {
                frame.stack.push(value);
            }
            self.invoke_ctx(&callee.key, &callee.ctx, &arguments)?;
            if let Ty::Prim(var) = ty {
                frame.stack.push(read(storage, var));
            }
            return Ok(());
        }
        let name = self.type_name(callee.owner);
        if name == "System.String" {
            let wanted = self
                .parameter_gens(&callee.key, &callee.ctx)
                .ok_or_else(|| Fault::Unsupported("string constructor signature".into()))?;
            let metadata = &set.modules[callee.owner.module].metadata;
            let range = metadata
                .method_range(callee.owner.row)
                .map_err(|e| Fault::Unsupported(e.to_string()))?;
            for row in range {
                if metadata.string_cell(TableId::MethodDef, row, 3).ok() != Some("Ctor") {
                    continue;
                }
                let candidate = MethodKey { module: callee.owner.module, row };
                let Some(found) = self.parameter_gens(&candidate, &Ctx::default()) else {
                    continue;
                };
                if found.len() != wanted.len() || found[1..] != wanted[1..] {
                    continue;
                }
                let helper = self.load(&candidate)?;
                let mut arguments: Vec<V> = Vec::new();
                if !helper.is_static {
                    arguments.push(V::O(0));
                }
                arguments.extend(frame.stack.split_off(frame.stack.len() - count));
                let result = self.invoke(&candidate, &arguments)?;
                frame.stack.push(result.unwrap_or(V::O(0)));
                return Ok(());
            }
            return unsupported("string constructor without a helper");
        }
        let flags = set.modules[callee.owner.module]
            .metadata
            .cell(TableId::TypeDef, callee.owner.row, 0)
            .unwrap_or(0);
        if flags & 0xA0 != 0 {
            return unsupported("construction of an abstract type");
        }
        let record = self.record_of(&owner)?;
        let object = runtime::allocate_managed(record, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        let at = frame.stack.len() - count;
        frame.stack.insert(at, V::O(object));
        let arguments: Vec<V> = frame.stack[at..].to_vec();
        frame.stack.truncate(at + 1);
        self.invoke_ctx(&callee.key, &callee.ctx, &arguments)?;
        Ok(())
    }

    fn construct_default(&self, exact: &Gen) -> Run<u64> {
        let handle = exact
            .handle()
            .ok_or_else(|| Fault::Unsupported("construction of a non class".into()))?;
        self.ensure_initialized_gen(exact)?;
        let record = self.record_of(exact)?;
        let object = runtime::allocate_managed(record, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        let metadata = &self.model.set.modules[handle.module].metadata;
        if let Ok(range) = metadata.method_range(handle.row) {
            for row in range {
                let key = MethodKey { module: handle.module, row };
                if metadata.string_cell(TableId::MethodDef, row, 3).ok() == Some(".ctor")
                    && self.model.method_sig(&key).is_ok_and(|sig| sig.parameters.is_empty() && sig.has_this)
                {
                    let _pin = Pin::new(&[V::O(object)]);
                    self.invoke_ctx(&key, &exact.context(), &[V::O(object)])?;
                    return Ok(object);
                }
            }
        }
        unsupported("type has no parameterless constructor")
    }

    fn default_comparer(&self, argument: &Gen, equality: bool) -> Run<u64> {
        let nullable = self.nullable_argument(argument);
        let interface = map(self.types.core_handle(if equality {
            "System.IEquatable`1"
        } else {
            "System.IComparable`1"
        }))?;
        let implements = self.types.all_interfaces(argument).iter().any(|g| {
            g.handle() == Some(interface) && g.arguments().first() == Some(argument)
        });
        let is_enum = argument
            .handle()
            .is_some_and(|h| self.model.set.is_enum(h).unwrap_or(false));
        let string = argument
            .handle()
            .is_some_and(|h| self.type_name(h) == "System.String");
        let (name, inner): (&'static str, Gen) = match (&nullable, equality) {
            (Some(inner), true) => ("System.Collections.Generic.NullableEqualityComparer`1", inner.clone()),
            (Some(inner), false) => ("System.Collections.Generic.NullableComparer`1", inner.clone()),
            (None, true) if is_enum => ("System.Collections.Generic.EnumEqualityComparer`1", argument.clone()),
            (None, false) if is_enum => ("System.Collections.Generic.EnumComparer`1", argument.clone()),
            (None, true) if implements || string => {
                ("System.Collections.Generic.GenericEqualityComparer`1", argument.clone())
            }
            (None, false) if implements || string => ("System.Collections.Generic.GenericComparer`1", argument.clone()),
            (None, true) => ("System.Collections.Generic.ObjectEqualityComparer`1", argument.clone()),
            (None, false) => ("System.Collections.Generic.ObjectComparer`1", argument.clone()),
        };
        let handle = map(self.types.core_handle(name))?;
        let exact = Gen::Named(handle, Rc::from(vec![inner]));
        let record = self.record_of(&exact)?;
        match runtime::allocate_managed(record, 0) {
            0 => self.throw("OutOfMemoryException"),
            object => Ok(object),
        }
    }

    fn nullable_argument(&self, exact: &Gen) -> Option<Gen> {
        let Gen::Named(handle, arguments) = exact else {
            return None;
        };
        (arguments.len() == 1 && self.type_name(*handle) == "System.Nullable`1").then(|| arguments[0].clone())
    }

    fn nullable_fields(&self, exact: &Gen) -> Run<(u64, u64)> {
        let layout = self.layout_of(exact)?;
        let handle = layout.handle;
        let metadata = &self.model.set.modules[handle.module].metadata;
        let mut has_value = None;
        let mut value = None;
        for (row, offset) in &layout.fields {
            match metadata.string_cell(TableId::Field, *row, 1).ok() {
                Some("hasValue") => has_value = Some(u64::from(*offset)),
                Some("value") => value = Some(u64::from(*offset)),
                _ => {}
            }
        }
        match (has_value, value) {
            (Some(a), Some(b)) => Ok((a, b)),
            _ => unsupported("nullable layout"),
        }
    }

    fn box_nullable(&self, exact: &Gen, inner: &Gen, value: V) -> Run<u64> {
        let (has_value, offset) = self.nullable_fields(exact)?;
        let V::S(boxed) = value else {
            return unsupported("a nullable value was expected");
        };
        if unsafe { *((boxed + 8 + has_value) as *const u8) } == 0 {
            return Ok(0);
        }
        let _pin = Pin::new(&[value]);
        let ty = self.gen_ty(inner)?;
        let inner_value = self.read_value(boxed + 8 + offset, &ty, &[value])?;
        self.box_any(inner, &ty, inner_value)
    }

    fn unbox_nullable(&self, exact: &Gen, inner: &Gen, object: u64) -> Run<V> {
        let (has_value, offset) = self.nullable_fields(exact)?;
        let _pin = Pin::new(&[V::O(object)]);
        let boxed = self.allocate_box(exact)?;
        if object != 0 {
            if !self.unbox_compatible(object, inner)? {
                return self.throw("InvalidCastException");
            }
            let ty = self.gen_ty(inner)?;
            let size = ty.size() as usize;
            unsafe {
                *((boxed + 8 + has_value) as *mut u8) = 1;
                std::ptr::copy((object + 8) as *const u8, (boxed + 8 + offset) as *mut u8, size);
            }
        }
        Ok(V::S(boxed))
    }

    fn type_object(&self, exact: &Gen) -> Run<u64> {
        if let Some(found) = self.type_objects.borrow().get(exact) {
            return Ok(*found);
        }
        let handle = self.corelib_type("System", "RuntimeType")?;
        let record = self.record(RuntimeType::Class(handle))?;
        let object = runtime::allocate_managed(record, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        type_roots().push(object);
        self.type_objects.borrow_mut().insert(exact.clone(), object);
        if let Ok(module) = self.gen_module(exact) {
            self.mark_used(module);
        }
        self.object_types.borrow_mut().insert(object, exact.clone());
        if let Ok(offset) = self.class_field_offset("System", "RuntimeType", "m_handle") {
            let table = self.method_table(exact)?;
            unsafe { *((object + u64::from(offset)) as *mut u64) = table };
        }
        Ok(object)
    }

    fn type_handle_value(&self, exact: &Gen) -> Run<V> {
        let object = self.type_object(exact)?;
        let handle = Gen::Named(self.corelib_type("System", "RuntimeTypeHandle")?, Rc::from(Vec::new()));
        let boxed = self.allocate_box(&handle)?;
        unsafe { *((boxed + 8) as *mut u64) = object };
        Ok(V::S(boxed))
    }

    fn encode_function(&self, key: &MethodKey, ctx: &Ctx) -> u64 {
        let mut functions = self.functions.borrow_mut();
        let index = match functions.iter().position(|(k, c)| k == key && c == ctx) {
            Some(index) => index,
            None => {
                functions.push((key.clone(), ctx.clone()));
                functions.len() - 1
            }
        };
        FUNCTION_TAG | index as u64
    }

    fn decode_function(&self, value: u64) -> Option<(MethodKey, Ctx)> {
        if value & 0xFF00_0000_0000_0000 != FUNCTION_TAG {
            return None;
        }
        self.functions
            .borrow()
            .get((value & 0x00FF_FFFF_FFFF_FFFF) as usize)
            .cloned()
    }

    fn find_value_override(
        &self,
        exact: &Gen,
        key: &MethodKey,
        ctx: &Ctx,
        instance: bool,
    ) -> Run<Option<(MethodKey, Ctx)>> {
        let Some(handle) = exact.handle() else {
            return Ok(None);
        };
        let set = self.model.set;
        let target_metadata = &set.modules[handle.module].metadata;
        let type_ctx = Ctx { types: exact.arguments(), methods: ctx.methods.clone() };
        for row in 1..=target_metadata.rows(TableId::MethodImpl) {
            if target_metadata.cell(TableId::MethodImpl, row, 0).ok() != Some(handle.row) {
                continue;
            }
            let (Ok(Some(body)), Ok(Some(declaration))) = (
                target_metadata.coded(TableId::MethodImpl, row, 1),
                target_metadata.coded(TableId::MethodImpl, row, 2),
            ) else {
                continue;
            };
            let Ok((declared, declared_ctx)) = self.method_inst(handle.module, declaration, &type_ctx) else {
                continue;
            };
            if &declared == key && (ctx.types.is_empty() || declared_ctx.types == ctx.types)
                && let Ok((body, _)) = self.method_inst(handle.module, body, &type_ctx)
            {
                return Ok(Some((body, type_ctx)));
            }
        }
        let name = set.modules[key.module]
            .metadata
            .string_cell(TableId::MethodDef, key.row, 3)
            .unwrap_or("");
        let Ok(range) = target_metadata.method_range(handle.row) else {
            return Ok(None);
        };
        for row in range {
            if target_metadata.string_cell(TableId::MethodDef, row, 3).ok() != Some(name) {
                continue;
            }
            let candidate = MethodKey { module: handle.module, row };
            let Ok(candidate_sig) = self.model.method_sig(&candidate) else {
                continue;
            };
            if candidate_sig.has_this == instance && self.signatures_match(&candidate, &type_ctx, key, ctx) {
                return Ok(Some((candidate, type_ctx)));
            }
        }
        Ok(None)
    }

    fn box_any(&self, exact: &Gen, ty: &Ty, value: V) -> Run<u64> {
        if let Some(inner) = self.nullable_argument(exact)
            && matches!(value, V::S(_))
        {
            return self.box_nullable(exact, &inner, value);
        }
        match ty {
            Ty::Prim(var) => self.box_value(exact, *var, value),
            Ty::Struct(_) => {
                let _pin = Pin::new(&[value]);
                let boxed = self.allocate_box(exact)?;
                self.write_value(boxed + 8, ty, value)?;
                Ok(boxed)
            }
        }
    }

    fn box_value(&self, exact: &Gen, var: VarType, value: V) -> Run<u64> {
        let record = self.record_of(exact)?;
        let object = runtime::allocate_managed(record, 0);
        if object == 0 {
            return unsupported("out of memory");
        }
        write(object + 8, var, value);
        Ok(object)
    }

    fn unbox_compatible(&self, object: u64, target: &Gen) -> Run<bool> {
        let exact = self.object_gen(object)?;
        if &exact == target {
            return Ok(true);
        }
        let (Some(class), Some(handle)) = (exact.handle(), target.handle()) else {
            return Ok(false);
        };
        let set = self.model.set;
        let underlying = |h: TypeHandle| -> Option<u32> {
            if set.is_enum(h).unwrap_or(false) {
                set.value_type_layout(h).ok().map(|l| l.size)
            } else {
                None
            }
        };
        let primitive_size = |h: TypeHandle| -> Option<u32> {
            match self.type_name(h).as_str() {
                "System.Byte" | "System.SByte" | "System.Boolean" => Some(1),
                "System.Int16" | "System.UInt16" | "System.Char" => Some(2),
                "System.Int32" | "System.UInt32" => Some(4),
                "System.Int64" | "System.UInt64" => Some(8),
                _ => None,
            }
        };
        let a = underlying(class).or_else(|| primitive_size(class));
        let b = underlying(handle).or_else(|| primitive_size(handle));
        Ok(a.is_some() && a == b && (underlying(class).is_some() || underlying(handle).is_some()))
    }

    fn element(&self, array: u64, index: i64, size: u32) -> Run<u64> {
        if array == 0 {
            return self.throw("NullReferenceException");
        }
        let length = i64::from(unsafe { *((array + 8) as *const u32) });
        if index < 0 || index >= length {
            return self.throw("IndexOutOfRangeException");
        }
        Ok(array + u64::from(ARRAY_DATA) + index as u64 * u64::from(size))
    }

    pub fn find_method(&self, namespace: &str, type_name: &str, name: &str, parameters: usize) -> Option<MethodKey> {
        let owner = self.corelib_type(namespace, type_name).ok()?;
        let metadata = &self.model.set.modules[owner.module].metadata;
        metadata.method_range(owner.row).ok()?.find_map(|row| {
            let key = MethodKey { module: owner.module, row };
            (metadata.string_cell(TableId::MethodDef, row, 3).ok() == Some(name)
                && self.model.method_sig(&key).is_ok_and(|sig| sig.parameters.len() == parameters))
            .then_some(key)
        })
    }

    pub fn has_attribute(&self, key: &MethodKey, attribute: &str) -> bool {
        let set = self.model.set;
        let metadata = &set.modules[key.module].metadata;
        (1..=metadata.rows(TableId::CustomAttribute)).any(|row| {
            let (Ok(Some(owner)), Ok(Some(constructor))) = (
                metadata.coded(TableId::CustomAttribute, row, 0),
                metadata.coded(TableId::CustomAttribute, row, 1),
            ) else {
                return false;
            };
            if owner.table() != Some(TableId::MethodDef) || owner.row() != key.row {
                return false;
            }
            let name = match constructor.table() {
                Some(TableId::MemberRef) => metadata
                    .coded(TableId::MemberRef, constructor.row(), 0)
                    .ok()
                    .flatten()
                    .filter(|parent| parent.table() == Some(TableId::TypeRef))
                    .and_then(|parent| metadata.string_cell(TableId::TypeRef, parent.row(), 1).ok()),
                Some(TableId::MethodDef) => set.modules[key.module]
                    .method_owner(constructor.row())
                    .ok()
                    .flatten()
                    .and_then(|owner| metadata.string_cell(TableId::TypeDef, owner, 1).ok()),
                _ => None,
            };
            name == Some(attribute)
        })
    }

    pub fn set_entry_module(&self, module: usize) {
        self.entry_module.set(module);
    }

    pub fn parameter_count(&self, key: &MethodKey) -> Option<usize> {
        self.load(key).ok().map(|loaded| loaded.parameters.len())
    }

    pub fn string_array(&self, items: &[String]) -> Run<u64> {
        let string = map(self.types.core("String"))?;
        let record = self.array_record(&string)?;
        let array = runtime::allocate_managed(record, items.len() as u64);
        if array == 0 {
            return self.throw("OutOfMemoryException");
        }
        let _pin = Pin::new(&[V::O(array)]);
        for (index, item) in items.iter().enumerate() {
            let units: Vec<u16> = item.encode_utf16().collect();
            let text = runtime::new_string(&units);
            unsafe { *((array + u64::from(ARRAY_DATA) + 8 * index as u64) as *mut u64) = text };
        }
        Ok(array)
    }

    pub fn exception_message(&self, object: u64) -> Option<String> {
        let handle = self.corelib_type("System", "Exception").ok()?;
        let (_, placements) = self.model.set.class_fields(handle).ok()?;
        let metadata = &self.model.set.modules[handle.module].metadata;
        let offset = placements
            .iter()
            .find(|p| metadata.string_cell(TableId::Field, p.field, 1).ok() == Some("_message"))?
            .offset;
        let message = unsafe { *((object + 8 + u64::from(offset)) as *const u64) };
        (message != 0).then(|| String::from_utf16_lossy(string_units(message)))
    }

    fn boxed_address(&self, value: V) -> u64 {
        match value {
            V::P(pointer) => pointer.wrapping_sub(8),
            other => other.address(),
        }
    }

    fn enum_value(&self, value: V) -> Run<(VarType, u64)> {
        let object = self.boxed_address(value);
        if object == 0 {
            return self.throw("NullReferenceException");
        }
        let exact = self.object_gen(object)?;
        let Ty::Prim(var) = self.gen_ty(&exact)? else {
            return unsupported("enum without a primitive value");
        };
        let size = ElementKind::from_var(var).map_or(8, ElementKind::size) as usize;
        let mut bits = 0u64;
        unsafe { std::ptr::copy_nonoverlapping((object + 8) as *const u8, (&raw mut bits).cast(), size) };
        Ok((var, bits))
    }

    pub fn has_native(&self, key: &MethodKey) -> bool {
        self.load(key).is_ok_and(|loaded| loaded.native.is_some())
    }

    pub fn blocker(&self, key: &MethodKey) -> Option<String> {
        match self.load(key) {
            Ok(_) => None,
            Err(Fault::Unsupported(reason)) if reason == "abstract method" => None,
            Err(Fault::Unsupported(reason)) if reason == "open generic method" => {
                let object = self.types.core("Object").ok()?;
                let owner = self.model.method_owner(key).ok()?;
                let count = self.types.parameter_count(TableId::TypeDef, owner.row, owner.module);
                let methods = self.model.method_sig(key).ok()?.generic_parameters as usize;
                let ctx = Ctx {
                    types: Rc::from(vec![object.clone(); count]),
                    methods: Rc::from(vec![object; methods]),
                };
                match self.load_ctx(key, &ctx) {
                    Ok(_) => None,
                    Err(Fault::Unsupported(reason)) => Some(reason),
                    Err(Fault::Exception(_)) => Some("exception while loading".into()),
                }
            }
            Err(Fault::Unsupported(reason)) => Some(reason),
            Err(Fault::Exception(_)) => Some("exception while loading".into()),
        }
    }

    pub fn queries_hardware(&self, key: &MethodKey) -> bool {
        let mut seen: HashSet<MethodKey> = HashSet::new();
        let mut pending = vec![(key.clone(), 0u32)];
        while let Some((current, depth)) = pending.pop() {
            if depth > 6 || !seen.insert(current.clone()) {
                continue;
            }
            let Ok(loaded) = self.load(&current) else {
                continue;
            };
            if matches!(loaded.native, Some(Native::Intrinsic(Intrinsic::False)))
                || self.type_name(loaded.owner).starts_with("System.Numerics.Vector")
            {
                return true;
            }
            let body = crate::il::MethodBody {
                max_stack: 0,
                local_signature: Token(0),
                init_locals: false,
                code: loaded.code,
                clauses: Vec::new(),
            };
            for instruction in body.instructions().flatten() {
                if matches!(instruction.opcode.value, 0x28 | 0x6F | 0x73)
                    && let Argument::Token(token) = instruction.argument
                    && let Ok(target) = self.method_token(current.module, token)
                {
                    pending.push((target, depth + 1));
                }
            }
        }
        false
    }

    pub fn reachable_natives(&self, key: &MethodKey, limit: usize) -> Result<usize, String> {
        let describe = |fault: Fault| match fault {
            Fault::Unsupported(reason) => reason,
            Fault::Exception(_) => "exception while loading".to_owned(),
        };
        let mut seen: HashSet<InstKey> = HashSet::new();
        let mut pending = vec![(key.clone(), Ctx::default())];
        while let Some((current, ctx)) = pending.pop() {
            if !seen.insert(self.inst_key(&current, &ctx)) {
                continue;
            }
            if seen.len() > limit {
                return Err("call graph is too large to check".into());
            }
            let loaded = self.load_ctx(&current, &ctx).map_err(describe)?;
            if loaded.native.is_some() {
                continue;
            }
            let body = crate::il::MethodBody {
                max_stack: 0,
                local_signature: Token(0),
                init_locals: false,
                code: loaded.code,
                clauses: Vec::new(),
            };
            for instruction in body.instructions() {
                let instruction = instruction.map_err(|e| e.to_string())?;
                let Argument::Token(token) = instruction.argument else {
                    continue;
                };
                match instruction.opcode.value {
                    0x28 | 0x6F | 0x73 | 0xFE06..=0xFE07 => {
                        let (target, target_ctx) =
                            self.method_inst(current.module, token, &ctx).map_err(describe)?;
                        let flags = self.model.set.modules[target.module]
                            .metadata
                            .cell(TableId::MethodDef, target.row, 2)
                            .unwrap_or(0);
                        if flags & ABSTRACT == 0 {
                            pending.push((target, target_ctx));
                        }
                    }
                    0x7E..=0x80 => {
                        if let Ok(field) = self.field_ref(current.module, token, &ctx)
                            && let Gen::Named(owner, _) = &field.owner
                        {
                            let metadata = &self.model.set.modules[owner.module].metadata;
                            if let Ok(range) = metadata.method_range(owner.row) {
                                for row in range {
                                    if metadata.string_cell(TableId::MethodDef, row, 3).ok() == Some(".cctor") {
                                        pending.push((
                                            MethodKey { module: owner.module, row },
                                            field.owner.context(),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(seen.len())
    }
}

fn decode_at(code: &[u8], at: usize) -> Run<(crate::il::Instruction<'_>, usize)> {
    let tail = code
        .get(at..)
        .ok_or_else(|| Fault::Unsupported("branch outside the method".into()))?;
    let body = crate::il::MethodBody {
        max_stack: 0,
        local_signature: Token(0),
        init_locals: false,
        code: tail,
        clauses: Vec::new(),
    };
    let mut iterator = body.instructions();
    let instruction = iterator
        .next()
        .ok_or_else(|| Fault::Unsupported("method falls off its end".into()))?
        .map_err(|e| Fault::Unsupported(e.to_string()))?;
    let length = instruction_length(tail, &instruction);
    let mut instruction = instruction;
    instruction.offset += at as u32;
    instruction.argument = match instruction.argument {
        Argument::Target(t) => Argument::Target(t + at as i64),
        other => other,
    };
    Ok((instruction, at + length))
}

fn instruction_length(tail: &[u8], instruction: &crate::il::Instruction<'_>) -> usize {
    use crate::il::Operand as O;
    let prefix = if tail[0] == 0xFE { 2 } else { 1 };
    prefix
        + match instruction.opcode.operand {
            O::None => 0,
            O::I8 | O::U8 | O::Branch8 => 1,
            O::U16 => 2,
            O::I32 | O::F32 | O::Token | O::Branch32 => 4,
            O::I64 | O::F64 => 8,
            O::Switch => {
                let count = u32::from_le_bytes([tail[1], tail[2], tail[3], tail[4]]) as usize;
                4 + 4 * count
            }
        }
}

fn contains_try(clause: &ExceptionClause, offset: u32) -> bool {
    offset >= clause.try_offset && offset < clause.try_offset + clause.try_length
}

fn variable(argument: Argument<'_>) -> usize {
    match argument {
        Argument::Variable(index) => index as usize,
        _ => 0,
    }
}

fn int(argument: Argument<'_>) -> i64 {
    match argument {
        Argument::Int(value) => value,
        _ => 0,
    }
}

fn float(argument: Argument<'_>) -> f64 {
    match argument {
        Argument::Float(value) => value,
        _ => 0.0,
    }
}

fn target(argument: Argument<'_>) -> Run<usize> {
    match argument {
        Argument::Target(t) if t >= 0 => Ok(t as usize),
        _ => unsupported("branch target"),
    }
}

fn truthy(value: V) -> bool {
    match value {
        V::I4(v) => v != 0,
        V::F4(v) => v != 0.0,
        V::F8(v) => v != 0.0,
        other => other.int() != 0,
    }
}

fn compare(relation: u16, a: V, b: V) -> bool {
    if a.is_float() || b.is_float() {
        let (x, y) = (a.float(), b.float());
        let unordered = x.is_nan() || y.is_nan();
        return match relation {
            0 => x == y,
            1 => x >= y,
            2 => x > y,
            3 => x <= y,
            4 => x < y,
            5 => x != y || unordered,
            6 => x >= y || unordered,
            7 => x > y || unordered,
            8 => x <= y || unordered,
            9 => x < y || unordered,
            _ => false,
        };
    }
    let wide = !matches!((a, b), (V::I4(_), V::I4(_)));
    let (x, y) = if wide {
        (a.int(), b.int())
    } else {
        (a.int() as i32 as i64, b.int() as i32 as i64)
    };
    let (ux, uy) = if wide {
        (x as u64, y as u64)
    } else {
        (x as u32 as u64, y as u32 as u64)
    };
    match relation {
        0 => x == y,
        1 => x >= y,
        2 => x > y,
        3 => x <= y,
        4 => x < y,
        5 => x != y,
        6 => ux >= uy,
        7 => ux > uy,
        8 => ux <= uy,
        9 => ux < uy,
        _ => false,
    }
}

fn arithmetic(opcode: u16, a: V, b: V) -> Result<V, &'static str> {
    if a.is_float() || b.is_float() {
        if matches!(opcode, 0x5F..=0x61 | 0xD6..=0xDB | 0x5C | 0x5E) {
            return Err("InvalidProgramException");
        }
        if let (V::F4(x), V::F4(y)) = (a, b) {
            return Ok(V::F4(match opcode {
                0x58 => x + y,
                0x59 => x - y,
                0x5A => x * y,
                0x5B => x / y,
                _ => x % y,
            }));
        }
        let (x, y) = (a.float(), b.float());
        return Ok(V::F8(match opcode {
            0x58 => x + y,
            0x59 => x - y,
            0x5A => x * y,
            0x5B => x / y,
            _ => x % y,
        }));
    }
    let pointer = matches!(a, V::P(_) | V::O(_)) || matches!(b, V::P(_) | V::O(_));
    if let (V::I4(x), V::I4(y)) = (a, b) {
        let (ux, uy) = (x as u32, y as u32);
        return Ok(V::I4(match opcode {
            0x58 => x.wrapping_add(y),
            0x59 => x.wrapping_sub(y),
            0x5A => x.wrapping_mul(y),
            0x5B | 0x5D => {
                if y == 0 {
                    return Err("DivideByZeroException");
                }
                if x == i32::MIN && y == -1 {
                    return Err("OverflowException");
                }
                if opcode == 0x5B { x / y } else { x % y }
            }
            0x5C | 0x5E => {
                if uy == 0 {
                    return Err("DivideByZeroException");
                }
                (if opcode == 0x5C { ux / uy } else { ux % uy }) as i32
            }
            0x5F => x & y,
            0x60 => x | y,
            0x61 => x ^ y,
            0xD6 => x.checked_add(y).ok_or("OverflowException")?,
            0xD7 => ux.checked_add(uy).ok_or("OverflowException")? as i32,
            0xD8 => x.checked_mul(y).ok_or("OverflowException")?,
            0xD9 => ux.checked_mul(uy).ok_or("OverflowException")? as i32,
            0xDA => x.checked_sub(y).ok_or("OverflowException")?,
            _ => ux.checked_sub(uy).ok_or("OverflowException")? as i32,
        }));
    }
    let (x, y) = (a.int(), b.int());
    let (ux, uy) = (x as u64, y as u64);
    let result = match opcode {
        0x58 => x.wrapping_add(y),
        0x59 => x.wrapping_sub(y),
        0x5A => x.wrapping_mul(y),
        0x5B | 0x5D => {
            if y == 0 {
                return Err("DivideByZeroException");
            }
            if x == i64::MIN && y == -1 {
                return Err("OverflowException");
            }
            if opcode == 0x5B { x / y } else { x % y }
        }
        0x5C | 0x5E => {
            if uy == 0 {
                return Err("DivideByZeroException");
            }
            (if opcode == 0x5C { ux / uy } else { ux % uy }) as i64
        }
        0x5F => x & y,
        0x60 => x | y,
        0x61 => x ^ y,
        0xD6 => x.checked_add(y).ok_or("OverflowException")?,
        0xD7 => ux.checked_add(uy).ok_or("OverflowException")? as i64,
        0xD8 => x.checked_mul(y).ok_or("OverflowException")?,
        0xD9 => ux.checked_mul(uy).ok_or("OverflowException")? as i64,
        0xDA => x.checked_sub(y).ok_or("OverflowException")?,
        _ => ux.checked_sub(uy).ok_or("OverflowException")? as i64,
    };
    Ok(if pointer && matches!(opcode, 0x58 | 0x59 | 0xD7 | 0xDB) {
        V::P(result as u64)
    } else {
        V::I8(result)
    })
}

fn shift(opcode: u16, value: V, amount: u32) -> V {
    match value {
        V::I4(v) => {
            let amount = amount & 31;
            V::I4(match opcode {
                0x62 => v.wrapping_shl(amount),
                0x63 => v >> amount,
                _ => ((v as u32) >> amount) as i32,
            })
        }
        other => {
            let v = other.int();
            let amount = amount & 63;
            V::I8(match opcode {
                0x62 => v.wrapping_shl(amount),
                0x63 => v >> amount,
                _ => ((v as u64) >> amount) as i64,
            })
        }
    }
}

fn convert(opcode: u16, value: V) -> V {
    if value.is_float() {
        let x = value.float();
        return match opcode {
            0x67 => V::I4(x as i32 as i8 as i32),
            0x68 => V::I4(x as i32 as i16 as i32),
            0x69 => V::I4(x as i32),
            0x6A | 0xD3 => V::I8(x as i64),
            0x6B => V::F4(x as f32),
            0x6C => V::F8(x),
            0x6D => V::I4(x as u32 as i32),
            0x6E | 0xE0 => V::I8(x as u64 as i64),
            0x76 => V::F8(x),
            0xD1 => V::I4(x as i32 as u16 as i32),
            _ => V::I4(x as i32 as u8 as i32),
        };
    }
    let v = value.int();
    let narrow = matches!(value, V::I4(_));
    match opcode {
        0x67 => V::I4(v as i8 as i32),
        0x68 => V::I4(v as i16 as i32),
        0x69 => V::I4(v as i32),
        0x6A | 0xD3 => V::I8(v),
        0x6B => V::F4(v as f32),
        0x6C => V::F8(v as f64),
        0x6D => V::I4(v as u32 as i32),
        0x6E | 0xE0 => V::I8(if narrow { v as i32 as u32 as i64 } else { v }),
        0x76 => {
            let unsigned = if narrow { v as u32 as u64 } else { v as u64 };
            V::F8(unsigned as f64)
        }
        0xD1 => V::I4(v as u16 as i32),
        _ => V::I4(v as u8 as i32),
    }
}

fn convert_checked(opcode: u16, value: V) -> Option<V> {
    let unsigned_source = matches!(opcode, 0x82..=0x8B);
    let (low, high, wide): (i128, i128, bool) = match opcode {
        0x82 | 0xB3 => (i8::MIN as i128, i8::MAX as i128, false),
        0x83 | 0xB5 => (i16::MIN as i128, i16::MAX as i128, false),
        0x84 | 0xB7 => (i32::MIN as i128, i32::MAX as i128, false),
        0x85 | 0xB9 | 0x8A | 0xD4 => (i64::MIN as i128, i64::MAX as i128, true),
        0x86 | 0xB4 => (0, u8::MAX as i128, false),
        0x87 | 0xB6 => (0, u16::MAX as i128, false),
        0x88 | 0xB8 => (0, u32::MAX as i128, false),
        _ => (0, u64::MAX as i128, true),
    };
    let number: i128 = if value.is_float() {
        let x = value.float();
        if x.is_nan() {
            return None;
        }
        let truncated = x.trunc();
        if truncated < low as f64 - 1.0 || truncated > high as f64 + 1.0 {
            return None;
        }
        truncated as i128
    } else if unsigned_source {
        match value {
            V::I4(v) => v as u32 as i128,
            other => other.int() as u64 as i128,
        }
    } else {
        value.int() as i128
    };
    if number < low || number > high {
        return None;
    }
    Some(if wide {
        V::I8(number as u64 as i64)
    } else {
        V::I4(number as i64 as i32)
    })
}

fn var_size(var: VarType) -> u32 {
    match var {
        VarType::I8 | VarType::U8 => 1,
        VarType::I16 | VarType::U16 => 2,
        VarType::I32 | VarType::F32 => 4,
        _ => 8,
    }
}

fn read(address: u64, var: VarType) -> V {
    unsafe {
        match var {
            VarType::I8 => V::I4(i32::from(*(address as *const i8))),
            VarType::U8 => V::I4(i32::from(*(address as *const u8))),
            VarType::I16 => V::I4(i32::from(*(address as *const i16))),
            VarType::U16 => V::I4(i32::from(*(address as *const u16))),
            VarType::I32 => V::I4(*(address as *const i32)),
            VarType::I64 => V::I8(*(address as *const i64)),
            VarType::F32 => V::F4(*(address as *const f32)),
            VarType::F64 => V::F8(*(address as *const f64)),
            VarType::Ref => V::O(*(address as *const u64)),
            VarType::Pointer => V::P(*(address as *const u64)),
        }
    }
}

fn write(address: u64, var: VarType, value: V) {
    unsafe {
        match var {
            VarType::I8 | VarType::U8 => *(address as *mut u8) = value.int() as u8,
            VarType::I16 | VarType::U16 => *(address as *mut u16) = value.int() as u16,
            VarType::I32 => *(address as *mut i32) = value.int() as i32,
            VarType::F32 => *(address as *mut f32) = value.float() as f32,
            VarType::F64 => *(address as *mut f64) = value.float(),
            _ => *(address as *mut u64) = value.bits(),
        }
    }
}

fn load_raw(var: VarType, raw: u64) -> V {
    match var {
        VarType::I8 => V::I4(i32::from(raw as i8)),
        VarType::U8 => V::I4(i32::from(raw as u8)),
        VarType::I16 => V::I4(i32::from(raw as i16)),
        VarType::U16 => V::I4(i32::from(raw as u16)),
        VarType::I32 => V::I4(raw as i32),
        VarType::I64 => V::I8(raw as i64),
        VarType::F32 => V::F4(f32::from_bits(raw as u32)),
        VarType::F64 => V::F8(f64::from_bits(raw)),
        VarType::Ref => V::O(raw),
        VarType::Pointer => V::P(raw),
    }
}

fn store_raw(var: VarType, value: V) -> u64 {
    match var {
        VarType::I8 | VarType::U8 => value.int() as u8 as u64,
        VarType::I16 | VarType::U16 => value.int() as u16 as u64,
        VarType::I32 => value.int() as u32 as u64,
        VarType::F32 => u64::from((value.float() as f32).to_bits()),
        VarType::F64 => value.float().to_bits(),
        _ => value.bits(),
    }
}

fn element_core_name(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::I8 => "SByte",
        ElementKind::U8 => "Byte",
        ElementKind::I16 => "Int16",
        ElementKind::U16 => "UInt16",
        ElementKind::I32 => "Int32",
        ElementKind::I64 => "Int64",
        ElementKind::F32 => "Single",
        ElementKind::F64 => "Double",
        ElementKind::Reference => "Object",
    }
}

fn identity_hash(object: u64) -> i32 {
    if object == 0 {
        return 0;
    }
    let mixed = (object >> 3).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((mixed >> 33) as i32) & 0x3FFF_FFFF
}

fn processor_count() -> i32 {
    static COUNT: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *COUNT.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get() as i32))
}

fn unmodified(sig: &TypeSig) -> &TypeSig {
    match sig {
        TypeSig::Modified { inner, .. } => unmodified(inner),
        other => other,
    }
}

fn blittable(element: &TypeSig, unicode: bool) -> bool {
    matches!(
        unmodified(element),
        TypeSig::I1
            | TypeSig::U1
            | TypeSig::I2
            | TypeSig::U2
            | TypeSig::I4
            | TypeSig::U4
            | TypeSig::I8
            | TypeSig::U8
            | TypeSig::R4
            | TypeSig::R8
            | TypeSig::IntPtr
            | TypeSig::UIntPtr
    ) || (unicode && matches!(unmodified(element), TypeSig::Char))
}

fn words(bytes: &[u8]) -> Vec<u64> {
    let mut words = vec![0u64; bytes.len().div_ceil(8).max(1)];
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), words.as_mut_ptr().cast(), bytes.len());
    }
    words
}

fn coerce(value: V, var: VarType) -> V {
    load_raw(var, store_raw(var, value))
}

fn string_units<'a>(string: u64) -> &'a [u16] {
    unsafe {
        let length = *((string + 8) as *const u32) as usize;
        std::slice::from_raw_parts((string + u64::from(STRING_DATA)) as *const u16, length)
    }
}

pub fn result_raw(var: Option<VarType>, value: Option<V>) -> [u64; 2] {
    match (var, value) {
        (Some(VarType::F32), Some(v)) => [0, u64::from((v.float() as f32).to_bits())],
        (Some(VarType::F64), Some(v)) => [0, v.float().to_bits()],
        (Some(var), Some(v)) => [store_raw(var, v), 0],
        _ => [0, 0],
    }
}

pub fn method_returns(interpreter: &Interpreter<'_>, key: &MethodKey) -> Option<VarType> {
    interpreter.load(key).ok().and_then(|l| l.returns.as_ref().and_then(Ty::prim))
}

mod accessors;
mod arrays;
mod attributes;
mod com;
mod marshal;
mod members;
mod reflection;
mod threading;

mod crt {
    use std::sync::OnceLock;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryA(name: *const u8) -> usize;
        fn GetProcAddress(module: usize, name: *const u8) -> usize;
    }

    fn library() -> usize {
        static LIBRARY: OnceLock<usize> = OnceLock::new();
        *LIBRARY.get_or_init(|| unsafe { LoadLibraryA(c"ucrtbase.dll".as_ptr().cast()) })
    }

    fn symbol(name: &std::ffi::CStr, cell: &OnceLock<usize>) -> usize {
        *cell.get_or_init(|| match library() {
            0 => 0,
            module => unsafe { GetProcAddress(module, name.as_ptr().cast()) },
        })
    }

    macro_rules! wrap {
        ($($name:ident: fn($($argument:ident),*) -> $kind:ty;)*) => {
            $(pub fn $name($($argument: $kind),*) -> $kind {
                static CELL: OnceLock<usize> = OnceLock::new();
                let address = symbol(
                    std::ffi::CStr::from_bytes_with_nul(concat!(stringify!($name), " ").as_bytes()).unwrap_or(c"?"),
                    &CELL,
                );
                if address == 0 {
                    return <$kind>::NAN;
                }
                let function: extern "C" fn($($argument: $kind),*) -> $kind =
                    unsafe { std::mem::transmute(address) };
                function($($argument),*)
            })*
        };
    }
    wrap! {
        sin: fn(x) -> f64;
        cos: fn(x) -> f64;
        tan: fn(x) -> f64;
        asin: fn(x) -> f64;
        acos: fn(x) -> f64;
        atan: fn(x) -> f64;
        sinh: fn(x) -> f64;
        cosh: fn(x) -> f64;
        tanh: fn(x) -> f64;
        asinh: fn(x) -> f64;
        acosh: fn(x) -> f64;
        atanh: fn(x) -> f64;
        exp: fn(x) -> f64;
        log: fn(x) -> f64;
        log10: fn(x) -> f64;
        log2: fn(x) -> f64;
        cbrt: fn(x) -> f64;
        pow: fn(x, y) -> f64;
        atan2: fn(y, x) -> f64;
        fmod: fn(x, y) -> f64;
        sinf: fn(x) -> f32;
        cosf: fn(x) -> f32;
        tanf: fn(x) -> f32;
        asinf: fn(x) -> f32;
        acosf: fn(x) -> f32;
        atanf: fn(x) -> f32;
        sinhf: fn(x) -> f32;
        coshf: fn(x) -> f32;
        tanhf: fn(x) -> f32;
        asinhf: fn(x) -> f32;
        acoshf: fn(x) -> f32;
        atanhf: fn(x) -> f32;
        expf: fn(x) -> f32;
        logf: fn(x) -> f32;
        log10f: fn(x) -> f32;
        log2f: fn(x) -> f32;
        cbrtf: fn(x) -> f32;
        powf: fn(x, y) -> f32;
        atan2f: fn(y, x) -> f32;
        fmodf: fn(x, y) -> f32;
        fma: fn(x, y, z) -> f64;
        fmaf: fn(x, y, z) -> f32;
    }
}

fn reciprocal(value: f64) -> f64 {
    1.0 / value
}

fn reciprocal_sqrt(value: f64) -> f64 {
    1.0 / value.sqrt()
}

fn reciprocal_single(value: f32) -> f32 {
    use std::arch::x86_64::{_mm_cvtss_f32, _mm_rcp_ss, _mm_set_ss};
    unsafe { _mm_cvtss_f32(_mm_rcp_ss(_mm_set_ss(value))) }
}

fn reciprocal_sqrt_single(value: f32) -> f32 {
    use std::arch::x86_64::{_mm_cvtss_f32, _mm_rsqrt_ss, _mm_set_ss};
    unsafe { _mm_cvtss_f32(_mm_rsqrt_ss(_mm_set_ss(value))) }
}

fn array_component(array: u64) -> u64 {
    let record = unsafe { *(array as *const u64) } & !1;
    unsafe { *(record as *const u64) >> 32 }
}

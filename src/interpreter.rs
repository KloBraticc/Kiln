use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::codegen::{MethodKey, VarType};
use crate::il::{Argument, ClauseKind, ExceptionClause, MethodBody};
use crate::jit::Image;
use crate::loader::TypeHandle;
use crate::members::Member;
use crate::metadata::{TableId, Token};
use crate::objects::{ARRAY_DATA, ElementKind, ObjectModel, RuntimeType, STRING_DATA};
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
}

impl V {
    fn bits(self) -> u64 {
        match self {
            V::I4(v) => v as i64 as u64,
            V::I8(v) => v as u64,
            V::F4(v) => u64::from(v.to_bits()),
            V::F8(v) => v.to_bits(),
            V::O(v) | V::P(v) => v,
        }
    }

    fn int(self) -> i64 {
        match self {
            V::I4(v) => i64::from(v),
            V::I8(v) => v,
            V::O(v) | V::P(v) => v as i64,
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
            V::O(v) | V::P(v) => v,
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
    Err(Fault::Unsupported(what.into()))
}

fn map<T>(result: Result<T, String>) -> Run<T> {
    result.map_err(Fault::Unsupported)
}

#[derive(Clone, Copy)]
enum Native {
    Intrinsic(Intrinsic),
    DelegateConstruct,
    DelegateInvoke,
}

#[derive(Clone, Copy)]
enum Intrinsic {
    False,
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
}

type UnaryPair = (fn(f64) -> f64, fn(f32) -> f32);
type BinaryPair = (fn(f64, f64) -> f64, fn(f32, f32) -> f32);

struct Loaded {
    key: MethodKey,
    owner: TypeHandle,
    code: &'static [u8],
    clauses: Vec<ExceptionClause>,
    parameters: Vec<VarType>,
    returns: Option<VarType>,
    locals: Vec<VarType>,
    native: Option<Native>,
    is_static: bool,
    is_virtual: bool,
}

struct Frame {
    slots: Box<[u64]>,
    kinds: Vec<VarType>,
    stack: Vec<V>,
    caught: Vec<u64>,
    scratch: Vec<Box<[u8]>>,
}

static FRAMES: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
static PINNED: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

fn frames() -> std::sync::MutexGuard<'static, Vec<usize>> {
    FRAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn pinned() -> std::sync::MutexGuard<'static, Vec<u64>> {
    PINNED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn scan_roots(out: &mut Vec<u64>) {
    {
        for &frame in frames().iter() {
            let frame = unsafe { &*(frame as *const Frame) };
            for (index, kind) in frame.kinds.iter().enumerate() {
                if *kind == VarType::Ref {
                    out.push(frame.slots[index]);
                }
            }
            for value in &frame.stack {
                if let V::O(object) = value {
                    out.push(*object);
                }
            }
            out.extend_from_slice(&frame.caught);
        }
    }
    out.extend_from_slice(&pinned());
}

struct Pin(usize);

impl Pin {
    fn new(values: &[V]) -> Self {
        let mut pinned = pinned();
        let mark = pinned.len();
        pinned.extend(values.iter().filter_map(|v| match v {
            V::O(o) => Some(*o),
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

enum Exit {
    Return(Option<V>),
    EndFinally,
    EndFilter(bool),
    Leave(u32, u32),
}

pub struct Interpreter<'s> {
    pub model: &'s ObjectModel<'s>,
    image: Cell<*const Image>,
    loaded: RefCell<HashMap<MethodKey, Rc<Loaded>>>,
    interfaces: RefCell<HashMap<usize, HashMap<u32, Vec<Token>>>>,
    implementations: RefCell<HashMap<(u64, MethodKey), MethodKey>>,
    names: RefCell<HashMap<TypeHandle, String>>,
    resources: RefCell<HashMap<usize, HashMap<String, String>>>,
    failed_types: RefCell<HashMap<TypeHandle, Option<String>>>,
    depth: Cell<usize>,
    pub interpreted: Cell<u64>,
    pub compiled_calls: Cell<u64>,
    pending: RefCell<Option<Pending>>,
    pub budget: Cell<u64>,
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
        unsafe { &*(*bridge).interpreter }
            .pending
            .borrow_mut()
            .get_or_insert(Pending::Throw(name));
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
        None => eprintln!("fault {code:#x} reading {address:#x} outside compiled code at {rip:#x}"),
    }
    CONTINUE_SEARCH
}

pub fn install_fault_report() {
    unsafe { AddVectoredExceptionHandler(0, report_fault) };
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
    keys: RefCell<Vec<MethodKey>>,
    stubs: RefCell<HashMap<MethodKey, u64>>,
}

fn resolve_stub(key: &MethodKey) -> Option<u64> {
    let bridge = BRIDGE.load(std::sync::atomic::Ordering::Acquire);
    if bridge.is_null() {
        return None;
    }
    unsafe { &*(*bridge).interpreter }.stub_for(key)
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
    let Some(key) = bridge.keys.borrow().get(index as usize).cloned() else {
        runtime::trap();
        return 0;
    };
    let key = &key;
    let loaded = match interpreter.load(key) {
        Ok(loaded) => loaded,
        Err(fault) => abort_compiled(interpreter, frame, fault),
    };
    let values: Vec<V> = loaded
        .parameters
        .iter()
        .enumerate()
        .map(|(position, var)| load_raw(*var, unsafe { *arguments.add(position) }))
        .collect();
    runtime::state().transitions.push((frame, stack));
    let outcome = interpreter.invoke(key, &values);
    runtime::state().transitions.pop();
    match outcome {
        Ok(value) => {
            let raw = result_raw(loaded.returns, value);
            if matches!(loaded.returns, Some(VarType::F32 | VarType::F64)) {
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
        let state = runtime::install();
        state.root_hook = Some(scan_roots);
        Self {
            model,
            image: Cell::new(image.map_or(std::ptr::null(), |i| i as *const Image)),
            loaded: RefCell::new(HashMap::new()),
            interfaces: RefCell::new(HashMap::new()),
            implementations: RefCell::new(HashMap::new()),
            names: RefCell::new(HashMap::new()),
            resources: RefCell::new(HashMap::new()),
            failed_types: RefCell::new(HashMap::new()),
            depth: Cell::new(0),
            interpreted: Cell::new(0),
            compiled_calls: Cell::new(0),
            pending: RefCell::new(None),
            budget: Cell::new(u64::MAX),
        }
    }

    pub fn install_bridge(&self) {
        let bridge = Box::new(Bridge {
            interpreter: (self as *const Interpreter<'_>).cast(),
            keys: RefCell::new(Vec::new()),
            stubs: RefCell::new(HashMap::new()),
        });
        BRIDGE.store(Box::into_raw(bridge), std::sync::atomic::Ordering::Release);
        runtime::state().late_resolver = Some(resolve_stub);
        unsafe { AddVectoredExceptionHandler(1, hardware_fault) };
    }

    pub fn attach(&self, image: &Image) {
        self.image.set(image as *const Image);
    }

    pub fn stub_for(&self, key: &MethodKey) -> Option<u64> {
        let bridge = BRIDGE.load(std::sync::atomic::Ordering::Acquire);
        if bridge.is_null() {
            return None;
        }
        let bridge = unsafe { &*bridge };
        if let Some(address) = bridge.stubs.borrow().get(key) {
            return Some(*address);
        }
        let loaded = self.load(key).ok()?;
        let index = bridge.keys.borrow().len() as u64;
        let address = crate::jit::emit_fallback_stub(&loaded.parameters, index)?;
        bridge.keys.borrow_mut().push(key.clone());
        bridge.stubs.borrow_mut().insert(key.clone(), address);
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
        let handle = self.corelib_type("System", name)?;
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
        let record = unsafe { *(object as *const u64) } & !1;
        let identity = unsafe { *((record + 40) as *const u64) };
        if identity != u64::MAX {
            return Ok(TypeHandle {
                module: (identity >> 32) as usize,
                row: identity as u32,
            });
        }
        let flags = unsafe { *((record + 8) as *const u64) };
        if flags & runtime::FLAG_STRING != 0 {
            self.corelib_type("System", "String")
        } else {
            self.corelib_type("System", "Array")
        }
    }

    fn load(&self, key: &MethodKey) -> Run<Rc<Loaded>> {
        if let Some(loaded) = self.loaded.borrow().get(key) {
            return Ok(loaded.clone());
        }
        let loaded = Rc::new(self.load_uncached(key)?);
        self.loaded.borrow_mut().insert(key.clone(), loaded.clone());
        Ok(loaded)
    }

    fn signature_var(&self, module: usize, sig: &TypeSig) -> Run<VarType> {
        match map(self.model.var_type(module, sig))? {
            Some(var) => Ok(var),
            None => unsupported(format!("value of type {sig:?}")),
        }
    }

    fn load_uncached(&self, key: &MethodKey) -> Run<Loaded> {
        let set = self.model.set;
        let module = &set.modules[key.module];
        let metadata = &module.metadata;
        let owner = map(self.model.method_owner(key))?;
        if self.model.is_generic(owner) {
            return unsupported("methods of generic types");
        }
        let sig = map(self.model.method_sig(key))?;
        if sig.generic_parameters != 0 {
            return unsupported("generic methods");
        }
        if sig.convention != CallingConvention::Default {
            return unsupported("non default calling convention");
        }
        let flags = metadata.cell(TableId::MethodDef, key.row, 2).unwrap_or(0);
        let implementation = metadata.cell(TableId::MethodDef, key.row, 1).unwrap_or(0);
        let rva = metadata.cell(TableId::MethodDef, key.row, 0).unwrap_or(0);
        let owner_is_value = set.is_value_type(owner).unwrap_or(false);
        let mut parameters = Vec::new();
        if sig.has_this {
            parameters.push(if owner_is_value {
                VarType::Pointer
            } else {
                VarType::Ref
            });
        }
        for parameter in &sig.parameters {
            parameters.push(self.signature_var(key.module, parameter)?);
        }
        let returns = match &sig.return_type {
            TypeSig::Void => None,
            TypeSig::Modified { inner, .. } if **inner == TypeSig::Void => None,
            other => Some(self.signature_var(key.module, other)?),
        };
        let name = metadata.string_cell(TableId::MethodDef, key.row, 3).unwrap_or("");
        let native = self.native(owner, name, &parameters, returns);
        let mut code: &'static [u8] = &[];
        let mut clauses = Vec::new();
        let mut locals = Vec::new();
        if native.is_none() {
            if flags & PINVOKE != 0 {
                return unsupported("platform invoke");
            }
            if implementation & INTERNAL_CALL != 0 || implementation & RUNTIME_IMPL != 0 {
                return unsupported(format!(
                    "internal call {}::{name}",
                    self.type_name(owner)
                ));
            }
            if rva == 0 {
                return unsupported(if flags & ABSTRACT != 0 {
                    "abstract method".to_owned()
                } else {
                    format!("method without a body {}::{name}", self.type_name(owner))
                });
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
                            TypeSig::Pinned(inner) => self.signature_var(key.module, inner)?,
                            other => self.signature_var(key.module, other)?,
                        });
                    }
                }
            }
            code = body.code;
            clauses = body.clauses;
        }
        Ok(Loaded {
            key: key.clone(),
            owner,
            code,
            clauses,
            parameters,
            returns,
            locals,
            native,
            is_static: flags & STATIC != 0,
            is_virtual: flags & VIRTUAL != 0,
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
        let float = parameters.first() == Some(&VarType::F32);
        if self.is_delegate(owner) && type_name != "System.Delegate" && type_name != "System.MulticastDelegate" {
            return match name {
                ".ctor" => Some(Native::DelegateConstruct),
                "Invoke" => Some(Native::DelegateInvoke),
                _ => None,
            };
        }
        let intrinsic = match (type_name.as_str(), name) {
            (t, "get_IsSupported") if t.starts_with("System.Runtime.Intrinsics.") => {
                Intrinsic::False
            }
            (t, "get_IsHardwareAccelerated") if t.starts_with("System.Runtime.Intrinsics.") => {
                Intrinsic::False
            }
            (t, "UsingResourceKeys") if t == "System.SR" || t.ends_with(".SR") => Intrinsic::False,
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
            _ => {
                if (type_name == "System.SR" || type_name.ends_with(".SR"))
                    && name == "GetResourceString"
                    && (1..=2).contains(&parameters.len())
                {
                    Intrinsic::ResourceString(owner.module)
                } else {
                    return None;
                }
            }
        };
        Some(Native::Intrinsic(intrinsic))
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
        let _pin = Pin::new(arguments);
        let loaded = self.load(key)?;
        if arguments.len() != loaded.parameters.len() {
            return unsupported("argument count mismatch");
        }
        if let Some(native) = loaded.native {
            return self.run_native(&loaded, native, arguments);
        }
        let image = self.image.get();
        if !image.is_null()
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
        Some(Ok(loaded.returns.map(|var| match var {
            VarType::F32 => V::F4(f32::from_bits(result[1] as u32)),
            VarType::F64 => V::F8(f64::from_bits(result[1])),
            other => load_raw(other, result[0]),
        })))
    }

    fn run_native(&self, loaded: &Loaded, native: Native, arguments: &[V]) -> Run<Option<V>> {
        match native {
            Native::Intrinsic(intrinsic) => self.intrinsic(intrinsic, arguments),
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
                let (target, function) = self.delegate_fields()?;
                let (target, function) = unsafe {
                    (
                        *((this + target) as *const u64),
                        *((this + function) as *const u64),
                    )
                };
                let key = decode_function(function)
                    .ok_or_else(|| Fault::Unsupported("delegate to native code".into()))?;
                let callee = self.load(&key)?;
                let mut forwarded = Vec::with_capacity(arguments.len());
                if !callee.is_static {
                    forwarded.push(V::O(target));
                }
                forwarded.extend_from_slice(&arguments[1..]);
                let result = self.invoke(&key, &forwarded)?;
                Ok(result.map(|v| coerce(v, loaded.returns.unwrap_or(VarType::I64))))
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

    fn intrinsic(&self, intrinsic: Intrinsic, a: &[V]) -> Run<Option<V>> {
        Ok(Some(match intrinsic {
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
                    None if a.len() == 2 => a[1],
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

    fn ensure_initialized(&self, owner: TypeHandle) -> Run<()> {
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
        unsafe { *flag = 1 };
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
                    unsafe { *flag = 0 };
                    self.failed_types.borrow_mut().insert(owner, Some(reason.clone()));
                    return unsupported(reason);
                }
                Err(Fault::Exception(_)) => {
                    unsafe { *flag = 0 };
                    self.failed_types.borrow_mut().insert(owner, None);
                    return self.throw("TypeInitializationException");
                }
            }
        }
        Ok(())
    }

    fn interpret(&self, loaded: &Loaded, arguments: &[V]) -> Run<Option<V>> {
        let depth = self.depth.get();
        if depth >= MAX_DEPTH {
            return unsupported("interpreter call depth");
        }
        self.depth.set(depth + 1);
        self.interpreted.set(self.interpreted.get() + 1);
        let count = loaded.parameters.len() + loaded.locals.len();
        let mut kinds = loaded.parameters.clone();
        kinds.extend_from_slice(&loaded.locals);
        let mut frame = Box::new(Frame {
            slots: vec![0u64; count].into_boxed_slice(),
            kinds,
            stack: Vec::new(),
            caught: Vec::new(),
            scratch: Vec::new(),
        });
        for (index, value) in arguments.iter().enumerate() {
            frame.slots[index] = store_raw(loaded.parameters[index], *value);
        }
        let registered = Registered::new(&frame);
        let result = self.execute(loaded, &mut frame, 0);
        drop(registered);
        self.depth.set(depth);
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
        let token = Token(token);
        let handle = match self.model.set.resolve_type_token(method.key.module, token) {
            Ok(handle) => handle,
            Err(_) => return unsupported("catch type does not resolve"),
        };
        Ok(self.is_assignable(object, handle))
    }

    fn is_assignable(&self, object: u64, target: TypeHandle) -> bool {
        let Ok(class) = self.object_class(object) else {
            return false;
        };
        let name = self.type_name(target);
        if name == "System.Object" {
            return true;
        }
        let mut current = Some(class);
        let mut depth = 0;
        while let Some(handle) = current {
            if handle == target || self.implements(handle, target) {
                return true;
            }
            depth += 1;
            if depth > 64 {
                break;
            }
            current = self.model.set.base_type(handle).ok().flatten();
        }
        false
    }

    fn implements(&self, handle: TypeHandle, interface: TypeHandle) -> bool {
        let tokens = self.interface_tokens(handle);
        for token in tokens {
            if let Ok(found) = self.model.set.resolve_type_token(handle.module, token)
                && (found == interface || self.implements(found, interface))
            {
                return true;
            }
        }
        false
    }

    fn interface_tokens(&self, handle: TypeHandle) -> Vec<Token> {
        let mut cache = self.interfaces.borrow_mut();
        let index = cache.entry(handle.module).or_insert_with(|| {
            let metadata = &self.model.set.modules[handle.module].metadata;
            let mut index: HashMap<u32, Vec<Token>> = HashMap::new();
            for row in 1..=metadata.rows(TableId::InterfaceImpl) {
                if let (Ok(class), Ok(Some(interface))) = (
                    metadata.cell(TableId::InterfaceImpl, row, 0),
                    metadata.coded(TableId::InterfaceImpl, row, 1),
                ) && interface.table() != Some(TableId::TypeSpec)
                {
                    index.entry(class).or_default().push(interface);
                }
            }
            index
        });
        index.get(&handle.row).cloned().unwrap_or_default()
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

    fn resolve_virtual(&self, key: &MethodKey, this: u64) -> Run<MethodKey> {
        let record = unsafe { *(this as *const u64) } & !1;
        if let Some(found) = self.implementations.borrow().get(&(record, key.clone())) {
            return Ok(found.clone());
        }
        let class = self.object_class(this)?;
        let owner = map(self.model.method_owner(key))?;
        let owner_info = map(self.model.class(owner))?;
        let found = if owner_info.is_interface {
            self.find_interface_implementation(class, key)?
        } else {
            let slot = map(self.model.virtual_slot(key))?;
            let info = map(self.model.class(class))?;
            info.vtable
                .get(slot)
                .cloned()
                .flatten()
                .ok_or_else(|| Fault::Unsupported("virtual slot is empty".into()))?
        };
        self.implementations
            .borrow_mut()
            .insert((record, key.clone()), found.clone());
        Ok(found)
    }

    fn find_interface_implementation(&self, class: TypeHandle, key: &MethodKey) -> Run<MethodKey> {
        let set = self.model.set;
        let name = set.modules[key.module]
            .metadata
            .string_cell(TableId::MethodDef, key.row, 3)
            .unwrap_or("");
        let sig = map(self.model.method_sig(key))?;
        let mut chain = Vec::new();
        let mut current = Some(class);
        while let Some(handle) = current {
            chain.push(handle);
            if chain.len() > 64 {
                break;
            }
            current = set.base_type(handle).ok().flatten();
        }
        for handle in &chain {
            let metadata = &set.modules[handle.module].metadata;
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
                if self.model.method_key(handle.module, declaration).as_ref() == Some(key)
                    && let Some(body) = self.model.method_key(handle.module, body)
                {
                    return Ok(body);
                }
            }
        }
        for handle in &chain {
            let metadata = &set.modules[handle.module].metadata;
            let Ok(range) = metadata.method_range(handle.row) else {
                continue;
            };
            for row in range {
                if metadata.string_cell(TableId::MethodDef, row, 3).ok() != Some(name) {
                    continue;
                }
                let candidate = MethodKey {
                    module: handle.module,
                    row,
                };
                let flags = metadata.cell(TableId::MethodDef, row, 2).unwrap_or(0);
                if flags & VIRTUAL == 0 {
                    continue;
                }
                let Ok(candidate_sig) = self.model.method_sig(&candidate) else {
                    continue;
                };
                if set
                    .methods_equal(handle.module, &candidate_sig, key.module, &sig)
                    .unwrap_or(false)
                {
                    return Ok(candidate);
                }
            }
        }
        let flags = set.modules[key.module]
            .metadata
            .cell(TableId::MethodDef, key.row, 2)
            .unwrap_or(0);
        if flags & ABSTRACT == 0 {
            return Ok(key.clone());
        }
        unsupported("interface method has no implementation")
    }

    fn field_address(&self, method: &Loaded, token: Token, target: V) -> Run<(u64, VarType)> {
        let field = map(self.model.field(method.key.module, token))?;
        let owner_is_value = self.model.set.is_value_type(field.owner).unwrap_or(false);
        if field.is_static {
            self.ensure_initialized(field.owner)?;
            let address = if field.has_rva {
                map(runtime::rva_data(self.model.set, field.module, field.row))?
            } else {
                runtime::state().static_slot(self.model, field.module, field.row, field.var == VarType::Ref)
            };
            return Ok((address, field.var));
        }
        let offset = if owner_is_value {
            let (_, placements) = self
                .model
                .set
                .value_type_fields(field.owner)
                .map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
            u64::from(
                placements
                    .iter()
                    .find(|p| p.field == field.row)
                    .map(|p| p.offset)
                    .ok_or_else(|| Fault::Unsupported("value type field".into()))?,
            )
        } else {
            u64::from(map(self.model.instance_offset(&field))?)
        };
        let base = match target {
            V::O(object) => {
                if object == 0 {
                    return self.throw("NullReferenceException");
                }
                if owner_is_value { object + 8 } else { object }
            }
            other => match other.address() {
                0 => return self.throw("NullReferenceException"),
                pointer => pointer,
            },
        };
        Ok((base + offset, field.var))
    }

    fn type_var(&self, module: usize, token: Token) -> Run<(VarType, Option<TypeHandle>)> {
        match token.table() {
            Some(TableId::TypeSpec) => {
                let metadata = &self.model.set.modules[module].metadata;
                let sig = metadata
                    .cell(TableId::TypeSpec, token.row(), 0)
                    .and_then(|b| metadata.blob(b))
                    .and_then(Signature::parse_type)
                    .map_err(|e| Fault::Unsupported(e.to_string()))?;
                Ok((self.signature_var(module, &sig)?, None))
            }
            _ => {
                let handle = self
                    .model
                    .set
                    .resolve_type_token(module, token)
                    .map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
                if !self.model.set.is_value_type(handle).unwrap_or(false) {
                    return Ok((VarType::Ref, Some(handle)));
                }
                let kind = map(self.model.element_kind(module, token))?;
                Ok((kind.var(), Some(handle)))
            }
        }
    }

    fn cast_target(&self, module: usize, token: Token) -> Run<CastTarget> {
        if token.table() == Some(TableId::TypeSpec) {
            let metadata = &self.model.set.modules[module].metadata;
            let sig = metadata
                .cell(TableId::TypeSpec, token.row(), 0)
                .and_then(|b| metadata.blob(b))
                .and_then(Signature::parse_type)
                .map_err(|e| Fault::Unsupported(e.to_string()))?;
            return match sig {
                TypeSig::SzArray(inner) => {
                    let kind = match self.model.var_type(module, &inner) {
                        Ok(Some(var)) => ElementKind::from_var(var),
                        _ => None,
                    };
                    match kind {
                        Some(kind) => Ok(CastTarget::Array(kind)),
                        None => unsupported("cast to a struct array"),
                    }
                }
                _ => unsupported("cast to a generic instantiation"),
            };
        }
        let handle = self
            .model
            .set
            .resolve_type_token(module, token)
            .map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
        Ok(CastTarget::Type(handle))
    }

    fn matches_cast(&self, object: u64, target: &CastTarget) -> Run<bool> {
        Ok(match target {
            CastTarget::Type(handle) => {
                let name = self.type_name(*handle);
                let record = unsafe { *(object as *const u64) } & !1;
                let flags = unsafe { *((record + 8) as *const u64) };
                if flags & runtime::FLAG_ARRAY != 0 {
                    matches!(
                        name.as_str(),
                        "System.Object"
                            | "System.Array"
                            | "System.ICloneable"
                            | "System.Collections.IList"
                            | "System.Collections.ICollection"
                            | "System.Collections.IEnumerable"
                    )
                } else {
                    self.is_assignable(object, *handle)
                }
            }
            CastTarget::Array(kind) => {
                let record = unsafe { *(object as *const u64) } & !1;
                record == self.record(RuntimeType::Array(*kind))?
            }
        })
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
                0x02..=0x09 => {
                    let index = if opcode <= 0x05 {
                        (opcode - 0x02) as usize
                    } else {
                        method.parameters.len() + (opcode - 0x06) as usize
                    };
                    let kind = *frame.kinds.get(index).ok_or_else(|| Fault::Unsupported("variable index".into()))?;
                    frame.stack.push(load_raw(kind, frame.slots[index]));
                }
                0x0A..=0x0D => {
                    let index = method.parameters.len() + (opcode - 0x0A) as usize;
                    let value = pop!();
                    let kind = *frame.kinds.get(index).ok_or_else(|| Fault::Unsupported("variable index".into()))?;
                    frame.slots[index] = store_raw(kind, value);
                }
                0x0E | 0xFE09 | 0x11 | 0xFE0C => {
                    let index = variable(argument) + if matches!(opcode, 0x11 | 0xFE0C) { method.parameters.len() } else { 0 };
                    let kind = *frame.kinds.get(index).ok_or_else(|| Fault::Unsupported("variable index".into()))?;
                    frame.stack.push(load_raw(kind, frame.slots[index]));
                }
                0x0F | 0xFE0A | 0x12 | 0xFE0D => {
                    let index = variable(argument) + if matches!(opcode, 0x12 | 0xFE0D) { method.parameters.len() } else { 0 };
                    if index >= frame.slots.len() {
                        return unsupported("variable index");
                    }
                    frame.stack.push(V::P(&frame.slots[index] as *const u64 as u64));
                }
                0x10 | 0xFE0B | 0x13 | 0xFE0E => {
                    let index = variable(argument) + if matches!(opcode, 0x13 | 0xFE0E) { method.parameters.len() } else { 0 };
                    let value = pop!();
                    let kind = *frame.kinds.get(index).ok_or_else(|| Fault::Unsupported("variable index".into()))?;
                    frame.slots[index] = store_raw(kind, value);
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
                    let value = match method.returns {
                        Some(var) => Some(coerce(pop!(), var)),
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
                    let key = self.method_token(module, token)?;
                    let callee = self.load(&key)?;
                    if opcode == 0x73 {
                        self.construct(frame, &callee)?;
                    } else {
                        if callee.is_static {
                            self.ensure_initialized(callee.owner)?;
                        }
                        let count = callee.parameters.len();
                        if frame.stack.len() < count {
                            return unsupported("evaluation stack underflow");
                        }
                        let mut arguments: Vec<V> = frame.stack.split_off(frame.stack.len() - count);
                        let _pin = Pin::new(&arguments);
                        let mut target = key.clone();
                        if let Some(prefix) = constrained.take() {
                            let (var, handle) = self.type_var(module, prefix)?;
                            if var == VarType::Ref {
                                arguments[0] = V::O(read(arguments[0].address(), VarType::Ref).address());
                            } else if let Some(handle) = handle {
                                match self.find_value_override(handle, &key)? {
                                    Some(found) => target = found,
                                    None => {
                                        let boxed = self.box_value(handle, var, read(arguments[0].address(), var))?;
                                        arguments[0] = V::O(boxed);
                                    }
                                }
                            }
                        }
                        if opcode == 0x6F
                            && !callee.is_static
                            && let V::O(this) = arguments[0]
                        {
                            if this == 0 {
                                return self.throw("NullReferenceException");
                            }
                            if callee.is_virtual && target == key {
                                target = self.resolve_virtual(&key, this)?;
                            }
                        }
                        let result = self.invoke(&target, &arguments)?;
                        if let Some(value) = result {
                            frame.stack.push(value);
                        }
                    }
                }
                0x29 => return unsupported("calli"),
                0x27 => return unsupported("jmp"),
                0x74 | 0x75 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("cast token");
                    };
                    let value = pop!();
                    let object = value.address();
                    let target = self.cast_target(module, token)?;
                    let ok = object == 0 || self.matches_cast(object, &target)?;
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
                    let (address, var) = self.field_address(method, token, target)?;
                    let value = if matches!(opcode, 0x7C | 0x7F) {
                        V::P(address)
                    } else {
                        read(address, var)
                    };
                    frame.stack.push(value);
                }
                0x7D | 0x80 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("field token");
                    };
                    let value = pop!();
                    let target = if opcode == 0x7D { pop!() } else { V::O(0) };
                    let _pin = Pin::new(&[value]);
                    let (address, var) = self.field_address(method, token, target)?;
                    write(address, var, value);
                }
                0x71 | 0x81 | 0x70 | 0xA5 | 0x79 | 0x8C | 0xFE15 | 0xFE1C => {
                    let Argument::Token(token) = argument else {
                        return unsupported("type token");
                    };
                    let (var, handle) = self.type_var(module, token)?;
                    match opcode {
                        0x71 => {
                            let address = pop!().address();
                            if address == 0 {
                                return self.throw("NullReferenceException");
                            }
                            frame.stack.push(read(address, var));
                        }
                        0x81 => {
                            let value = pop!();
                            let address = pop!().address();
                            if address == 0 {
                                return self.throw("NullReferenceException");
                            }
                            write(address, var, value);
                        }
                        0x70 => {
                            let source = pop!().address();
                            let destination = pop!().address();
                            write(destination, var, read(source, var));
                        }
                        0xFE15 => {
                            let address = pop!().address();
                            write(address, var, if var == VarType::Ref { V::O(0) } else { V::I8(0) });
                        }
                        0xFE1C => frame.stack.push(V::I4(var_size(var) as i32)),
                        0x8C => {
                            let value = pop!();
                            if var == VarType::Ref {
                                frame.stack.push(value);
                            } else {
                                let handle = handle.ok_or_else(|| Fault::Unsupported("box of a generic instantiation".into()))?;
                                let boxed = self.box_value(handle, var, value)?;
                                frame.stack.push(V::O(boxed));
                            }
                        }
                        _ => {
                            let object = pop!().address();
                            if var == VarType::Ref {
                                if opcode == 0x79 {
                                    return unsupported("unbox of a reference type");
                                }
                                let target = self.cast_target(module, token)?;
                                if object != 0 && !self.matches_cast(object, &target)? {
                                    return self.throw("InvalidCastException");
                                }
                                frame.stack.push(V::O(object));
                            } else {
                                if object == 0 {
                                    return self.throw("NullReferenceException");
                                }
                                let handle = handle.ok_or_else(|| Fault::Unsupported("unbox of a generic instantiation".into()))?;
                                if !self.unbox_compatible(object, handle)? {
                                    return self.throw("InvalidCastException");
                                }
                                frame.stack.push(if opcode == 0x79 {
                                    V::P(object + 8)
                                } else {
                                    read(object + 8, var)
                                });
                            }
                        }
                    }
                }
                0x8D => {
                    let Argument::Token(token) = argument else {
                        return unsupported("array token");
                    };
                    let kind = map(self.model.element_kind(module, token))?;
                    let count = pop!().int();
                    if count < 0 {
                        return self.throw("OverflowException");
                    }
                    let record = self.record(RuntimeType::Array(kind))?;
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
                    let kind = match opcode {
                        0x90 => ElementKind::I8,
                        0x91 => ElementKind::U8,
                        0x92 => ElementKind::I16,
                        0x93 => ElementKind::U16,
                        0x94 | 0x95 => ElementKind::I32,
                        0x96 | 0x97 => ElementKind::I64,
                        0x98 => ElementKind::F32,
                        0x99 => ElementKind::F64,
                        0x9A => ElementKind::Reference,
                        _ => {
                            let Argument::Token(token) = argument else {
                                return unsupported("element token");
                            };
                            map(self.model.element_kind(module, token))?
                        }
                    };
                    let index = pop!().int();
                    let array = pop!().address();
                    let address = self.element(array, index, kind)?;
                    let value = if opcode == 0x8F {
                        V::P(address)
                    } else {
                        read(address, kind.var())
                    };
                    frame.stack.push(value);
                }
                0x9B..=0xA2 | 0xA4 => {
                    let kind = match opcode {
                        0x9B => ElementKind::I64,
                        0x9C => ElementKind::I8,
                        0x9D => ElementKind::I16,
                        0x9E => ElementKind::I32,
                        0x9F => ElementKind::I64,
                        0xA0 => ElementKind::F32,
                        0xA1 => ElementKind::F64,
                        0xA2 => ElementKind::Reference,
                        _ => {
                            let Argument::Token(token) = argument else {
                                return unsupported("element token");
                            };
                            map(self.model.element_kind(module, token))?
                        }
                    };
                    let value = pop!();
                    let index = pop!().int();
                    let array = pop!().address();
                    let address = self.element(array, index, kind)?;
                    write(address, kind.var(), value);
                }
                0xD0 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("token");
                    };
                    match token.table() {
                        Some(TableId::Field) | Some(TableId::MemberRef) => {
                            let field = map(self.model.field(module, token))?;
                            if !field.has_rva {
                                return unsupported("field handles");
                            }
                            frame.stack.push(V::P(map(runtime::rva_data(self.model.set, field.module, field.row))?));
                        }
                        _ => return unsupported("type and method handles"),
                    }
                }
                0xFE06 | 0xFE07 => {
                    let Argument::Token(token) = argument else {
                        return unsupported("function token");
                    };
                    let mut key = self.method_token(module, token)?;
                    if opcode == 0xFE07 {
                        let this = pop!().address();
                        if this == 0 {
                            return self.throw("NullReferenceException");
                        }
                        let callee = self.load(&key)?;
                        if callee.is_virtual {
                            key = self.resolve_virtual(&key, this)?;
                        }
                    }
                    frame.stack.push(V::I8(encode_function(&key) as i64));
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
        let owner = callee.owner;
        let set = self.model.set;
        if set.is_value_type(owner).unwrap_or(false) {
            return unsupported("value type construction");
        }
        let name = self.type_name(owner);
        if name == "System.String" {
            return unsupported("string constructors");
        }
        self.ensure_initialized(owner)?;
        let info = map(self.model.class(owner))?;
        if info.is_abstract || info.is_interface {
            return unsupported("construction of an abstract type");
        }
        let record = self.record(RuntimeType::Class(owner))?;
        let count = callee.parameters.len() - 1;
        if frame.stack.len() < count {
            return unsupported("evaluation stack underflow");
        }
        let object = runtime::allocate_managed(record, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        let at = frame.stack.len() - count;
        frame.stack.insert(at, V::O(object));
        let arguments: Vec<V> = frame.stack[at..].to_vec();
        frame.stack.truncate(at + 1);
        self.invoke(&callee.key, &arguments)?;
        Ok(())
    }

    fn find_value_override(&self, handle: TypeHandle, key: &MethodKey) -> Run<Option<MethodKey>> {
        let set = self.model.set;
        let metadata = &set.modules[key.module].metadata;
        let name = metadata.string_cell(TableId::MethodDef, key.row, 3).unwrap_or("");
        let sig = map(self.model.method_sig(key))?;
        let target_metadata = &set.modules[handle.module].metadata;
        let Ok(range) = target_metadata.method_range(handle.row) else {
            return Ok(None);
        };
        for row in range {
            if target_metadata.string_cell(TableId::MethodDef, row, 3).ok() != Some(name) {
                continue;
            }
            let candidate = MethodKey {
                module: handle.module,
                row,
            };
            let Ok(candidate_sig) = self.model.method_sig(&candidate) else {
                continue;
            };
            if candidate_sig.has_this
                && set
                    .methods_equal(handle.module, &candidate_sig, key.module, &sig)
                    .unwrap_or(false)
            {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    fn box_value(&self, handle: TypeHandle, var: VarType, value: V) -> Run<u64> {
        let record = self.record(RuntimeType::Class(handle))?;
        let object = runtime::allocate_managed(record, 0);
        if object == 0 {
            return unsupported("out of memory");
        }
        write(object + 8, var, value);
        Ok(object)
    }

    fn unbox_compatible(&self, object: u64, handle: TypeHandle) -> Run<bool> {
        let class = self.object_class(object)?;
        if class == handle {
            return Ok(true);
        }
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

    fn element(&self, array: u64, index: i64, kind: ElementKind) -> Run<u64> {
        if array == 0 {
            return self.throw("NullReferenceException");
        }
        let length = i64::from(unsafe { *((array + 8) as *const u32) });
        if index < 0 || index >= length {
            return self.throw("IndexOutOfRangeException");
        }
        Ok(array + u64::from(ARRAY_DATA) + index as u64 * u64::from(kind.size()))
    }

    pub fn blocker(&self, key: &MethodKey) -> Option<String> {
        match self.load(key) {
            Ok(_) => None,
            Err(Fault::Unsupported(reason)) if reason == "abstract method" => None,
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
            if matches!(loaded.native, Some(Native::Intrinsic(Intrinsic::False))) {
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
        let mut seen: HashSet<MethodKey> = HashSet::new();
        let mut pending = vec![key.clone()];
        while let Some(current) = pending.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            if seen.len() > limit {
                return Err("call graph is too large to check".into());
            }
            let loaded = self.load(&current).map_err(|fault| match fault {
                Fault::Unsupported(reason) => reason,
                Fault::Exception(_) => "exception while loading".into(),
            })?;
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
                let (0x28 | 0x6F | 0x73 | 0xFE06..=0xFE07) = instruction.opcode.value else {
                    if matches!(instruction.opcode.value, 0x7E..=0x80)
                        && let Argument::Token(token) = instruction.argument
                        && let Ok(field) = self.model.field(current.module, token)
                    {
                        let metadata = &self.model.set.modules[field.owner.module].metadata;
                        if let Ok(range) = metadata.method_range(field.owner.row) {
                            for row in range {
                                if metadata.string_cell(TableId::MethodDef, row, 3).ok() == Some(".cctor") {
                                    pending.push(MethodKey { module: field.owner.module, row });
                                }
                            }
                        }
                    }
                    continue;
                };
                let Argument::Token(token) = instruction.argument else {
                    continue;
                };
                let target = self.method_token(current.module, token).map_err(|fault| match fault {
                    Fault::Unsupported(reason) => reason,
                    Fault::Exception(_) => "exception".into(),
                })?;
                let flags = self.model.set.modules[target.module]
                    .metadata
                    .cell(TableId::MethodDef, target.row, 2)
                    .unwrap_or(0);
                if flags & ABSTRACT != 0 {
                    continue;
                }
                pending.push(target);
            }
        }
        Ok(seen.len())
    }
}

enum CastTarget {
    Type(TypeHandle),
    Array(ElementKind),
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

fn coerce(value: V, var: VarType) -> V {
    load_raw(var, store_raw(var, value))
}

fn string_units<'a>(string: u64) -> &'a [u16] {
    unsafe {
        let length = *((string + 8) as *const u32) as usize;
        std::slice::from_raw_parts((string + u64::from(STRING_DATA)) as *const u16, length)
    }
}

fn encode_function(key: &MethodKey) -> u64 {
    FUNCTION_TAG | ((key.module as u64) << 32) | u64::from(key.row)
}

fn decode_function(value: u64) -> Option<MethodKey> {
    if value & 0xFF00_0000_0000_0000 != FUNCTION_TAG {
        return None;
    }
    Some(MethodKey {
        module: ((value >> 32) & 0xFF_FFFF) as usize,
        row: value as u32,
    })
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
    interpreter.load(key).ok().and_then(|l| l.returns)
}

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
    }
}

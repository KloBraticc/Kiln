use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};

use crate::loader::{AssemblySet, TypeHandle};
use crate::metadata::TableId;
use crate::objects::{ARRAY_DATA, ElementKind, ObjectModel, RuntimeType, STRING_DATA};

pub const TYPE_VTABLE: i32 = 56;
pub const FLAG_ARRAY: u64 = 1;
pub const FLAG_STRING: u64 = 2;
pub const FLAG_REFERENCE_ELEMENTS: u64 = 4;
pub const FLAG_OBJECT: u64 = 8;
pub const FLAG_STRUCT_ELEMENTS: u64 = 16;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut u8, size: usize, allocation: u32, protection: u32) -> *mut u8;
}

const CHUNK: usize = 64 * 1024 * 1024;
const MINIMUM: usize = 16;
const MARK: u64 = 1;

struct Chunk {
    base: usize,
    length: usize,
    used: usize,
}

pub struct Heap {
    chunks: Vec<Chunk>,
    free: Vec<(usize, usize)>,
    pub allocated: u64,
    pub live: u64,
    budget: u64,
}

impl Heap {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            free: Vec::new(),
            allocated: 0,
            live: 0,
            budget: CHUNK as u64,
        }
    }

    pub fn allocate(&mut self, size: usize) -> *mut u8 {
        let size = size.next_multiple_of(8).max(MINIMUM);
        match self.take_free(size).or_else(|| self.bump(size)) {
            Some(address) => {
                self.allocated += size as u64;
                address as *mut u8
            }
            None => std::ptr::null_mut(),
        }
    }

    fn take_free(&mut self, size: usize) -> Option<usize> {
        let index = self
            .free
            .iter()
            .position(|&(_, length)| length == size || length >= size + MINIMUM)?;
        let (address, length) = self.free[index];
        if length == size {
            self.free.remove(index);
        } else {
            self.free[index] = (address + size, length - size);
            write_free(address + size, length - size);
        }
        Some(address)
    }

    fn bump(&mut self, size: usize) -> Option<usize> {
        if let Some(chunk) = self.chunks.last_mut()
            && chunk.used + size <= chunk.length
        {
            let address = chunk.base + chunk.used;
            chunk.used += size;
            return Some(address);
        }
        let length = size.max(CHUNK).next_multiple_of(4096);
        let base = unsafe { VirtualAlloc(std::ptr::null_mut(), length, 0x3000, 0x04) } as usize;
        if base == 0 {
            return None;
        }
        if let Some(previous) = self.chunks.last_mut() {
            let rest = previous.length - previous.used;
            if rest >= MINIMUM {
                write_free(previous.base + previous.used, rest);
                self.free.push((previous.base + previous.used, rest));
                previous.used = previous.length;
            }
        }
        self.chunks.push(Chunk {
            base,
            length,
            used: size,
        });
        Some(base)
    }

    pub fn contains(&self, address: usize) -> bool {
        self.chunks
            .iter()
            .any(|chunk| address >= chunk.base && address < chunk.base + chunk.used)
    }

    fn should_collect(&self) -> bool {
        self.allocated >= self.budget
    }

    fn sweep(&mut self) {
        self.free.clear();
        let mut live = 0u64;
        for chunk in &self.chunks {
            let end = chunk.base + chunk.used;
            let mut cursor = chunk.base;
            let mut run: Option<usize> = None;
            while cursor < end {
                let header = unsafe { *(cursor as *const u64) };
                let size = block_size(cursor);
                if header & MARK != 0 {
                    unsafe { *(cursor as *mut u64) = header & !MARK };
                    live += size as u64;
                    if let Some(start) = run.take() {
                        write_free(start, cursor - start);
                        self.free.push((start, cursor - start));
                    }
                } else if run.is_none() {
                    run = Some(cursor);
                }
                cursor += size;
            }
            if let Some(start) = run {
                write_free(start, end - start);
                self.free.push((start, end - start));
            }
        }
        self.live = live;
        self.allocated = 0;
        self.budget = (CHUNK as u64).max(live);
    }
}

fn write_free(address: usize, length: usize) {
    unsafe {
        *(address as *mut u64) = 0;
        *((address + 8) as *mut u64) = length as u64;
    }
}

fn block_size(address: usize) -> usize {
    let header = unsafe { *(address as *const u64) } & !MARK;
    if header == 0 {
        return unsafe { *((address + 8) as *const u64) } as usize;
    }
    let record = header as *const u64;
    let (base, component, flags) =
        unsafe { (*record & 0xFFFF_FFFF, *record >> 32, *record.add(1)) };
    let count = if flags & (FLAG_ARRAY | FLAG_STRING) != 0 {
        u64::from(unsafe { *((address + 8) as *const u32) })
    } else {
        0
    };
    ((base + component * count) as usize)
        .next_multiple_of(8)
        .max(MINIMUM)
}

pub struct TypeRecord {
    pub words: Box<[u64]>,
    pub offsets: Box<[u32]>,
}

pub struct RuntimeState {
    pub heap: Heap,
    pub types: HashMap<RuntimeType, TypeRecord>,
    pub statics: HashMap<(usize, u32), Box<u64>>,
    pub flags: HashMap<TypeHandle, Box<u8>>,
    pub strings: HashMap<(usize, u32), u64>,
    pub reference_statics: Vec<usize>,
    struct_statics: HashMap<(usize, u32), Box<[u64]>>,
    pub string_type: u64,
    pub empty_string: u64,
    frozen: Vec<Box<[u64]>>,
    methods: Vec<(usize, usize, Box<[i32]>)>,
    pub method_names: HashMap<usize, String>,
    pub transitions: Vec<(u64, u64)>,
    pub root_hook: Option<fn(&mut Vec<u64>)>,
    pub code_addresses: HashMap<crate::codegen::MethodKey, u64>,
    pub trap_address: u64,
    pub late_resolver: Option<fn(&crate::codegen::MethodKey) -> Option<u64>>,
}

static STATE: AtomicPtr<RuntimeState> = AtomicPtr::new(std::ptr::null_mut());
pub static TRAPPED: AtomicU64 = AtomicU64::new(0);
pub static STRESS: AtomicBool = AtomicBool::new(false);
pub static COLLECTIONS: AtomicU64 = AtomicU64::new(0);

pub fn state() -> &'static mut RuntimeState {
    unsafe { &mut *STATE.load(Ordering::Acquire) }
}

pub fn install() -> &'static mut RuntimeState {
    let existing = STATE.load(Ordering::Acquire);
    if !existing.is_null() {
        return unsafe { &mut *existing };
    }
    let state = Box::into_raw(Box::new(RuntimeState {
        heap: Heap::new(),
        types: HashMap::new(),
        statics: HashMap::new(),
        flags: HashMap::new(),
        strings: HashMap::new(),
        reference_statics: Vec::new(),
        struct_statics: HashMap::new(),
        string_type: 0,
        empty_string: 0,
        frozen: Vec::new(),
        methods: Vec::new(),
        method_names: HashMap::new(),
        transitions: Vec::new(),
        root_hook: None,
        code_addresses: HashMap::new(),
        trap_address: 0,
        late_resolver: None,
    }));
    STATE.store(state, Ordering::Release);
    unsafe { &mut *state }
}

impl RuntimeState {
    pub fn type_record(
        &mut self,
        model: &ObjectModel<'_>,
        kind: RuntimeType,
    ) -> Result<u64, String> {
        if let Some(record) = self.types.get(&kind) {
            return Ok(record.words.as_ptr() as u64);
        }
        let (base, component, flags, parent, offsets, vtable_length) = match kind {
            RuntimeType::Class(handle) => {
                let info = model.class(handle)?;
                let parent = match info.parent {
                    Some(parent) => self.type_record(model, RuntimeType::Class(parent))?,
                    None => 0,
                };
                let is_object = info.parent.is_none()
                    && !info.is_interface
                    && !info.is_value_type
                    && model.set.full_type_name(handle).ok().as_deref() == Some("System.Object");
                (
                    u64::from(info.base_size),
                    0u64,
                    if is_object { FLAG_OBJECT } else { 0 },
                    parent,
                    info.reference_offsets.clone(),
                    info.vtable.len(),
                )
            }
            RuntimeType::Array(element) => {
                let flags = FLAG_ARRAY
                    | if element == ElementKind::Reference {
                        FLAG_REFERENCE_ELEMENTS
                    } else {
                        0
                    };
                (
                    u64::from(ARRAY_DATA),
                    u64::from(element.size()),
                    flags,
                    0,
                    Vec::new(),
                    0,
                )
            }
            RuntimeType::ValueArray(handle) => {
                let layout = model.struct_layout(handle)?;
                (
                    u64::from(ARRAY_DATA),
                    u64::from(layout.size),
                    FLAG_ARRAY | FLAG_STRUCT_ELEMENTS,
                    0,
                    layout.references.clone(),
                    0,
                )
            }
            RuntimeType::String => (u64::from(STRING_DATA) + 2, 2, FLAG_STRING, 0, Vec::new(), 0),
        };
        let offsets: Box<[u32]> = offsets.into_boxed_slice();
        let mut words = vec![0u64; 7 + vtable_length].into_boxed_slice();
        words[0] = base | (component << 32);
        words[1] = flags;
        words[2] = parent;
        words[3] = offsets.as_ptr() as u64;
        words[4] = offsets.len() as u64;
        words[5] = match kind {
            RuntimeType::Class(handle) => ((handle.module as u64) << 32) | u64::from(handle.row),
            _ => u64::MAX,
        };
        words[6] = vtable_length as u64;
        if let RuntimeType::Class(handle) = kind
            && self.trap_address != 0
        {
            let info = model.class(handle)?;
            for (slot, method) in info.vtable.iter().enumerate() {
                words[7 + slot] = method
                    .as_ref()
                    .and_then(|key| {
                        self.code_addresses
                            .get(key)
                            .copied()
                            .or_else(|| self.late_resolver.and_then(|resolve| resolve(key)))
                    })
                    .unwrap_or(self.trap_address);
            }
        }
        let address = words.as_ptr() as u64;
        self.types.insert(kind, TypeRecord { words, offsets });
        Ok(address)
    }

    pub fn static_slot(
        &mut self,
        model: &ObjectModel<'_>,
        module: usize,
        row: u32,
        reference: bool,
    ) -> u64 {
        let fresh = !self.statics.contains_key(&(module, row));
        let entry = self
            .statics
            .entry((module, row))
            .or_insert_with(|| Box::new(0));
        let address = &mut **entry as *mut u64 as u64;
        if fresh && reference && is_string_empty(model, module, row) {
            let empty = self.empty_string(model);
            if let Some(slot) = self.statics.get_mut(&(module, row)) {
                **slot = empty;
            }
        }
        if reference && !self.reference_statics.contains(&(address as usize)) {
            self.reference_statics.push(address as usize);
        }
        address
    }

    pub fn static_block(&mut self, module: usize, row: u32, size: u32, references: &[u32]) -> u64 {
        if let Some(block) = self.struct_statics.get(&(module, row)) {
            return block.as_ptr() as u64;
        }
        let block = vec![0u64; (size as usize).div_ceil(8).max(1)].into_boxed_slice();
        let address = block.as_ptr() as u64;
        for offset in references {
            self.reference_statics.push(address as usize + *offset as usize);
        }
        self.struct_statics.insert((module, row), block);
        address
    }

    pub fn init_flag(&mut self, handle: TypeHandle) -> u64 {
        let entry = self.flags.entry(handle).or_insert_with(|| Box::new(0));
        &mut **entry as *mut u8 as u64
    }

    pub fn string_literal(
        &mut self,
        set: &AssemblySet,
        model: &ObjectModel<'_>,
        module: usize,
        offset: u32,
    ) -> Result<u64, String> {
        if let Some(existing) = self.strings.get(&(module, offset)) {
            return Ok(*existing);
        }
        let text = set.modules[module]
            .metadata
            .user_string(offset)
            .map_err(|e| e.to_string())?;
        let units: Vec<u16> = text.encode_utf16().collect();
        let address = if units.is_empty() {
            self.empty_string(model)
        } else {
            self.ensure_string_type(model)?;
            self.frozen_string(&units)
        };
        self.strings.insert((module, offset), address);
        Ok(address)
    }

    fn ensure_string_type(&mut self, model: &ObjectModel<'_>) -> Result<(), String> {
        if self.string_type == 0 {
            model.string_type().ok_or("System.String was not found")?;
            self.string_type = self.type_record(model, RuntimeType::String)?;
        }
        Ok(())
    }

    pub fn empty_string(&mut self, model: &ObjectModel<'_>) -> u64 {
        if self.empty_string == 0 && self.ensure_string_type(model).is_ok() {
            self.empty_string = self.frozen_string(&[]);
        }
        self.empty_string
    }

    fn frozen_string(&mut self, units: &[u16]) -> u64 {
        let bytes = STRING_DATA as usize + 2 * (units.len() + 1);
        let mut words = vec![0u64; bytes.div_ceil(8)].into_boxed_slice();
        words[0] = self.string_type;
        let raw = words.as_mut_ptr() as *mut u8;
        unsafe {
            *(raw.add(8) as *mut u32) = units.len() as u32;
            std::ptr::copy_nonoverlapping(
                units.as_ptr(),
                raw.add(STRING_DATA as usize) as *mut u16,
                units.len(),
            );
        }
        let address = words.as_ptr() as u64;
        self.frozen.push(words);
        address
    }
}

impl RuntimeState {
    pub fn register_method(&mut self, start: usize, end: usize, references: &[i32]) {
        let at = self.methods.partition_point(|entry| entry.0 < start);
        self.methods
            .insert(at, (start, end, references.to_vec().into_boxed_slice()));
    }

    pub fn method_containing(&self, address: usize) -> Option<(usize, String)> {
        let at = self.methods.partition_point(|entry| entry.0 <= address);
        let (start, end, _) = self.methods.get(at.checked_sub(1)?)?;
        if address >= *end {
            return None;
        }
        Some((address - start, self.method_names.get(start)?.clone()))
    }

    pub fn is_compiled(&self, address: usize) -> bool {
        self.method_at(address).is_some()
    }

    fn method_at(&self, address: usize) -> Option<&[i32]> {
        let at = self.methods.partition_point(|entry| entry.0 <= address);
        let (start, end, references) = self.methods.get(at.checked_sub(1)?)?;
        (address > *start && address <= *end).then_some(&**references)
    }

    pub fn collect(&mut self, frame: u64, stack: u64) {
        let mut pending: Vec<usize> = Vec::new();
        for &slot in &self.reference_statics {
            self.mark(unsafe { *(slot as *const u64) } as usize, &mut pending);
        }
        let chains: Vec<(u64, u64)> = std::iter::once((frame, stack))
            .chain(self.transitions.iter().copied())
            .collect();
        for (frame, stack) in chains {
            if frame == 0 || stack == 0 {
                continue;
            }
            let mut base = frame as usize;
            let mut returned = unsafe { *((stack - 8) as *const u64) } as usize;
            while base != 0 {
                let Some(references) = self.method_at(returned) else {
                    break;
                };
                for &offset in references {
                    let slot = base.wrapping_add_signed(offset as isize);
                    self.mark(unsafe { *(slot as *const u64) } as usize, &mut pending);
                }
                returned = unsafe { *((base + 8) as *const u64) } as usize;
                base = unsafe { *(base as *const u64) } as usize;
            }
        }
        if let Some(hook) = self.root_hook {
            let mut roots = Vec::new();
            hook(&mut roots);
            for root in roots {
                self.mark(root as usize, &mut pending);
            }
        }
        while let Some(object) = pending.pop() {
            let record = (unsafe { *(object as *const u64) } & !MARK) as *const u64;
            let flags = unsafe { *record.add(1) };
            if flags & FLAG_REFERENCE_ELEMENTS != 0 {
                let count = unsafe { *((object + 8) as *const u32) } as usize;
                for index in 0..count {
                    let element = object + ARRAY_DATA as usize + 8 * index;
                    self.mark(unsafe { *(element as *const u64) } as usize, &mut pending);
                }
            } else if flags & FLAG_STRUCT_ELEMENTS != 0 {
                let count = unsafe { *((object + 8) as *const u32) } as usize;
                let component = (unsafe { *record } >> 32) as usize;
                let (offsets, references) =
                    unsafe { (*record.add(3) as *const u32, *record.add(4) as usize) };
                for index in 0..count {
                    let element = object + ARRAY_DATA as usize + component * index;
                    for position in 0..references {
                        let slot = element + unsafe { *offsets.add(position) } as usize;
                        self.mark(unsafe { *(slot as *const u64) } as usize, &mut pending);
                    }
                }
            } else if flags & (FLAG_ARRAY | FLAG_STRING) == 0 {
                let (offsets, count) = unsafe { (*record.add(3) as *const u32, *record.add(4)) };
                for index in 0..count as usize {
                    let field = object + unsafe { *offsets.add(index) } as usize;
                    self.mark(unsafe { *(field as *const u64) } as usize, &mut pending);
                }
            }
        }
        self.heap.sweep();
        COLLECTIONS.fetch_add(1, Ordering::Relaxed);
    }

    fn mark(&self, object: usize, pending: &mut Vec<usize>) {
        if object == 0 || !self.heap.contains(object) {
            return;
        }
        let header = unsafe { &mut *(object as *mut u64) };
        if *header & MARK != 0 || *header == 0 {
            return;
        }
        *header |= MARK;
        pending.push(object);
    }
}

pub fn rva_data(set: &AssemblySet, module: usize, field: u32) -> Result<u64, String> {
    let metadata = &set.modules[module].metadata;
    for row in 1..=metadata.rows(TableId::FieldRva) {
        if metadata
            .cell(TableId::FieldRva, row, 1)
            .map_err(|e| e.to_string())?
            == field
        {
            let rva = metadata
                .cell(TableId::FieldRva, row, 0)
                .map_err(|e| e.to_string())?;
            let data = set.modules[module]
                .image
                .tail_at_rva(rva)
                .map_err(|e| e.to_string())?;
            return Ok(data.as_ptr() as u64);
        }
    }
    Err("field has no RVA data".into())
}

pub extern "win64" fn allocate(type_record: u64, count: u64, frame: u64, stack: u64) -> u64 {
    let state = state();
    if frame != 0 && (STRESS.load(Ordering::Relaxed) || state.heap.should_collect()) {
        state.collect(frame, stack);
    }
    let record = type_record as *const u64;
    let (base, component, flags) =
        unsafe { (*record & 0xFFFF_FFFF, *record >> 32, *record.add(1)) };
    let size = base + component * count;
    let object = state.heap.allocate(size as usize);
    if object.is_null() {
        return 0;
    }
    unsafe {
        std::ptr::write_bytes(object, 0, size as usize);
        *(object as *mut u64) = type_record;
        if flags & (FLAG_ARRAY | FLAG_STRING) != 0 {
            *(object.add(8) as *mut u32) = count as u32;
        }
    }
    object as u64
}

pub fn allocate_managed(type_record: u64, count: u64) -> u64 {
    let state = state();
    if STRESS.load(Ordering::Relaxed) || state.heap.should_collect() {
        state.collect(0, 0);
    }
    allocate(type_record, count, 0, 0)
}

pub extern "win64" fn is_instance(object: u64, target: u64) -> u64 {
    if object == 0 {
        return 0;
    }
    if unsafe { *((target + 8) as *const u64) } & FLAG_OBJECT != 0 {
        return object;
    }
    let mut current = unsafe { *(object as *const u64) } & !1;
    while current != 0 {
        if current == target {
            return object;
        }
        current = unsafe { *(current as *const u64).add(2) };
    }
    0
}

pub extern "win64" fn initialize_array(array: u64, data: u64) {
    if array == 0 {
        return;
    }
    unsafe {
        let record = *(array as *const u64) as *const u64;
        let component = *record >> 32;
        let length = u64::from(*((array + 8) as *const u32));
        std::ptr::copy_nonoverlapping(
            data as *const u8,
            (array + u64::from(ARRAY_DATA)) as *mut u8,
            (component * length) as usize,
        );
    }
}

pub extern "win64" fn trap() -> u64 {
    TRAPPED.fetch_add(1, Ordering::Relaxed);
    0
}

pub fn new_string(units: &[u16]) -> u64 {
    let record = state().string_type;
    if record == 0 {
        return 0;
    }
    if units.is_empty() && state().empty_string != 0 {
        return state().empty_string;
    }
    let object = allocate(record, units.len() as u64, 0, 0);
    if object != 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(
                units.as_ptr(),
                (object + u64::from(STRING_DATA)) as *mut u16,
                units.len(),
            );
        }
    }
    object
}

fn is_string_empty(model: &ObjectModel<'_>, module: usize, row: u32) -> bool {
    let metadata = &model.set.modules[module].metadata;
    if metadata.string_cell(TableId::Field, row, 1).ok() != Some("Empty") {
        return false;
    }
    model.set.modules[module]
        .field_owner(row)
        .ok()
        .flatten()
        .and_then(|owner| {
            model
                .set
                .full_type_name(TypeHandle { module, row: owner })
                .ok()
        })
        .is_some_and(|name| name == "System.String")
}

use std::collections::HashMap;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use crate::loader::{AssemblySet, TypeHandle};
use crate::metadata::TableId;
use crate::objects::{ARRAY_DATA, ElementKind, ObjectModel, RuntimeType, STRING_DATA};

pub const TYPE_VTABLE: i32 = 56;
pub const FLAG_ARRAY: u64 = 1;
pub const FLAG_STRING: u64 = 2;
pub const FLAG_REFERENCE_ELEMENTS: u64 = 4;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(address: *mut u8, size: usize, allocation: u32, protection: u32) -> *mut u8;
}

const CHUNK: usize = 64 * 1024 * 1024;

pub struct Heap {
    chunks: Vec<(usize, usize)>,
    cursor: usize,
    limit: usize,
    pub allocated: u64,
}

impl Heap {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            cursor: 0,
            limit: 0,
            allocated: 0,
        }
    }

    pub fn allocate(&mut self, size: usize) -> *mut u8 {
        let size = size.next_multiple_of(8);
        if self.cursor + size > self.limit {
            let length = size.max(CHUNK);
            let base = unsafe { VirtualAlloc(std::ptr::null_mut(), length, 0x3000, 0x04) } as usize;
            if base == 0 {
                return std::ptr::null_mut();
            }
            self.chunks.push((base, length));
            self.cursor = base;
            self.limit = base + length;
        }
        let address = self.cursor;
        self.cursor += size;
        self.allocated += size as u64;
        address as *mut u8
    }

    pub fn contains(&self, address: usize) -> bool {
        self.chunks
            .iter()
            .any(|&(base, length)| address >= base && address < base + length)
    }
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
    pub string_type: u64,
    pub empty_string: u64,
    frozen: Vec<Box<[u64]>>,
}

static STATE: AtomicPtr<RuntimeState> = AtomicPtr::new(std::ptr::null_mut());
pub static TRAPPED: AtomicU64 = AtomicU64::new(0);

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
        string_type: 0,
        empty_string: 0,
        frozen: Vec::new(),
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
                (
                    u64::from(info.base_size),
                    0u64,
                    0u64,
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

pub extern "win64" fn allocate(type_record: u64, count: u64, _frame: u64) -> u64 {
    let state = state();
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

pub extern "win64" fn is_instance(object: u64, target: u64) -> u64 {
    if object == 0 {
        return 0;
    }
    let mut current = unsafe { *(object as *const u64) };
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
    let object = allocate(record, units.len() as u64, 0);
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

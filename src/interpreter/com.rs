use super::*;

const S_OK: u64 = 0;
const E_NOINTERFACE: u64 = 0x8000_4002;
const E_NOTIMPL: u64 = 0x8000_4001;
const E_FAIL: u64 = 0x8000_4005;
const IUNKNOWN: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];

struct Interface {
    exact: Gen,
    guid: [u8; 16],
    methods: Vec<MethodKey>,
    dispatch: bool,
    vtable: u64,
}

#[derive(Default)]
pub(super) struct Wrappers {
    interfaces: Vec<Interface>,
    by_type: HashMap<u32, usize>,
    wrappers: HashMap<(u64, usize), u64>,
    objects: HashMap<u64, u64>,
    rcws: HashMap<u64, u64>,
    counts: HashMap<u64, u32>,
    platforms: HashMap<(MethodKey, u64), usize>,
}

unsafe fn com_handler(context: usize, slot: usize, integers: *const u64, floats: *const u64) -> u64 {
    let interpreter = unsafe { &*(context as *const Interpreter<'static>) };
    interpreter.com_entry(slot, integers, floats)
}

fn parse_guid(text: &str) -> Option<[u8; 16]> {
    let hex: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 32 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok();
    let mut guid = [0u8; 16];
    for (target, source) in [(0, 3), (1, 2), (2, 1), (3, 0), (4, 5), (5, 4), (6, 7), (7, 6)] {
        guid[target] = byte(source)?;
    }
    for index in 8..16 {
        guid[index] = byte(index)?;
    }
    Some(guid)
}

impl Interpreter<'_> {
    pub(super) fn is_com_interface(&self, exact: &Gen) -> bool {
        exact.handle().is_some_and(|handle| {
            let flags = self.model.set.modules[handle.module]
                .metadata
                .cell(TableId::TypeDef, handle.row, 0)
                .unwrap_or(0);
            flags & 0x20 != 0 && self.interface_guid(handle).is_some()
        })
    }

    pub(super) fn interface_guid(&self, handle: TypeHandle) -> Option<[u8; 16]> {
        let metadata = &self.model.set.modules[handle.module].metadata;
        (1..=metadata.rows(TableId::CustomAttribute)).find_map(|row| {
            let owner = metadata.coded(TableId::CustomAttribute, row, 0).ok().flatten()?;
            if owner.table() != Some(TableId::TypeDef) || owner.row() != handle.row {
                return None;
            }
            let constructor = metadata.coded(TableId::CustomAttribute, row, 1).ok().flatten()?;
            let parent = metadata.coded(TableId::MemberRef, constructor.row(), 0).ok().flatten()?;
            if constructor.table() != Some(TableId::MemberRef) || parent.table() != Some(TableId::TypeRef) {
                return None;
            }
            if metadata.string_cell(TableId::TypeRef, parent.row(), 1).ok()? != "GuidAttribute" {
                return None;
            }
            let blob = metadata.blob(metadata.cell(TableId::CustomAttribute, row, 2).ok()?).ok()?;
            let length = *blob.get(2)? as usize;
            parse_guid(std::str::from_utf8(blob.get(3..3 + length)?).ok()?)
        })
    }

    fn com_interface(&self, exact: &Gen) -> Run<usize> {
        let id = self.types.id(exact);
        if let Some(found) = self.wrappers.borrow().by_type.get(&id) {
            return Ok(*found);
        }
        let handle = exact.handle().ok_or_else(|| Fault::Unsupported("COM interface without a type".into()))?;
        let guid = self
            .interface_guid(handle)
            .ok_or_else(|| Fault::Unsupported("COM interface without a GUID".into()))?;
        let metadata = &self.model.set.modules[handle.module].metadata;
        let methods: Vec<MethodKey> = metadata
            .method_range(handle.row)
            .map_err(|e| Fault::Unsupported(e.to_string()))?
            .map(|row| MethodKey { module: handle.module, row })
            .collect();
        let dispatch = self.interface_kind(handle) == 2;
        let index = self.wrappers.borrow().interfaces.len();
        let mut slots = Vec::new();
        let base = if dispatch { 7 } else { 3 };
        for slot in 0..base + methods.len() {
            let stub = pinvoke::make_stub(com_handler, self as *const Self as usize, (index << 16) | slot)
                .ok_or_else(|| Fault::Unsupported("no executable memory for a COM wrapper".into()))?;
            slots.push(stub as u64);
        }
        let vtable: &'static mut [u64] = Box::leak(slots.into_boxed_slice());
        let mut wrappers = self.wrappers.borrow_mut();
        wrappers.interfaces.push(Interface {
            exact: exact.clone(),
            guid,
            methods,
            dispatch,
            vtable: vtable.as_ptr() as u64,
        });
        wrappers.by_type.insert(id, index);
        Ok(index)
    }

    fn interface_kind(&self, handle: TypeHandle) -> u32 {
        let metadata = &self.model.set.modules[handle.module].metadata;
        (1..=metadata.rows(TableId::CustomAttribute))
            .find_map(|row| {
                let owner = metadata.coded(TableId::CustomAttribute, row, 0).ok().flatten()?;
                if owner.table() != Some(TableId::TypeDef) || owner.row() != handle.row {
                    return None;
                }
                let constructor = metadata.coded(TableId::CustomAttribute, row, 1).ok().flatten()?;
                let parent = metadata.coded(TableId::MemberRef, constructor.row(), 0).ok().flatten()?;
                if metadata.string_cell(TableId::TypeRef, parent.row(), 1).ok()? != "InterfaceTypeAttribute" {
                    return None;
                }
                let blob = metadata.blob(metadata.cell(TableId::CustomAttribute, row, 2).ok()?).ok()?;
                blob.get(2..4).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])))
            })
            .unwrap_or(0)
    }

    pub(super) fn com_wrapper(&self, object: u64, interface: &Gen) -> Run<u64> {
        let index = self.com_interface(interface)?;
        if let Some(found) = self.wrappers.borrow().wrappers.get(&(object, index)) {
            return Ok(*found);
        }
        let vtable = self.wrappers.borrow().interfaces[index].vtable;
        let block: &'static mut [u64] = Box::leak(vec![vtable, object, 1, index as u64].into_boxed_slice());
        type_roots().push(object);
        let address = block.as_ptr() as u64;
        self.wrappers.borrow_mut().wrappers.insert((object, index), address);
        Ok(address)
    }

    fn com_entry(&self, slot: usize, integers: *const u64, floats: *const u64) -> u64 {
        let (index, method) = (slot >> 16, slot & 0xFFFF);
        let this = unsafe { *integers };
        let wrapper = this as *mut u64;
        match method {
            0 => {
                let iid = unsafe { std::slice::from_raw_parts(*integers.add(1) as *const u8, 16) };
                let target = unsafe { *integers.add(2) } as *mut u64;
                let wrappers = self.wrappers.borrow();
                let interface = &wrappers.interfaces[index];
                let dispatch_iid = [0, 0x04, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];
                if iid == IUNKNOWN || iid == interface.guid || (interface.dispatch && iid == dispatch_iid) {
                    unsafe {
                        *target = this;
                        *wrapper.add(2) += 1;
                    }
                    S_OK
                } else {
                    unsafe { *target = 0 };
                    E_NOINTERFACE
                }
            }
            1 => unsafe {
                *wrapper.add(2) += 1;
                *wrapper.add(2)
            },
            2 => unsafe {
                *wrapper.add(2) = (*wrapper.add(2)).saturating_sub(1).max(1);
                *wrapper.add(2)
            },
            _ => {
                let (exact, key, base) = {
                    let wrappers = self.wrappers.borrow();
                    let interface = &wrappers.interfaces[index];
                    let base = if interface.dispatch { 7 } else { 3 };
                    if method < base {
                        return E_NOTIMPL;
                    }
                    match interface.methods.get(method - base) {
                        Some(key) => (interface.exact.clone(), key.clone(), base),
                        None => return E_NOTIMPL,
                    }
                };
                let _ = base;
                let object = unsafe { *wrapper.add(1) };
                match self.com_invoke(object, &exact, &key, integers, floats) {
                    Ok(code) => code,
                    Err(Fault::Exception(exception)) => self.exception_hresult(exception),
                    Err(Fault::Unsupported(_)) => E_NOTIMPL,
                }
            }
        }
    }

    fn exception_hresult(&self, exception: u64) -> u64 {
        self.class_field_offset("System", "Exception", "_HResult")
            .map(|offset| u64::from(unsafe { *((exception + u64::from(offset)) as *const u32) }))
            .unwrap_or(E_FAIL)
    }

    fn com_invoke(&self, object: u64, interface: &Gen, key: &MethodKey, integers: *const u64, floats: *const u64) -> Run<u64> {
        let ctx = interface.context();
        let sig = map(self.model.method_sig(key))?;
        let implementation = self.load_ctx(key, &ctx)?;
        let metadata = &self.model.set.modules[key.module].metadata;
        let preserve = metadata.cell(TableId::MethodDef, key.row, 1).unwrap_or(0) & 0x80 != 0;
        let mut arguments = vec![V::O(object)];
        for (index, (parameter, ty)) in sig.parameters.iter().zip(&implementation.parameters[1..]).enumerate() {
            let position = index + 1;
            let raw = unsafe {
                match ty {
                    Ty::Prim(VarType::F32 | VarType::F64) if position < 4 => *floats.add(position),
                    _ => *integers.add(position),
                }
            };
            let value = match (unmodified(parameter), ty) {
                (_, Ty::Prim(VarType::Ref)) => return unsupported("COM argument that is an object"),
                (_, Ty::Prim(var)) => load_raw(*var, raw),
                (_, Ty::Struct(layout)) if layout.references.is_empty() => {
                    let boxed = self.allocate_box(&self.layout_gen(layout))?;
                    let source = if layout.size <= 8 { (&raw const raw) as u64 } else { raw };
                    unsafe { std::ptr::copy_nonoverlapping(source as *const u8, (boxed + 8) as *mut u8, layout.size as usize) };
                    V::S(boxed)
                }
                _ => return unsupported("COM argument that is a struct with references"),
            };
            arguments.push(value);
        }
        let _pin = Pin::new(&arguments);
        let target = self.resolve_virtual(key, &ctx, object)?;
        let result = self.invoke_ctx(&target.0, &target.1, &arguments)?;
        if preserve {
            return Ok(result.map_or(0, |value| value.bits() & 0xFFFF_FFFF));
        }
        if let (Some(value), Some(ty)) = (result, &implementation.returns) {
            let out = unsafe { *integers.add(sig.parameters.len() + 1) };
            if out != 0 {
                self.write_value(out, ty, value)?;
            }
        }
        Ok(S_OK)
    }
}

type QueryInterface = unsafe extern "system" fn(u64, *const u8, *mut u64) -> u32;
type Release = unsafe extern "system" fn(u64) -> u32;

fn vtable_entry(pointer: u64, slot: usize) -> u64 {
    unsafe { *((*(pointer as *const u64)) as *const u64).add(slot) }
}

fn query(pointer: u64, iid: &[u8; 16]) -> Option<u64> {
    let function: QueryInterface = unsafe { std::mem::transmute(vtable_entry(pointer, 0) as usize) };
    let mut result = 0u64;
    let code = unsafe { function(pointer, iid.as_ptr(), &raw mut result) };
    (code == 0 && result != 0).then_some(result)
}

fn release(pointer: u64) {
    let function: Release = unsafe { std::mem::transmute(vtable_entry(pointer, 2) as usize) };
    unsafe { function(pointer) };
}

impl Interpreter<'_> {
    pub(super) fn rcw_for(&self, pointer: u64) -> Run<u64> {
        if pointer == 0 {
            return Ok(0);
        }
        let unknown = query(pointer, &IUNKNOWN).unwrap_or(pointer);
        let existing = self.wrappers.borrow().objects.get(&unknown).copied();
        if let Some(found) = existing {
            if unknown != pointer {
                release(unknown);
            }
            *self.wrappers.borrow_mut().counts.entry(found).or_insert(0) += 1;
            return Ok(found);
        }
        let handle = self.corelib_type("System", "__ComObject")?;
        let object = runtime::allocate_managed(self.record_of(&Gen::Named(handle, Rc::from(Vec::new())))?, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        type_roots().push(object);
        let mut wrappers = self.wrappers.borrow_mut();
        wrappers.objects.insert(unknown, object);
        wrappers.rcws.insert(object, unknown);
        wrappers.counts.insert(object, 1);
        Ok(object)
    }

    pub(super) fn rcw_release(&self, object: u64, last: bool) -> u32 {
        let mut wrappers = self.wrappers.borrow_mut();
        let remaining = match wrappers.counts.get_mut(&object) {
            Some(count) if !last && *count > 1 => {
                *count -= 1;
                return *count;
            }
            _ => 0,
        };
        wrappers.counts.remove(&object);
        if let Some(unknown) = wrappers.rcws.remove(&object) {
            wrappers.objects.remove(&unknown);
            drop(wrappers);
            release(unknown);
        }
        remaining
    }

    pub(super) fn rcw_pointer(&self, object: u64) -> Option<u64> {
        self.wrappers.borrow().rcws.get(&object).copied()
    }

    pub(super) fn rcw_interface(&self, object: u64, interface: &Gen) -> Run<Option<u64>> {
        let Some(unknown) = self.rcw_pointer(object) else {
            return Ok(None);
        };
        let Some(handle) = interface.handle() else {
            return Ok(None);
        };
        let Some(guid) = self.interface_guid(handle) else {
            return Ok(None);
        };
        Ok(query(unknown, &guid))
    }

    pub(super) fn rcw_supports(&self, object: u64, interface: &Gen) -> bool {
        match self.rcw_interface(object, interface) {
            Ok(Some(pointer)) => {
                release(pointer);
                true
            }
            _ => false,
        }
    }

    pub(super) fn rcw_call(&self, key: &MethodKey, ctx: &Ctx, arguments: &[V]) -> Run<Option<V>> {
        let object = arguments[0].address();
        let owner = map(self.model.method_owner(key))?;
        let interface = Gen::Named(owner, ctx.types.clone());
        let pointer = self
            .rcw_interface(object, &interface)?
            .ok_or_else(|| Fault::Unsupported("COM object does not implement the interface".into()))?;
        let metadata = &self.model.set.modules[owner.module].metadata;
        let range = metadata.method_range(owner.row).map_err(|e| Fault::Unsupported(e.to_string()))?;
        let position = range.clone().position(|row| row == key.row).unwrap_or(0);
        let base = if self.interface_kind(owner) == 2 { 7 } else { 3 };
        let function = vtable_entry(pointer, base + position);
        let index = self.com_platform(key, ctx, function)?;
        let mut forwarded = Vec::with_capacity(arguments.len() + 1);
        forwarded.push(V::P(pointer));
        forwarded.extend_from_slice(&arguments[1..]);
        let preserve = self.model.set.modules[key.module].metadata.cell(TableId::MethodDef, key.row, 1).unwrap_or(0) & 0x80 != 0;
        let loaded = self.load_ctx(key, ctx)?;
        let mut retval = [0u64; 4];
        if !preserve && loaded.returns.is_some() {
            forwarded.push(V::P(retval.as_mut_ptr() as u64));
        }
        let result = self.platform_call(index, &forwarded);
        release(pointer);
        let result = result?;
        if preserve {
            return Ok(result);
        }
        let code = result.map_or(0, |v| v.int() as i32);
        if code < 0 {
            return self.throw_hresult(code);
        }
        let Some(returns) = &loaded.returns else {
            return Ok(None);
        };
        let sig = map(self.model.method_sig(key))?;
        let declared = map(self.types.resolve(key.module, &sig.return_type, ctx))?;
        Ok(Some(match returns {
            Ty::Prim(VarType::Ref) if declared.handle().is_some_and(|h| self.type_name(h) == "System.String") => {
                V::O(if retval[0] == 0 { 0 } else { runtime::new_string(&reflection::wide_units(retval[0])) })
            }
            Ty::Prim(VarType::Ref) => V::O(self.rcw_for(retval[0])?),
            Ty::Prim(var) => load_raw(*var, retval[0]),
            ty => self.read_value(retval.as_ptr() as u64, ty, &[])?,
        }))
    }

    fn com_platform(&self, key: &MethodKey, ctx: &Ctx, function: u64) -> Run<usize> {
        if let Some(found) = self.wrappers.borrow().platforms.get(&(key.clone(), function)) {
            return Ok(*found);
        }
        let loaded = self.load_ctx(key, ctx)?;
        let sig = map(self.model.method_sig(key))?;
        let metadata = &self.model.set.modules[key.module].metadata;
        let preserve = metadata.cell(TableId::MethodDef, key.row, 1).unwrap_or(0) & 0x80 != 0;
        let mut passes = vec![Pass::Raw(VarType::Pointer)];
        for (index, (parameter, ty)) in sig.parameters.iter().zip(&loaded.parameters[1..]).enumerate() {
            let marshal = pinvoke::marshals(metadata, key.row);
            let native = marshal.iter().find(|(s, _)| *s == index as u32 + 1).map(|(_, n)| *n);
            passes.push(self.platform_pass(key.module, unmodified(parameter), ty, true, native, ctx, false)?);
        }
        let back = if preserve {
            self.platform_back(key.module, &sig, &loaded.returns, None, ctx)?
        } else {
            if loaded.returns.is_some() {
                passes.push(Pass::Raw(VarType::Pointer));
            }
            Back::Raw(VarType::I32)
        };
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
        self.wrappers.borrow_mut().platforms.insert((key.clone(), function), index);
        Ok(index)
    }

    pub(super) fn throw_hresult<T>(&self, code: i32) -> Run<T> {
        let handle = self.corelib_type("System.Runtime.InteropServices", "COMException")?;
        let object = runtime::allocate_managed(self.record(RuntimeType::Class(handle))?, 0);
        if object == 0 {
            return unsupported("out of memory");
        }
        if let Ok(offset) = self.class_field_offset("System", "Exception", "_HResult") {
            unsafe { *((object + u64::from(offset)) as *mut i32) = code };
        }
        Err(Fault::Exception(object))
    }
}

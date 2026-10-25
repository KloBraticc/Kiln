use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Member {
    NativeMessage,
    DelegateForPointer,
    TickCount,
    TickCount64,
    RunClassConstructor,
    ApartmentState,
    SetApartmentState,
    EnumValues,
    MulticastStub,
    AllocateLike,
    CreateArray,
    ResolveMethod,
    ResolveType,
    ResolveField,
    MethodUtf8Name,
    FieldUtf8Name,
    BoxData,
    BuildSignature,
    CompareSignatures,
    ConstructInstantiation,
    False,
    FieldAttributes,
    FieldDeclaringType,
    FieldForType,
    FieldGet,
    FieldName,
    FieldSet,
    FieldToken,
    Fields,
    FirstMethod,
    FunctionPointer,
    GenericParameterCount,
    HasMethodInstantiation,
    Identity,
    Ignore,
    Interfaces,
    InvokeMethod,
    IsConstructor,
    IsGenericMethodDefinition,
    IsTypicalMethodDefinition,
    MethodAttributes,
    MethodDeclaringType,
    MethodFromCanonical,
    MethodImplAttributes,
    MethodInstantiation,
    MethodName,
    MethodToken,
    NextMethod,
    NoModifiers,
    Null,
    StubIfNeeded,
    True,
    TypeFromMethodTable,
    InitSignature,
    MethodSlot,
    MethodAtSlot,
    AllocateArray,
    DelegateMethodsEqual,
    BindMethod,
    BindMethodName,
    QCallVirtualCount,
    InterfaceImplementation,
    TypicalDefinition,
    Uninitialized,
    VirtualCount,
}


pub(super) fn member_internal(type_name: &str, name: &str, count: usize) -> Option<Member> {
    Some(match (type_name, name, count) {
        ("System.RuntimeTypeHandle", "GetFirstIntroducedMethod", 1) => Member::FirstMethod,
        ("System.RuntimeTypeHandle", "GetNextIntroducedMethod", 1) => Member::NextMethod,
        ("System.RuntimeTypeHandle", "GetFields", 3) => Member::Fields,
        ("System.RuntimeTypeHandle", "GetInterfaces", 1) => Member::Interfaces,
        ("System.RuntimeTypeHandle", "IsGenericVariable", 1) => Member::False,
        ("System.RuntimeTypeHandle", "GetGenericVariableIndex", 1) => Member::False,
        ("System.RuntimeTypeHandle", "IsUnmanagedFunctionPointer", 1) => Member::False,
        ("System.RuntimeTypeHandle", "GetNumVirtuals", 1) => Member::VirtualCount,
        ("System.RuntimeTypeHandle", "GetDeclaringMethod", 1) => Member::Null,
        ("System.RuntimeMethodHandle", "GetAttributes", 1) => Member::MethodAttributes,
        ("System.RuntimeMethodHandle", "GetImplAttributes", 1) => Member::MethodImplAttributes,
        ("System.RuntimeMethodHandle", "GetDeclaringType", 1) => Member::MethodDeclaringType,
        ("System.RuntimeMethodHandle", "GetName", 1) => Member::MethodName,
        ("System.RuntimeMethodHandle", "_GetUtf8Name" | "GetUtf8NameInternal", 1) => Member::MethodUtf8Name,
        ("System.RuntimeMethodHandle", "IsConstructor", 1) => Member::IsConstructor,
        ("System.RuntimeMethodHandle", "GetGenericParameterCount", 1) => Member::GenericParameterCount,
        ("System.RuntimeMethodHandle", "HasMethodInstantiation", 1) => Member::HasMethodInstantiation,
        ("System.RuntimeMethodHandle", "IsGenericMethodDefinition", 1) => Member::IsGenericMethodDefinition,
        ("System.RuntimeMethodHandle", "IsTypicalMethodDefinition", 1) => Member::IsTypicalMethodDefinition,
        ("System.RuntimeMethodHandle", "GetMethodDef", 1) => Member::MethodToken,
        ("System.RuntimeMethodHandle", "GetSlot", 1) => Member::MethodSlot,
        ("System.RuntimeMethodHandle", "IsDynamicMethod", 1) => Member::False,
        ("System.RuntimeMethodHandle", "GetLoaderAllocator" | "GetResolver", 1) => Member::Null,
        ("System.RuntimeMethodHandle", "GetStubIfNeeded", 3) => Member::StubIfNeeded,
        ("System.RuntimeMethodHandle", "GetMethodFromCanonical", 2) => Member::MethodFromCanonical,
        ("System.RuntimeMethodHandle", "ReboxFromNullable", 1) => Member::Identity,
        ("System.RuntimeMethodHandle", "ReboxToNullable", 2) => Member::Identity,
        ("System.RuntimeMethodHandle", "InvokeMethod", 4) => Member::InvokeMethod,
        ("System.RuntimeFieldHandle", "GetName", 1) => Member::FieldName,
        ("System.RuntimeFieldHandle", "_GetUtf8Name" | "GetUtf8NameInternal", 1) => Member::FieldUtf8Name,
        ("System.RuntimeFieldHandle", "GetAttributes", 1) => Member::FieldAttributes,
        ("System.RuntimeFieldHandle", "GetApproxDeclaringType", 1) => Member::FieldDeclaringType,
        ("System.RuntimeFieldHandle", "GetToken", 1) => Member::FieldToken,
        ("System.RuntimeFieldHandle", "GetValue", 5) => Member::FieldGet,
        ("System.RuntimeFieldHandle", "SetValue", 6) => Member::FieldSet,
        ("System.RuntimeFieldHandle", "IsFastPathSupported" | "AcquiresContextFromThis", 1) => Member::False,
        ("System.RuntimeFieldHandle", "GetStaticFieldForGenericType", 2) => Member::FieldForType,
        ("System.RuntimeFieldHandle", "GetLoaderAllocator", 1) => Member::Null,
        ("System.Signature", "GetSignature", 6) => Member::BuildSignature,
        ("System.Signature", "CompareSig", 2) => Member::CompareSignatures,
        ("System.Signature", "GetParameterOffset", 2) => Member::False,
        ("System.Signature", "GetTypeParameterOffset", 3) => Member::False,
        ("System.Signature", "GetCustomModifiersAtOffset", 3) => Member::NoModifiers,
        ("System.Type", "GetTypeFromHandleUnsafe", 1) => Member::TypeFromMethodTable,
        ("System.Delegate", "GetMulticastInvoke" | "GetInvokeMethod", 1) => Member::MulticastStub,
        ("System.Threading.Thread", "GetApartmentStateNative", 1) => Member::ApartmentState,
        ("System.Environment", "get_TickCount", 0) => Member::TickCount,
        ("System.Exception", "GetFrozenStackTrace", 1) => Member::Null,
        ("System.Exception", "IsImmutableAgileException", 1) => Member::False,
        ("System.Exception", "PrepareForForeignExceptionRaise", 0) => Member::Ignore,
        ("System.Exception", "GetExceptionCount", 0) => Member::False,
        ("System.Environment", "get_TickCount64", 0) => Member::TickCount64,
        ("System.Threading.Thread", "SetApartmentStateNative", 2) => Member::SetApartmentState,
        ("System.Runtime.CompilerServices.RuntimeHelpers", "Box", 2) => Member::BoxData,
        _ => return None,
    })
}

pub(super) fn member_qcall(entry: &str, count: usize) -> Option<Member> {
    Some(match (entry, count) {
        ("RuntimeMethodHandle_GetMethodInstantiation", 3) => Member::MethodInstantiation,
        ("Signature_Init", 5) => Member::InitSignature,
        ("RuntimeTypeHandle_GetMethodAt", 2) => Member::MethodAtSlot,
        ("GCInterface_AllocateNewArray", 4) => Member::AllocateArray,
        ("Delegate_InternalEqualMethodHandles", 2) => Member::DelegateMethodsEqual,
        ("ExceptionNative_GetFrozenStackTrace", 2) => Member::Ignore,
        ("Delegate_BindToMethodInfo", 5) => Member::BindMethod,
        ("Delegate_BindToMethodName", 5) => Member::BindMethodName,
        ("AssemblyNative_InitializeAssemblyLoadContext", 3) => Member::Identity,
        ("RuntimeTypeHandle_GetNumVirtualsAndStaticVirtuals", 1) => Member::QCallVirtualCount,
        ("RuntimeTypeHandle_GetInterfaceMethodImplementation", 3) => Member::InterfaceImplementation,
        ("RuntimeMethodHandle_GetTypicalMethodDefinition", 2) => Member::TypicalDefinition,
        ("RuntimeMethodHandle_StripMethodInstantiation", 2) => Member::TypicalDefinition,
        ("RuntimeMethodHandle_ConstructInstantiation", 3) => Member::ConstructInstantiation,
        ("RuntimeMethodHandle_GetFunctionPointer", 1) => Member::FunctionPointer,
        ("ModuleHandle_ResolveMethod", 6) => Member::ResolveMethod,
        ("Array_CreateInstance", 6) => Member::CreateArray,
        ("ExceptionNative_GetMessageFromNativeResources", 2) => Member::NativeMessage,
        ("ExceptionNative_GetMethodFromStackTrace", 2) => Member::Ignore,
        ("MarshalNative_GetDelegateForFunctionPointerInternal", 3) => Member::DelegateForPointer,
        ("ReflectionInvocation_RunClassConstructor", 1) => Member::RunClassConstructor,
        ("ReflectionInvocation_RunModuleConstructor", 1) => Member::Ignore,
        ("Enum_GetValuesAndNames", 4) => Member::EnumValues,
        ("Delegate_InternalAllocLike", 1) => Member::AllocateLike,
        ("ModuleHandle_ResolveType", 7) => Member::ResolveType,
        ("ModuleHandle_ResolveField", 7) => Member::ResolveField,
        ("RuntimeMethodHandle_Destroy", 1) => Member::Ignore,
        ("RuntimeMethodHandle_GetIsCollectible", 1) => Member::False,
        ("RuntimeMethodHandle_IsCAVisibleFromDecoratedType", 4) => Member::True,
        ("ReflectionSerialization_GetUninitializedObject", 2) => Member::Uninitialized,
        ("ReflectionInvocation_CompileMethod", 1) | ("ReflectionInvocation_PrepareMethod", 3) => Member::Ignore,
        _ => return None,
    })
}

struct MethodEntry {
    key: MethodKey,
    ctx: Ctx,
    declaring: Gen,
}

struct FieldEntry {
    module: usize,
    row: u32,
    declaring: Gen,
}

#[derive(Default)]
pub(super) struct Handles {
    methods: Vec<MethodEntry>,
    method_index: HashMap<(MethodKey, Vec<u32>, Vec<u32>, u32), u64>,
    fields: Vec<FieldEntry>,
    field_index: HashMap<(usize, u32, u32), u64>,
}

impl Interpreter<'_> {
    pub(super) fn method_handle(&self, key: &MethodKey, ctx: &Ctx, declaring: &Gen) -> u64 {
        let index_key = (
            key.clone(),
            ctx.types.iter().map(|g| self.types.id(g)).collect(),
            ctx.methods.iter().map(|g| self.types.id(g)).collect(),
            self.types.id(declaring),
        );
        let mut handles = self.handles.borrow_mut();
        if let Some(found) = handles.method_index.get(&index_key) {
            return *found;
        }
        let block: &'static mut [u64] = Box::leak(vec![handles.methods.len() as u64, 0].into_boxed_slice());
        let address = block.as_ptr() as u64;
        handles.methods.push(MethodEntry {
            key: key.clone(),
            ctx: ctx.clone(),
            declaring: declaring.clone(),
        });
        handles.method_index.insert(index_key, address);
        address
    }

    fn method_entry(&self, handle: u64) -> Run<(MethodKey, Ctx, Gen)> {
        if handle == 0 {
            return self.throw("ArgumentNullException");
        }
        let index = unsafe { *(handle as *const u64) } as usize;
        let handles = self.handles.borrow();
        let entry = handles
            .methods
            .get(index)
            .ok_or_else(|| Fault::Unsupported("method handle from outside the fallback".into()))?;
        Ok((entry.key.clone(), entry.ctx.clone(), entry.declaring.clone()))
    }

    fn field_handle(&self, module: usize, row: u32, declaring: &Gen) -> u64 {
        let index_key = (module, row, self.types.id(declaring));
        let mut handles = self.handles.borrow_mut();
        if let Some(found) = handles.field_index.get(&index_key) {
            return *found;
        }
        let block: &'static mut [u64] = Box::leak(vec![handles.fields.len() as u64, 0].into_boxed_slice());
        let address = block.as_ptr() as u64;
        handles.fields.push(FieldEntry {
            module,
            row,
            declaring: declaring.clone(),
        });
        handles.field_index.insert(index_key, address);
        address
    }

    fn field_entry(&self, handle: u64) -> Run<(usize, u32, Gen)> {
        if handle == 0 {
            return self.throw("ArgumentNullException");
        }
        let index = unsafe { *(handle as *const u64) } as usize;
        let handles = self.handles.borrow();
        let entry = handles
            .fields
            .get(index)
            .ok_or_else(|| Fault::Unsupported("field handle from outside the fallback".into()))?;
        Ok((entry.module, entry.row, entry.declaring.clone()))
    }

    fn handle_value(value: V) -> u64 {
        match value {
            V::S(boxed) if boxed != 0 => unsafe { *((boxed + 8) as *const u64) },
            V::P(pointer) => pointer,
            other => other.bits(),
        }
    }

    fn method_value(&self, value: V) -> Run<u64> {
        match value {
            V::O(object) if object != 0 => self.method_info_handle(object),
            other => Ok(Self::handle_value(other)),
        }
    }

    fn field_value(&self, value: V) -> Run<u64> {
        match value {
            V::O(object) if object != 0 => self.field_info_handle(object),
            other => Ok(Self::handle_value(other)),
        }
    }

    fn info_handle(&self, object: u64, names: &[&str]) -> Run<u64> {
        if object == 0 {
            return self.throw("NullReferenceException");
        }
        let exact = self.object_gen(object)?;
        let mut current = Some(exact);
        while let Some(Gen::Named(handle, arguments)) = current {
            let (_, placements) = self
                .model
                .set
                .class_fields(handle)
                .map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
            let metadata = &self.model.set.modules[handle.module].metadata;
            if let Some(placement) = placements.iter().find(|p| {
                metadata
                    .string_cell(TableId::Field, p.field, 1)
                    .is_ok_and(|name| names.contains(&name))
            }) {
                return Ok(unsafe { *((object + 8 + u64::from(placement.offset)) as *const u64) });
            }
            current = map(self.types.base(&Gen::Named(handle, arguments)))?;
        }
        unsupported("reflection object without a handle")
    }

    pub(super) fn method_info_handle_of(&self, object: u64) -> Run<u64> {
        self.method_info_handle(object)
    }

    pub(super) fn method_key_of(&self, handle: u64) -> Run<(MethodKey, Ctx)> {
        self.method_entry(handle).map(|(key, ctx, _)| (key, ctx))
    }

    fn method_info_handle(&self, object: u64) -> Run<u64> {
        self.info_handle(object, &["m_handle", "m_value"])
    }

    fn field_info_handle(&self, object: u64) -> Run<u64> {
        self.info_handle(object, &["m_fieldHandle"])
    }

    fn type_method_range(&self, exact: &Gen) -> Option<(TypeHandle, std::ops::Range<u32>)> {
        let handle = exact.handle()?;
        let range = self.model.set.modules[handle.module].metadata.method_range(handle.row).ok()?;
        Some((handle, range))
    }

    fn method_stub(&self, handle: u64) -> Run<u64> {
        let stub_type = self.corelib_type("System", "RuntimeMethodInfoStub")?;
        let record = self.record_of(&Gen::Named(stub_type, Rc::from(Vec::new())))?;
        let object = runtime::allocate_managed(record, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        let offset = self.class_field_offset("System", "RuntimeMethodInfoStub", "m_value")?;
        unsafe { *((object + u64::from(offset)) as *mut u64) = handle };
        Ok(object)
    }

    fn field_stub(&self, handle: u64) -> Run<u64> {
        let stub_type = self.corelib_type("System", "RuntimeFieldInfoStub")?;
        let record = self.record_of(&Gen::Named(stub_type, Rc::from(Vec::new())))?;
        let object = runtime::allocate_managed(record, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        let offset = self.class_field_offset("System", "RuntimeFieldInfoStub", "m_fieldHandle")?;
        unsafe { *((object + u64::from(offset)) as *mut u64) = handle };
        Ok(object)
    }

    fn runtime_type_array(&self, types: &[Gen]) -> Run<u64> {
        let element = self.corelib_type("System", "RuntimeType")?;
        let record = self.array_record(&Gen::Named(element, Rc::from(Vec::new())))?;
        let array = runtime::allocate_managed(record, types.len() as u64);
        if array == 0 {
            return self.throw("OutOfMemoryException");
        }
        let _pin = Pin::new(&[V::O(array)]);
        for (index, exact) in types.iter().enumerate() {
            let object = self.type_object(exact)?;
            unsafe { *((array + u64::from(ARRAY_DATA) + 8 * index as u64) as *mut u64) = object };
        }
        Ok(array)
    }

    pub(super) fn signature_field(&self, name: &str) -> Run<u32> {
        self.class_field_offset("System", "Signature", &format!("m_{name}"))
            .or_else(|_| self.class_field_offset("System", "Signature", &format!("_{name}")))
    }

    fn build_signature(&self, this: u64, field_handle: u64, handle: u64, declaring_object: u64, blob_pointer: u64, blob_length: i64) -> Run<()> {
        let set = self.model.set;
        let set_field = |name: &str, value: u64| -> Run<()> {
            let offset = self.signature_field(name)?;
            unsafe { *((this + u64::from(offset)) as *mut u64) = value };
            Ok(())
        };
        let set_int = |name: &str, value: i32| -> Run<()> {
            let offset = self.signature_field(name)?;
            unsafe { *((this + u64::from(offset)) as *mut i32) = value };
            Ok(())
        };
        set_field("declaringType", declaring_object)?;
        if handle != 0 {
            let (key, ctx, _) = self.method_entry(handle)?;
            let sig = map(self.model.method_sig(&key))?;
            let arguments = sig
                .parameters
                .iter()
                .map(|p| map(self.types.resolve(key.module, p, &ctx)))
                .collect::<Run<Vec<_>>>()?;
            let returns = match &sig.return_type {
                TypeSig::Void => map(self.types.core("Void"))?,
                other => map(self.types.resolve(key.module, other, &ctx))?,
            };
            let metadata = &set.modules[key.module].metadata;
            let blob = metadata.cell(TableId::MethodDef, key.row, 4).and_then(|b| metadata.blob(b)).unwrap_or(&[]);
            let array = self.runtime_type_array(&arguments)?;
            set_field("arguments", array)?;
            set_field("returnTypeORfieldType", self.type_object(&returns)?)?;
            set_field("sig", blob.as_ptr() as u64)?;
            set_int("csig", blob.len() as i32)?;
            set_int("managedCallingConventionAndArgIteratorFlags", i32::from(*blob.first().unwrap_or(&0)))?;
            set_field("pMethod", handle)?;
        } else if field_handle != 0 {
            let (module, row, declaring) = self.field_entry(field_handle)?;
            let sig = map(self.model.field_signature(module, row))?;
            let exact = map(self.types.resolve(module, &sig, &declaring.context()))?;
            let metadata = &set.modules[module].metadata;
            let blob = metadata.cell(TableId::Field, row, 2).and_then(|b| metadata.blob(b)).unwrap_or(&[]);
            set_field("returnTypeORfieldType", self.type_object(&exact)?)?;
            set_field("sig", blob.as_ptr() as u64)?;
            set_int("csig", blob.len() as i32)?;
        } else {
            let declaring = self.type_gen(V::O(declaring_object))?;
            let module = self.gen_module(&declaring)?;
            let ctx = declaring.context();
            let blob = unsafe { std::slice::from_raw_parts(blob_pointer as *const u8, blob_length.max(0) as usize) };
            let parsed = crate::signature::Signature::parse(blob).map_err(|e| Fault::Unsupported(e.to_string()))?;
            let (returns, parameters) = match parsed {
                crate::signature::Signature::Property { property_type, parameters, .. } => (property_type, parameters),
                crate::signature::Signature::Method(sig) => (sig.return_type, sig.parameters),
                crate::signature::Signature::Field(ty) => (ty, Vec::new()),
                _ => return unsupported("signature blob of an unexpected kind"),
            };
            let resolve = |sig: &TypeSig| -> Run<Gen> {
                match sig {
                    TypeSig::Void => map(self.types.core("Void")),
                    other => map(self.types.resolve(module, other, &ctx)),
                }
            };
            let arguments = parameters.iter().map(resolve).collect::<Run<Vec<_>>>()?;
            let returns = resolve(&returns)?;
            let array = self.runtime_type_array(&arguments)?;
            set_field("arguments", array)?;
            set_field("returnTypeORfieldType", self.type_object(&returns)?)?;
            set_field("sig", blob.as_ptr() as u64)?;
            set_int("csig", blob.len() as i32)?;
            set_int("managedCallingConventionAndArgIteratorFlags", i32::from(*blob.first().unwrap_or(&0)))?;
        }
        Ok(())
    }

    fn bind_delegate(&self, delegate: u64, target: u64, key: MethodKey, ctx: Ctx, flags: i32) -> Run<bool> {
        let delegate = self.non_null(V::O(delegate))?;
        let loaded = self.load_ctx(&key, &ctx)?;
        if (flags & 1 != 0 && !loaded.is_static) || (flags & 2 != 0 && loaded.is_static) {
            return Ok(false);
        }
        if target == 0 && !loaded.is_static && flags & 8 != 0 {
            return Ok(false);
        }
        let (key, ctx) = if target != 0 && loaded.is_virtual {
            self.resolve_virtual(&key, &ctx, target)?
        } else {
            (key, ctx)
        };
        let (target_offset, function_offset) = self.delegate_fields()?;
        let encoded = self.encode_function(&key, &ctx);
        unsafe {
            *((delegate + target_offset) as *mut u64) = target;
            *((delegate + function_offset) as *mut u64) = encoded;
        }
        Ok(true)
    }

    fn find_bindable(&self, owner: &Gen, name: &str, flags: i32, has_target: bool) -> Run<Option<(MethodKey, Ctx)>> {
        let caseless = flags & 32 != 0;
        let mut current = Some(owner.clone());
        while let Some(exact) = current {
            if let Gen::Named(handle, _) = &exact {
                let metadata = &self.model.set.modules[handle.module].metadata;
                if let Ok(range) = metadata.method_range(handle.row) {
                    for row in range {
                        let candidate = metadata.string_cell(TableId::MethodDef, row, 3).unwrap_or("");
                        let matches = if caseless { candidate.eq_ignore_ascii_case(name) } else { candidate == name };
                        if !matches {
                            continue;
                        }
                        let method_flags = metadata.cell(TableId::MethodDef, row, 2).unwrap_or(0);
                        let is_static = method_flags & 0x10 != 0;
                        if (flags & 1 != 0 && !is_static) || (flags & 2 != 0 && is_static) || (is_static && has_target && flags & 8 == 0 && flags & 1 != 0) {
                            continue;
                        }
                        return Ok(Some((MethodKey { module: handle.module, row }, exact.context())));
                    }
                }
            }
            current = map(self.types.base(&exact))?;
        }
        Ok(None)
    }

    pub(super) fn member_call(&self, operation: Member, a: &[V]) -> Run<Option<V>> {
        let set = self.model.set;
        Ok(Some(match operation {
            Member::False => V::I4(0),
            Member::True => V::I4(1),
            Member::Null => V::O(0),
            Member::Identity => a[0],
            Member::Ignore => return Ok(None),
            Member::FirstMethod => {
                let exact = self.type_gen(a[0])?;
                match self.type_method_range(&exact) {
                    Some((handle, mut range)) => match range.next() {
                        Some(row) => V::P(self.method_handle(&MethodKey { module: handle.module, row }, &exact.context(), &exact)),
                        None => V::P(0),
                    },
                    None => V::P(0),
                }
            }
            Member::NextMethod => {
                let slot = a[0].address();
                let current = unsafe { *(slot as *const u64) };
                let next = if current == 0 {
                    0
                } else {
                    let (key, ctx, declaring) = self.method_entry(current)?;
                    match self.type_method_range(&declaring) {
                        Some((_, range)) if key.row + 1 < range.end => {
                            self.method_handle(&MethodKey { module: key.module, row: key.row + 1 }, &ctx, &declaring)
                        }
                        _ => 0,
                    }
                };
                unsafe { *(slot as *mut u64) = next };
                return Ok(None);
            }
            Member::Fields => {
                let exact = self.type_gen(a[0])?;
                let rows: Vec<u32> = match exact.handle() {
                    Some(handle) => set.modules[handle.module]
                        .metadata
                        .field_range(handle.row)
                        .map(|range| range.collect())
                        .unwrap_or_default(),
                    None => Vec::new(),
                };
                let capacity = unsafe { *(a[2].address() as *const i32) };
                unsafe { *(a[2].address() as *mut i32) = rows.len() as i32 };
                if (capacity as usize) < rows.len() {
                    V::I4(0)
                } else {
                    let module = exact.handle().map_or(0, |h| h.module);
                    let buffer = Self::handle_value(a[1]);
                    for (index, row) in rows.iter().enumerate() {
                        let handle = self.field_handle(module, *row, &exact);
                        unsafe { *((buffer + 8 * index as u64) as *mut u64) = handle };
                    }
                    V::I4(1)
                }
            }
            Member::Interfaces => {
                let exact = self.type_gen(a[0])?;
                let interfaces = self.types.all_interfaces(&exact);
                V::O(self.type_array(&interfaces)?)
            }
            Member::BindMethod => {
                let read = |value: V| -> u64 {
                    let slot = unsafe { *(Self::struct_data(value) as *const u64) };
                    if slot == 0 { 0 } else { unsafe { *(slot as *const u64) } }
                };
                let (delegate, target) = (read(a[0]), read(a[1]));
                let (key, ctx, _) = self.method_entry(Self::handle_value(a[2]))?;
                V::I4(i32::from(self.bind_delegate(delegate, target, key, ctx, a[4].int() as i32)?))
            }
            Member::BindMethodName => {
                let read = |value: V| -> u64 {
                    let slot = unsafe { *(Self::struct_data(value) as *const u64) };
                    if slot == 0 { 0 } else { unsafe { *(slot as *const u64) } }
                };
                let (delegate, target) = (read(a[0]), read(a[1]));
                let owner = self.qcall_type(a[2])?;
                let name = unsafe { std::ffi::CStr::from_ptr(a[3].address() as *const std::ffi::c_char) }
                    .to_string_lossy()
                    .into_owned();
                let flags = a[4].int() as i32;
                let found = self.find_bindable(&owner, &name, flags, target != 0)?;
                match found {
                    Some((key, ctx)) => V::I4(i32::from(self.bind_delegate(delegate, target, key, ctx, flags)?)),
                    None => V::I4(0),
                }
            }
            Member::DelegateMethodsEqual => {
                let read = |value: V| -> u64 {
                    let slot = unsafe { *(Self::struct_data(value) as *const u64) };
                    if slot == 0 { 0 } else { unsafe { *(slot as *const u64) } }
                };
                let (left, right) = (read(a[0]), read(a[1]));
                let (_, function) = self.delegate_fields()?;
                let method = |object: u64| if object == 0 { 0 } else { unsafe { *((object + function) as *const u64) } };
                V::I4(i32::from(method(left) == method(right)))
            }
            Member::AllocateArray => {
                let element = match self.method_table_type(a[0].bits())? {
                    Gen::SzArray(inner) => (*inner).clone(),
                    other => other,
                };
                let length = a[1].int();
                if length < 0 {
                    return self.throw("OverflowException");
                }
                let array = runtime::allocate_managed(self.array_record(&element)?, length as u64);
                if array == 0 {
                    return self.throw("OutOfMemoryException");
                }
                self.store_handle(a[3], array);
                return Ok(None);
            }
            Member::MethodSlot => {
                let (key, _, declaring) = self.method_entry(self.method_value(a[0])?)?;
                let table = self.vtable_of(&declaring)?;
                let slot = table.slot_of.get(&key).copied().unwrap_or(table.slots.len());
                V::I4(slot as i32)
            }
            Member::MethodAtSlot => {
                let exact = self.method_table_type(a[0].bits())?;
                let table = self.vtable_of(&exact)?;
                match table.slots.get(a[1].int().max(0) as usize).cloned().flatten() {
                    Some((key, owner)) => V::P(self.method_handle(&key, &owner.context(), &owner)),
                    None => return self.throw("ArgumentOutOfRangeException"),
                }
            }
            Member::InterfaceImplementation => {
                let class = self.qcall_type(a[0])?;
                let (key, ctx, _) = self.method_entry(Self::handle_value(a[2]))?;
                match self.find_interface_implementation(&class, &key, &ctx) {
                    Ok((target, context)) => {
                        let owner = map(self.model.method_owner(&target))?;
                        V::P(self.method_handle(&target, &context, &Gen::Named(owner, context.types.clone())))
                    }
                    Err(Fault::Unsupported(_)) => V::P(0),
                    Err(other) => return Err(other),
                }
            }
            Member::QCallVirtualCount => {
                let exact = self.qcall_type(a[0])?;
                V::I4(self.vtable_of(&exact).map_or(0, |table| table.slots.len() as i32))
            }
            Member::VirtualCount => {
                let exact = self.type_gen(a[0])?;
                V::I4(self.vtable_of(&exact).map_or(0, |table| table.slots.len() as i32))
            }
            Member::MethodAttributes | Member::MethodImplAttributes | Member::MethodName | Member::MethodUtf8Name => {
                let handle = match operation {
                    Member::MethodImplAttributes => self.method_info_handle(a[0].address())?,
                    _ => self.method_value(a[0])?,
                };
                let (key, _, _) = self.method_entry(handle)?;
                let metadata = &set.modules[key.module].metadata;
                match operation {
                    Member::MethodAttributes => V::I4(metadata.cell(TableId::MethodDef, key.row, 2).unwrap_or(0) as i32),
                    Member::MethodImplAttributes => V::I4(metadata.cell(TableId::MethodDef, key.row, 1).unwrap_or(0) as i32),
                    Member::MethodName => {
                        let name = metadata.string_cell(TableId::MethodDef, key.row, 3).unwrap_or("");
                        V::O(runtime::new_string(&name.encode_utf16().collect::<Vec<_>>()))
                    }
                    _ => V::P(metadata.string_cell(TableId::MethodDef, key.row, 3).map_or(0, |s| s.as_ptr() as u64)),
                }
            }
            Member::MethodDeclaringType => {
                let (_, _, declaring) = self.method_entry(self.method_value(a[0])?)?;
                V::O(self.type_object(&declaring)?)
            }
            Member::IsConstructor => {
                let (key, _, _) = self.method_entry(self.method_value(a[0])?)?;
                let metadata = &set.modules[key.module].metadata;
                let name = metadata.string_cell(TableId::MethodDef, key.row, 3).unwrap_or("");
                let flags = metadata.cell(TableId::MethodDef, key.row, 2).unwrap_or(0);
                V::I4(i32::from(name == ".ctor" && flags & STATIC == 0))
            }
            Member::GenericParameterCount | Member::HasMethodInstantiation | Member::IsGenericMethodDefinition => {
                let (key, ctx, _) = self.method_entry(self.method_value(a[0])?)?;
                let parameters = map(self.model.method_sig(&key))?.generic_parameters as i32;
                V::I4(match operation {
                    Member::GenericParameterCount => parameters,
                    Member::HasMethodInstantiation => i32::from(!ctx.methods.is_empty()),
                    _ => i32::from(parameters > 0 && ctx.methods.is_empty()),
                })
            }
            Member::IsTypicalMethodDefinition => {
                let (_, ctx, _) = self.method_entry(self.method_info_handle(a[0].address())?)?;
                V::I4(i32::from(ctx.methods.is_empty() && ctx.types.is_empty()))
            }
            Member::MethodToken => {
                let (key, _, _) = self.method_entry(self.method_info_handle(a[0].address())?)?;
                V::I4((0x0600_0000 | key.row) as i32)
            }
            Member::StubIfNeeded => {
                let (key, _, _) = self.method_entry(self.method_value(a[0])?)?;
                let declaring = self.type_gen(a[1])?;
                let methods = match a[2].address() {
                    0 => Vec::new(),
                    array => {
                        let length = unsafe { *((array + 8) as *const u32) } as u64;
                        (0..length)
                            .map(|i| self.type_gen(V::O(unsafe { *((array + u64::from(ARRAY_DATA) + 8 * i) as *const u64) })))
                            .collect::<Run<Vec<_>>>()?
                    }
                };
                let ctx = Ctx {
                    types: declaring.arguments(),
                    methods: Rc::from(methods),
                };
                V::P(self.method_handle(&key, &ctx, &declaring))
            }
            Member::MethodFromCanonical => {
                let (key, ctx, _) = self.method_entry(self.method_value(a[0])?)?;
                let declaring = self.type_gen(a[1])?;
                let ctx = Ctx {
                    types: declaring.arguments(),
                    methods: ctx.methods,
                };
                V::P(self.method_handle(&key, &ctx, &declaring))
            }
            Member::MethodInstantiation => {
                let (_, ctx, _) = self.method_entry(self.method_value(a[0])?)?;
                let array = self.runtime_type_array(&ctx.methods)?;
                self.store_handle(a[1], array);
                return Ok(None);
            }
            Member::TypicalDefinition => {
                let (key, _, declaring) = self.method_entry(self.method_value(a[0])?)?;
                let definition = match &declaring {
                    Gen::Named(handle, _) => Gen::Named(*handle, Rc::from(Vec::new())),
                    other => other.clone(),
                };
                let handle = self.method_handle(&key, &Ctx::default(), &definition);
                let stub = self.method_stub(handle)?;
                self.store_handle(a[1], stub);
                return Ok(None);
            }
            Member::ConstructInstantiation => {
                let (_, ctx, _) = self.method_entry(self.method_value(a[0])?)?;
                let text = if ctx.methods.is_empty() {
                    String::new()
                } else {
                    let names: Vec<String> = ctx.methods.iter().map(|g| self.types.full_name(g)).collect();
                    format!("[{}]", names.join(","))
                };
                self.store_text(a[2], &text);
                return Ok(None);
            }
            Member::ResolveMethod | Member::ResolveType | Member::ResolveField => {
                let module = self.qcall_module(a[0])?;
                let token = Token(a[1].int() as u32);
                let read_list = |pointer: u64, count: i64| -> Run<Vec<Gen>> {
                    (0..count.max(0) as u64)
                        .map(|i| self.method_table_type(unsafe { *((pointer + 8 * i) as *const u64) }))
                        .collect()
                };
                let ctx = Ctx {
                    types: Rc::from(read_list(a[2].address(), a[3].int())?),
                    methods: Rc::from(read_list(a[4].address(), a[5].int())?),
                };
                match operation {
                    Member::ResolveMethod => {
                        let (key, call_ctx) = self.method_inst(module, token, &ctx)?;
                        let owner = map(self.model.method_owner(&key))?;
                        let declaring = Gen::Named(owner, call_ctx.types.clone());
                        V::P(self.method_handle(&key, &call_ctx, &declaring))
                    }
                    Member::ResolveType => {
                        let exact = map(self.types.resolve_token(module, token, &ctx))?;
                        let object = self.type_object(&exact)?;
                        self.store_handle(a[6], object);
                        return Ok(None);
                    }
                    _ => {
                        let field = self.field_ref(module, token, &ctx)?;
                        let handle = self.field_handle(field.module, field.row, &field.owner);
                        let stub = self.field_stub(handle)?;
                        self.store_handle(a[6], stub);
                        return Ok(None);
                    }
                }
            }
            Member::MulticastStub => V::P(1),
            Member::NativeMessage => {
                let text = match a[0].int() {
                    0 => "Exception of type 'System.Threading.ThreadAbortException' was thrown.",
                    1 => "Arithmetic operation resulted in an overflow.",
                    2 => "Insufficient memory to continue the execution of the program.",
                    _ => "Operation caused an exception.",
                };
                self.store_text(a[1], text);
                return Ok(None);
            }
            Member::DelegateForPointer => {
                let exact = self.qcall_type(a[1])?;
                let object = self.native_delegate(a[0].bits(), &exact)?;
                self.store_handle(a[2], object);
                return Ok(None);
            }
            Member::TickCount => V::I4(crate::pinvoke::tick_count() as i32),
            Member::TickCount64 => V::I8(crate::pinvoke::tick_count() as i64),
            Member::RunClassConstructor => {
                let exact = self.qcall_type(a[0])?;
                self.ensure_initialized_gen(&exact)?;
                return Ok(None);
            }
            Member::ApartmentState => V::I4(crate::pinvoke::apartment_state()),
            Member::SetApartmentState => V::I4(crate::pinvoke::enter_apartment(a[1].int() as i32)),
            Member::EnumValues => {
                let exact = self.qcall_type(a[0])?;
                let Some(handle) = exact.handle() else {
                    return unsupported("enum values of a type that is not named");
                };
                let metadata = &set.modules[handle.module].metadata;
                let mut entries: Vec<(u64, &str)> = Vec::new();
                for row in metadata.field_range(handle.row).map_err(|e| Fault::Unsupported(e.to_string()))? {
                    if metadata.cell(TableId::Field, row, 0).unwrap_or(0) & 0x50 != 0x50 {
                        continue;
                    }
                    let constant = (1..=metadata.rows(TableId::Constant)).find(|&c| {
                        metadata
                            .coded(TableId::Constant, c, 1)
                            .ok()
                            .flatten()
                            .is_some_and(|p| p.table() == Some(TableId::Field) && p.row() == row)
                    });
                    let blob = constant
                        .and_then(|c| metadata.cell(TableId::Constant, c, 2).ok())
                        .and_then(|b| metadata.blob(b).ok())
                        .unwrap_or(&[]);
                    let mut value = 0u64;
                    unsafe { std::ptr::copy_nonoverlapping(blob.as_ptr(), (&raw mut value).cast(), blob.len().min(8)) };
                    entries.push((value, metadata.string_cell(TableId::Field, row, 1).unwrap_or("")));
                }
                let underlying = metadata
                    .field_range(handle.row)
                    .ok()
                    .and_then(|mut range| range.find(|&row| metadata.string_cell(TableId::Field, row, 1).ok() == Some("value__")))
                    .ok_or_else(|| Fault::Unsupported("enum without a value field".into()))?;
                let storage_sig = map(self.model.field_signature(handle.module, underlying))?;
                let storage = map(self.types.resolve(handle.module, &storage_sig, &Ctx::default()))?;
                let (_, size) = self.element_size(&storage)?;
                let record = self.array_record(&storage)?;
                let values = runtime::allocate_managed(record, entries.len() as u64);
                if values == 0 {
                    return self.throw("OutOfMemoryException");
                }
                for (index, (value, _)) in entries.iter().enumerate() {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            (value as *const u64).cast::<u8>(),
                            (values + u64::from(ARRAY_DATA) + u64::from(size) * index as u64) as *mut u8,
                            size as usize,
                        )
                    };
                }
                let _pin = Pin::new(&[V::O(values)]);
                self.store_handle(a[1], values);
                if a[3].int() != 0 {
                    let names: Vec<String> = entries.iter().map(|(_, name)| (*name).to_owned()).collect();
                    let array = self.string_array(&names)?;
                    self.store_handle(a[2], array);
                }
                return Ok(None);
            }
            Member::AllocateLike => {
                let slot = Self::handle_value(a[0]) as *mut u64;
                let source = unsafe { *slot };
                let exact = self.object_gen(self.non_null(V::O(source))?)?;
                let object = runtime::allocate_managed(self.record_of(&exact)?, 0);
                if object == 0 {
                    return self.throw("OutOfMemoryException");
                }
                unsafe { *slot = object };
                return Ok(None);
            }
            Member::CreateArray => {
                let given = self.qcall_type(a[0])?;
                let rank = a[1].int();
                let lower = a[3].address();
                let element = match (&given, a[4].int() != 0) {
                    (Gen::SzArray(inner) | Gen::Array(inner, _), true) => (**inner).clone(),
                    _ => given.clone(),
                };
                let lower_zero = lower == 0 || unsafe { *(lower as *const i32) } == 0;
                if rank != 1 || !lower_zero || matches!(given, Gen::Array(..)) {
                    return unsupported("multidimensional arrays from reflection");
                }
                let length = unsafe { *(a[2].address() as *const i32) };
                let record = self.array_record(&element)?;
                let array = runtime::allocate_managed(record, length.max(0) as u64);
                if array == 0 {
                    return self.throw("OutOfMemoryException");
                }
                self.store_handle(a[5], array);
                return Ok(None);
            }
            Member::FunctionPointer => {
                let (key, ctx, _) = self.method_entry(self.method_value(a[0])?)?;
                V::P(self.encode_function(&key, &ctx))
            }
            Member::Uninitialized => {
                let exact = self.qcall_type(a[0])?;
                let object = match self.gen_ty(&exact)? {
                    Ty::Prim(VarType::Ref) => {
                        let object = runtime::allocate_managed(self.record_of(&exact)?, 0);
                        if object == 0 {
                            return self.throw("OutOfMemoryException");
                        }
                        object
                    }
                    _ => self.allocate_box(&exact)?,
                };
                self.store_handle(a[1], object);
                return Ok(None);
            }
            Member::FieldName | Member::FieldToken => {
                let (module, row, _) = self.field_entry(self.field_info_handle(a[0].address())?)?;
                match operation {
                    Member::FieldName => {
                        let name = set.modules[module].metadata.string_cell(TableId::Field, row, 1).unwrap_or("");
                        V::O(runtime::new_string(&name.encode_utf16().collect::<Vec<_>>()))
                    }
                    _ => V::I4((0x0400_0000 | row) as i32),
                }
            }
            Member::FieldUtf8Name | Member::FieldAttributes | Member::FieldDeclaringType => {
                let (module, row, declaring) = self.field_entry(self.field_value(a[0])?)?;
                let metadata = &set.modules[module].metadata;
                match operation {
                    Member::FieldUtf8Name => V::P(metadata.string_cell(TableId::Field, row, 1).map_or(0, |s| s.as_ptr() as u64)),
                    Member::FieldAttributes => V::I4(metadata.cell(TableId::Field, row, 0).unwrap_or(0) as i32),
                    _ => V::O(self.type_object(&declaring)?),
                }
            }
            Member::FieldForType => {
                let (module, row, _) = self.field_entry(self.field_value(a[0])?)?;
                let declaring = self.type_gen(a[1])?;
                V::P(self.field_handle(module, row, &declaring))
            }
            Member::FieldGet | Member::FieldSet => {
                let (module, row, declaring) = self.field_entry(self.field_info_handle(a[0].address())?)?;
                let target = a[1];
                let target = match target {
                    V::O(object) if object != 0 && self.types.is_value_type(&declaring) => V::S(object),
                    other => other,
                };
                let (address, ty) = self.field_address_in(module, Token::new(TableId::Field, row), &declaring.context(), target)?;
                let sig = map(self.model.field_signature(module, row))?;
                let exact = map(self.types.resolve(module, &sig, &declaring.context()))?;
                if matches!(operation, Member::FieldGet) {
                    let value = self.read_value(address, &ty, &[target])?;
                    match &ty {
                        Ty::Prim(VarType::Ref) => value,
                        _ => V::O(self.box_any(&exact, &ty, value)?),
                    }
                } else {
                    let value = a[2];
                    match &ty {
                        Ty::Prim(VarType::Ref) => write(address, VarType::Ref, value),
                        Ty::Prim(var) => {
                            let boxed = value.address();
                            if boxed != 0 {
                                write(address, *var, read(boxed + 8, *var));
                            }
                        }
                        Ty::Struct(_) => self.write_value(address, &ty, value)?,
                    }
                    return Ok(None);
                }
            }
            Member::BuildSignature => {
                let this = self.non_null(a[0])?;
                let handle = match a[4].address() {
                    0 => 0,
                    method_info => self.method_info_handle(method_info)?,
                };
                self.build_signature(this, Self::handle_value(a[3]), handle, a[5].address(), a[1].address(), a[2].int())?;
                return Ok(None);
            }
            Member::InitSignature => {
                let slot = unsafe { *(Self::struct_data(a[0]) as *const u64) };
                let this = if slot == 0 { 0 } else { unsafe { *(slot as *const u64) } };
                let this = self.non_null(V::O(this))?;
                let offset = self.signature_field("declaringType")?;
                let declaring_object = unsafe { *((this + u64::from(offset)) as *const u64) };
                self.build_signature(this, Self::handle_value(a[3]), Self::handle_value(a[4]), declaring_object, a[1].address(), a[2].int())?;
                return Ok(None);
            }
            Member::CompareSignatures => {
                let read_sig = |object: u64| -> Run<(u64, i32)> {
                    let sig = self.signature_field("sig")?;
                    let length = self.signature_field("csig")?;
                    Ok(unsafe { (*((object + u64::from(sig)) as *const u64), *((object + u64::from(length)) as *const i32)) })
                };
                let (left, right) = (read_sig(self.non_null(a[0])?)?, read_sig(self.non_null(a[1])?)?);
                let same = left.1 == right.1
                    && unsafe {
                        std::slice::from_raw_parts(left.0 as *const u8, left.1.max(0) as usize)
                            == std::slice::from_raw_parts(right.0 as *const u8, right.1.max(0) as usize)
                    };
                V::I4(i32::from(same))
            }
            Member::NoModifiers => V::O(self.type_array(&[])?),
            Member::TypeFromMethodTable => match a[0].bits() {
                0 => V::O(0),
                handle => V::O(self.type_object(&self.method_table_type(handle)?)?),
            },
            Member::BoxData => {
                let (type_value, data) = match a[1] {
                    V::S(_) => (a[1], a[0].address()),
                    other => (a[0], other.address()),
                };
                let exact = self.type_gen(type_value)?;
                let ty = self.gen_ty(&exact)?;
                let value = self.read_value(data, &ty, &[])?;
                V::O(self.box_any(&exact, &ty, value)?)
            }
            Member::InvokeMethod => {
                let target = a[0].address();
                let arguments = a[1].address();
                let signature = self.non_null(a[2])?;
                let is_constructor = a[3].int() != 0;
                let offset = self.signature_field("pMethod")?;
                let handle = unsafe { *((signature + u64::from(offset)) as *const u64) };
                let (key, ctx, _) = self.method_entry(handle)?;
                let loaded = self.load_ctx(&key, &ctx)?;
                let mut values = Vec::with_capacity(loaded.parameters.len());
                let skip = usize::from(!loaded.is_static);
                if !loaded.is_static {
                    values.push(V::O(target));
                }
                let mut keep = Vec::new();
                for (index, ty) in loaded.parameters[skip..].iter().enumerate() {
                    let pointer = unsafe { *((arguments + 8 * index as u64) as *const u64) };
                    let value = self.read_value(pointer, ty, &keep)?;
                    keep.push(value);
                    values.push(value);
                }
                let _pin = Pin::new(&values);
                let result = self.invoke_ctx(&key, &ctx, &values)?;
                if is_constructor {
                    return Ok(Some(V::O(target)));
                }
                match (result, &loaded.returns) {
                    (Some(value), Some(Ty::Prim(VarType::Ref))) => value,
                    (Some(value), Some(ty)) => {
                        let sig = map(self.model.method_sig(&key))?;
                        let exact = map(self.types.resolve(key.module, &sig.return_type, &ctx))?;
                        V::O(self.box_any(&exact, ty, value)?)
                    }
                    _ => V::O(0),
                }
            }
        }))
    }
}

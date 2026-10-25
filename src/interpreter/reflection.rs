use super::*;
use crate::loader::AssemblySet;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Reflect {
    EntryAssembly,
    SimpleName,
    Version,
    Locale,
    PublicKey,
    Flags,
    HashAlgorithm,
    FullName,
    Location,
    CodeBase,
    ImageRuntimeVersion,
    IsCollectible,
    LoadContext,
    Resource,
    ResourceNames,
    ResourceInfo,
    ExecutingAssembly,
    LoadedAssemblies,
    TypeCore,
    Load,
    ManifestModule,
    HandleElementKind,
    ComInterface,
    ReleaseCom(bool),
    Modules,
    IsDynamic,
    AssemblyToken,
    TypeModule,
    TypeAssembly,
    TypeToken,
    TypeAttributes,
    BaseType,
    ElementKind,
    InstanceOfType,
    DeclaringType,
    ElementType,
    ArrayRank,
    ContainsGenericVariables,
    SameType,
    Utf8Name,
    StreamVersion,
    ModuleToken,
    ScopeName,
    QualifiedName,
    PeKind,
    MetadataScope,
    MdName,
    MdNamespace,
    MdParent,
    MdCustomAttribute,
    MdMethodSig,
    MdFieldSig,
    MdMemberRefSig,
    MdSignature,
    MdDefault,
    MdFieldFlags,
    MdProperty,
    MdEvent,
    MdParam,
    MdGenericParam,
    MdClassLayout,
    MdScope,
    MdValid,
    MdPInvoke,
    MdFieldMarshal,
    MdMarshalAs,
    MdEnum,
    ObjectMethodTable,
    PrimitiveElementKind,
    InstanceFieldBytes,
    Ignore,
    SizeOf,
    GenericDefinition,
    Instantiation,
    Instantiate,
    MakeVector,
    MakeArray,
    MakeByRef,
    MakePointer,
    ConstructName,
    IsVisible,
    AnotherParameter,
    StructureToPointer,
    PointerToStructure,
    DestroyStructure,
    Instance,
    ClassFromClsid,
    ComObjectGuid,
    ModuleType,
    OsThreadId,
    WaitOne,
    WaitMany,
    SignalAndWait,
    SpinIterations,
    Member(super::members::Member),
    CreateAttribute,
    AttributeNamed,
    AttributeUsage,
}

pub(super) fn qcall(entry: &str, count: usize) -> Option<Reflect> {
    if let Some(found) = super::members::member_qcall(entry, count) {
        return Some(Reflect::Member(found));
    }
    if let Some(found) = type_qcall(entry, count) {
        return Some(found);
    }
    Some(match (entry, count) {
        ("AssemblyNative_GetEntryAssembly", 1) => Reflect::EntryAssembly,
        ("AssemblyNative_GetSimpleName", 2) => Reflect::SimpleName,
        ("AssemblyNative_GetVersion", 5) => Reflect::Version,
        ("AssemblyNative_GetLocale", 2) => Reflect::Locale,
        ("AssemblyNative_GetPublicKey", 2) => Reflect::PublicKey,
        ("AssemblyNative_GetFlags", 1) => Reflect::Flags,
        ("AssemblyNative_GetHashAlgorithm", 1) => Reflect::HashAlgorithm,
        ("AssemblyNative_GetFullName", 2) => Reflect::FullName,
        ("AssemblyNative_GetLocation", 2) => Reflect::Location,
        ("AssemblyNative_GetCodeBase", 2) => Reflect::CodeBase,
        ("AssemblyNative_GetImageRuntimeVersion", 2) => Reflect::ImageRuntimeVersion,
        ("AssemblyNative_GetIsCollectible", 1) => Reflect::IsCollectible,
        ("AssemblyNative_GetLoadContextForAssembly", 1) => Reflect::LoadContext,
        ("AssemblyNative_GetResource", 3) => Reflect::Resource,
        ("AssemblyNative_GetManifestResourceNames", 2) => Reflect::ResourceNames,
        ("AssemblyNative_GetManifestResourceInfo", 4) => Reflect::ResourceInfo,
        ("AssemblyNative_GetExecutingAssembly", 2) => Reflect::ExecutingAssembly,
        ("AssemblyNative_GetLoadedAssemblies", 1) => Reflect::LoadedAssemblies,
        ("AssemblyNative_GetModules", 4) => Reflect::Modules,
        ("TypeHandle_GetCorElementType", 1) => Reflect::HandleElementKind,
        ("MarshalNative_GetComInterfaceForObject", 3) => Reflect::ComInterface,
        ("MarshalNative_ReleaseComObject", 1) => Reflect::ReleaseCom(false),
        ("MarshalNative_FinalReleaseComObject", 1) => Reflect::ReleaseCom(true),
        ("AssemblyNative_GetTypeCore", 5) => Reflect::TypeCore,
        ("AssemblyNative_InternalLoad", 6) => Reflect::Load,
        ("RuntimeModule_GetScopeName", 2) => Reflect::ScopeName,
        ("RuntimeModule_GetFullyQualifiedName", 2) => Reflect::QualifiedName,
        ("ModuleHandle_GetPEKind", 3) => Reflect::PeKind,
        ("MetadataImport_Enum", 6) => Reflect::MdEnum,
        _ => return None,
    })
}

pub(super) fn internal(type_name: &str, name: &str, count: usize) -> Option<Reflect> {
    if let Some(found) = super::members::member_internal(type_name, name, count) {
        return Some(Reflect::Member(found));
    }
    Some(match (type_name, name, count) {
        ("System.Reflection.RuntimeAssembly", "GetManifestModule", 1) => Reflect::ManifestModule,
        ("System.Reflection.RuntimeAssembly", "FCallIsDynamic" | "GetIsDynamic", 1) => Reflect::IsDynamic,
        ("System.Reflection.RuntimeAssembly", "GetToken", 1) => Reflect::AssemblyToken,
        ("System.RuntimeTypeHandle", "GetModule", 1) => Reflect::TypeModule,
        ("System.RuntimeTypeHandle", "GetAssembly", 1) => Reflect::TypeAssembly,
        ("System.RuntimeTypeHandle", "GetToken", 1) => Reflect::TypeToken,
        ("System.RuntimeTypeHandle", "GetAttributes", 1) => Reflect::TypeAttributes,
        ("System.RuntimeTypeHandle", "GetBaseType", 1) => Reflect::BaseType,
        ("System.RuntimeTypeHandle", "GetCorElementType", 1) => Reflect::ElementKind,
        ("System.RuntimeTypeHandle" | "System.RuntimeType", "IsInstanceOfType", 2) => Reflect::InstanceOfType,
        ("System.RuntimeTypeHandle", "GetDeclaringType", 1) => Reflect::DeclaringType,
        ("System.RuntimeTypeHandle", "GetElementType", 1) => Reflect::ElementType,
        ("System.RuntimeTypeHandle", "GetArrayRank", 1) => Reflect::ArrayRank,
        ("System.RuntimeTypeHandle", "ContainsGenericVariables", 1) => Reflect::ContainsGenericVariables,
        ("System.RuntimeTypeHandle", "IsEquivalentTo" | "CompareCanonicalHandles", 2) => Reflect::SameType,
        ("System.RuntimeTypeHandle", "_GetUtf8Name" | "GetUtf8NameInternal", 1) => Reflect::Utf8Name,
        ("System.ModuleHandle", "GetMDStreamVersion", 1) => Reflect::StreamVersion,
        ("System.ModuleHandle", "GetToken", 1) => Reflect::ModuleToken,
        ("System.Reflection.MetadataImport", name, count) => return metadata_internal(name, count),
        ("System.Runtime.CompilerServices.RuntimeHelpers", "GetMethodTable", 1) => Reflect::ObjectMethodTable,
        ("System.Runtime.InteropServices.Marshal", "StructureToPtr", 3) => Reflect::StructureToPointer,
        ("System.Threading.WaitHandle", "WaitOneCore", 3) => Reflect::WaitOne,
        ("System.Threading.Thread", "get_OptimalMaxSpinWaitsPerSpinIteration", 0) => Reflect::SpinIterations,
        ("System.Runtime.InteropServices.Marshal", "PtrToStructureHelper", 3) => Reflect::PointerToStructure,
        ("System.Runtime.InteropServices.Marshal", "DestroyStructure", 2) => Reflect::DestroyStructure,
        ("System.Runtime.CompilerServices.MethodTable", "GetPrimitiveCorElementType", 1) => Reflect::PrimitiveElementKind,
        ("System.Runtime.CompilerServices.MethodTable", "GetNumInstanceFieldBytes", 1) => Reflect::InstanceFieldBytes,
        _ => return None,
    })
}

impl Interpreter<'_> {
    pub(super) fn reflect(&self, operation: Reflect, a: &[V], _ctx: &Ctx) -> Run<Option<V>> {
        let set = self.model.set;
        Ok(Some(match operation {
            Reflect::Member(member) => return self.member_call(member, a),
            Reflect::CreateAttribute => return self.create_attribute(a),
            Reflect::AttributeNamed => return self.attribute_named(a),
            Reflect::AttributeUsage => {
                let blob = unsafe { std::slice::from_raw_parts(a[0].address() as *const u8, a[1].int().max(0) as usize) };
                let (targets, inherited, multiple) = attribute_usage(blob);
                unsafe {
                    *(a[2].address() as *mut i32) = targets;
                    *(a[3].address() as *mut i32) = multiple;
                    *(a[4].address() as *mut i32) = inherited;
                }
                V::I4(1)
            }
            Reflect::MdEnum => return self.metadata_enum(a),
            Reflect::ObjectMethodTable => {
                let object = self.non_null(a[0])?;
                V::P(self.method_table(&self.object_gen(object)?)?)
            }
            Reflect::PrimitiveElementKind => V::I4(self.primitive_element_kind(&self.method_table_type(a[0].bits())?)),
            Reflect::InstanceFieldBytes => {
                let exact = self.method_table_type(a[0].bits())?;
                V::I4(match self.gen_ty(&exact)? {
                    Ty::Struct(layout) => layout.size as i32,
                    Ty::Prim(VarType::Ref) if !self.types.is_value_type(&exact) => self
                        .record_of(&exact)
                        .map(|record| (unsafe { *(record as *const u64) } & 0xFFFF_FFFF) as i32 - 8)
                        .unwrap_or(0),
                    Ty::Prim(var) => ElementKind::from_var(var).map_or(8, ElementKind::size) as i32,
                })
            }
            Reflect::StreamVersion => V::I4(0x0002_0000),
            Reflect::ModuleToken => V::I4(1),
            Reflect::ScopeName => {
                let module = self.qcall_module(a[0])?;
                self.store_text(a[1], set.modules[module].metadata.string_cell(TableId::Module, 1, 1).unwrap_or(""));
                return Ok(None);
            }
            Reflect::QualifiedName => {
                let module = self.qcall_module(a[0])?;
                self.store_text(a[1], &set.modules[module].path.display().to_string());
                return Ok(None);
            }
            Reflect::PeKind => {
                unsafe {
                    *(a[1].address() as *mut i32) = 1;
                    *(a[2].address() as *mut i32) = 0x14C;
                }
                return Ok(None);
            }
            Reflect::EntryAssembly => {
                let assembly = self.assembly_object(self.entry_module.get())?;
                self.store_handle(a[0], assembly);
                return Ok(None);
            }
            Reflect::SimpleName => {
                let module = self.qcall_assembly(a[0])?;
                self.store_text(a[1], set.modules[module].name);
                return Ok(None);
            }
            Reflect::Version => {
                let module = self.qcall_assembly(a[0])?;
                for (index, value) in a[1..5].iter().enumerate() {
                    let part = assembly_cell(set, module, index + 1);
                    unsafe { *(value.address() as *mut i32) = part as u16 as i32 };
                }
                return Ok(None);
            }
            Reflect::Locale => {
                let module = self.qcall_assembly(a[0])?;
                self.store_text(a[1], assembly_culture(set, module));
                return Ok(None);
            }
            Reflect::PublicKey => {
                let module = self.qcall_assembly(a[0])?;
                let key = assembly_public_key(set, module);
                let array = self.byte_array(key)?;
                self.store_handle(a[1], array);
                return Ok(None);
            }
            Reflect::Flags => {
                let module = self.qcall_assembly(a[0])?;
                V::I4(assembly_cell(set, module, 5) as i32)
            }
            Reflect::HashAlgorithm => {
                let module = self.qcall_assembly(a[0])?;
                V::I4(assembly_cell(set, module, 0) as i32)
            }
            Reflect::FullName => {
                let module = self.qcall_assembly(a[0])?;
                self.store_text(a[1], &assembly_full_name(set, module));
                return Ok(None);
            }
            Reflect::Location | Reflect::CodeBase => {
                let module = self.qcall_assembly(a[0])?;
                self.store_text(a[1], &set.modules[module].path.display().to_string());
                if matches!(operation, Reflect::CodeBase) {
                    V::I4(1)
                } else {
                    return Ok(None);
                }
            }
            Reflect::ImageRuntimeVersion => {
                self.store_text(a[1], "v4.0.30319");
                return Ok(None);
            }
            Reflect::IsCollectible => V::I4(0),
            Reflect::LoadContext => V::P(0),
            Reflect::Resource => {
                let module = self.qcall_assembly(a[0])?;
                let name = wide_text(a[1].address());
                match self.resource(module, &name) {
                    Some(data) => {
                        unsafe { *(a[2].address() as *mut u32) = data.len() as u32 };
                        V::P(data.as_ptr() as u64)
                    }
                    None => {
                        unsafe { *(a[2].address() as *mut u32) = 0 };
                        V::P(0)
                    }
                }
            }
            Reflect::ResourceNames => {
                let module = self.qcall_assembly(a[0])?;
                let names: Vec<String> = crate::resources::manifest(&set.modules[module])
                    .map(|list| list.into_iter().map(|(name, _)| name.to_owned()).collect())
                    .unwrap_or_default();
                let array = self.string_array(&names)?;
                self.store_handle(a[1], array);
                return Ok(None);
            }
            Reflect::ResourceInfo => {
                let module = self.qcall_assembly(a[0])?;
                let name = wide_text(a[1].address());
                V::I4(if self.resource(module, &name).is_some() { 5 } else { 0 })
            }
            Reflect::ExecutingAssembly => {
                let mark = unsafe { *(Self::struct_data(a[0]) as *const u64) };
                let depth = if mark == 0 { 1 } else { unsafe { *(mark as *const i32) }.max(1) as usize };
                let module = self.caller_module(depth);
                let assembly = self.assembly_object(module)?;
                self.store_handle(a[1], assembly);
                return Ok(None);
            }
            Reflect::ReleaseCom(last) => {
                let slot = unsafe { *(Self::struct_data(a[0]) as *const u64) };
                let object = if slot == 0 { 0 } else { unsafe { *(slot as *const u64) } };
                let remaining = self.rcw_release(object, last);
                if last {
                    return Ok(None);
                }
                V::I4(remaining as i32)
            }
            Reflect::ComInterface => {
                let slot = unsafe { *(Self::struct_data(a[0]) as *const u64) };
                let object = if slot == 0 { 0 } else { unsafe { *(slot as *const u64) } };
                if object == 0 {
                    return self.throw("ArgumentNullException");
                }
                let interface = self.qcall_type(a[1])?;
                V::I8(self.com_wrapper(object, &interface)? as i64)
            }
            Reflect::Modules => {
                let module = self.qcall_assembly(a[0])?;
                let handle = self.corelib_type("System.Reflection", "RuntimeModule")?;
                let record = self.array_record(&Gen::Named(handle, Rc::from(Vec::new())))?;
                let array = runtime::allocate_managed(record, 1);
                if array == 0 {
                    return self.throw("OutOfMemoryException");
                }
                let _pin = Pin::new(&[V::O(array)]);
                let object = self.module_object(module)?;
                unsafe { *((array + u64::from(ARRAY_DATA)) as *mut u64) = object };
                self.store_handle(a[3], array);
                return Ok(None);
            }
            Reflect::LoadedAssemblies => {
                let handle = self.corelib_type("System.Reflection", "Assembly")?;
                let record = self.array_record(&Gen::Named(handle, Rc::from(Vec::new())))?;
                self.mark_used(self.entry_module.get());
                if let Ok(core) = self.corelib_type("System", "Object") {
                    self.mark_used(core.module);
                }
                let mut used: Vec<usize> = self.used_modules.borrow().iter().copied().collect();
                used.sort_unstable();
                let array = runtime::allocate_managed(record, used.len() as u64);
                if array == 0 {
                    return self.throw("OutOfMemoryException");
                }
                let _pin = Pin::new(&[V::O(array)]);
                for (slot, module) in used.iter().enumerate() {
                    let assembly = self.assembly_object(*module)?;
                    unsafe { *((array + u64::from(ARRAY_DATA) + 8 * slot as u64) as *mut u64) = assembly };
                }
                self.store_handle(a[0], array);
                return Ok(None);
            }
            Reflect::TypeCore => {
                let module = self.qcall_assembly(a[0])?;
                let full = utf8_text(a[1].address());
                let nested: Vec<String> = (0..a[3].int().max(0) as usize)
                    .map(|index| utf8_text(unsafe { *((a[2].address() + 8 * index as u64) as *const u64) }))
                    .collect();
                let (namespace, name) = full.rsplit_once('.').unwrap_or(("", full.as_str()));
                let mut found = set.find_named(module, namespace, name);
                for inner in &nested {
                    found = found.and_then(|outer| set.find_nested(outer, inner));
                }
                let object = match found {
                    Some(handle) => self.type_object(&Gen::Named(handle, Rc::from(Vec::new())))?,
                    None => 0,
                };
                self.store_handle(a[4], object);
                return Ok(None);
            }
            Reflect::Load => {
                let parts = a[0].address();
                let name = wide_text(unsafe { *(parts as *const u64) });
                match set.find_assembly(&name) {
                    Some(module) => {
                        let assembly = self.assembly_object(module)?;
                        self.store_handle(a[5], assembly);
                    }
                    None if a[3].int() != 0 => {
                        if self.trace_calls {
                            eprintln!("assembly {name} was not found");
                        }
                        return self.throw("FileNotFoundException");
                    }
                    None => self.store_handle(a[5], 0),
                }
                return Ok(None);
            }
            Reflect::ManifestModule => {
                let module = self.module_of_assembly(self.non_null(a[0])?)?;
                V::O(self.module_object(module)?)
            }
            Reflect::IsDynamic => V::I4(0),
            Reflect::AssemblyToken => V::I4(0x2000_0001),
            Reflect::TypeModule | Reflect::TypeAssembly => {
                let exact = self.type_gen(a[0])?;
                let module = self.gen_module(&exact)?;
                V::O(match operation {
                    Reflect::TypeModule => self.module_object(module)?,
                    _ => self.assembly_object(module)?,
                })
            }
            Reflect::TypeToken => {
                let exact = self.type_gen(a[0])?;
                match (exact.handle(), self.types.parameter_row(&exact)) {
                    (_, Some((_, row))) => V::I4((0x2A00_0000 | row) as i32),
                    (Some(handle), _) => V::I4((0x0200_0000 | handle.row) as i32),
                    _ => V::I4(0x0200_0000),
                }
            }
            Reflect::TypeAttributes => match self.type_gen(a[0])? {
                Gen::Named(handle, _) => {
                    V::I4(set.modules[handle.module].metadata.cell(TableId::TypeDef, handle.row, 0).unwrap_or(0) as i32)
                }
                Gen::Var(..) | Gen::MethodVar(..) => V::I4(1),
                _ => V::I4(0x2101),
            },
            Reflect::BaseType => {
                let exact = self.type_gen(a[0])?;
                let base = match &exact {
                    Gen::Named(..) => map(self.types.base(&exact))?,
                    Gen::SzArray(_) | Gen::Array(..) => Some(map(self.types.core("Array"))?),
                    Gen::Var(..) | Gen::MethodVar(..) => Some(map(self.types.core("Object"))?),
                    _ => None,
                };
                V::O(match base {
                    Some(base) => self.type_object(&base)?,
                    None => 0,
                })
            }
            Reflect::ElementKind => V::I4(self.element_kind(&self.type_gen(a[0])?)),
            Reflect::HandleElementKind => V::I4(self.element_kind(&self.method_table_type(a[0].bits())?)),
            Reflect::InstanceOfType => {
                let exact = self.type_gen(a[0])?;
                let exact = self.nullable_argument(&exact).unwrap_or(exact);
                let object = a[1].address();
                let result = object != 0 && self.is_assignable(object, &exact);
                if !result && object != 0 && self.trace_calls {
                    let actual = self.object_gen(object).map(|g| self.types.full_name(&g)).unwrap_or_default();
                    eprintln!("{actual} is not an instance of {}", self.types.full_name(&exact));
                }
                V::I4(i32::from(result))
            }
            Reflect::DeclaringType => {
                let declaring = match self.type_gen(a[0])? {
                    Gen::Named(handle, arguments) => set.modules[handle.module]
                        .enclosing_type(handle.row)
                        .map(|row| {
                            let outer = TypeHandle { module: handle.module, row };
                            let count = self.types.parameter_count(TableId::TypeDef, row, handle.module);
                            Gen::Named(outer, Rc::from(arguments.iter().take(count).cloned().collect::<Vec<_>>()))
                        }),
                    Gen::Var(handle, _) => Some(Gen::Named(handle, Rc::from(Vec::new()))),
                    Gen::MethodVar(module, row, _) => self
                        .model
                        .method_owner(&MethodKey { module, row })
                        .ok()
                        .map(|owner| Gen::Named(owner, Rc::from(Vec::new()))),
                    _ => None,
                };
                V::O(match declaring {
                    Some(outer) => self.type_object(&outer)?,
                    None => 0,
                })
            }
            Reflect::ElementType => V::O(match self.type_gen(a[0])? {
                Gen::SzArray(inner) | Gen::Array(inner, _) | Gen::Pointer(inner) | Gen::ByRef(inner) => {
                    self.type_object(&inner)?
                }
                _ => 0,
            }),
            Reflect::ArrayRank => V::I4(match self.type_gen(a[0])? {
                Gen::SzArray(_) => 1,
                Gen::Array(_, rank) => rank as i32,
                _ => 0,
            }),
            Reflect::ContainsGenericVariables => {
                let exact = self.type_gen(a[0])?;
                V::I4(i32::from(self.contains_variables(&exact)))
            }
            Reflect::SameType => V::I4(i32::from(a[0].address() == a[1].address())),
            Reflect::Utf8Name => match self.type_gen(a[0])? {
                Gen::Named(handle, _) => V::P(
                    set.modules[handle.module]
                        .metadata
                        .string_cell(TableId::TypeDef, handle.row, 1)
                        .map_or(0, |name| name.as_ptr() as u64),
                ),
                exact @ (Gen::Var(..) | Gen::MethodVar(..)) => V::P(
                    self.types
                        .parameter_row(&exact)
                        .and_then(|(module, row)| set.modules[module].metadata.string_cell(TableId::GenericParam, row, 3).ok())
                        .map_or(0, |name| name.as_ptr() as u64),
                ),
                _ => V::P(0),
            },
            Reflect::Ignore
            | Reflect::SizeOf
            | Reflect::GenericDefinition
            | Reflect::Instantiation
            | Reflect::Instantiate
            | Reflect::MakeVector
            | Reflect::MakeArray
            | Reflect::MakeByRef
            | Reflect::MakePointer
            | Reflect::ConstructName
            | Reflect::IsVisible
            | Reflect::AnotherParameter
            | Reflect::StructureToPointer
            | Reflect::PointerToStructure
            | Reflect::DestroyStructure
            | Reflect::Instance
            | Reflect::ClassFromClsid
            | Reflect::ComObjectGuid
            | Reflect::ModuleType
            | Reflect::OsThreadId
            | Reflect::WaitOne
            | Reflect::WaitMany
            | Reflect::SignalAndWait
            | Reflect::SpinIterations => return self.type_call(operation, a),
            _ => return self.metadata_call(operation, a),
        }))
    }

    fn resource(&self, module: usize, name: &str) -> Option<&'static [u8]> {
        crate::resources::manifest(&self.model.set.modules[module])
            .ok()?
            .into_iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, data)| data)
    }

    fn caller_module(&self, depth: usize) -> usize {
        let core = self.corelib_type("System", "Object").map(|h| h.module).ok();
        runtime::roots()
            .modules
            .iter()
            .rev()
            .filter(|module| Some(**module) != core)
            .nth(depth - 1)
            .copied()
            .unwrap_or(self.entry_module.get())
    }

    fn element_kind(&self, exact: &Gen) -> i32 {
        match exact {
            Gen::SzArray(_) => 0x1D,
            Gen::Array(..) => 0x14,
            Gen::Pointer(_) => 0x0F,
            Gen::ByRef(_) => 0x10,
            Gen::FunctionPointer => 0x1B,
            Gen::Var(..) => 0x13,
            Gen::MethodVar(..) => 0x1E,
            Gen::Named(handle, _) => match self.type_name(*handle).as_str() {
                "System.Void" => 0x01,
                "System.Boolean" => 0x02,
                "System.Char" => 0x03,
                "System.SByte" => 0x04,
                "System.Byte" => 0x05,
                "System.Int16" => 0x06,
                "System.UInt16" => 0x07,
                "System.Int32" => 0x08,
                "System.UInt32" => 0x09,
                "System.Int64" => 0x0A,
                "System.UInt64" => 0x0B,
                "System.Single" => 0x0C,
                "System.Double" => 0x0D,
                "System.String" => 0x0E,
                "System.IntPtr" => 0x18,
                "System.UIntPtr" => 0x19,
                "System.Object" => 0x1C,
                _ if self.types.is_value_type(exact) => 0x11,
                _ => 0x12,
            },
        }
    }

    pub(super) fn contains_variables(&self, exact: &Gen) -> bool {
        match exact {
            Gen::Var(..) | Gen::MethodVar(..) => true,
            Gen::Named(handle, arguments) if arguments.is_empty() => {
                self.types.parameter_count(TableId::TypeDef, handle.row, handle.module) > 0
            }
            Gen::Named(_, arguments) => arguments.iter().any(|argument| self.contains_variables(argument)),
            Gen::SzArray(inner) | Gen::Array(inner, _) | Gen::Pointer(inner) | Gen::ByRef(inner) => self.contains_variables(inner),
            Gen::FunctionPointer => false,
        }
    }

    pub(super) fn gen_module(&self, exact: &Gen) -> Run<usize> {
        match exact {
            Gen::Named(handle, _) => Ok(handle.module),
            Gen::SzArray(inner) | Gen::Array(inner, _) | Gen::Pointer(inner) | Gen::ByRef(inner) => self.gen_module(inner),
            Gen::FunctionPointer => self.corelib_type("System", "Object").map(|h| h.module),
            Gen::Var(handle, _) => Ok(handle.module),
            Gen::MethodVar(module, _, _) => Ok(*module),
        }
    }

    pub(super) fn type_gen(&self, value: V) -> Run<Gen> {
        let object = match value {
            V::S(boxed) if boxed != 0 => unsafe { *((boxed + 8) as *const u64) },
            V::P(pointer) if pointer != 0 && !self.object_types.borrow().contains_key(&pointer) => {
                let inner = unsafe { *(pointer as *const u64) };
                if self.object_types.borrow().contains_key(&inner) { inner } else { pointer }
            }
            other => other.address(),
        };
        if object == 0 {
            return self.throw("NullReferenceException");
        }
        if !self.object_types.borrow().contains_key(&object)
            && let Ok(exact) = self.method_table_type(object)
        {
            return Ok(exact);
        }
        self.object_types
            .borrow()
            .get(&object)
            .cloned()
            .ok_or_else(|| Fault::Unsupported("type object from outside the fallback".into()))
    }

    pub(super) fn mark_used(&self, module: usize) {
        if !self.used_modules.borrow().contains(&module) {
            self.used_modules.borrow_mut().insert(module);
        }
    }

    pub(super) fn assembly_object(&self, module: usize) -> Run<u64> {
        if let Some(found) = self.assemblies.borrow().get(&module) {
            return Ok(*found);
        }
        self.mark_used(module);
        let object = self.allocate_runtime_object("System.Reflection", "RuntimeAssembly")?;
        let offset = self.class_field_offset("System.Reflection", "RuntimeAssembly", "m_assembly")?;
        unsafe { *((object + u64::from(offset)) as *mut u64) = module as u64 + 1 };
        self.assemblies.borrow_mut().insert(module, object);
        Ok(object)
    }

    pub(super) fn module_object(&self, module: usize) -> Run<u64> {
        if let Some(found) = self.module_objects.borrow().get(&module) {
            return Ok(*found);
        }
        let assembly = self.assembly_object(module)?;
        let object = self.allocate_runtime_object("System.Reflection", "RuntimeModule")?;
        let data = self.class_field_offset("System.Reflection", "RuntimeModule", "m_pData")?;
        let owner = self.class_field_offset("System.Reflection", "RuntimeModule", "m_runtimeAssembly")?;
        unsafe {
            *((object + u64::from(data)) as *mut u64) = module as u64 + 1;
            *((object + u64::from(owner)) as *mut u64) = assembly;
        }
        self.module_objects.borrow_mut().insert(module, object);
        Ok(object)
    }

    pub(super) fn module_of_assembly(&self, object: u64) -> Run<usize> {
        let offset = self.class_field_offset("System.Reflection", "RuntimeAssembly", "m_assembly")?;
        match unsafe { *((object + u64::from(offset)) as *const u64) } {
            0 => unsupported("assembly object from outside the fallback"),
            handle => Ok(handle as usize - 1),
        }
    }

    pub(super) fn qcall_assembly(&self, value: V) -> Run<usize> {
        let offset = self.qcall_field("QCallAssembly", "_assembly")?;
        match unsafe { *((Self::struct_data(value) + u64::from(offset)) as *const u64) } {
            0 => unsupported("assembly handle from outside the fallback"),
            handle => Ok(handle as usize - 1),
        }
    }

    pub(super) fn qcall_module(&self, value: V) -> Run<usize> {
        let offset = self.qcall_field("QCallModule", "_module")?;
        match unsafe { *((Self::struct_data(value) + u64::from(offset)) as *const u64) } {
            0 => unsupported("module handle from outside the fallback"),
            handle => Ok(handle as usize - 1),
        }
    }

    fn qcall_field(&self, name: &str, field: &str) -> Run<u32> {
        let handle = self.corelib_type("System.Runtime.CompilerServices", name)?;
        let layout = self.layout_of(&Gen::Named(handle, Rc::from(Vec::new())))?;
        self.struct_field_offset(&layout, field)
    }

    fn allocate_runtime_object(&self, namespace: &str, name: &str) -> Run<u64> {
        let handle = self.corelib_type(namespace, name)?;
        let record = self.record_of(&Gen::Named(handle, Rc::from(Vec::new())))?;
        let object = runtime::allocate_managed(record, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        type_roots().push(object);
        Ok(object)
    }

    pub(super) fn class_field_offset(&self, namespace: &str, name: &str, field: &str) -> Run<u32> {
        let handle = self.corelib_type(namespace, name)?;
        let (_, placements) = self
            .model
            .set
            .class_fields(handle)
            .map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
        let metadata = &self.model.set.modules[handle.module].metadata;
        placements
            .iter()
            .find(|p| metadata.string_cell(TableId::Field, p.field, 1).ok() == Some(field))
            .map(|p| p.offset + 8)
            .ok_or_else(|| Fault::Unsupported(format!("{name} has no field {field}")))
    }

    fn byte_array(&self, bytes: &[u8]) -> Run<u64> {
        let record = self.record(RuntimeType::Array(ElementKind::U8))?;
        let array = runtime::allocate_managed(record, bytes.len() as u64);
        if array == 0 {
            return self.throw("OutOfMemoryException");
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), (array + u64::from(ARRAY_DATA)) as *mut u8, bytes.len());
        }
        Ok(array)
    }

    pub(super) fn struct_data(value: V) -> u64 {
        match value {
            V::S(boxed) | V::O(boxed) if boxed != 0 => boxed + 8,
            other => other.address(),
        }
    }

    pub(super) fn store_handle(&self, handle: V, object: u64) {
        let slot = unsafe { *(Self::struct_data(handle) as *const u64) };
        if slot != 0 {
            unsafe { *(slot as *mut u64) = object };
        }
    }

    pub(super) fn store_text(&self, handle: V, text: &str) {
        let units: Vec<u16> = text.encode_utf16().collect();
        let string = runtime::new_string(&units);
        self.store_handle(handle, string);
    }
}

fn attribute_usage(blob: &[u8]) -> (i32, i32, i32) {
    let targets = blob.get(2..6).map_or(0x7FFF, |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (mut inherited, mut multiple) = (1, 0);
    let count = blob.get(6..8).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    let mut at = 8;
    for _ in 0..count {
        let Some(&length) = blob.get(at + 2) else { break };
        let name_start = at + 3;
        let name_end = name_start + length as usize;
        let name = blob.get(name_start..name_end).map(String::from_utf8_lossy).unwrap_or_default();
        let value = i32::from(blob.get(name_end).copied().unwrap_or(0) != 0);
        match name.as_ref() {
            "Inherited" => inherited = value,
            "AllowMultiple" => multiple = value,
            _ => {}
        }
        at = name_end + 1;
    }
    (targets, inherited, multiple)
}

fn assembly_cell(set: &AssemblySet, module: usize, column: usize) -> u32 {
    set.modules[module].metadata.cell(TableId::Assembly, 1, column).unwrap_or(0)
}

fn assembly_culture(set: &AssemblySet, module: usize) -> &'static str {
    set.modules[module].metadata.string_cell(TableId::Assembly, 1, 8).unwrap_or("")
}

fn assembly_public_key(set: &AssemblySet, module: usize) -> &'static [u8] {
    let metadata = &set.modules[module].metadata;
    metadata
        .cell(TableId::Assembly, 1, 6)
        .and_then(|blob| metadata.blob(blob))
        .unwrap_or(&[])
}

fn assembly_full_name(set: &AssemblySet, module: usize) -> String {
    let version: Vec<String> = (1..=4).map(|column| (assembly_cell(set, module, column) as u16).to_string()).collect();
    let culture = match assembly_culture(set, module) {
        "" => "neutral",
        other => other,
    };
    let key = assembly_public_key(set, module);
    let token = if key.is_empty() {
        "null".to_owned()
    } else {
        let digest = sha1(key);
        digest[12..].iter().rev().map(|b| format!("{b:02x}")).collect()
    };
    format!(
        "{}, Version={}, Culture={culture}, PublicKeyToken={token}",
        set.modules[module].name,
        version.join(".")
    )
}

pub(super) fn wide_units(pointer: u64) -> Vec<u16> {
    let mut length = 0usize;
    while unsafe { *((pointer as *const u16).add(length)) } != 0 {
        length += 1;
    }
    unsafe { std::slice::from_raw_parts(pointer as *const u16, length) }.to_vec()
}

pub(super) fn ansi_units(pointer: u64) -> Vec<u16> {
    let mut length = 0usize;
    while unsafe { *((pointer as *const u8).add(length)) } != 0 {
        length += 1;
    }
    unsafe { std::slice::from_raw_parts(pointer as *const u8, length) }
        .iter()
        .map(|b| u16::from(*b))
        .collect()
}

fn wide_text(pointer: u64) -> String {
    if pointer == 0 {
        return String::new();
    }
    let mut length = 0usize;
    while unsafe { *((pointer as *const u16).add(length)) } != 0 {
        length += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer as *const u16, length) })
}

fn utf8_text(pointer: u64) -> String {
    if pointer == 0 {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(pointer as *const std::ffi::c_char) }
        .to_string_lossy()
        .into_owned()
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in message.chunks(64) {
        let mut words = [0u32; 80];
        for (index, word) in chunk.chunks(4).enumerate() {
            words[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..80 {
            words[index] = (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (index, word) in words.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut digest = [0u8; 20];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

pub(super) fn metadata_internal(name: &str, count: usize) -> Option<Reflect> {
    Some(match (name, count) {
        ("GetMetadataImport", 1) => Reflect::MetadataScope,
        ("GetName", 3) => Reflect::MdName,
        ("GetNamespace", 3) => Reflect::MdNamespace,
        ("GetParentToken", 3) => Reflect::MdParent,
        ("GetCustomAttributeProps", 4) => Reflect::MdCustomAttribute,
        ("GetSigOfMethodDef", 3) => Reflect::MdMethodSig,
        ("GetSigOfFieldDef", 3) => Reflect::MdFieldSig,
        ("GetMemberRefProps", 3) => Reflect::MdMemberRefSig,
        ("GetSignatureFromToken", 3) => Reflect::MdSignature,
        ("GetDefaultValue", 6) => Reflect::MdDefault,
        ("GetFieldDefProps", 3) => Reflect::MdFieldFlags,
        ("GetPropertyProps", 5) => Reflect::MdProperty,
        ("GetEventProps", 4) => Reflect::MdEvent,
        ("GetParamDefProps", 4) => Reflect::MdParam,
        ("GetGenericParamProps", 3) => Reflect::MdGenericParam,
        ("GetClassLayout", 4) => Reflect::MdClassLayout,
        ("GetScopeProps", 2) => Reflect::MdScope,
        ("IsValidToken", 2) => Reflect::MdValid,
        ("GetPInvokeMap", 5) => Reflect::MdPInvoke,
        ("GetFieldMarshal", 3) => Reflect::MdFieldMarshal,
        ("GetMarshalAs", 11) => Reflect::MdMarshalAs,
        _ => return None,
    })
}

impl Interpreter<'_> {
    pub(super) fn metadata_call(&self, operation: Reflect, a: &[V]) -> Run<Option<V>> {
        let set = self.model.set;
        let scope = self.metadata_scope(a[0]);
        if operation == Reflect::MetadataScope {
            let object = self.non_null(a[0])?;
            let offset = self.class_field_offset("System.Reflection", "RuntimeModule", "m_pData")?;
            return Ok(Some(V::P(unsafe { *((object + u64::from(offset)) as *const u64) })));
        }
        if scope == 0 || scope > set.modules.len() {
            return unsupported("metadata scope from outside the fallback");
        }
        let module = scope - 1;
        let metadata = &set.modules[module].metadata;
        let token = Token(a.get(1).map_or(0, |v| v.int() as u32));
        let row = token.row();
        let out = |index: usize| a[index].address();
        let write_i32 = |index: usize, value: i32| unsafe { *(out(index) as *mut i32) = value };
        let write_u64 = |index: usize, value: u64| unsafe { *(out(index) as *mut u64) = value };
        let name_column = |table: TableId| match table {
            TableId::TypeDef | TableId::TypeRef | TableId::Field | TableId::Property | TableId::Event | TableId::MemberRef | TableId::Module => Some(1),
            TableId::MethodDef | TableId::GenericParam => Some(3),
            TableId::Param | TableId::ExportedType => Some(2),
            TableId::ModuleRef => Some(0),
            _ => None,
        };
        let hresult = match operation {
            Reflect::MdName | Reflect::MdNamespace => {
                let column = match (operation, token.table()) {
                    (Reflect::MdName, Some(table)) => name_column(table),
                    (_, Some(TableId::TypeDef | TableId::TypeRef)) => Some(2),
                    (_, Some(TableId::ExportedType)) => Some(3),
                    _ => None,
                };
                let pointer = match (token.table(), column) {
                    (Some(table), Some(column)) => metadata.string_cell(table, row, column).map_or(0, |s| s.as_ptr() as u64),
                    _ => 0,
                };
                write_u64(2, pointer);
                0
            }
            Reflect::MdParent => {
                let parent = self.parent_token(module, token);
                write_i32(2, parent.0 as i32);
                0
            }
            Reflect::MdCustomAttribute => {
                let constructor = metadata.coded(TableId::CustomAttribute, row, 1).ok().flatten().map_or(0, |t| t.0);
                write_i32(2, constructor as i32);
                let blob = metadata.cell(TableId::CustomAttribute, row, 2).and_then(|b| metadata.blob(b)).unwrap_or(&[]);
                self.write_const_array(out(3), blob)?;
                0
            }
            Reflect::MdMethodSig | Reflect::MdFieldSig | Reflect::MdMemberRefSig | Reflect::MdSignature => {
                let (table, column) = match (operation, token.table()) {
                    (Reflect::MdMethodSig, _) => (TableId::MethodDef, 4),
                    (Reflect::MdFieldSig, _) => (TableId::Field, 2),
                    (Reflect::MdMemberRefSig, _) => (TableId::MemberRef, 2),
                    (_, Some(TableId::TypeSpec)) => (TableId::TypeSpec, 0),
                    (_, Some(TableId::MethodDef)) => (TableId::MethodDef, 4),
                    (_, Some(TableId::Field)) => (TableId::Field, 2),
                    (_, Some(TableId::MemberRef)) => (TableId::MemberRef, 2),
                    _ => (TableId::StandAloneSig, 0),
                };
                let blob = metadata.cell(table, row, column).and_then(|b| metadata.blob(b)).unwrap_or(&[]);
                self.write_const_array(out(2), blob)?;
                0
            }
            Reflect::MdDefault => {
                let found = (1..=metadata.rows(TableId::Constant)).find(|&c| {
                    metadata.coded(TableId::Constant, c, 1).ok().flatten().is_some_and(|p| p.table() == token.table() && p.row() == row)
                });
                match found {
                    Some(constant) => {
                        let kind = metadata.cell(TableId::Constant, constant, 0).unwrap_or(0) as i32;
                        let blob = metadata.cell(TableId::Constant, constant, 2).and_then(|b| metadata.blob(b)).unwrap_or(&[]);
                        let mut value = 0u64;
                        unsafe { std::ptr::copy_nonoverlapping(blob.as_ptr(), (&raw mut value).cast(), blob.len().min(8)) };
                        write_u64(2, value);
                        write_u64(3, if kind == 0x0E { blob.as_ptr() as u64 } else { 0 });
                        write_i32(4, if kind == 0x0E { (blob.len() / 2) as i32 } else { blob.len() as i32 });
                        write_i32(5, kind);
                    }
                    None => {
                        write_u64(2, 0);
                        write_u64(3, 0);
                        write_i32(4, 0);
                        write_i32(5, 1);
                    }
                }
                0
            }
            Reflect::MdFieldFlags => {
                write_i32(2, metadata.cell(TableId::Field, row, 0).unwrap_or(0) as i32);
                0
            }
            Reflect::MdProperty => {
                write_u64(2, metadata.string_cell(TableId::Property, row, 1).map_or(0, |s| s.as_ptr() as u64));
                write_i32(3, metadata.cell(TableId::Property, row, 0).unwrap_or(0) as i32);
                let blob = metadata.cell(TableId::Property, row, 2).and_then(|b| metadata.blob(b)).unwrap_or(&[]);
                self.write_const_array(out(4), blob)?;
                0
            }
            Reflect::MdEvent => {
                write_u64(2, metadata.string_cell(TableId::Event, row, 1).map_or(0, |s| s.as_ptr() as u64));
                write_i32(3, metadata.cell(TableId::Event, row, 0).unwrap_or(0) as i32);
                0
            }
            Reflect::MdParam => {
                write_i32(2, metadata.cell(TableId::Param, row, 1).unwrap_or(0) as i32);
                write_i32(3, metadata.cell(TableId::Param, row, 0).unwrap_or(0) as i32);
                0
            }
            Reflect::MdGenericParam => {
                write_i32(2, metadata.cell(TableId::GenericParam, row, 1).unwrap_or(0) as i32);
                0
            }
            Reflect::MdClassLayout => {
                match (1..=metadata.rows(TableId::ClassLayout)).find(|&l| metadata.cell(TableId::ClassLayout, l, 2).ok() == Some(row)) {
                    Some(layout) => {
                        write_i32(2, metadata.cell(TableId::ClassLayout, layout, 0).unwrap_or(0) as i32);
                        write_i32(3, metadata.cell(TableId::ClassLayout, layout, 1).unwrap_or(0) as i32);
                        0
                    }
                    None => 0x8013_1130u32 as i32,
                }
            }
            Reflect::MdScope => {
                let guid = metadata.cell(TableId::Module, 1, 2).ok().and_then(|g| metadata.guid(g).ok().flatten()).unwrap_or([0; 16]);
                unsafe { std::ptr::copy_nonoverlapping(guid.as_ptr(), a[1].address() as *mut u8, 16) };
                0
            }
            Reflect::MdValid => {
                let valid = token.table().is_some_and(|table| row >= 1 && row <= metadata.rows(table));
                return Ok(Some(V::I4(i32::from(valid))));
            }
            Reflect::MdPInvoke => match crate::pinvoke::find_import(metadata, row) {
                Some(import) => {
                    write_i32(2, import.flags as i32);
                    write_u64(3, import.entry.as_ptr() as u64);
                    write_u64(4, import.library.as_ptr() as u64);
                    0
                }
                None => 0x8013_1130u32 as i32,
            },
            Reflect::MdFieldMarshal => {
                let blob = (1..=metadata.rows(TableId::FieldMarshal))
                    .find(|&m| metadata.coded(TableId::FieldMarshal, m, 0).ok().flatten().is_some_and(|p| p.table() == token.table() && p.row() == row))
                    .and_then(|m| metadata.cell(TableId::FieldMarshal, m, 1).ok())
                    .and_then(|b| metadata.blob(b).ok())
                    .unwrap_or(&[]);
                self.write_const_array(out(2), blob)?;
                0
            }
            Reflect::MdMarshalAs => return Ok(Some(V::I4(0))),
            _ => return unsupported("metadata call"),
        };
        Ok(Some(V::I4(hresult)))
    }

    fn metadata_scope(&self, value: V) -> usize {
        let raw = value.bits();
        if raw as usize <= self.model.set.modules.len() {
            return raw as usize;
        }
        unsafe { *(raw as *const u64) as usize }
    }

    fn write_const_array(&self, address: u64, blob: &'static [u8]) -> Run<()> {
        let handle = self.corelib_type("System.Reflection", "ConstArray")?;
        let layout = self.layout_of(&Gen::Named(handle, Rc::from(Vec::new())))?;
        let length = self.struct_field_offset(&layout, "m_length")?;
        let data = self.struct_field_offset(&layout, "m_constArray")?;
        unsafe {
            *((address + u64::from(length)) as *mut i32) = blob.len() as i32;
            *((address + u64::from(data)) as *mut u64) = blob.as_ptr() as u64;
        }
        Ok(())
    }

    fn parent_token(&self, module: usize, token: Token) -> Token {
        let set = self.model.set;
        let metadata = &set.modules[module].metadata;
        let row = token.row();
        let owner_of = |table: TableId, list_column: usize, child: u32| -> u32 {
            let rows = metadata.rows(table);
            let mut found = 0;
            for candidate in 1..=rows {
                if metadata.cell(table, candidate, list_column).unwrap_or(u32::MAX) <= child {
                    found = candidate;
                } else {
                    break;
                }
            }
            found
        };
        match token.table() {
            Some(TableId::MethodDef) => Token::new(TableId::TypeDef, set.modules[module].method_owner(row).ok().flatten().unwrap_or(0)),
            Some(TableId::Field) => Token::new(TableId::TypeDef, owner_of(TableId::TypeDef, 4, row)),
            Some(TableId::Param) => Token::new(TableId::MethodDef, owner_of(TableId::MethodDef, 5, row)),
            Some(TableId::Property) => {
                let map = owner_of(TableId::PropertyMap, 1, row);
                Token::new(TableId::TypeDef, metadata.cell(TableId::PropertyMap, map, 0).unwrap_or(0))
            }
            Some(TableId::Event) => {
                let map = owner_of(TableId::EventMap, 1, row);
                Token::new(TableId::TypeDef, metadata.cell(TableId::EventMap, map, 0).unwrap_or(0))
            }
            Some(TableId::CustomAttribute) => metadata.coded(TableId::CustomAttribute, row, 0).ok().flatten().unwrap_or(Token(0)),
            Some(TableId::MemberRef) => metadata.coded(TableId::MemberRef, row, 0).ok().flatten().unwrap_or(Token(0)),
            Some(TableId::GenericParam) => metadata.coded(TableId::GenericParam, row, 2).ok().flatten().unwrap_or(Token(0)),
            Some(TableId::TypeDef) => Token::new(TableId::TypeDef, set.modules[module].enclosing_type(row).unwrap_or(0)),
            _ => Token(0),
        }
    }

    pub(super) fn metadata_enum(&self, a: &[V]) -> Run<Option<V>> {
        let set = self.model.set;
        let scope = self.metadata_scope(a[0]);
        if scope == 0 || scope > set.modules.len() {
            return unsupported("metadata scope from outside the fallback");
        }
        let module = scope - 1;
        let metadata = &set.modules[module].metadata;
        let kind = a[1].int() as u32;
        let parent = Token(a[2].int() as u32);
        let list = |table: TableId, list_column: usize, owner_row: u32, child: TableId| -> Vec<u32> {
            let first = metadata.cell(table, owner_row, list_column).unwrap_or(1);
            let end = if owner_row < metadata.rows(table) {
                metadata.cell(table, owner_row + 1, list_column).unwrap_or(first)
            } else {
                metadata.rows(child) + 1
            };
            (first..end).collect()
        };
        let mapped = |map: TableId, child: TableId| -> Vec<u32> {
            (1..=metadata.rows(map))
                .find(|&m| metadata.cell(map, m, 0).ok() == Some(parent.row()))
                .map(|m| list(map, 1, m, child))
                .unwrap_or_default()
        };
        let tokens: Vec<u32> = match kind {
            0x0200_0000 => (1..=metadata.rows(TableId::NestedClass))
                .filter(|&n| metadata.cell(TableId::NestedClass, n, 1).ok() == Some(parent.row()))
                .filter_map(|n| metadata.cell(TableId::NestedClass, n, 0).ok())
                .map(|r| 0x0200_0000 | r)
                .collect(),
            0x0600_0000 if matches!(parent.table(), Some(TableId::Property | TableId::Event)) => (1..=metadata.rows(TableId::MethodSemantics))
                .filter(|&m| {
                    metadata
                        .coded(TableId::MethodSemantics, m, 2)
                        .ok()
                        .flatten()
                        .is_some_and(|p| p.table() == parent.table() && p.row() == parent.row())
                })
                .flat_map(|m| {
                    [
                        0x0600_0000 | metadata.cell(TableId::MethodSemantics, m, 1).unwrap_or(0),
                        metadata.cell(TableId::MethodSemantics, m, 0).unwrap_or(0),
                    ]
                })
                .collect(),
            0x0600_0000 => list(TableId::TypeDef, 5, parent.row(), TableId::MethodDef).into_iter().map(|r| 0x0600_0000 | r).collect(),
            0x0400_0000 => list(TableId::TypeDef, 4, parent.row(), TableId::Field).into_iter().map(|r| 0x0400_0000 | r).collect(),
            0x0800_0000 => list(TableId::MethodDef, 5, parent.row(), TableId::Param).into_iter().map(|r| 0x0800_0000 | r).collect(),
            0x1700_0000 => mapped(TableId::PropertyMap, TableId::Property).into_iter().map(|r| 0x1700_0000 | r).collect(),
            0x1400_0000 => mapped(TableId::EventMap, TableId::Event).into_iter().map(|r| 0x1400_0000 | r).collect(),
            0x0900_0000 => (1..=metadata.rows(TableId::InterfaceImpl))
                .filter(|&i| metadata.cell(TableId::InterfaceImpl, i, 0).ok() == Some(parent.row()))
                .map(|r| 0x0900_0000 | r)
                .collect(),
            0x0C00_0000 => (1..=metadata.rows(TableId::CustomAttribute))
                .filter(|&c| metadata.coded(TableId::CustomAttribute, c, 0).ok().flatten().is_some_and(|p| p.table() == parent.table() && p.row() == parent.row()))
                .map(|r| 0x0C00_0000 | r)
                .collect(),
            0x2A00_0000 => (1..=metadata.rows(TableId::GenericParam))
                .filter(|&g| metadata.coded(TableId::GenericParam, g, 2).ok().flatten().is_some_and(|p| p.table() == parent.table() && p.row() == parent.row()))
                .map(|r| 0x2A00_0000 | r)
                .collect(),
            0x2C00_0000 => (1..=metadata.rows(TableId::GenericParamConstraint))
                .filter(|&g| metadata.cell(TableId::GenericParamConstraint, g, 0).ok() == Some(parent.row()))
                .map(|r| 0x2C00_0000 | r)
                .collect(),
            0x1800_0000 => (1..=metadata.rows(TableId::MethodSemantics))
                .filter(|&s| metadata.coded(TableId::MethodSemantics, s, 2).ok().flatten().is_some_and(|p| p.table() == parent.table() && p.row() == parent.row()))
                .map(|r| 0x1800_0000 | r)
                .collect(),
            other => return unsupported(format!("metadata enumeration of 0x{other:08x}")),
        };
        unsafe { *(a[3].address() as *mut i32) = tokens.len() as i32 };
        if tokens.len() <= 16 {
            for (index, token) in tokens.iter().enumerate() {
                unsafe { *((a[4].address() + 4 * index as u64) as *mut u32) = *token };
            }
        } else {
            let record = self.record(RuntimeType::Array(ElementKind::I32))?;
            let array = runtime::allocate_managed(record, tokens.len() as u64);
            if array == 0 {
                return self.throw("OutOfMemoryException");
            }
            for (index, token) in tokens.iter().enumerate() {
                unsafe { *((array + u64::from(ARRAY_DATA) + 4 * index as u64) as *mut u32) = *token };
            }
            self.store_handle(a[5], array);
        }
        Ok(None)
    }
}

const FLAG_COMPONENT_SIZE: u32 = 0x8000_0000;
const FLAG_GC_POINTERS: u32 = 0x0100_0000;
const FLAG_DEFAULT_CONSTRUCTOR: u32 = 0x0000_0200;
const CATEGORY_VALUE_TYPE: u32 = 0x0004_0000;
const CATEGORY_NULLABLE: u32 = 0x0005_0000;
const CATEGORY_ENUM: u32 = 0x0006_0000;
const CATEGORY_PRIMITIVE: u32 = 0x0007_0000;
const CATEGORY_ARRAY: u32 = 0x0008_0000;
const CATEGORY_INTERFACE: u32 = 0x000C_0000;
const GENERIC_INSTANTIATION: u32 = 0x10;
const GENERIC_DEFINITION: u32 = 0x30;

impl Interpreter<'_> {
    pub(super) fn method_table(&self, exact: &Gen) -> Run<u64> {
        let id = self.types.id(exact);
        if let Some(found) = self.method_tables.borrow().get(&id) {
            return Ok(*found);
        }
        let block: &'static mut [u64] = Box::leak(vec![0u64; 10].into_boxed_slice());
        let address = block.as_ptr() as u64;
        let tagged = match exact {
            Gen::Pointer(_) | Gen::ByRef(_) | Gen::FunctionPointer | Gen::Var(..) | Gen::MethodVar(..) => address | 2,
            _ => address,
        };
        self.method_tables.borrow_mut().insert(id, tagged);
        self.method_table_types.borrow_mut().insert(address, exact.clone());
        let auxiliary: &'static mut [u64] = Box::leak(vec![0u64; 1].into_boxed_slice());
        block[4] = auxiliary.as_ptr() as u64;
        let (flags, base_size) = match exact {
            Gen::SzArray(element) | Gen::Array(element, _) => {
                let (ty, size) = self.element_size(element)?;
                let references = match &ty {
                    Ty::Prim(VarType::Ref) => true,
                    Ty::Struct(layout) => !layout.references.is_empty(),
                    _ => false,
                };
                block[6] = self.method_table(element)?;
                let rank = match exact {
                    Gen::Array(_, rank) => 24 + 8 * *rank,
                    _ => 24,
                };
                (
                    FLAG_COMPONENT_SIZE | CATEGORY_ARRAY | if references { FLAG_GC_POINTERS } else { 0 } | (size & 0xFFFF),
                    rank,
                )
            }
            Gen::Named(handle, arguments) => {
                let name = self.type_name(*handle);
                let flags_column = self.model.set.modules[handle.module]
                    .metadata
                    .cell(TableId::TypeDef, handle.row, 0)
                    .unwrap_or(0);
                let parameters = self.types.parameter_count(TableId::TypeDef, handle.row, handle.module);
                let generics = match (parameters, arguments.is_empty()) {
                    (0, _) => 0,
                    (_, true) => GENERIC_DEFINITION,
                    (_, false) => GENERIC_INSTANTIATION,
                };
                let is_enum = self.model.set.is_enum(*handle).unwrap_or(false);
                let category = if flags_column & 0x20 != 0 {
                    CATEGORY_INTERFACE
                } else if name == "System.Nullable`1" {
                    CATEGORY_NULLABLE
                } else if is_enum {
                    CATEGORY_ENUM
                } else if is_true_primitive(&name) {
                    CATEGORY_PRIMITIVE
                } else if self.types.is_value_type(exact) {
                    CATEGORY_VALUE_TYPE
                } else {
                    0
                };
                let open = generics == GENERIC_DEFINITION;
                let references = !open
                    && match self.gen_ty(exact) {
                        Ok(Ty::Struct(layout)) => !layout.references.is_empty(),
                        Ok(Ty::Prim(VarType::Ref)) if category == 0 => self
                            .record_of(exact)
                            .map(|record| !runtime::state().record_offsets(record).is_empty())
                            .unwrap_or(false),
                        _ => false,
                    };
                let default_constructor = category != CATEGORY_INTERFACE && self.has_default_constructor(*handle);
                if !open && let Ok(Some(base)) = map(self.types.base(exact)) {
                    block[2] = self.method_table(&base)?;
                }
                let interfaces = if open { Vec::new() } else { self.types.all_interfaces(exact) };
                if !interfaces.is_empty() {
                    let map: &'static mut [u64] = Box::leak(vec![0u64; interfaces.len()].into_boxed_slice());
                    for (slot, interface) in map.iter_mut().zip(&interfaces) {
                        *slot = self.method_table(interface)?;
                    }
                    block[7] = map.as_ptr() as u64;
                }
                block[1] = (interfaces.len() as u64 & 0xFFFF) << 48;
                (
                    category
                        | generics
                        | if references { FLAG_GC_POINTERS } else { 0 }
                        | if default_constructor { FLAG_DEFAULT_CONSTRUCTOR } else { 0 },
                    24,
                )
            }
            _ => (0, 0),
        };
        block[0] = u64::from(flags) | (u64::from(base_size) << 32);
        Ok(tagged)
    }

    pub(super) fn method_table_type(&self, handle: u64) -> Run<Gen> {
        self.method_table_types
            .borrow()
            .get(&(handle & !3))
            .cloned()
            .ok_or_else(|| Fault::Unsupported("type handle from outside the fallback".into()))
    }

    fn has_default_constructor(&self, handle: TypeHandle) -> bool {
        let metadata = &self.model.set.modules[handle.module].metadata;
        let Ok(range) = metadata.method_range(handle.row) else {
            return false;
        };
        range.into_iter().any(|row| {
            let flags = metadata.cell(TableId::MethodDef, row, 2).unwrap_or(0);
            metadata.string_cell(TableId::MethodDef, row, 3).ok() == Some(".ctor")
                && flags & STATIC == 0
                && self
                    .model
                    .method_sig(&MethodKey { module: handle.module, row })
                    .is_ok_and(|sig| sig.parameters.is_empty())
        })
    }

    pub(super) fn primitive_element_kind(&self, exact: &Gen) -> i32 {
        if let Gen::Named(handle, _) = exact
            && self.model.set.is_enum(*handle).unwrap_or(false)
        {
            let metadata = &self.model.set.modules[handle.module].metadata;
            let underlying = metadata.field_range(handle.row).ok().and_then(|mut range| {
                range.find(|&row| metadata.string_cell(TableId::Field, row, 1).ok() == Some("value__"))
            });
            if let Some(row) = underlying
                && let Ok(sig) = self.model.field_signature(handle.module, row)
            {
                return match unmodified(&sig) {
                    TypeSig::Boolean => 0x02,
                    TypeSig::Char => 0x03,
                    TypeSig::I1 => 0x04,
                    TypeSig::U1 => 0x05,
                    TypeSig::I2 => 0x06,
                    TypeSig::U2 => 0x07,
                    TypeSig::I4 => 0x08,
                    TypeSig::U4 => 0x09,
                    TypeSig::I8 => 0x0A,
                    TypeSig::U8 => 0x0B,
                    TypeSig::IntPtr => 0x18,
                    TypeSig::UIntPtr => 0x19,
                    _ => 0x08,
                };
            }
        }
        self.element_kind(exact)
    }
}

fn is_true_primitive(name: &str) -> bool {
    matches!(
        name,
        "System.Boolean"
            | "System.Char"
            | "System.SByte"
            | "System.Byte"
            | "System.Int16"
            | "System.UInt16"
            | "System.Int32"
            | "System.UInt32"
            | "System.Int64"
            | "System.UInt64"
            | "System.Single"
            | "System.Double"
            | "System.IntPtr"
            | "System.UIntPtr"
    )
}

pub(super) fn type_qcall(entry: &str, count: usize) -> Option<Reflect> {
    Some(match (entry, count) {
        ("MarshalNative_SizeOfHelper", 2) => Reflect::SizeOf,
        ("MarshalNative_GetHINSTANCE", 1) => Reflect::Instance,
        ("MarshalNative_GetTypeFromCLSID", 3) => Reflect::ClassFromClsid,
        ("ReflectionInvocation_GetComObjectGuid", 2) => Reflect::ComObjectGuid,
        ("ModuleHandle_GetModuleType", 2) => Reflect::ModuleType,
        ("CustomAttribute_CreateCustomAttributeInstance", 7) => Reflect::CreateAttribute,
        ("CustomAttribute_CreatePropertyOrFieldData", 7) => Reflect::AttributeNamed,
        ("CustomAttribute_ParseAttributeUsageAttribute", 5) => Reflect::AttributeUsage,
        ("ThreadNative_GetCurrentOSThreadId", 0) => Reflect::OsThreadId,
        ("WaitHandle_WaitOneCore", 3) => Reflect::WaitOne,
        ("WaitHandle_WaitMultipleIgnoringSyncContext", 4) => Reflect::WaitMany,
        ("WaitHandle_SignalAndWait", 3) => Reflect::SignalAndWait,
        ("RuntimeTypeHandle_GetGenericTypeDefinition", 2) => Reflect::GenericDefinition,
        ("RuntimeTypeHandle_GetInstantiation", 3) => Reflect::Instantiation,
        ("RuntimeTypeHandle_Instantiate", 4) => Reflect::Instantiate,
        ("RuntimeTypeHandle_MakeSZArray", 2) => Reflect::MakeVector,
        ("RuntimeTypeHandle_MakeArray", 3) => Reflect::MakeArray,
        ("RuntimeTypeHandle_MakeByRef", 2) => Reflect::MakeByRef,
        ("RuntimeTypeHandle_MakePointer", 2) => Reflect::MakePointer,
        ("RuntimeTypeHandle_ConstructName", 3) => Reflect::ConstructName,
        ("RuntimeTypeHandle_IsVisible", 1) => Reflect::IsVisible,
        ("RuntimeTypeHandle_IsCollectible", 1) => Reflect::IsCollectible,
        ("RuntimeTypeHandle_CreateInstanceForAnotherGenericParameter", 4) => Reflect::AnotherParameter,
        ("RuntimeTypeHandle_VerifyInterfaceIsImplemented", 2) => Reflect::Ignore,
        ("RuntimeTypeHandle_RegisterCollectibleTypeDependency", 2) => Reflect::Ignore,
        _ => return None,
    })
}

impl Interpreter<'_> {
    pub(super) fn qcall_type(&self, value: V) -> Run<Gen> {
        let offset = self.qcall_field("QCallTypeHandle", "_handle")?;
        let handle = unsafe { *((Self::struct_data(value) + u64::from(offset)) as *const u64) };
        self.method_table_type(handle)
    }

    pub(super) fn type_call(&self, operation: Reflect, a: &[V]) -> Run<Option<V>> {
        Ok(Some(match operation {
            Reflect::Ignore => return Ok(None),
            Reflect::SizeOf => {
                let exact = self.qcall_type(a[0])?;
                match self.gen_ty(&exact)? {
                    Ty::Prim(VarType::Ref) if !self.has_layout(&exact) => {
                        if a[1].int() != 0 {
                            return self.throw("ArgumentException");
                        }
                        V::I4(0)
                    }
                    Ty::Prim(var) if !matches!(var, VarType::Ref) => {
                        V::I4(ElementKind::from_var(var).map_or(8, ElementKind::size) as i32)
                    }
                    _ => V::I4(self.native_layout(&exact)?.size as i32),
                }
            }
            Reflect::Instance => V::P(crate::pinvoke::process_module()),
            Reflect::ClassFromClsid => {
                let clsid = unsafe { *(Self::struct_data(a[0]) as *const [u8; 16]) };
                let object = self.com_class_type(clsid)?;
                self.store_handle(a[2], object);
                return Ok(None);
            }
            Reflect::ComObjectGuid => {
                let offset = self.qcall_field("QCallTypeHandle", "_ptr")?;
                let slot = unsafe { *((Self::struct_data(a[0]) + u64::from(offset)) as *const u64) };
                let object = if slot == 0 { 0 } else { unsafe { *(slot as *const u64) } };
                let clsid = self.com_class_id(object).unwrap_or([0; 16]);
                unsafe { *(a[1].address() as *mut [u8; 16]) = clsid };
                return Ok(None);
            }
            Reflect::StructureToPointer | Reflect::PointerToStructure => {
                let (object, pointer) = match operation {
                    Reflect::StructureToPointer => (self.non_null(a[0])?, a[1].bits()),
                    _ => (self.non_null(a[1])?, a[0].bits()),
                };
                let exact = self.object_gen(object)?;
                let layout = self.native_layout(&exact)?;
                let managed = if self.types.is_value_type(&exact) { object + 8 } else { object };
                if matches!(operation, Reflect::StructureToPointer) {
                    let mut keep = Vec::new();
                    self.marshal_out(&layout, managed, pointer, &mut keep)?;
                    std::mem::forget(keep);
                } else {
                    self.marshal_in(&layout, pointer, managed)?;
                }
                return Ok(None);
            }
            Reflect::DestroyStructure => return Ok(None),
            Reflect::ModuleType => {
                let module = self.qcall_module(a[0])?;
                let object = self.type_object(&Gen::Named(TypeHandle { module, row: 1 }, Rc::from(Vec::new())))?;
                self.store_handle(a[1], object);
                return Ok(None);
            }
            Reflect::OsThreadId => V::I8(crate::pinvoke::current_thread_id() as i64),
            Reflect::WaitOne => V::I4(crate::pinvoke::wait_one(a[0].bits(), a[1].int() as i32)),
            Reflect::WaitMany => V::I4(crate::pinvoke::wait_many(a[0].bits(), a[1].int() as i32, a[2].int() != 0, a[3].int() as i32)),
            Reflect::SignalAndWait => V::I4(crate::pinvoke::signal_and_wait(a[0].bits(), a[1].bits(), a[2].int() as i32)),
            Reflect::SpinIterations => V::I4(8),
            Reflect::GenericDefinition => {
                let exact = self.qcall_type(a[0])?;
                let definition = match &exact {
                    Gen::Named(handle, arguments) if !arguments.is_empty() => Gen::Named(*handle, Rc::from(Vec::new())),
                    other => other.clone(),
                };
                let object = self.type_object(&definition)?;
                self.store_handle(a[1], object);
                return Ok(None);
            }
            Reflect::Instantiation => {
                let exact = self.qcall_type(a[0])?;
                let arguments = self.open_context(&exact).types;
                let array = self.type_array(&arguments)?;
                self.store_handle(a[1], array);
                return Ok(None);
            }
            Reflect::Instantiate | Reflect::AnotherParameter => {
                let exact = self.qcall_type(a[0])?;
                let pointer = a[1].address();
                let arguments = (0..a[2].int().max(0) as usize)
                    .map(|index| self.method_table_type(unsafe { *((pointer + 8 * index as u64) as *const u64) }))
                    .collect::<Run<Vec<_>>>()?;
                let Some(handle) = exact.handle() else {
                    return unsupported("instantiating a type that is not generic");
                };
                let instance = Gen::Named(handle, Rc::from(arguments));
                let object = if matches!(operation, Reflect::Instantiate) {
                    self.type_object(&instance)?
                } else {
                    self.construct_default(&instance)?
                };
                self.store_handle(a[3], object);
                return Ok(None);
            }
            Reflect::MakeVector | Reflect::MakeByRef | Reflect::MakePointer => {
                let inner = Rc::new(self.qcall_type(a[0])?);
                let made = match operation {
                    Reflect::MakeVector => Gen::SzArray(inner),
                    Reflect::MakeByRef => Gen::ByRef(inner),
                    _ => Gen::Pointer(inner),
                };
                let object = self.type_object(&made)?;
                self.store_handle(a[1], object);
                return Ok(None);
            }
            Reflect::MakeArray => {
                let inner = Rc::new(self.qcall_type(a[0])?);
                let object = self.type_object(&Gen::Array(inner, a[1].int() as u32))?;
                self.store_handle(a[2], object);
                return Ok(None);
            }
            Reflect::ConstructName => {
                let exact = self.qcall_type(a[0])?;
                let text = self.format_type(&exact, a[1].int() as u32)?;
                self.store_text(a[2], &text);
                return Ok(None);
            }
            Reflect::IsVisible => V::I4(1),
            Reflect::IsCollectible => V::I4(0),
            _ => return unsupported("type call"),
        }))
    }

    pub(super) fn format_type(&self, exact: &Gen, flags: u32) -> Run<String> {
        let set = self.model.set;
        let mut text = match exact {
            Gen::SzArray(inner) => format!("{}[]", self.format_type(inner, flags & !4)?),
            Gen::Array(inner, rank) => format!(
                "{}[{}]",
                self.format_type(inner, flags & !4)?,
                if *rank == 1 { "*".to_owned() } else { ",".repeat(*rank as usize - 1) }
            ),
            Gen::Pointer(inner) => format!("{}*", self.format_type(inner, flags & !4)?),
            Gen::ByRef(inner) => format!("{}&", self.format_type(inner, flags & !4)?),
            Gen::FunctionPointer => "*()".to_owned(),
            Gen::Var(..) | Gen::MethodVar(..) => return Ok(self.types.parameter_name(exact)),
            Gen::Named(handle, arguments) => {
                let metadata = &set.modules[handle.module].metadata;
                let mut chain = vec![handle.row];
                while let Some(outer) = set.modules[handle.module].enclosing_type(*chain.last().unwrap_or(&0)) {
                    chain.push(outer);
                }
                chain.reverse();
                if flags & 1 == 0 {
                    chain.drain(..chain.len() - 1);
                }
                let mut name = String::new();
                for (index, row) in chain.iter().enumerate() {
                    if index == 0 && flags & 1 != 0 {
                        let namespace = metadata.string_cell(TableId::TypeDef, *row, 2).unwrap_or("");
                        if !namespace.is_empty() {
                            name.push_str(namespace);
                            name.push('.');
                        }
                    }
                    if index > 0 {
                        name.push('+');
                    }
                    name.push_str(metadata.string_cell(TableId::TypeDef, *row, 1).unwrap_or(""));
                }
                if flags & 3 != 0 && !arguments.is_empty() {
                    let parts = arguments
                        .iter()
                        .map(|argument| {
                            if flags & 2 != 0 {
                                self.format_type(argument, 1 | 2 | 4).map(|t| format!("[{t}]"))
                            } else {
                                self.format_type(argument, flags & !4)
                            }
                        })
                        .collect::<Run<Vec<_>>>()?;
                    name.push('[');
                    name.push_str(&parts.join(","));
                    name.push(']');
                }
                name
            }
        };
        if flags & 4 != 0 {
            let module = self.gen_module(exact)?;
            text.push_str(", ");
            text.push_str(&assembly_full_name(set, module));
        }
        Ok(text)
    }

    pub(super) fn type_array(&self, types: &[Gen]) -> Run<u64> {
        let handle = self.corelib_type("System", "Type")?;
        let record = self.array_record(&Gen::Named(handle, Rc::from(Vec::new())))?;
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
}

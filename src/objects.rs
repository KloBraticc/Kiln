use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::codegen::{MethodKey, VarType};
use crate::loader::{AssemblySet, ResolveError, TypeHandle};
use crate::members::Member;
use crate::metadata::{TableId, Token};
use crate::signature::{Signature, TypeSig};

pub const OBJECT_HEADER: u32 = 8;
pub const ARRAY_DATA: u32 = 16;
pub const STRING_DATA: u32 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ElementKind {
    I8,
    U8,
    I16,
    U16,
    I32,
    I64,
    F32,
    F64,
    Reference,
}

impl ElementKind {
    pub fn size(self) -> u32 {
        match self {
            ElementKind::I8 | ElementKind::U8 => 1,
            ElementKind::I16 | ElementKind::U16 => 2,
            ElementKind::I32 | ElementKind::F32 => 4,
            _ => 8,
        }
    }

    pub fn var(self) -> VarType {
        match self {
            ElementKind::I8 => VarType::I8,
            ElementKind::U8 => VarType::U8,
            ElementKind::I16 => VarType::I16,
            ElementKind::U16 => VarType::U16,
            ElementKind::I32 => VarType::I32,
            ElementKind::I64 => VarType::I64,
            ElementKind::F32 => VarType::F32,
            ElementKind::F64 => VarType::F64,
            ElementKind::Reference => VarType::Ref,
        }
    }

    pub fn from_var(var: VarType) -> Option<ElementKind> {
        Some(match var {
            VarType::I8 => ElementKind::I8,
            VarType::U8 => ElementKind::U8,
            VarType::I16 => ElementKind::I16,
            VarType::U16 => ElementKind::U16,
            VarType::I32 => ElementKind::I32,
            VarType::I64 => ElementKind::I64,
            VarType::F32 => ElementKind::F32,
            VarType::F64 => ElementKind::F64,
            VarType::Ref => ElementKind::Reference,
            VarType::Pointer => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeType {
    Class(TypeHandle),
    Array(ElementKind),
    ValueArray(TypeHandle),
    String,
    Exact(u32),
    ExactArray(u32),
}

#[derive(Debug)]
pub struct StructLayout {
    pub handle: TypeHandle,
    pub instance: Option<u32>,
    pub size: u32,
    pub references: Vec<u32>,
    pub fields: HashMap<u32, u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldShape {
    Primitive,
    Reference,
    Struct(TypeHandle),
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub module: usize,
    pub row: u32,
    pub owner: TypeHandle,
    pub is_static: bool,
    pub has_rva: bool,
    pub var: VarType,
}

#[derive(Debug, Clone)]
pub struct ClassInfo {
    pub handle: TypeHandle,
    pub base_size: u32,
    pub reference_offsets: Vec<u32>,
    pub parent: Option<TypeHandle>,
    pub vtable: Vec<Option<MethodKey>>,
    pub cctor: Option<MethodKey>,
    pub is_abstract: bool,
    pub is_interface: bool,
    pub is_value_type: bool,
}

type Outcome<T> = Result<T, String>;

fn resolve(error: ResolveError) -> String {
    format!("{error:?}")
}

pub struct ObjectModel<'s> {
    pub set: &'s AssemblySet,
    classes: RefCell<HashMap<TypeHandle, Rc<ClassInfo>>>,
    slots: RefCell<HashMap<MethodKey, usize>>,
    generic: RefCell<HashMap<usize, std::collections::HashSet<u32>>>,
    structs: RefCell<HashMap<TypeHandle, Rc<StructLayout>>>,
}

impl<'s> ObjectModel<'s> {
    pub fn new(set: &'s AssemblySet) -> Self {
        Self {
            set,
            classes: RefCell::new(HashMap::new()),
            slots: RefCell::new(HashMap::new()),
            generic: RefCell::new(HashMap::new()),
            structs: RefCell::new(HashMap::new()),
        }
    }

    pub fn shape(&self, module: usize, sig: &TypeSig) -> Outcome<FieldShape> {
        match sig {
            TypeSig::Modified { inner, .. } | TypeSig::Pinned(inner) => self.shape(module, inner),
            TypeSig::ValueType(token) => {
                let handle = self
                    .set
                    .resolve_type_token(module, *token)
                    .map_err(resolve)?;
                if self.set.is_enum(handle).map_err(resolve)? {
                    Ok(FieldShape::Primitive)
                } else {
                    Ok(FieldShape::Struct(handle))
                }
            }
            TypeSig::GenericInst {
                value_type: true, ..
            } => Err("generic value types".into()),
            TypeSig::TypedReference => Err("typed references".into()),
            other => match VarType::from_sig(other) {
                Some(VarType::Ref) => Ok(FieldShape::Reference),
                Some(_) => Ok(FieldShape::Primitive),
                None => Err(format!("field type {other:?}")),
            },
        }
    }

    pub fn field_signature(&self, module: usize, row: u32) -> Outcome<TypeSig> {
        let metadata = &self.set.modules[module].metadata;
        let blob = metadata
            .cell(TableId::Field, row, 2)
            .and_then(|b| metadata.blob(b))
            .map_err(|e| e.to_string())?;
        match Signature::parse(blob).map_err(|e| e.to_string())? {
            Signature::Field(sig) => Ok(sig),
            _ => Err("field signature is not a field".into()),
        }
    }

    pub fn struct_layout(&self, handle: TypeHandle) -> Outcome<Rc<StructLayout>> {
        if let Some(found) = self.structs.borrow().get(&handle) {
            return Ok(found.clone());
        }
        if self.is_generic(handle) {
            return Err("generic value types".into());
        }
        let (layout, placements) = self.set.value_type_fields(handle).map_err(resolve)?;
        let mut references = Vec::new();
        let mut fields = HashMap::new();
        for placement in &placements {
            fields.insert(placement.field, placement.offset);
            if !placement.layout.has_references {
                continue;
            }
            let sig = self.field_signature(handle.module, placement.field)?;
            match self.shape(handle.module, &sig)? {
                FieldShape::Reference => references.push(placement.offset),
                FieldShape::Struct(inner) => {
                    let inner = self.struct_layout(inner)?;
                    references.extend(inner.references.iter().map(|r| placement.offset + r));
                }
                FieldShape::Primitive => {}
            }
        }
        if let Some(length) = self.set.modules[handle.module].inline_array_length(handle.row)
            && let Some(first) = placements.first()
        {
            let stride = first.layout.size;
            let element: Vec<u32> = references.clone();
            for index in 1..length {
                references.extend(element.iter().map(|r| r + index * stride));
            }
        }
        let built = Rc::new(StructLayout {
            handle,
            instance: None,
            size: layout.size,
            references,
            fields,
        });
        self.structs.borrow_mut().insert(handle, built.clone());
        Ok(built)
    }

    pub fn string_type(&self) -> Option<TypeHandle> {
        let core = self.set.find_assembly("System.Private.CoreLib")?;
        self.set.find_type_by_name(core, "System", "String")
    }

    pub fn is_generic(&self, handle: TypeHandle) -> bool {
        let mut cache = self.generic.borrow_mut();
        let rows = cache.entry(handle.module).or_insert_with(|| {
            self.set
                .generic_definitions(handle.module)
                .unwrap_or_default()
        });
        rows.contains(&handle.row)
    }

    pub fn method_flags(&self, key: &MethodKey) -> Outcome<u32> {
        self.set.modules[key.module]
            .metadata
            .cell(TableId::MethodDef, key.row, 2)
            .map_err(|e| e.to_string())
    }

    pub fn method_owner(&self, key: &MethodKey) -> Outcome<TypeHandle> {
        let row = self.set.modules[key.module]
            .method_owner(key.row)
            .map_err(|e| e.to_string())?
            .ok_or("method has no owner")?;
        Ok(TypeHandle {
            module: key.module,
            row,
        })
    }

    pub fn field(&self, module: usize, token: Token) -> Outcome<FieldInfo> {
        let (module, row) = match token.table() {
            Some(TableId::Field) => (module, token.row()),
            Some(TableId::MemberRef) => match self
                .set
                .resolve_member_ref(module, token.row())
                .map_err(resolve)?
            {
                Member::Field(handle) => (handle.module, handle.token.row()),
                _ => return Err("member reference is not a field".into()),
            },
            _ => return Err("token is not a field".into()),
        };
        let metadata = &self.set.modules[module].metadata;
        let flags = metadata
            .cell(TableId::Field, row, 0)
            .map_err(|e| e.to_string())?;
        let blob = metadata
            .cell(TableId::Field, row, 2)
            .and_then(|b| metadata.blob(b))
            .map_err(|e| e.to_string())?;
        let Signature::Field(sig) = Signature::parse(blob).map_err(|e| e.to_string())? else {
            return Err("field signature is not a field".into());
        };
        let var = self
            .var_type(module, &sig)?
            .ok_or_else(|| format!("field type {sig:?}"))?;
        let owner = self.set.modules[module]
            .field_owner(row)
            .map_err(|e| e.to_string())?
            .ok_or("field has no owner")?;
        Ok(FieldInfo {
            module,
            row,
            owner: TypeHandle { module, row: owner },
            is_static: flags & 0x10 != 0,
            has_rva: flags & 0x100 != 0,
            var,
        })
    }

    pub fn var_type(&self, module: usize, sig: &TypeSig) -> Outcome<Option<VarType>> {
        if let Some(var) = VarType::from_sig(sig) {
            return Ok(Some(var));
        }
        Ok(match sig {
            TypeSig::ValueType(token) => {
                let handle = self
                    .set
                    .resolve_type_token(module, *token)
                    .map_err(resolve)?;
                if self.set.is_enum(handle).map_err(resolve)? {
                    match self.set.value_type_layout(handle).map_err(resolve)?.size {
                        1 => Some(VarType::U8),
                        2 => Some(VarType::U16),
                        4 => Some(VarType::I32),
                        _ => Some(VarType::I64),
                    }
                } else {
                    None
                }
            }
            TypeSig::Modified { inner, .. } => self.var_type(module, inner)?,
            _ => None,
        })
    }

    pub fn element_kind(&self, module: usize, token: Token) -> Outcome<ElementKind> {
        let sig = match token.table() {
            Some(TableId::TypeSpec) => {
                let metadata = &self.set.modules[module].metadata;
                Signature::parse_type(
                    metadata
                        .cell(TableId::TypeSpec, token.row(), 0)
                        .and_then(|b| metadata.blob(b))
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?
            }
            Some(TableId::TypeDef) | Some(TableId::TypeRef) => {
                let handle = self
                    .set
                    .resolve_type_token(module, token)
                    .map_err(resolve)?;
                if !self.set.is_value_type(handle).map_err(resolve)? {
                    return Ok(ElementKind::Reference);
                }
                let primitive = match self.set.full_type_name(handle).map_err(resolve)?.as_str() {
                    "System.Boolean" | "System.Byte" => Some(ElementKind::U8),
                    "System.SByte" => Some(ElementKind::I8),
                    "System.Int16" => Some(ElementKind::I16),
                    "System.UInt16" | "System.Char" => Some(ElementKind::U16),
                    "System.Int32" | "System.UInt32" => Some(ElementKind::I32),
                    "System.Int64" | "System.UInt64" | "System.IntPtr" | "System.UIntPtr" => {
                        Some(ElementKind::I64)
                    }
                    "System.Single" => Some(ElementKind::F32),
                    "System.Double" => Some(ElementKind::F64),
                    _ => None,
                };
                if let Some(kind) = primitive {
                    return Ok(kind);
                }
                if !self.set.is_enum(handle).map_err(resolve)? {
                    return Err("struct array elements".into());
                }
                return Ok(
                    match self.set.value_type_layout(handle).map_err(resolve)?.size {
                        1 => ElementKind::U8,
                        2 => ElementKind::U16,
                        4 => ElementKind::I32,
                        _ => ElementKind::I64,
                    },
                );
            }
            _ => return Err("element type token".into()),
        };
        self.var_type(module, &sig)?
            .and_then(ElementKind::from_var)
            .ok_or_else(|| format!("array element {sig:?}"))
    }

    pub fn instance_offset(&self, field: &FieldInfo) -> Outcome<u32> {
        let info = self.class(field.owner)?;
        if info.is_value_type {
            return Err("fields of value types".into());
        }
        let (_, placements) = self.set.class_fields(field.owner).map_err(resolve)?;
        placements
            .iter()
            .find(|p| p.field == field.row)
            .map(|p| p.offset + OBJECT_HEADER)
            .ok_or_else(|| "field is not an instance field of its owner".into())
    }

    pub fn class(&self, handle: TypeHandle) -> Outcome<Rc<ClassInfo>> {
        if let Some(info) = self.classes.borrow().get(&handle) {
            return Ok(info.clone());
        }
        if self.is_generic(handle) {
            return Err("generic types".into());
        }
        let metadata = &self.set.modules[handle.module].metadata;
        let flags = metadata
            .cell(TableId::TypeDef, handle.row, 0)
            .map_err(|e| e.to_string())?;
        let is_interface = flags & 0x20 != 0;
        let is_value_type = self.set.is_value_type(handle).map_err(resolve)?;
        let parent = if is_interface {
            None
        } else {
            self.set.base_type(handle).map_err(resolve)?
        };
        if let Some(parent) = parent
            && self.is_generic(parent)
        {
            return Err("generic base classes".into());
        }
        let mut reference_offsets = Vec::new();
        let mut base_size = OBJECT_HEADER;
        if !is_interface && !is_value_type {
            if let Some(parent) = parent {
                let parent_info = self.class(parent)?;
                reference_offsets.extend_from_slice(&parent_info.reference_offsets);
            }
            let (end, placements) = self.set.class_fields(handle).map_err(resolve)?;
            for placement in &placements {
                if !placement.layout.has_references {
                    continue;
                }
                let sig = self.field_signature(handle.module, placement.field)?;
                match self.shape(handle.module, &sig)? {
                    FieldShape::Reference => {
                        reference_offsets.push(placement.offset + OBJECT_HEADER)
                    }
                    FieldShape::Struct(inner) => {
                        let inner = self.struct_layout(inner)?;
                        reference_offsets.extend(
                            inner
                                .references
                                .iter()
                                .map(|r| placement.offset + OBJECT_HEADER + r),
                        );
                    }
                    FieldShape::Primitive => {}
                }
            }
            base_size = (end + OBJECT_HEADER).next_multiple_of(8).max(16);
        } else if is_value_type {
            let layout = self.struct_layout(handle)?;
            reference_offsets.extend(layout.references.iter().map(|r| r + OBJECT_HEADER));
            base_size = (layout.size + OBJECT_HEADER).next_multiple_of(8).max(16);
        }
        let mut vtable = match parent {
            Some(parent) => self.class(parent)?.vtable.clone(),
            _ => Vec::new(),
        };
        let mut cctor = None;
        let methods = metadata
            .method_range(handle.row)
            .map_err(|e| e.to_string())?;
        let mut slots = self.slots.borrow_mut();
        for row in methods.clone() {
            let key = MethodKey {
                module: handle.module,
                row,
            };
            let method_flags = metadata
                .cell(TableId::MethodDef, row, 2)
                .map_err(|e| e.to_string())?;
            let name = metadata
                .string_cell(TableId::MethodDef, row, 3)
                .map_err(|e| e.to_string())?;
            if name == ".cctor" {
                cctor = Some(key.clone());
            }
            if method_flags & 0x40 == 0 || is_interface {
                continue;
            }
            let mut assigned = None;
            if method_flags & 0x100 == 0 {
                let sig = self.method_sig(&key)?;
                for (slot, existing) in vtable.iter().enumerate().rev() {
                    let Some(existing) = existing else { continue };
                    let existing_metadata = &self.set.modules[existing.module].metadata;
                    if existing_metadata
                        .string_cell(TableId::MethodDef, existing.row, 3)
                        .map_err(|e| e.to_string())?
                        != name
                    {
                        continue;
                    }
                    let existing_sig = self.method_sig(existing)?;
                    if self
                        .set
                        .methods_equal(key.module, &sig, existing.module, &existing_sig)
                        .map_err(resolve)?
                    {
                        assigned = Some(slot);
                        break;
                    }
                }
            }
            let slot = match assigned {
                Some(slot) => {
                    vtable[slot] = Some(key.clone());
                    slot
                }
                None => {
                    vtable.push(Some(key.clone()));
                    vtable.len() - 1
                }
            };
            slots.insert(key, slot);
        }
        for row in 1..=metadata.rows(TableId::MethodImpl) {
            if metadata
                .cell(TableId::MethodImpl, row, 0)
                .map_err(|e| e.to_string())?
                != handle.row
            {
                continue;
            }
            let body = metadata
                .coded(TableId::MethodImpl, row, 1)
                .map_err(|e| e.to_string())?;
            let declaration = metadata
                .coded(TableId::MethodImpl, row, 2)
                .map_err(|e| e.to_string())?;
            let (Some(body), Some(declaration)) = (body, declaration) else {
                continue;
            };
            let Some(body) = self.method_key(handle.module, body) else {
                continue;
            };
            let Some(declaration) = self.method_key(handle.module, declaration) else {
                continue;
            };
            if let Some(&slot) = slots.get(&declaration)
                && slot < vtable.len()
            {
                vtable[slot] = Some(body);
            }
        }
        drop(slots);
        let info = Rc::new(ClassInfo {
            handle,
            base_size,
            reference_offsets,
            parent: if is_value_type { None } else { parent },
            vtable,
            cctor,
            is_abstract: flags & 0x80 != 0,
            is_interface,
            is_value_type,
        });
        self.classes.borrow_mut().insert(handle, info.clone());
        Ok(info)
    }

    pub fn method_key(&self, module: usize, token: Token) -> Option<MethodKey> {
        match token.table() {
            Some(TableId::MethodDef) => Some(MethodKey {
                module,
                row: token.row(),
            }),
            Some(TableId::MemberRef) => {
                match self.set.resolve_member_ref(module, token.row()).ok()? {
                    Member::Method(handle) if handle.token.table() == Some(TableId::MethodDef) => {
                        Some(MethodKey {
                            module: handle.module,
                            row: handle.token.row(),
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub fn method_sig(&self, key: &MethodKey) -> Outcome<crate::signature::MethodSig> {
        let metadata = &self.set.modules[key.module].metadata;
        let blob = metadata
            .cell(TableId::MethodDef, key.row, 4)
            .and_then(|b| metadata.blob(b))
            .map_err(|e| e.to_string())?;
        match Signature::parse(blob).map_err(|e| e.to_string())? {
            Signature::Method(sig) => Ok(sig),
            _ => Err("not a method signature".into()),
        }
    }

    pub fn virtual_slot(&self, key: &MethodKey) -> Outcome<usize> {
        let owner = self.method_owner(key)?;
        self.class(owner)?;
        self.slots
            .borrow()
            .get(key)
            .copied()
            .ok_or_else(|| "method has no virtual slot".into())
    }
}

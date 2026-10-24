use crate::loader::{AssemblySet, ResolveError, TypeHandle};
use crate::metadata::{TableId, Token};
use crate::signature::{Signature, TypeSig};

type Outcome<T> = std::result::Result<T, ResolveError>;

const POINTER_SIZE: u32 = 8;
const MAX_DEPTH: u32 = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub size: u32,
    pub alignment: u32,
    pub has_references: bool,
}

impl Layout {
    const fn primitive(size: u32) -> Self {
        Self {
            size,
            alignment: size,
            has_references: false,
        }
    }

    const REFERENCE: Layout = Layout {
        size: POINTER_SIZE,
        alignment: POINTER_SIZE,
        has_references: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldPlacement {
    pub field: u32,
    pub offset: u32,
    pub layout: Layout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayoutKind {
    Auto,
    Sequential,
    Explicit,
}

#[derive(Clone)]
struct Context<'s> {
    module: usize,
    arguments: &'s [(usize, TypeSig)],
}

impl AssemblySet {
    pub fn is_value_type(&self, handle: TypeHandle) -> Outcome<bool> {
        let Some(base) = self.base_type(handle)? else {
            return Ok(false);
        };
        let base_name = self.full_type_name(base)?;
        if base_name == "System.Enum" {
            return Ok(true);
        }
        Ok(base_name == "System.ValueType" && self.full_type_name(handle)? != "System.Enum")
    }

    pub fn is_enum(&self, handle: TypeHandle) -> Outcome<bool> {
        Ok(match self.base_type(handle)? {
            Some(base) => self.full_type_name(base)? == "System.Enum",
            None => false,
        })
    }

    pub fn full_type_name(&self, handle: TypeHandle) -> Outcome<String> {
        Ok(self.modules[handle.module].full_name(handle.row)?)
    }

    pub fn value_type_layout(&self, handle: TypeHandle) -> Outcome<Layout> {
        self.layout_of(handle, &[], 0)
    }

    pub fn value_type_fields(&self, handle: TypeHandle) -> Outcome<(Layout, Vec<FieldPlacement>)> {
        self.layout_detail(handle, &[], 0)
    }

    fn layout_of(
        &self,
        handle: TypeHandle,
        arguments: &[(usize, TypeSig)],
        depth: u32,
    ) -> Outcome<Layout> {
        if let Some(layout) = self.intrinsic_layout(handle)? {
            return Ok(layout);
        }
        Ok(self.layout_detail(handle, arguments, depth)?.0)
    }

    fn layout_detail(
        &self,
        handle: TypeHandle,
        arguments: &[(usize, TypeSig)],
        depth: u32,
    ) -> Outcome<(Layout, Vec<FieldPlacement>)> {
        if depth > MAX_DEPTH {
            return Err(ResolveError::Malformed(
                "value type layout is recursive".into(),
            ));
        }
        let module = &self.modules[handle.module];
        let metadata = &module.metadata;
        let flags = metadata.cell(TableId::TypeDef, handle.row, 0)?;
        let kind = match flags & 0x18 {
            0x08 => LayoutKind::Sequential,
            0x10 => LayoutKind::Explicit,
            _ => LayoutKind::Auto,
        };
        let (packing, class_size) = module.class_layout(handle.row)?.unwrap_or((0, 0));
        let packing = if packing == 0 { u32::MAX } else { packing };
        let context = Context {
            module: handle.module,
            arguments,
        };

        let mut fields = Vec::new();
        for field in metadata.field_range(handle.row)? {
            if metadata.cell(TableId::Field, field, 0)? & 0x10 != 0 {
                continue;
            }
            let Signature::Field(sig) =
                Signature::parse(metadata.blob(metadata.cell(TableId::Field, field, 2)?)?)?
            else {
                return Err(ResolveError::Malformed(
                    "field signature is not a field".into(),
                ));
            };
            let layout = self.field_layout(&context, &sig, depth + 1)?;
            let category = if layout.has_references && is_primitive_like(&sig) {
                0
            } else if is_primitive_like(&sig) || self.is_enum_field(&context, &sig)? {
                1
            } else {
                2
            };
            fields.push((field, layout, category));
        }

        let place = |field: u32, offset: u32, layout: Layout| FieldPlacement {
            field,
            offset,
            layout,
        };

        if self.is_enum(handle)? {
            let value = fields
                .first()
                .ok_or_else(|| ResolveError::Malformed("enum has no value field".into()))?;
            return Ok((value.1, vec![place(value.0, 0, value.1)]));
        }

        if let Some(length) = module.inline_array_length(handle.row) {
            let element = fields.first().ok_or_else(|| {
                ResolveError::Malformed("inline array has no element field".into())
            })?;
            let layout = Layout {
                size: (element.1.size * length).max(class_size),
                alignment: element.1.alignment,
                has_references: element.1.has_references,
            };
            return Ok((layout, vec![place(element.0, 0, element.1)]));
        }

        let has_references = fields.iter().any(|f| f.1.has_references);
        let mut alignment = 1;
        let mut size = 0u32;
        let mut placements = Vec::with_capacity(fields.len());
        match kind {
            LayoutKind::Explicit => {
                for (field, layout, _) in &fields {
                    let offset = module.field_offset(*field).ok_or_else(|| {
                        ResolveError::Malformed("explicit field has no offset".into())
                    })?;
                    size = size.max(offset + layout.size);
                    alignment = alignment.max(layout.alignment.min(packing));
                    placements.push(place(*field, offset, *layout));
                }
            }
            LayoutKind::Sequential if !has_references => {
                for (field, layout, _) in &fields {
                    let field_alignment = layout.alignment.min(packing);
                    let offset = size.next_multiple_of(field_alignment);
                    size = offset + layout.size;
                    alignment = alignment.max(field_alignment);
                    placements.push(place(*field, offset, *layout));
                }
            }
            _ => {
                let mut ordered: Vec<&(u32, Layout, u8)> = fields.iter().collect();
                ordered.sort_by_key(|(_, layout, category)| {
                    (
                        *category,
                        std::cmp::Reverse(if *category == 2 { 0 } else { layout.size }),
                    )
                });
                for (field, layout, _) in ordered {
                    let offset = size.next_multiple_of(layout.alignment);
                    size = offset + layout.size;
                    alignment = alignment.max(layout.alignment);
                    placements.push(place(*field, offset, *layout));
                }
            }
        }
        size = if class_size > 0 {
            size.max(class_size)
        } else if (kind == LayoutKind::Auto || has_references) && size > POINTER_SIZE {
            size.next_multiple_of(alignment.max(POINTER_SIZE))
        } else {
            size.next_multiple_of(alignment)
        }
        .max(1);
        Ok((
            Layout {
                size,
                alignment,
                has_references,
            },
            placements,
        ))
    }

    fn is_enum_field(&self, context: &Context<'_>, sig: &TypeSig) -> Outcome<bool> {
        match sig {
            TypeSig::Modified { inner, .. } => self.is_enum_field(context, inner),
            TypeSig::ValueType(token) => {
                self.is_enum(self.resolve_type_token(context.module, *token)?)
            }
            TypeSig::GenericInst {
                value_type: true,
                definition,
                ..
            } => self.is_enum(self.resolve_type_token(context.module, *definition)?),
            _ => Ok(false),
        }
    }

    fn intrinsic_layout(&self, handle: TypeHandle) -> Outcome<Option<Layout>> {
        let module = &self.modules[handle.module];
        if module.metadata.rows(TableId::Assembly) > 0
            && !module.name.eq_ignore_ascii_case("System.Private.CoreLib")
        {
            return Ok(None);
        }
        let name = self.full_type_name(handle)?;
        Ok(match name.as_str() {
            "System.Runtime.Intrinsics.Vector64`1" => Some(Layout::primitive(8)),
            "System.Runtime.Intrinsics.Vector128`1" => Some(Layout::primitive(16)),
            "System.Runtime.Intrinsics.Vector256`1" => Some(Layout::primitive(32)),
            "System.Runtime.Intrinsics.Vector512`1" => Some(Layout::primitive(64)),
            "System.Int128" | "System.UInt128" => Some(Layout {
                size: 16,
                alignment: 16,
                has_references: false,
            }),
            _ => None,
        })
    }

    fn field_layout(&self, context: &Context<'_>, sig: &TypeSig, depth: u32) -> Outcome<Layout> {
        use TypeSig as T;
        Ok(match sig {
            T::Boolean | T::I1 | T::U1 => Layout::primitive(1),
            T::Char | T::I2 | T::U2 => Layout::primitive(2),
            T::I4 | T::U4 | T::R4 => Layout::primitive(4),
            T::I8 | T::U8 | T::R8 => Layout::primitive(8),
            T::IntPtr | T::UIntPtr | T::Pointer(_) | T::FunctionPointer(_) => {
                Layout::primitive(POINTER_SIZE)
            }
            T::ByRef(_) => Layout::primitive(POINTER_SIZE),
            T::TypedReference => Layout {
                size: 16,
                alignment: 8,
                has_references: false,
            },
            T::String | T::Object | T::Class(_) | T::SzArray(_) | T::Array(_) => Layout::REFERENCE,
            T::Modified { inner, .. } | T::Pinned(inner) => {
                self.field_layout(context, inner, depth)?
            }
            T::ValueType(token) => {
                let handle = self.resolve_type_token(context.module, *token)?;
                self.layout_of(handle, &[], depth)?
            }
            T::GenericInst {
                value_type: false, ..
            } => Layout::REFERENCE,
            T::GenericInst {
                value_type: true,
                definition,
                arguments,
            } => {
                let handle = self.resolve_type_token(context.module, *definition)?;
                let substituted: Vec<(usize, TypeSig)> = arguments
                    .iter()
                    .map(|a| self.substitute(context, a))
                    .collect();
                self.layout_of(handle, &substituted, depth)?
            }
            T::TypeVar(index) => {
                let (module, argument) = context
                    .arguments
                    .get(*index as usize)
                    .ok_or(ResolveError::Unsupported("open generic value type layout"))?;
                self.field_layout(
                    &Context {
                        module: *module,
                        arguments: &[],
                    },
                    argument,
                    depth + 1,
                )?
            }
            T::MethodVar(_) => {
                return Err(ResolveError::Unsupported("open generic value type layout"));
            }
            T::Void | T::Sentinel => {
                return Err(ResolveError::Malformed("field has no storage type".into()));
            }
        })
    }

    fn substitute(&self, context: &Context<'_>, sig: &TypeSig) -> (usize, TypeSig) {
        if let TypeSig::TypeVar(index) = sig
            && let Some(bound) = context.arguments.get(*index as usize)
        {
            return bound.clone();
        }
        (context.module, sig.clone())
    }

    pub fn types_with_attribute(
        &self,
        module_index: usize,
        namespace: &str,
        name: &str,
    ) -> Outcome<std::collections::HashSet<u32>> {
        let module = &self.modules[module_index];
        let metadata = &module.metadata;
        let mut found = std::collections::HashSet::new();
        for row in 1..=metadata.rows(TableId::CustomAttribute) {
            let Some(parent) = metadata.coded(TableId::CustomAttribute, row, 0)? else {
                continue;
            };
            if parent.table() != Some(TableId::TypeDef) {
                continue;
            }
            let Some(constructor) = metadata.coded(TableId::CustomAttribute, row, 1)? else {
                continue;
            };
            let owner = match constructor.table() {
                Some(TableId::MethodDef) => module
                    .method_owner(constructor.row())?
                    .map(|row| Token::new(TableId::TypeDef, row)),
                Some(TableId::MemberRef) => {
                    metadata.coded(TableId::MemberRef, constructor.row(), 0)?
                }
                _ => None,
            };
            let Some(owner) = owner else { continue };
            let table = match owner.table() {
                Some(table @ (TableId::TypeDef | TableId::TypeRef)) => table,
                _ => continue,
            };
            if metadata.string_cell(table, owner.row(), 2)? == namespace
                && metadata.string_cell(table, owner.row(), 1)? == name
            {
                found.insert(parent.row());
            }
        }
        Ok(found)
    }

    pub fn generic_definitions(
        &self,
        module_index: usize,
    ) -> Outcome<std::collections::HashSet<u32>> {
        let metadata = &self.modules[module_index].metadata;
        let mut found = std::collections::HashSet::new();
        for row in 1..=metadata.rows(TableId::GenericParam) {
            if let Some(owner) = metadata.coded(TableId::GenericParam, row, 2)?
                && owner.table() == Some(TableId::TypeDef)
            {
                found.insert(owner.row());
            }
        }
        Ok(found)
    }
}

fn is_primitive_like(sig: &TypeSig) -> bool {
    use TypeSig as T;
    match sig {
        T::Modified { inner, .. } => is_primitive_like(inner),
        T::ValueType(_)
        | T::GenericInst {
            value_type: true, ..
        }
        | T::TypeVar(_)
        | T::MethodVar(_)
        | T::TypedReference => false,
        _ => true,
    }
}

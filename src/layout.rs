use crate::loader::{AssemblySet, ResolveError, TypeHandle};
use crate::metadata::{TableId, Token};
use crate::signature::{Signature, TypeSig};

type Outcome<T> = std::result::Result<T, ResolveError>;

const POINTER_SIZE: u32 = 8;
const LOG2_POINTER: usize = 3;
const MAX_LOG2_PRIMITIVE: usize = 3;
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
enum Slot {
    Reference,
    Primitive(usize),
    Value(Layout),
}

impl Slot {
    fn layout(self) -> Layout {
        match self {
            Slot::Reference => Layout::REFERENCE,
            Slot::Primitive(log2) => Layout::primitive(1 << log2),
            Slot::Value(layout) => layout,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayoutKind {
    Auto,
    Sequential,
    Explicit,
}

struct AutoResult {
    placements: Vec<FieldPlacement>,
    end: u32,
    largest_alignment: u32,
    contains_references: bool,
}

type BaseClass = (TypeHandle, Vec<(usize, TypeSig)>);

#[derive(Clone)]
struct Context<'s> {
    module: usize,
    arguments: &'s [(usize, TypeSig)],
}

fn align(value: u32, alignment: u32) -> u32 {
    value.next_multiple_of(alignment.max(1))
}

fn auto_layout(start: u32, fields: &[(u32, Slot)]) -> AutoResult {
    let size_of = |slot: Slot| match slot {
        Slot::Reference => Some(LOG2_POINTER),
        Slot::Primitive(log2) => Some(log2),
        Slot::Value(_) => None,
    };
    let mut counts = [0u32; MAX_LOG2_PRIMITIVE + 1];
    let mut first = [usize::MAX; MAX_LOG2_PRIMITIVE + 1];
    let mut references = 0u32;
    for (index, (_, slot)) in fields.iter().enumerate() {
        if let Some(log2) = size_of(*slot) {
            counts[log2] += 1;
            if first[log2] == usize::MAX {
                first[log2] = index;
            }
            if *slot == Slot::Reference {
                references += 1;
            }
        }
    }

    let mut offsets: Vec<Option<u32>> = vec![None; fields.len()];
    let mut position = start;
    if !position.is_multiple_of(POINTER_SIZE) {
        let mut i = 0usize;
        while i < MAX_LOG2_PRIMITIVE {
            if position.is_multiple_of(1 << (i + 1)) {
                i += 1;
                continue;
            }
            if !(i + 1..=MAX_LOG2_PRIMITIVE).any(|j| counts[j] != 0) {
                break;
            }
            let Some(j) = (0..=i).rev().find(|&j| counts[j] != 0) else {
                break;
            };
            i = j;
            let index = if i == LOG2_POINTER {
                match (first[i]..fields.len())
                    .find(|&k| offsets[k].is_none() && fields[k].1 == Slot::Primitive(i))
                {
                    Some(k) => k,
                    None => break,
                }
            } else {
                first[i]
            };
            position = align(position, 1 << i);
            offsets[index] = Some(position);
            position += 1 << i;
            counts[i] -= 1;
            if counts[i] != 0
                && first[i] == index
                && let Some(next) = (index + 1..fields.len())
                    .find(|&k| offsets[k].is_none() && size_of(fields[k].1) == Some(i))
            {
                first[i] = next;
            }
            i += 1;
        }
    }

    let mut region = [0u32; MAX_LOG2_PRIMITIVE + 1];
    for i in (0..=MAX_LOG2_PRIMITIVE).rev() {
        if counts[i] == 0 {
            continue;
        }
        position = align(position, 1 << i);
        region[i] = position;
        position += counts[i] << i;
        counts[i] = 0;
    }
    let reference_start = region[LOG2_POINTER];
    region[LOG2_POINTER] += references << LOG2_POINTER;
    let mut placed_references = 0u32;
    for (index, (_, slot)) in fields.iter().enumerate() {
        if offsets[index].is_some() {
            continue;
        }
        match *slot {
            Slot::Reference => {
                offsets[index] = Some(reference_start + (placed_references << LOG2_POINTER));
                placed_references += 1;
            }
            Slot::Primitive(log2) => {
                offsets[index] = Some(region[log2] + (counts[log2] << log2));
                counts[log2] += 1;
            }
            Slot::Value(_) => {}
        }
    }

    let mut contains_references = references > 0;
    let mut largest_alignment = 1u32;
    for (index, (_, slot)) in fields.iter().enumerate() {
        match *slot {
            Slot::Value(layout) => {
                let requirement = if layout.has_references {
                    contains_references = true;
                    POINTER_SIZE
                } else {
                    layout.alignment
                };
                largest_alignment = largest_alignment.max(requirement);
                position = align(position, requirement);
                offsets[index] = Some(position);
                position += layout.size;
            }
            _ => largest_alignment = largest_alignment.max(POINTER_SIZE),
        }
    }

    let placements = fields
        .iter()
        .zip(offsets)
        .map(|((field, slot), offset)| FieldPlacement {
            field: *field,
            offset: offset.unwrap_or(0),
            layout: slot.layout(),
        })
        .collect();
    AutoResult {
        placements,
        end: position,
        largest_alignment,
        contains_references,
    }
}

fn sequential_layout(
    start: u32,
    fields: &[(u32, Slot)],
    packing: u32,
) -> (Vec<FieldPlacement>, u32, u32) {
    let mut size = start;
    let mut alignment = 1;
    let mut placements = Vec::with_capacity(fields.len());
    for (field, slot) in fields {
        let layout = slot.layout();
        let field_alignment = layout.alignment.min(packing);
        let offset = align(size, field_alignment);
        size = offset + layout.size;
        alignment = alignment.max(field_alignment);
        placements.push(FieldPlacement {
            field: *field,
            offset,
            layout,
        });
    }
    (placements, size, alignment)
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
        self.value_detail(handle, &[], 0)
    }

    pub fn class_fields(&self, handle: TypeHandle) -> Outcome<(u32, Vec<FieldPlacement>)> {
        let (size, placements) = self.class_detail(handle, &[], 0)?;
        Ok((size, placements))
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
        Ok(self.value_detail(handle, arguments, depth)?.0)
    }

    fn layout_kind(&self, handle: TypeHandle) -> Outcome<LayoutKind> {
        let flags = self.modules[handle.module]
            .metadata
            .cell(TableId::TypeDef, handle.row, 0)?;
        Ok(match flags & 0x18 {
            0x08 => LayoutKind::Sequential,
            0x10 => LayoutKind::Explicit,
            _ => LayoutKind::Auto,
        })
    }

    fn instance_fields(
        &self,
        context: &Context<'_>,
        handle: TypeHandle,
        depth: u32,
    ) -> Outcome<Vec<(u32, Slot)>> {
        let metadata = &self.modules[handle.module].metadata;
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
            fields.push((field, self.classify(context, &sig, depth + 1)?));
        }
        Ok(fields)
    }

    fn value_detail(
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
        let context = Context {
            module: handle.module,
            arguments,
        };
        let fields = self.instance_fields(&context, handle, depth)?;
        let (packing, class_size) = module.class_layout(handle.row)?.unwrap_or((0, 0));
        let packing = if packing == 0 { u32::MAX } else { packing };

        if self.is_enum(handle)? {
            let (field, slot) = *fields
                .first()
                .ok_or_else(|| ResolveError::Malformed("enum has no value field".into()))?;
            let layout = slot.layout();
            return Ok((
                layout,
                vec![FieldPlacement {
                    field,
                    offset: 0,
                    layout,
                }],
            ));
        }

        if let Some(length) = module.inline_array_length(handle.row) {
            let (field, slot) = *fields.first().ok_or_else(|| {
                ResolveError::Malformed("inline array has no element field".into())
            })?;
            let element = slot.layout();
            let layout = Layout {
                size: (element.size * length).max(class_size),
                alignment: element.alignment,
                has_references: element.has_references,
            };
            return Ok((
                layout,
                vec![FieldPlacement {
                    field,
                    offset: 0,
                    layout: element,
                }],
            ));
        }

        let has_references = fields.iter().any(|(_, slot)| slot.layout().has_references);
        let kind = match self.layout_kind(handle)? {
            LayoutKind::Sequential if has_references => LayoutKind::Auto,
            kind => kind,
        };

        match kind {
            LayoutKind::Explicit => {
                let mut size = 0u32;
                let mut alignment = 1u32;
                let mut placements = Vec::with_capacity(fields.len());
                for (field, slot) in &fields {
                    let layout = slot.layout();
                    let offset = module.field_offset(*field).ok_or_else(|| {
                        ResolveError::Malformed("explicit field has no offset".into())
                    })?;
                    size = size.max(offset + layout.size);
                    alignment = alignment.max(layout.alignment.min(packing));
                    placements.push(FieldPlacement {
                        field: *field,
                        offset,
                        layout,
                    });
                }
                let size = if class_size > 0 {
                    size.max(class_size)
                } else {
                    align(size, alignment)
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
            LayoutKind::Sequential => {
                let (placements, size, alignment) = sequential_layout(0, &fields, packing);
                let size = if class_size > 0 {
                    size.max(class_size)
                } else {
                    align(size, alignment)
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
            LayoutKind::Auto => {
                let result = auto_layout(0, &fields);
                let size = result.end.max(1);
                let minimum = if size > POINTER_SIZE {
                    if result.contains_references {
                        POINTER_SIZE
                    } else {
                        result.largest_alignment
                    }
                } else {
                    size.next_power_of_two()
                };
                Ok((
                    Layout {
                        size: align(size, minimum),
                        alignment: minimum,
                        has_references: result.contains_references,
                    },
                    result.placements,
                ))
            }
        }
    }

    fn base_with_arguments(
        &self,
        context: &Context<'_>,
        handle: TypeHandle,
    ) -> Outcome<Option<BaseClass>> {
        let metadata = &self.modules[handle.module].metadata;
        let Some(extends) = metadata.coded(TableId::TypeDef, handle.row, 3)? else {
            return Ok(None);
        };
        if extends.table() != Some(TableId::TypeSpec) {
            return Ok(Some((
                self.resolve_type_token(handle.module, extends)?,
                Vec::new(),
            )));
        }
        let spec = Signature::parse_type(metadata.blob(metadata.cell(
            TableId::TypeSpec,
            extends.row(),
            0,
        )?)?)?;
        let TypeSig::GenericInst {
            definition,
            arguments,
            ..
        } = spec
        else {
            return Err(ResolveError::Malformed("base type is not a class".into()));
        };
        let base = self.resolve_type_token(handle.module, definition)?;
        let bound = arguments
            .iter()
            .map(|a| self.substitute(context, a))
            .collect();
        Ok(Some((base, bound)))
    }

    fn real_base(&self, context: &Context<'_>, handle: TypeHandle) -> Outcome<Option<BaseClass>> {
        Ok(match self.base_with_arguments(context, handle)? {
            Some((base, arguments)) if self.full_type_name(base)? != "System.Object" => {
                Some((base, arguments))
            }
            _ => None,
        })
    }

    pub fn layout_depends_on_other_modules(&self, handle: TypeHandle) -> Outcome<bool> {
        self.depends(handle, 0)
    }

    fn depends(&self, handle: TypeHandle, depth: u32) -> Outcome<bool> {
        if depth > MAX_DEPTH {
            return Ok(true);
        }
        let context = Context {
            module: handle.module,
            arguments: &[],
        };
        if !self.is_value_type(handle)?
            && let Some((base, _)) = self.real_base(&context, handle)?
            && (base.module != handle.module || self.depends(base, depth + 1)?)
        {
            return Ok(true);
        }
        let metadata = &self.modules[handle.module].metadata;
        for field in metadata.field_range(handle.row)? {
            if metadata.cell(TableId::Field, field, 0)? & 0x10 != 0 {
                continue;
            }
            let Signature::Field(sig) =
                Signature::parse(metadata.blob(metadata.cell(TableId::Field, field, 2)?)?)?
            else {
                continue;
            };
            let Some(target) = self.value_type_definition(&context, &sig)? else {
                continue;
            };
            if self.is_enum(target)? || target == handle {
                continue;
            }
            if target.module != handle.module || self.depends(target, depth + 1)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn value_type_definition(
        &self,
        context: &Context<'_>,
        sig: &TypeSig,
    ) -> Outcome<Option<TypeHandle>> {
        Ok(match sig {
            TypeSig::Modified { inner, .. } => self.value_type_definition(context, inner)?,
            TypeSig::ValueType(token) => Some(self.resolve_type_token(context.module, *token)?),
            TypeSig::GenericInst {
                value_type: true,
                definition,
                ..
            } => Some(self.resolve_type_token(context.module, *definition)?),
            _ => None,
        })
    }

    fn class_detail(
        &self,
        handle: TypeHandle,
        arguments: &[(usize, TypeSig)],
        depth: u32,
    ) -> Outcome<(u32, Vec<FieldPlacement>)> {
        if depth > MAX_DEPTH {
            return Err(ResolveError::Malformed(
                "class hierarchy is recursive".into(),
            ));
        }
        let module = &self.modules[handle.module];
        let context = Context {
            module: handle.module,
            arguments,
        };

        let mut start = 0u32;
        let mut parent_has_references = false;
        let mut parent_sequential = true;
        if let Some((base, base_arguments)) = self.real_base(&context, handle)? {
            let (base_size, base_placements) =
                self.class_detail(base, &base_arguments, depth + 1)?;
            start = base_size;
            parent_has_references = base_placements.iter().any(|p| p.layout.has_references);
            parent_sequential = self.layout_kind(base)? != LayoutKind::Auto;
            if module.ready_to_run
                && (base.module != handle.module || self.depends(base, depth + 1)?)
            {
                start = align(start, POINTER_SIZE);
            }
        }

        let fields = self.instance_fields(&context, handle, depth)?;
        let has_references =
            parent_has_references || fields.iter().any(|(_, slot)| slot.layout().has_references);
        let packing = match module.class_layout(handle.row)? {
            Some((packing, _)) if packing != 0 => packing,
            _ => u32::MAX,
        };
        let kind = match self.layout_kind(handle)? {
            LayoutKind::Sequential if has_references || !parent_sequential => LayoutKind::Auto,
            kind => kind,
        };

        match kind {
            LayoutKind::Explicit => {
                let mut size = start;
                let mut placements = Vec::with_capacity(fields.len());
                for (field, slot) in &fields {
                    let layout = slot.layout();
                    let offset = start
                        + module.field_offset(*field).ok_or_else(|| {
                            ResolveError::Malformed("explicit field has no offset".into())
                        })?;
                    size = size.max(offset + layout.size);
                    placements.push(FieldPlacement {
                        field: *field,
                        offset,
                        layout,
                    });
                }
                Ok((size, placements))
            }
            LayoutKind::Sequential => {
                let (placements, size, _) = sequential_layout(start, &fields, packing);
                Ok((size, placements))
            }
            LayoutKind::Auto => {
                let result = auto_layout(start, &fields);
                Ok((result.end, result.placements))
            }
        }
    }

    fn classify(&self, context: &Context<'_>, sig: &TypeSig, depth: u32) -> Outcome<Slot> {
        use TypeSig as T;
        Ok(match sig {
            T::Boolean | T::I1 | T::U1 => Slot::Primitive(0),
            T::Char | T::I2 | T::U2 => Slot::Primitive(1),
            T::I4 | T::U4 | T::R4 => Slot::Primitive(2),
            T::I8 | T::U8 | T::R8 => Slot::Primitive(3),
            T::IntPtr | T::UIntPtr | T::Pointer(_) | T::FunctionPointer(_) | T::ByRef(_) => {
                Slot::Primitive(LOG2_POINTER)
            }
            T::TypedReference => Slot::Value(Layout {
                size: 16,
                alignment: POINTER_SIZE,
                has_references: false,
            }),
            T::String | T::Object | T::Class(_) | T::SzArray(_) | T::Array(_) => Slot::Reference,
            T::GenericInst {
                value_type: false, ..
            } => Slot::Reference,
            T::Modified { inner, .. } | T::Pinned(inner) => self.classify(context, inner, depth)?,
            T::ValueType(token) => {
                let handle = self.resolve_type_token(context.module, *token)?;
                self.classify_value(handle, &[], depth)?
            }
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
                self.classify_value(handle, &substituted, depth)?
            }
            T::TypeVar(index) => {
                let (module, argument) = context
                    .arguments
                    .get(*index as usize)
                    .ok_or(ResolveError::Unsupported("open generic layout"))?;
                self.classify(
                    &Context {
                        module: *module,
                        arguments: &[],
                    },
                    argument,
                    depth + 1,
                )?
            }
            T::MethodVar(_) => return Err(ResolveError::Unsupported("open generic layout")),
            T::Void | T::Sentinel => {
                return Err(ResolveError::Malformed("field has no storage type".into()));
            }
        })
    }

    fn classify_value(
        &self,
        handle: TypeHandle,
        arguments: &[(usize, TypeSig)],
        depth: u32,
    ) -> Outcome<Slot> {
        let layout = self.layout_of(handle, arguments, depth)?;
        if self.is_enum(handle)? {
            return Ok(Slot::Primitive(layout.size.trailing_zeros() as usize));
        }
        Ok(Slot::Value(layout))
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
            "System.Int128" | "System.UInt128" => Some(Layout::primitive(16)),
            _ => None,
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
        self.rows_with_attribute(module_index, TableId::TypeDef, namespace, name)
    }

    pub fn rows_with_attribute(
        &self,
        module_index: usize,
        parent_table: TableId,
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
            if parent.table() != Some(parent_table) {
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

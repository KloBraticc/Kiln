use super::*;

pub(super) enum FieldKind {
    Copy(u32),
    Bool(u32),
    Wide,
    Ansi,
    InlineText { wide: bool, count: u32 },
    InlineArray { size: u32, count: u32 },
    Delegate,
    Nested(Rc<NativeLayout>, bool, Gen),
}

pub(super) struct NativeField {
    managed: u32,
    native: u32,
    kind: FieldKind,
}

pub(super) struct NativeLayout {
    pub(super) size: u32,
    pub(super) align: u32,
    pub(super) blittable: bool,
    fields: Vec<NativeField>,
}

fn compressed(blob: &[u8], at: usize) -> Option<(u32, usize)> {
    let first = *blob.get(at)?;
    if first & 0x80 == 0 {
        Some((u32::from(first), at + 1))
    } else if first & 0xC0 == 0x80 {
        Some(((u32::from(first & 0x3F) << 8) | u32::from(*blob.get(at + 1)?), at + 2))
    } else {
        let bytes = blob.get(at..at + 4)?;
        Some((
            (u32::from(bytes[0] & 0x1F) << 24) | (u32::from(bytes[1]) << 16) | (u32::from(bytes[2]) << 8) | u32::from(bytes[3]),
            at + 4,
        ))
    }
}

impl Interpreter<'_> {
    fn field_marshal(&self, module: usize, row: u32) -> Option<&'static [u8]> {
        let metadata = &self.model.set.modules[module].metadata;
        (1..=metadata.rows(TableId::FieldMarshal))
            .find(|&m| {
                metadata
                    .coded(TableId::FieldMarshal, m, 0)
                    .ok()
                    .flatten()
                    .is_some_and(|p| p.table() == Some(TableId::Field) && p.row() == row)
            })
            .and_then(|m| metadata.cell(TableId::FieldMarshal, m, 1).ok())
            .and_then(|b| metadata.blob(b).ok())
    }

    pub(super) fn native_layout(&self, exact: &Gen) -> Run<Rc<NativeLayout>> {
        let id = self.types.id(exact);
        if let Some(found) = self.native_layouts.borrow().get(&id) {
            return Ok(found.clone());
        }
        let Gen::Named(handle, _) = exact else {
            return unsupported("native layout of a type that is not named");
        };
        let set = self.model.set;
        let metadata = &set.modules[handle.module].metadata;
        let type_flags = metadata.cell(TableId::TypeDef, handle.row, 0).unwrap_or(0);
        let unicode = matches!(type_flags & 0x30000, 0x10000 | 0x20000);
        let explicit = type_flags & 0x18 == 0x10;
        let is_value = self.types.is_value_type(exact);
        let (pack, class_size) = (1..=metadata.rows(TableId::ClassLayout))
            .find(|&l| metadata.cell(TableId::ClassLayout, l, 2).ok() == Some(handle.row))
            .map(|l| {
                (
                    metadata.cell(TableId::ClassLayout, l, 0).unwrap_or(0),
                    metadata.cell(TableId::ClassLayout, l, 1).unwrap_or(0),
                )
            })
            .unwrap_or((0, 0));
        let pack = if pack == 0 { 8 } else { pack };
        let managed_offsets: HashMap<u32, u32> = if is_value {
            self.layout_of(exact)?.fields.clone()
        } else {
            let (_, placements) = set.class_fields(*handle).map_err(|e| Fault::Unsupported(format!("{e:?}")))?;
            placements.iter().map(|p| (p.field, p.offset + 8)).collect()
        };
        let mut fields = Vec::new();
        let mut offset = 0u32;
        let mut align = 1u32;
        let mut blittable = is_value;
        if !is_value
            && let Ok(Some(base)) = map(self.types.base(exact))
            && base.handle().is_some_and(|b| self.type_name(b) != "System.Object")
        {
            let parent = self.native_layout(&base)?;
            offset = parent.size;
            align = parent.align;
            for field in &parent.fields {
                fields.push(NativeField {
                    managed: field.managed,
                    native: field.native,
                    kind: match &field.kind {
                        FieldKind::Copy(n) => FieldKind::Copy(*n),
                        FieldKind::Bool(n) => FieldKind::Bool(*n),
                        FieldKind::Wide => FieldKind::Wide,
                        FieldKind::Ansi => FieldKind::Ansi,
                        FieldKind::InlineText { wide, count } => FieldKind::InlineText { wide: *wide, count: *count },
                        FieldKind::InlineArray { size, count } => FieldKind::InlineArray { size: *size, count: *count },
                        FieldKind::Delegate => FieldKind::Delegate,
                        FieldKind::Nested(layout, value, exact) => FieldKind::Nested(layout.clone(), *value, exact.clone()),
                    },
                });
            }
        }
        let range = metadata.field_range(handle.row).map_err(|e| Fault::Unsupported(e.to_string()))?;
        for row in range {
            if metadata.cell(TableId::Field, row, 0).unwrap_or(0) & 0x10 != 0 {
                continue;
            }
            let sig = map(self.model.field_signature(handle.module, row))?;
            let marshal = self.field_marshal(handle.module, row);
            let native = marshal.and_then(|blob| blob.first().copied());
            let (kind, size, field_align) = self.native_field(handle.module, unmodified(&sig), native, marshal, unicode, &exact.context())?;
            if !matches!(kind, FieldKind::Copy(_)) && !matches!(&kind, FieldKind::Nested(layout, true, _) if layout.blittable) {
                blittable = false;
            }
            let field_align = field_align.min(pack).max(1);
            let native_offset = if explicit {
                (1..=metadata.rows(TableId::FieldLayout))
                    .find(|&l| metadata.cell(TableId::FieldLayout, l, 1).ok() == Some(row))
                    .and_then(|l| metadata.cell(TableId::FieldLayout, l, 0).ok())
                    .unwrap_or(0)
            } else {
                offset.next_multiple_of(field_align)
            };
            offset = offset.max(native_offset + size);
            align = align.max(field_align);
            let managed = *managed_offsets
                .get(&row)
                .ok_or_else(|| Fault::Unsupported("field has no managed offset".into()))?;
            fields.push(NativeField {
                managed,
                native: native_offset,
                kind,
            });
        }
        let size = offset.next_multiple_of(align).max(class_size).max(1);
        let layout = Rc::new(NativeLayout {
            size,
            align,
            blittable,
            fields,
        });
        self.native_layouts.borrow_mut().insert(id, layout.clone());
        Ok(layout)
    }

    fn native_field(
        &self,
        module: usize,
        sig: &TypeSig,
        native: Option<u8>,
        marshal: Option<&[u8]>,
        unicode: bool,
        ctx: &Ctx,
    ) -> Run<(FieldKind, u32, u32)> {
        let count = || marshal.and_then(|blob| compressed(blob, 1)).map_or(1, |(value, _)| value);
        let primitive = |size: u32| Ok((FieldKind::Copy(size), size, size));
        match (sig, native) {
            (TypeSig::Boolean, Some(0x03 | 0x04)) => Ok((FieldKind::Bool(1), 1, 1)),
            (TypeSig::Boolean, _) => Ok((FieldKind::Bool(4), 4, 4)),
            (TypeSig::Char, _) if unicode || native == Some(0x07) => primitive(2),
            (TypeSig::Char, _) => primitive(1),
            (TypeSig::I1 | TypeSig::U1, _) => primitive(1),
            (TypeSig::I2 | TypeSig::U2, _) => primitive(2),
            (TypeSig::I4 | TypeSig::U4 | TypeSig::R4, _) => primitive(4),
            (TypeSig::I8 | TypeSig::U8 | TypeSig::R8 | TypeSig::IntPtr | TypeSig::UIntPtr, _) => primitive(8),
            (TypeSig::Pointer(_) | TypeSig::FunctionPointer(_), _) => primitive(8),
            (TypeSig::String, Some(0x17)) => {
                let count = count();
                let unit = if unicode { 2 } else { 1 };
                Ok((FieldKind::InlineText { wide: unicode, count }, unit * count, unit))
            }
            (TypeSig::String, Some(0x15)) => Ok((FieldKind::Wide, 8, 8)),
            (TypeSig::String, Some(0x14)) => Ok((FieldKind::Ansi, 8, 8)),
            (TypeSig::String, _) if unicode => Ok((FieldKind::Wide, 8, 8)),
            (TypeSig::String, _) => Ok((FieldKind::Ansi, 8, 8)),
            (TypeSig::SzArray(element), Some(0x1E)) => {
                let (_, size, element_align) = self.native_field(module, unmodified(element), None, None, unicode, ctx)?;
                let count = count();
                Ok((FieldKind::InlineArray { size, count }, size * count, element_align))
            }
            _ => {
                let exact = map(self.types.resolve(module, sig, ctx))?;
                if let Some(handle) = exact.handle()
                    && self.is_delegate(handle)
                {
                    return Ok((FieldKind::Delegate, 8, 8));
                }
                match self.gen_ty(&exact)? {
                    Ty::Prim(VarType::Ref) => {
                        let layout = self.native_layout(&exact)?;
                        let (size, align) = (layout.size, layout.align);
                        Ok((FieldKind::Nested(layout, false, exact), size, align))
                    }
                    Ty::Prim(var) => primitive(ElementKind::from_var(var).map_or(8, ElementKind::size)),
                    Ty::Struct(_) => {
                        let layout = self.native_layout(&exact)?;
                        let (size, align) = (layout.size, layout.align);
                        Ok((FieldKind::Nested(layout, true, exact), size, align))
                    }
                }
            }
        }
    }

    pub(super) fn marshal_out(&self, layout: &NativeLayout, managed: u64, native: u64, keep: &mut Vec<Vec<u64>>) -> Run<()> {
        unsafe { std::ptr::write_bytes(native as *mut u8, 0, layout.size as usize) };
        for field in &layout.fields {
            let source = managed + u64::from(field.managed);
            let target = native + u64::from(field.native);
            match &field.kind {
                FieldKind::Copy(size) => unsafe { std::ptr::copy_nonoverlapping(source as *const u8, target as *mut u8, *size as usize) },
                FieldKind::Bool(size) => {
                    let value = u32::from(unsafe { *(source as *const u8) } != 0);
                    unsafe { std::ptr::copy_nonoverlapping((&raw const value).cast::<u8>(), target as *mut u8, *size as usize) };
                }
                FieldKind::Wide | FieldKind::Ansi => {
                    let string = unsafe { *(source as *const u64) };
                    let pointer = if string == 0 {
                        0
                    } else {
                        let units = string_units(string);
                        let bytes: Vec<u8> = if matches!(field.kind, FieldKind::Wide) {
                            units.iter().chain(&[0]).flat_map(|u| u.to_le_bytes()).collect()
                        } else {
                            pinvoke::ansi(units)
                        };
                        keep.push(words(&bytes));
                        keep[keep.len() - 1].as_ptr() as u64
                    };
                    unsafe { *(target as *mut u64) = pointer };
                }
                FieldKind::InlineText { wide, count } => {
                    let string = unsafe { *(source as *const u64) };
                    if string != 0 && *count > 0 {
                        let units = string_units(string);
                        if *wide {
                            let length = units.len().min(*count as usize - 1);
                            unsafe { std::ptr::copy_nonoverlapping(units.as_ptr(), target as *mut u16, length) };
                        } else {
                            let bytes = pinvoke::ansi(units);
                            let length = (bytes.len().saturating_sub(1)).min(*count as usize - 1);
                            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, length) };
                        }
                    }
                }
                FieldKind::InlineArray { size, count } => {
                    let array = unsafe { *(source as *const u64) };
                    if array != 0 {
                        let length = (unsafe { *((array + 8) as *const u32) }).min(*count);
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                (array + u64::from(ARRAY_DATA)) as *const u8,
                                target as *mut u8,
                                (length * size) as usize,
                            )
                        };
                    }
                }
                FieldKind::Delegate => {
                    let object = unsafe { *(source as *const u64) };
                    let stub = if object == 0 { 0 } else { self.delegate_stub(object)? };
                    unsafe { *(target as *mut u64) = stub };
                }
                FieldKind::Nested(inner, true, _) => self.marshal_out(inner, source, target, keep)?,
                FieldKind::Nested(inner, false, _) => {
                    let object = unsafe { *(source as *const u64) };
                    if object != 0 {
                        self.marshal_out(inner, object, target, keep)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn marshal_in(&self, layout: &NativeLayout, native: u64, managed: u64) -> Run<()> {
        for field in &layout.fields {
            let source = native + u64::from(field.native);
            let target = managed + u64::from(field.managed);
            match &field.kind {
                FieldKind::Copy(size) => unsafe { std::ptr::copy_nonoverlapping(source as *const u8, target as *mut u8, *size as usize) },
                FieldKind::Bool(size) => {
                    let mut value = 0u32;
                    unsafe { std::ptr::copy_nonoverlapping(source as *const u8, (&raw mut value).cast(), *size as usize) };
                    unsafe { *(target as *mut u8) = u8::from(value != 0) };
                }
                FieldKind::Wide | FieldKind::Ansi => {
                    let pointer = unsafe { *(source as *const u64) };
                    let text = if pointer == 0 {
                        None
                    } else if matches!(field.kind, FieldKind::Wide) {
                        Some(reflection::wide_units(pointer))
                    } else {
                        Some(reflection::ansi_units(pointer))
                    };
                    let string = text.map_or(0, |units| runtime::new_string(&units));
                    unsafe { *(target as *mut u64) = string };
                }
                FieldKind::InlineText { wide, count } => {
                    let units: Vec<u16> = if *wide {
                        let all = unsafe { std::slice::from_raw_parts(source as *const u16, *count as usize) };
                        all.iter().take_while(|u| **u != 0).copied().collect()
                    } else {
                        let all = unsafe { std::slice::from_raw_parts(source as *const u8, *count as usize) };
                        all.iter().take_while(|b| **b != 0).map(|b| u16::from(*b)).collect()
                    };
                    unsafe { *(target as *mut u64) = runtime::new_string(&units) };
                }
                FieldKind::InlineArray { size, count } => {
                    let array = unsafe { *(target as *const u64) };
                    if array != 0 {
                        let length = (unsafe { *((array + 8) as *const u32) }).min(*count);
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                source as *const u8,
                                (array + u64::from(ARRAY_DATA)) as *mut u8,
                                (length * size) as usize,
                            )
                        };
                    }
                }
                FieldKind::Delegate => {}
                FieldKind::Nested(inner, true, _) => self.marshal_in(inner, source, target)?,
                FieldKind::Nested(inner, false, exact) => {
                    let mut object = unsafe { *(target as *const u64) };
                    if object == 0 {
                        object = self.construct_default(exact)?;
                        unsafe { *(target as *mut u64) = object };
                    }
                    self.marshal_in(inner, source, object)?;
                }
            }
        }
        Ok(())
    }
}

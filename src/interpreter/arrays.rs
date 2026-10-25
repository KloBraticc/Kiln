use super::*;

pub(super) enum ArrayAccess {
    Construct,
    Get,
    Set,
    Address,
}

pub(super) struct ArrayMethod {
    element: Gen,
    rank: u32,
    access: ArrayAccess,
    pub(super) parameters: usize,
}

const BOUNDS: u64 = 16;

impl Interpreter<'_> {
    pub(super) fn array_method(&self, module: usize, token: Token, ctx: &Ctx) -> Run<Option<ArrayMethod>> {
        if token.table() != Some(TableId::MemberRef) {
            return Ok(None);
        }
        let metadata = &self.model.set.modules[module].metadata;
        let Ok(Some(parent)) = metadata.coded(TableId::MemberRef, token.row(), 0) else {
            return Ok(None);
        };
        if parent.table() != Some(TableId::TypeSpec) {
            return Ok(None);
        }
        let Gen::Array(element, rank) = map(self.types.resolve_token(module, Token::new(TableId::TypeSpec, parent.row()), ctx))? else {
            return Ok(None);
        };
        let name = metadata.string_cell(TableId::MemberRef, token.row(), 1).unwrap_or("");
        let access = match name {
            ".ctor" => ArrayAccess::Construct,
            "Get" => ArrayAccess::Get,
            "Set" => ArrayAccess::Set,
            "Address" => ArrayAccess::Address,
            _ => return unsupported(format!("array method {name}")),
        };
        let blob = metadata
            .cell(TableId::MemberRef, token.row(), 2)
            .and_then(|b| metadata.blob(b))
            .map_err(|e| Fault::Unsupported(e.to_string()))?;
        let Ok(Signature::Method(sig)) = Signature::parse(blob) else {
            return unsupported("array method signature");
        };
        Ok(Some(ArrayMethod {
            element: (*element).clone(),
            rank,
            access,
            parameters: sig.parameters.len() + usize::from(sig.has_this),
        }))
    }

    pub(super) fn multi_array_record(&self, element: &Gen, rank: u32) -> Run<u64> {
        let exact = Gen::Array(Rc::new(element.clone()), rank);
        let id = self.types.id(&exact);
        let (ty, size) = self.element_size(element)?;
        let (flags, references) = match &ty {
            Ty::Prim(VarType::Ref) => (runtime::FLAG_ARRAY | runtime::FLAG_REFERENCE_ELEMENTS, Vec::new()),
            Ty::Struct(layout) if !layout.references.is_empty() => {
                (runtime::FLAG_ARRAY | runtime::FLAG_STRUCT_ELEMENTS, layout.references.clone())
            }
            _ => (runtime::FLAG_ARRAY, Vec::new()),
        };
        let vtable = record_vtable(self.record(RuntimeType::Array(ElementKind::Reference))?);
        Ok(runtime::state().custom_record(
            RuntimeType::Exact(id),
            BOUNDS + 8 * u64::from(rank),
            u64::from(size),
            flags,
            0,
            references,
            EXACT_TAG | u64::from(id),
            vtable,
        ))
    }

    pub(super) fn array_rank(object: u64) -> u32 {
        let record = unsafe { *(object as *const u64) } & !1;
        let base = unsafe { *(record as *const u64) } & 0xFFFF_FFFF;
        ((base - BOUNDS) / 8) as u32
    }

    pub(super) fn array_data(object: u64) -> u64 {
        let record = unsafe { *(object as *const u64) } & !1;
        object + (unsafe { *(record as *const u64) } & 0xFFFF_FFFF)
    }

    fn multi_element(&self, array: u64, rank: u32, indices: &[V], size: u32) -> Run<u64> {
        let array = self.non_null(V::O(array))?;
        let mut flat = 0i64;
        for (dimension, index) in indices.iter().enumerate().take(rank as usize) {
            let length = i64::from(unsafe { *((array + BOUNDS + 4 * dimension as u64) as *const i32) });
            let lower = i64::from(unsafe { *((array + BOUNDS + 4 * (u64::from(rank) + dimension as u64)) as *const i32) });
            let offset = i64::from(index.int() as i32) - lower;
            if offset < 0 || offset >= length {
                return self.throw("IndexOutOfRangeException");
            }
            flat = flat * length + offset;
        }
        Ok(Self::array_data(array) + flat as u64 * u64::from(size))
    }

    pub(super) fn array_call(&self, method: &ArrayMethod, arguments: &[V]) -> Run<Option<V>> {
        let rank = method.rank;
        let (ty, size) = self.element_size(&method.element)?;
        Ok(match method.access {
            ArrayAccess::Construct => {
                let pairs = arguments.len() == 2 * rank as usize;
                let mut lengths = Vec::with_capacity(rank as usize);
                let mut lowers = Vec::with_capacity(rank as usize);
                for dimension in 0..rank as usize {
                    let (lower, length) = if pairs {
                        (arguments[2 * dimension].int() as i32, arguments[2 * dimension + 1].int() as i32)
                    } else {
                        (0, arguments[dimension].int() as i32)
                    };
                    if length < 0 {
                        return self.throw("OverflowException");
                    }
                    lengths.push(length);
                    lowers.push(lower);
                }
                let count = lengths.iter().try_fold(1u64, |total, &length| total.checked_mul(length as u64));
                let Some(count) = count.filter(|&c| c <= u64::from(u32::MAX)) else {
                    return self.throw("OutOfMemoryException");
                };
                let record = self.multi_array_record(&method.element, rank)?;
                let array = runtime::allocate_managed(record, count);
                if array == 0 {
                    return self.throw("OutOfMemoryException");
                }
                for (dimension, (length, lower)) in lengths.iter().zip(&lowers).enumerate() {
                    unsafe {
                        *((array + BOUNDS + 4 * dimension as u64) as *mut i32) = *length;
                        *((array + BOUNDS + 4 * (u64::from(rank) + dimension as u64)) as *mut i32) = *lower;
                    }
                }
                Some(V::O(array))
            }
            ArrayAccess::Get => {
                let address = self.multi_element(arguments[0].address(), rank, &arguments[1..], size)?;
                Some(self.read_value(address, &ty, &arguments[..1])?)
            }
            ArrayAccess::Set => {
                let value = arguments[arguments.len() - 1];
                let address = self.multi_element(arguments[0].address(), rank, &arguments[1..arguments.len() - 1], size)?;
                self.write_value(address, &ty, value)?;
                None
            }
            ArrayAccess::Address => Some(V::P(self.multi_element(arguments[0].address(), rank, &arguments[1..], size)?)),
        })
    }
}

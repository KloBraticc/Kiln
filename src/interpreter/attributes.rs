use super::*;

#[derive(Clone)]
enum Kind {
    Prim(&'static str),
    Text,
    Type,
    Tagged,
    Enum(Gen),
    Array(Box<Kind>),
}

enum Decoded {
    Prim(&'static str, u64),
    Text(Option<String>),
    Type(Option<Gen>),
    Enum(Gen, u64),
    Array(Kind, Option<Vec<Decoded>>),
}

struct Cursor {
    at: u64,
    end: u64,
}

impl Cursor {
    fn take(&mut self, count: u64) -> Run<u64> {
        if self.at + count > self.end {
            return unsupported("custom attribute blob is truncated");
        }
        let at = self.at;
        self.at += count;
        Ok(at)
    }

    fn bytes(&mut self, count: u64) -> Run<u64> {
        let at = self.take(count)?;
        let mut value = 0u64;
        unsafe { std::ptr::copy_nonoverlapping(at as *const u8, (&raw mut value).cast(), count as usize) };
        Ok(value)
    }

    fn compressed(&mut self) -> Run<u32> {
        let first = self.bytes(1)? as u32;
        Ok(if first & 0x80 == 0 {
            first
        } else if first & 0xC0 == 0x80 {
            ((first & 0x3F) << 8) | self.bytes(1)? as u32
        } else {
            let rest = self.bytes(3)? as u32;
            ((first & 0x1F) << 24) | ((rest & 0xFF) << 16) | (rest & 0xFF00) | ((rest >> 16) & 0xFF)
        })
    }

    fn text(&mut self) -> Run<Option<String>> {
        if self.at < self.end && unsafe { *(self.at as *const u8) } == 0xFF {
            self.at += 1;
            return Ok(None);
        }
        let length = u64::from(self.compressed()?);
        let at = self.take(length)?;
        let bytes = unsafe { std::slice::from_raw_parts(at as *const u8, length as usize) };
        Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
    }
}

fn prim_size(name: &str) -> u64 {
    match name {
        "Boolean" | "SByte" | "Byte" => 1,
        "Char" | "Int16" | "UInt16" => 2,
        "Int32" | "UInt32" | "Single" => 4,
        _ => 8,
    }
}

fn prim_of_tag(tag: u8) -> Option<&'static str> {
    Some(match tag {
        0x02 => "Boolean",
        0x03 => "Char",
        0x04 => "SByte",
        0x05 => "Byte",
        0x06 => "Int16",
        0x07 => "UInt16",
        0x08 => "Int32",
        0x09 => "UInt32",
        0x0A => "Int64",
        0x0B => "UInt64",
        0x0C => "Single",
        0x0D => "Double",
        _ => return None,
    })
}

impl Interpreter<'_> {
    fn attribute_kind(&self, module: usize, sig: &TypeSig, ctx: &Ctx) -> Run<Kind> {
        Ok(match sig {
            TypeSig::Modified { inner, .. } => self.attribute_kind(module, inner, ctx)?,
            TypeSig::Boolean => Kind::Prim("Boolean"),
            TypeSig::Char => Kind::Prim("Char"),
            TypeSig::I1 => Kind::Prim("SByte"),
            TypeSig::U1 => Kind::Prim("Byte"),
            TypeSig::I2 => Kind::Prim("Int16"),
            TypeSig::U2 => Kind::Prim("UInt16"),
            TypeSig::I4 => Kind::Prim("Int32"),
            TypeSig::U4 => Kind::Prim("UInt32"),
            TypeSig::I8 => Kind::Prim("Int64"),
            TypeSig::U8 => Kind::Prim("UInt64"),
            TypeSig::R4 => Kind::Prim("Single"),
            TypeSig::R8 => Kind::Prim("Double"),
            TypeSig::String => Kind::Text,
            TypeSig::Object => Kind::Tagged,
            TypeSig::SzArray(element) => Kind::Array(Box::new(self.attribute_kind(module, element, ctx)?)),
            _ => {
                let exact = map(self.types.resolve(module, sig, ctx))?;
                match exact.handle().map(|h| self.type_name(h)) {
                    Some(name) if name == "System.Type" => Kind::Type,
                    Some(name) if name == "System.Object" => Kind::Tagged,
                    _ if exact.handle().is_some_and(|h| self.model.set.is_enum(h).unwrap_or(false)) => Kind::Enum(exact),
                    _ => return unsupported("custom attribute argument of an unsupported type"),
                }
            }
        })
    }

    fn tagged_kind(&self, cursor: &mut Cursor) -> Run<Kind> {
        let tag = cursor.bytes(1)? as u8;
        Ok(match tag {
            0x0E => Kind::Text,
            0x50 => Kind::Type,
            0x51 => Kind::Tagged,
            0x1D => Kind::Array(Box::new(self.tagged_kind(cursor)?)),
            0x55 => {
                let name = cursor.text()?.unwrap_or_default();
                Kind::Enum(self.type_by_name(&name)?)
            }
            other => Kind::Prim(prim_of_tag(other).ok_or_else(|| Fault::Unsupported(format!("custom attribute type tag 0x{other:02x}")))?),
        })
    }

    fn decode(&self, kind: &Kind, cursor: &mut Cursor) -> Run<Decoded> {
        Ok(match kind {
            Kind::Prim(name) => Decoded::Prim(name, cursor.bytes(prim_size(name))?),
            Kind::Text => Decoded::Text(cursor.text()?),
            Kind::Type => Decoded::Type(match cursor.text()? {
                Some(name) => Some(self.type_by_name(&name)?),
                None => None,
            }),
            Kind::Enum(exact) => {
                let size = match self.gen_ty(exact)? {
                    Ty::Prim(var) => ElementKind::from_var(var).map_or(4, ElementKind::size),
                    _ => 4,
                };
                Decoded::Enum(exact.clone(), cursor.bytes(u64::from(size))?)
            }
            Kind::Tagged => {
                let inner = self.tagged_kind(cursor)?;
                self.decode(&inner, cursor)?
            }
            Kind::Array(element) => {
                let count = cursor.bytes(4)? as u32;
                if count == u32::MAX {
                    Decoded::Array((**element).clone(), None)
                } else {
                    let mut items = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        items.push(self.decode(element, cursor)?);
                    }
                    Decoded::Array((**element).clone(), Some(items))
                }
            }
        })
    }

    fn kind_gen(&self, kind: &Kind) -> Run<Gen> {
        Ok(match kind {
            Kind::Prim(name) => self.core_named(name)?,
            Kind::Text => self.core_named("String")?,
            Kind::Type => self.core_named("Type")?,
            Kind::Tagged => self.core_named("Object")?,
            Kind::Enum(exact) => exact.clone(),
            Kind::Array(element) => Gen::SzArray(Rc::new(self.kind_gen(element)?)),
        })
    }

    fn core_named(&self, name: &'static str) -> Run<Gen> {
        map(self.types.core(name))
    }

    fn materialize(&self, decoded: &Decoded, boxed: bool) -> Run<V> {
        Ok(match decoded {
            Decoded::Prim(name, bits) => {
                let exact = self.core_named(name)?;
                let Ty::Prim(var) = self.gen_ty(&exact)? else {
                    return unsupported("primitive without a primitive layout");
                };
                let value = load_raw(var, *bits);
                if boxed { V::O(self.box_value(&exact, var, value)?) } else { value }
            }
            Decoded::Enum(exact, bits) => {
                let Ty::Prim(var) = self.gen_ty(exact)? else {
                    return unsupported("enum without a primitive layout");
                };
                let value = load_raw(var, *bits);
                if boxed { V::O(self.box_value(exact, var, value)?) } else { value }
            }
            Decoded::Text(text) => V::O(match text {
                Some(text) => runtime::new_string(&text.encode_utf16().collect::<Vec<_>>()),
                None => 0,
            }),
            Decoded::Type(exact) => V::O(match exact {
                Some(exact) => self.type_object(exact)?,
                None => 0,
            }),
            Decoded::Array(kind, items) => match items {
                None => V::O(0),
                Some(items) => {
                    let element = self.kind_gen(kind)?;
                    let (ty, size) = self.element_size(&element)?;
                    let record = self.array_record(&element)?;
                    let array = runtime::allocate_managed(record, items.len() as u64);
                    if array == 0 {
                        return self.throw("OutOfMemoryException");
                    }
                    let _pin = Pin::new(&[V::O(array)]);
                    let element_boxed = matches!(kind, Kind::Tagged);
                    for (index, item) in items.iter().enumerate() {
                        let value = self.materialize(item, element_boxed)?;
                        let address = array + u64::from(ARRAY_DATA) + index as u64 * u64::from(size);
                        self.write_value(address, &ty, value)?;
                    }
                    V::O(array)
                }
            },
        })
    }

    pub(super) fn type_by_name(&self, full: &str) -> Run<Gen> {
        let set = self.model.set;
        let mut depth = 0;
        let mut split = None;
        for (index, c) in full.char_indices() {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                ',' if depth == 0 => {
                    split = Some(index);
                    break;
                }
                _ => {}
            }
        }
        let (type_part, assembly) = match split {
            Some(index) => (full[..index].trim(), Some(full[index + 1..].split(',').next().unwrap_or("").trim())),
            None => (full.trim(), None),
        };
        let mut parts = type_part.split('+');
        let outer = parts.next().unwrap_or("");
        let (namespace, name) = outer.rsplit_once('.').unwrap_or(("", outer));
        let candidates: Vec<usize> = match assembly.and_then(|a| set.find_assembly(a)) {
            Some(module) => vec![module],
            None => {
                let mut all: Vec<usize> = (0..set.modules.len()).collect();
                all.sort_by_key(|m| *m != self.corelib_type("System", "Object").map(|h| h.module).unwrap_or(0));
                all
            }
        };
        for module in candidates {
            if let Some(mut handle) = set.find_named(module, namespace, name) {
                for inner in parts.clone() {
                    match set.find_nested(handle, inner) {
                        Some(found) => handle = found,
                        None => return unsupported(format!("nested type {inner} of {type_part} is missing")),
                    }
                }
                return Ok(Gen::Named(handle, Rc::from(Vec::new())));
            }
        }
        unsupported(format!("type {full} named by a custom attribute is missing"))
    }

    pub(super) fn create_attribute(&self, a: &[V]) -> Run<Option<V>> {
        let module = self.qcall_module(a[0])?;
        let slot = |value: V| unsafe { *(*(Self::slot_data(value) as *const u64) as *const u64) };
        let attribute_type = slot(a[1]);
        let constructor = slot(a[2]);
        let blob_slot = a[3].address();
        let mut cursor = Cursor {
            at: unsafe { *(blob_slot as *const u64) },
            end: a[4].bits(),
        };
        let exact = self.type_gen(V::O(attribute_type))?;
        let handle = self.method_info_handle_of(constructor)?;
        let (key, ctx) = self.method_key_of(handle)?;
        if cursor.at + 2 <= cursor.end {
            cursor.bytes(2)?;
        }
        let sig = map(self.model.method_sig(&key))?;
        let object = runtime::allocate_managed(self.record_of(&exact)?, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        let mut arguments = vec![V::O(object)];
        let _pin = Pin::new(&[V::O(object)]);
        for parameter in &sig.parameters {
            let kind = self.attribute_kind(key.module, parameter, &ctx)?;
            let decoded = self.decode(&kind, &mut cursor)?;
            let value = self.materialize(&decoded, matches!(kind, Kind::Tagged))?;
            arguments.push(value);
        }
        let _keep = Pin::new(&arguments);
        self.invoke_ctx(&key, &ctx, &arguments)?;
        let named = if cursor.at + 2 <= cursor.end { cursor.bytes(2)? as i32 } else { 0 };
        unsafe {
            *(blob_slot as *mut u64) = cursor.at;
            *(a[5].address() as *mut i32) = named;
        }
        self.store_handle(a[6], object);
        let _ = module;
        Ok(None)
    }

    pub(super) fn attribute_named(&self, a: &[V]) -> Run<Option<V>> {
        let blob_slot = a[1].address();
        let mut cursor = Cursor {
            at: unsafe { *(blob_slot as *const u64) },
            end: a[2].bits(),
        };
        let marker = cursor.bytes(1)? as u8;
        let kind = self.tagged_kind(&mut cursor)?;
        let name = cursor.text()?.unwrap_or_default();
        let decoded = self.decode(&kind, &mut cursor)?;
        let value = self.materialize(&decoded, true)?;
        let _pin = Pin::new(&[value]);
        let value = match (&kind, value) {
            (Kind::Text | Kind::Type | Kind::Array(_), v) => v,
            (_, v) => v,
        };
        let declared = self.kind_gen(&kind)?;
        self.store_text(a[3], &name);
        unsafe {
            *(a[4].address() as *mut u8) = u8::from(marker == 0x54);
            *(blob_slot as *mut u64) = cursor.at;
        }
        let type_object = self.type_object(&declared)?;
        self.store_handle(a[5], type_object);
        self.store_handle(a[6], value.address());
        Ok(None)
    }

    fn slot_data(value: V) -> u64 {
        match value {
            V::S(boxed) | V::O(boxed) if boxed != 0 => boxed + 8,
            other => other.address(),
        }
    }
}

use super::*;

#[derive(Debug)]
pub(super) enum Accessor {
    Constructor(Gen, MethodKey, Ctx),
    Method(MethodKey, Ctx),
    StaticMethod(MethodKey, Ctx),
    Field(usize, u32, Ctx),
    StaticField(usize, u32, Ctx),
}

impl Interpreter<'_> {
    fn accessor_attribute(&self, key: &MethodKey) -> Option<(i32, Option<String>)> {
        let metadata = &self.model.set.modules[key.module].metadata;
        (1..=metadata.rows(TableId::CustomAttribute)).find_map(|row| {
            let owner = metadata.coded(TableId::CustomAttribute, row, 0).ok().flatten()?;
            if owner.table() != Some(TableId::MethodDef) || owner.row() != key.row {
                return None;
            }
            let constructor = metadata.coded(TableId::CustomAttribute, row, 1).ok().flatten()?;
            let name = match constructor.table()? {
                TableId::MemberRef => {
                    let parent = metadata.coded(TableId::MemberRef, constructor.row(), 0).ok().flatten()?;
                    (parent.table() == Some(TableId::TypeRef)).then_some(())?;
                    metadata.string_cell(TableId::TypeRef, parent.row(), 1).ok()?
                }
                TableId::MethodDef => {
                    let owner = self.model.set.modules[key.module].method_owner(constructor.row()).ok().flatten()?;
                    metadata.string_cell(TableId::TypeDef, owner, 1).ok()?
                }
                _ => return None,
            };
            if name != "UnsafeAccessorAttribute" {
                return None;
            }
            let blob = metadata.blob(metadata.cell(TableId::CustomAttribute, row, 2).ok()?).ok()?;
            let kind = i32::from_le_bytes(blob.get(2..6)?.try_into().ok()?);
            let mut target = None;
            let count = u16::from_le_bytes(blob.get(6..8)?.try_into().ok()?);
            let mut at = 8;
            for _ in 0..count {
                at += 2;
                let length = *blob.get(at)? as usize;
                let field = std::str::from_utf8(blob.get(at + 1..at + 1 + length)?).ok()?;
                at += 1 + length;
                let value_length = *blob.get(at)? as usize;
                let value = std::str::from_utf8(blob.get(at + 1..at + 1 + value_length)?).ok()?;
                at += 1 + value_length;
                if field == "Name" {
                    target = Some(value.to_owned());
                }
            }
            Some((kind, target))
        })
    }

    pub(super) fn unsafe_accessor(&self, key: &MethodKey, ctx: &Ctx) -> Run<Option<Rc<Accessor>>> {
        let Some((kind, name)) = self.accessor_attribute(key) else {
            return Ok(None);
        };
        let metadata = &self.model.set.modules[key.module].metadata;
        let own_name = metadata.string_cell(TableId::MethodDef, key.row, 3).unwrap_or("").to_owned();
        let name = name.unwrap_or(own_name);
        let sig = map(self.model.method_sig(key))?;
        let resolve = |sig: &TypeSig| -> Run<Gen> {
            match unmodified(sig) {
                TypeSig::Void => map(self.types.core("Void")),
                TypeSig::ByRef(inner) => map(self.types.resolve(key.module, unmodified(inner), ctx)),
                other => map(self.types.resolve(key.module, other, ctx)),
            }
        };
        let parameters = sig.parameters.iter().map(&resolve).collect::<Run<Vec<_>>>()?;
        let accessor = match kind {
            0 => {
                let target = resolve(&sig.return_type)?;
                let (constructor, context) = self.accessor_method(&target, ".ctor", &parameters, false)?;
                Accessor::Constructor(target, constructor, context)
            }
            1 | 2 => {
                let target = parameters
                    .first()
                    .cloned()
                    .ok_or_else(|| Fault::Unsupported("unsafe accessor without a target".into()))?;
                let (method, context) = self.accessor_method(&target, &name, &parameters[1..], kind == 2)?;
                if kind == 1 { Accessor::Method(method, context) } else { Accessor::StaticMethod(method, context) }
            }
            3 | 4 => {
                let target = parameters
                    .first()
                    .cloned()
                    .ok_or_else(|| Fault::Unsupported("unsafe accessor without a target".into()))?;
                let Gen::Named(handle, _) = &target else {
                    return unsupported("unsafe accessor field on a type that is not named");
                };
                let field_metadata = &self.model.set.modules[handle.module].metadata;
                let row = field_metadata
                    .field_range(handle.row)
                    .ok()
                    .and_then(|mut range| range.find(|&row| field_metadata.string_cell(TableId::Field, row, 1).ok() == Some(name.as_str())))
                    .ok_or_else(|| Fault::Unsupported(format!("unsafe accessor field {name} was not found")))?;
                if kind == 3 {
                    Accessor::Field(handle.module, row, target.context())
                } else {
                    Accessor::StaticField(handle.module, row, target.context())
                }
            }
            other => return unsupported(format!("unsafe accessor kind {other}")),
        };
        Ok(Some(Rc::new(accessor)))
    }

    fn accessor_method(&self, target: &Gen, name: &str, parameters: &[Gen], is_static: bool) -> Run<(MethodKey, Ctx)> {
        let Gen::Named(handle, _) = target else {
            return unsupported("unsafe accessor on a type that is not named");
        };
        let metadata = &self.model.set.modules[handle.module].metadata;
        let context = target.context();
        let range = metadata.method_range(handle.row).map_err(|e| Fault::Unsupported(e.to_string()))?;
        for row in range {
            if metadata.string_cell(TableId::MethodDef, row, 3).ok() != Some(name) {
                continue;
            }
            let flags = metadata.cell(TableId::MethodDef, row, 2).unwrap_or(0);
            if (flags & 0x10 != 0) != is_static {
                continue;
            }
            let key = MethodKey { module: handle.module, row };
            let Ok(sig) = self.model.method_sig(&key) else {
                continue;
            };
            if sig.parameters.len() != parameters.len() {
                continue;
            }
            let matches = sig.parameters.iter().zip(parameters).all(|(candidate, wanted)| {
                let candidate = match unmodified(candidate) {
                    TypeSig::ByRef(inner) => self.types.resolve(handle.module, unmodified(inner), &context),
                    other => self.types.resolve(handle.module, other, &context),
                };
                candidate.is_ok_and(|candidate| candidate == *wanted)
            });
            if matches {
                return Ok((key, context));
            }
        }
        unsupported(format!("unsafe accessor target {name} was not found on {}", self.types.full_name(target)))
    }

    pub(super) fn run_accessor(&self, accessor: &Accessor, arguments: &[V]) -> Run<Option<V>> {
        Ok(match accessor {
            Accessor::Constructor(target, constructor, context) => {
                let value_type = self.types.is_value_type(target);
                let object = self.allocate_box(target)?;
                let _pin = Pin::new(&[V::O(object)]);
                let mut forwarded = Vec::with_capacity(arguments.len() + 1);
                forwarded.push(if value_type { V::P(object + 8) } else { V::O(object) });
                forwarded.extend_from_slice(arguments);
                self.invoke_ctx(constructor, context, &forwarded)?;
                Some(if value_type { V::S(object) } else { V::O(object) })
            }
            Accessor::Method(method, context) => self.invoke_ctx(method, context, arguments)?,
            Accessor::StaticMethod(method, context) => self.invoke_ctx(method, context, arguments.get(1..).unwrap_or(&[]))?,
            Accessor::Field(module, row, context) => {
                let target = arguments.first().copied().unwrap_or(V::O(0));
                let (address, _) = self.field_address_in(*module, Token::new(TableId::Field, *row), context, target)?;
                Some(V::P(address))
            }
            Accessor::StaticField(module, row, context) => {
                let (address, _) = self.field_address_in(*module, Token::new(TableId::Field, *row), context, V::O(0))?;
                Some(V::P(address))
            }
        })
    }
}

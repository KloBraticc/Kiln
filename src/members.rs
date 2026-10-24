use crate::loader::{AssemblySet, ResolveError, TypeHandle};
use crate::metadata::{TableId, Token};
use crate::signature::{MethodSig, Signature, TypeSig};

type Outcome<T> = std::result::Result<T, ResolveError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemberHandle {
    pub module: usize,
    pub token: Token,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Member {
    Method(MemberHandle),
    Field(MemberHandle),
    ArrayMethod,
}

impl AssemblySet {
    pub fn resolve_type_token(&self, module: usize, token: Token) -> Outcome<TypeHandle> {
        match token.table() {
            Some(TableId::TypeDef) => Ok(TypeHandle {
                module,
                row: token.row(),
            }),
            Some(TableId::TypeRef) => self.resolve_type_ref(module, token.row()),
            _ => Err(ResolveError::Malformed(format!(
                "{:#010x} is not a type definition or reference",
                token.0
            ))),
        }
    }

    pub fn resolve_member_ref(&self, module: usize, row: u32) -> Outcome<Member> {
        let metadata = &self.modules[module].metadata;
        let name = metadata.string_cell(TableId::MemberRef, row, 1)?;
        let signature =
            Signature::parse(metadata.blob(metadata.cell(TableId::MemberRef, row, 2)?)?)?;
        let parent = metadata
            .coded(TableId::MemberRef, row, 0)?
            .ok_or_else(|| ResolveError::Malformed("member reference has no parent".into()))?;

        let owner = match parent.table() {
            Some(TableId::MethodDef) => {
                return Ok(Member::Method(MemberHandle {
                    module,
                    token: parent,
                }));
            }
            Some(TableId::ModuleRef) => return self.find_global(module, name, &signature),
            Some(TableId::TypeDef) | Some(TableId::TypeRef) => {
                self.resolve_type_token(module, parent)?
            }
            Some(TableId::TypeSpec) => {
                let spec = Signature::parse_type(metadata.blob(metadata.cell(
                    TableId::TypeSpec,
                    parent.row(),
                    0,
                )?)?)?;
                match spec {
                    TypeSig::GenericInst { definition, .. } => {
                        self.resolve_type_token(module, definition)?
                    }
                    TypeSig::Array(_) | TypeSig::SzArray(_) => return Ok(Member::ArrayMethod),
                    TypeSig::Class(token) | TypeSig::ValueType(token) => {
                        self.resolve_type_token(module, token)?
                    }
                    TypeSig::TypeVar(_) | TypeSig::MethodVar(_) => {
                        return Err(ResolveError::Unsupported("members on generic parameters"));
                    }
                    _ => {
                        return Err(ResolveError::Malformed(
                            "member reference parent is not a type".into(),
                        ));
                    }
                }
            }
            _ => {
                return Err(ResolveError::Malformed(
                    "member reference has an invalid parent".into(),
                ));
            }
        };
        self.find_member(module, owner, name, &signature)
    }

    fn find_global(&self, module: usize, name: &str, signature: &Signature) -> Outcome<Member> {
        if self.modules[module].metadata.rows(TableId::TypeDef) == 0 {
            return Err(ResolveError::Malformed("module has no global type".into()));
        }
        self.find_member(module, TypeHandle { module, row: 1 }, name, signature)
    }

    fn find_member(
        &self,
        from: usize,
        owner: TypeHandle,
        name: &str,
        signature: &Signature,
    ) -> Outcome<Member> {
        let mut current = Some(owner);
        let mut depth = 0;
        while let Some(handle) = current {
            if let Some(member) = self.find_declared(from, handle, name, signature)? {
                return Ok(member);
            }
            depth += 1;
            if depth > 64 {
                break;
            }
            current = self.base_type(handle)?;
        }
        Err(ResolveError::MissingType {
            assembly: self.modules[owner.module].name.to_owned(),
            name: format!("{}::{name}", self.type_name(owner)),
        })
    }

    pub fn base_type(&self, handle: TypeHandle) -> Outcome<Option<TypeHandle>> {
        let metadata = &self.modules[handle.module].metadata;
        let Some(extends) = metadata.coded(TableId::TypeDef, handle.row, 3)? else {
            return Ok(None);
        };
        if extends.table() == Some(TableId::TypeSpec) {
            let spec = Signature::parse_type(metadata.blob(metadata.cell(
                TableId::TypeSpec,
                extends.row(),
                0,
            )?)?)?;
            return match spec {
                TypeSig::GenericInst { definition, .. } => {
                    Ok(Some(self.resolve_type_token(handle.module, definition)?))
                }
                _ => Err(ResolveError::Malformed("base type is not a class".into())),
            };
        }
        Ok(Some(self.resolve_type_token(handle.module, extends)?))
    }

    fn find_declared(
        &self,
        from: usize,
        owner: TypeHandle,
        name: &str,
        signature: &Signature,
    ) -> Outcome<Option<Member>> {
        let target = &self.modules[owner.module].metadata;
        match signature {
            Signature::Field(wanted) => {
                for field in target.field_range(owner.row)? {
                    if target.string_cell(TableId::Field, field, 1)? != name {
                        continue;
                    }
                    let candidate =
                        Signature::parse(target.blob(target.cell(TableId::Field, field, 2)?)?)?;
                    if let Signature::Field(found) = candidate
                        && self.types_equal(from, wanted, owner.module, &found)?
                    {
                        return Ok(Some(Member::Field(MemberHandle {
                            module: owner.module,
                            token: Token::new(TableId::Field, field),
                        })));
                    }
                }
            }
            Signature::Method(wanted) => {
                for method in target.method_range(owner.row)? {
                    if target.string_cell(TableId::MethodDef, method, 3)? != name {
                        continue;
                    }
                    let candidate = Signature::parse(target.blob(target.cell(
                        TableId::MethodDef,
                        method,
                        4,
                    )?)?)?;
                    if let Signature::Method(found) = candidate
                        && self.methods_equal(from, wanted, owner.module, &found)?
                    {
                        return Ok(Some(Member::Method(MemberHandle {
                            module: owner.module,
                            token: Token::new(TableId::MethodDef, method),
                        })));
                    }
                }
            }
            _ => {
                return Err(ResolveError::Malformed(
                    "member reference signature is not a field or method".into(),
                ));
            }
        }
        Ok(None)
    }

    fn methods_equal(&self, ma: usize, a: &MethodSig, mb: usize, b: &MethodSig) -> Outcome<bool> {
        let fixed = a.vararg_start.unwrap_or(a.parameters.len());
        if a.convention != b.convention
            || a.has_this != b.has_this
            || a.explicit_this != b.explicit_this
            || a.generic_parameters != b.generic_parameters
            || fixed != b.parameters.len()
        {
            return Ok(false);
        }
        if !self.types_equal(ma, &a.return_type, mb, &b.return_type)? {
            return Ok(false);
        }
        for (x, y) in a.parameters[..fixed].iter().zip(&b.parameters) {
            if !self.types_equal(ma, x, mb, y)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn tokens_equal(&self, ma: usize, a: Token, mb: usize, b: Token) -> Outcome<bool> {
        let spec_a = a.table() == Some(TableId::TypeSpec);
        let spec_b = b.table() == Some(TableId::TypeSpec);
        if spec_a || spec_b {
            if !(spec_a && spec_b) {
                return Ok(false);
            }
            let read = |module: usize, token: Token| -> Outcome<TypeSig> {
                let metadata = &self.modules[module].metadata;
                Ok(Signature::parse_type(metadata.blob(metadata.cell(
                    TableId::TypeSpec,
                    token.row(),
                    0,
                )?)?)?)
            };
            return self.types_equal(ma, &read(ma, a)?, mb, &read(mb, b)?);
        }
        Ok(self.resolve_type_token(ma, a)? == self.resolve_type_token(mb, b)?)
    }

    pub fn types_equal(&self, ma: usize, a: &TypeSig, mb: usize, b: &TypeSig) -> Outcome<bool> {
        use TypeSig as T;
        Ok(match (a, b) {
            (T::Class(x), T::Class(y)) | (T::ValueType(x), T::ValueType(y)) => {
                self.tokens_equal(ma, *x, mb, *y)?
            }
            (T::Pointer(x), T::Pointer(y))
            | (T::ByRef(x), T::ByRef(y))
            | (T::SzArray(x), T::SzArray(y))
            | (T::Pinned(x), T::Pinned(y)) => self.types_equal(ma, x, mb, y)?,
            (T::Array(x), T::Array(y)) => {
                x.rank == y.rank
                    && x.sizes == y.sizes
                    && x.lower_bounds == y.lower_bounds
                    && self.types_equal(ma, &x.element, mb, &y.element)?
            }
            (
                T::GenericInst {
                    value_type: vx,
                    definition: dx,
                    arguments: ax,
                },
                T::GenericInst {
                    value_type: vy,
                    definition: dy,
                    arguments: ay,
                },
            ) => {
                if vx != vy || ax.len() != ay.len() || !self.tokens_equal(ma, *dx, mb, *dy)? {
                    return Ok(false);
                }
                for (x, y) in ax.iter().zip(ay) {
                    if !self.types_equal(ma, x, mb, y)? {
                        return Ok(false);
                    }
                }
                true
            }
            (T::FunctionPointer(x), T::FunctionPointer(y)) => self.methods_equal(ma, x, mb, y)?,
            (
                T::Modified {
                    required: rx,
                    modifier: mx,
                    inner: ix,
                },
                T::Modified {
                    required: ry,
                    modifier: my,
                    inner: iy,
                },
            ) => {
                rx == ry
                    && self.tokens_equal(ma, *mx, mb, *my)?
                    && self.types_equal(ma, ix, mb, iy)?
            }
            (T::TypeVar(x), T::TypeVar(y)) | (T::MethodVar(x), T::MethodVar(y)) => x == y,
            _ => std::mem::discriminant(a) == std::mem::discriminant(b) && is_primitive(a),
        })
    }
}

fn is_primitive(sig: &TypeSig) -> bool {
    use TypeSig as T;
    matches!(
        sig,
        T::Void
            | T::Boolean
            | T::Char
            | T::I1
            | T::U1
            | T::I2
            | T::U2
            | T::I4
            | T::U4
            | T::I8
            | T::U8
            | T::R4
            | T::R8
            | T::String
            | T::Object
            | T::TypedReference
            | T::IntPtr
            | T::UIntPtr
            | T::Sentinel
    )
}

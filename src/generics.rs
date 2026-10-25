use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::loader::{AssemblySet, TypeHandle};
use crate::metadata::{TableId, Token};
use crate::signature::{Signature, TypeSig};

type Outcome<T> = Result<T, String>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Gen {
    Named(TypeHandle, Rc<[Gen]>),
    SzArray(Rc<Gen>),
    Array(Rc<Gen>, u32),
    Pointer(Rc<Gen>),
    ByRef(Rc<Gen>),
    FunctionPointer,
    Var(TypeHandle, u32),
    MethodVar(usize, u32, u32),
}

impl Gen {
    pub fn handle(&self) -> Option<TypeHandle> {
        match self {
            Gen::Named(handle, _) => Some(*handle),
            _ => None,
        }
    }

    pub fn arguments(&self) -> Rc<[Gen]> {
        match self {
            Gen::Named(_, arguments) => arguments.clone(),
            _ => Rc::from(Vec::new()),
        }
    }

    pub fn context(&self) -> Ctx {
        Ctx {
            types: self.arguments(),
            methods: Rc::from(Vec::new()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Ctx {
    pub types: Rc<[Gen]>,
    pub methods: Rc<[Gen]>,
}

impl Ctx {
    pub fn is_empty(&self) -> bool {
        self.types.is_empty() && self.methods.is_empty()
    }
}

pub struct Types<'s> {
    set: &'s AssemblySet,
    core: Option<usize>,
    named: RefCell<HashMap<&'static str, TypeHandle>>,
    ids: RefCell<HashMap<Gen, u32>>,
    list: RefCell<Vec<Gen>>,
    parameters: RefCell<HashMap<usize, HashMap<(u8, u32), usize>>>,
    interfaces: RefCell<HashMap<usize, HashMap<u32, Vec<Token>>>>,
}

fn primitive_name(sig: &TypeSig) -> Option<&'static str> {
    Some(match sig {
        TypeSig::Void => "Void",
        TypeSig::Boolean => "Boolean",
        TypeSig::Char => "Char",
        TypeSig::I1 => "SByte",
        TypeSig::U1 => "Byte",
        TypeSig::I2 => "Int16",
        TypeSig::U2 => "UInt16",
        TypeSig::I4 => "Int32",
        TypeSig::U4 => "UInt32",
        TypeSig::I8 => "Int64",
        TypeSig::U8 => "UInt64",
        TypeSig::R4 => "Single",
        TypeSig::R8 => "Double",
        TypeSig::String => "String",
        TypeSig::Object => "Object",
        TypeSig::TypedReference => "TypedReference",
        TypeSig::IntPtr => "IntPtr",
        TypeSig::UIntPtr => "UIntPtr",
        _ => return None,
    })
}

fn primitive_sig(name: &str) -> Option<TypeSig> {
    Some(match name {
        "System.Boolean" => TypeSig::Boolean,
        "System.Char" => TypeSig::Char,
        "System.SByte" => TypeSig::I1,
        "System.Byte" => TypeSig::U1,
        "System.Int16" => TypeSig::I2,
        "System.UInt16" => TypeSig::U2,
        "System.Int32" => TypeSig::I4,
        "System.UInt32" => TypeSig::U4,
        "System.Int64" => TypeSig::I8,
        "System.UInt64" => TypeSig::U8,
        "System.Single" => TypeSig::R4,
        "System.Double" => TypeSig::R8,
        "System.IntPtr" => TypeSig::IntPtr,
        "System.UIntPtr" => TypeSig::UIntPtr,
        "System.String" => TypeSig::String,
        "System.Object" => TypeSig::Object,
        _ => return None,
    })
}

impl<'s> Types<'s> {
    pub fn new(set: &'s AssemblySet) -> Self {
        Self {
            set,
            core: set.find_assembly("System.Private.CoreLib"),
            named: RefCell::new(HashMap::new()),
            ids: RefCell::new(HashMap::new()),
            list: RefCell::new(Vec::new()),
            parameters: RefCell::new(HashMap::new()),
            interfaces: RefCell::new(HashMap::new()),
        }
    }

    pub fn core(&self, name: &'static str) -> Outcome<Gen> {
        Ok(Gen::Named(self.core_handle(name)?, Rc::from(Vec::new())))
    }

    pub fn core_handle(&self, name: &'static str) -> Outcome<TypeHandle> {
        if let Some(found) = self.named.borrow().get(name) {
            return Ok(*found);
        }
        let (namespace, short) = name.rsplit_once('.').unwrap_or(("System", name));
        let namespace = if name.contains('.') { namespace } else { "System" };
        let core = self.core.ok_or("System.Private.CoreLib is not loaded")?;
        let handle = self
            .set
            .find_named(core, namespace, short)
            .ok_or_else(|| format!("{name} was not found"))?;
        self.named.borrow_mut().insert(name, handle);
        Ok(handle)
    }

    pub fn id(&self, exact: &Gen) -> u32 {
        if let Some(found) = self.ids.borrow().get(exact) {
            return *found;
        }
        let mut list = self.list.borrow_mut();
        let id = list.len() as u32;
        list.push(exact.clone());
        self.ids.borrow_mut().insert(exact.clone(), id);
        id
    }

    pub fn get(&self, id: u32) -> Option<Gen> {
        self.list.borrow().get(id as usize).cloned()
    }

    pub fn parameter_count(&self, table: TableId, owner: u32, module: usize) -> usize {
        let mut cache = self.parameters.borrow_mut();
        let counts = cache.entry(module).or_insert_with(|| {
            let metadata = &self.set.modules[module].metadata;
            let mut counts: HashMap<(u8, u32), usize> = HashMap::new();
            for row in 1..=metadata.rows(TableId::GenericParam) {
                if let Ok(Some(token)) = metadata.coded(TableId::GenericParam, row, 2) {
                    let kind = u8::from(token.table() == Some(TableId::MethodDef));
                    *counts.entry((kind, token.row())).or_default() += 1;
                }
            }
            counts
        });
        let kind = u8::from(table == TableId::MethodDef);
        counts.get(&(kind, owner)).copied().unwrap_or(0)
    }

    pub fn parameter_row(&self, exact: &Gen) -> Option<(usize, u32)> {
        let (module, method, owner, index) = match exact {
            Gen::Var(handle, index) => (handle.module, false, handle.row, *index),
            Gen::MethodVar(module, row, index) => (*module, true, *row, *index),
            _ => return None,
        };
        let metadata = &self.set.modules[module].metadata;
        (1..=metadata.rows(TableId::GenericParam)).find_map(|row| {
            let token = metadata.coded(TableId::GenericParam, row, 2).ok().flatten()?;
            let matches = (token.table() == Some(TableId::MethodDef)) == method
                && token.row() == owner
                && metadata.cell(TableId::GenericParam, row, 0).ok()? == index;
            matches.then_some((module, row))
        })
    }

    pub fn parameter_name(&self, exact: &Gen) -> String {
        self.parameter_row(exact)
            .and_then(|(module, row)| self.set.modules[module].metadata.string_cell(TableId::GenericParam, row, 3).ok())
            .map(str::to_owned)
            .unwrap_or_else(|| "T".into())
    }

    pub fn open_arguments(&self, handle: TypeHandle) -> Rc<[Gen]> {
        let count = self.parameter_count(TableId::TypeDef, handle.row, handle.module);
        (0..count as u32).map(|index| Gen::Var(handle, index)).collect()
    }

    pub fn open_method_arguments(&self, module: usize, row: u32) -> Rc<[Gen]> {
        let count = self.parameter_count(TableId::MethodDef, row, module);
        (0..count as u32).map(|index| Gen::MethodVar(module, row, index)).collect()
    }

    pub fn resolve(&self, module: usize, sig: &TypeSig, ctx: &Ctx) -> Outcome<Gen> {
        if let Some(name) = primitive_name(sig) {
            return self.core(name);
        }
        Ok(match sig {
            TypeSig::Pointer(inner) => Gen::Pointer(Rc::new(self.resolve(module, inner, ctx)?)),
            TypeSig::ByRef(inner) => Gen::ByRef(Rc::new(self.resolve(module, inner, ctx)?)),
            TypeSig::ValueType(token) | TypeSig::Class(token) => {
                Gen::Named(self.handle(module, *token)?, Rc::from(Vec::new()))
            }
            TypeSig::TypeVar(index) => ctx
                .types
                .get(*index as usize)
                .cloned()
                .ok_or("open type parameter")?,
            TypeSig::MethodVar(index) => ctx
                .methods
                .get(*index as usize)
                .cloned()
                .ok_or("open method type parameter")?,
            TypeSig::Array(shape) => Gen::Array(
                Rc::new(self.resolve(module, &shape.element, ctx)?),
                shape.rank,
            ),
            TypeSig::SzArray(inner) => Gen::SzArray(Rc::new(self.resolve(module, inner, ctx)?)),
            TypeSig::GenericInst {
                definition,
                arguments,
                ..
            } => {
                let handle = self.handle(module, *definition)?;
                let arguments = arguments
                    .iter()
                    .map(|argument| self.resolve(module, argument, ctx))
                    .collect::<Outcome<Vec<_>>>()?;
                Gen::Named(handle, Rc::from(arguments))
            }
            TypeSig::FunctionPointer(_) => Gen::FunctionPointer,
            TypeSig::Modified { inner, .. } | TypeSig::Pinned(inner) => {
                self.resolve(module, inner, ctx)?
            }
            other => return Err(format!("type {other:?}")),
        })
    }

    fn handle(&self, module: usize, token: Token) -> Outcome<TypeHandle> {
        self.set
            .resolve_type_token(module, token)
            .map_err(|e| format!("{e:?}"))
    }

    pub fn resolve_token(&self, module: usize, token: Token, ctx: &Ctx) -> Outcome<Gen> {
        match token.table() {
            Some(TableId::TypeSpec) => {
                let metadata = &self.set.modules[module].metadata;
                let sig = metadata
                    .cell(TableId::TypeSpec, token.row(), 0)
                    .and_then(|b| metadata.blob(b))
                    .and_then(Signature::parse_type)
                    .map_err(|e| e.to_string())?;
                self.resolve(module, &sig, ctx)
            }
            Some(TableId::TypeDef) | Some(TableId::TypeRef) => {
                let handle = self.handle(module, token)?;
                let count = self.parameter_count(TableId::TypeDef, handle.row, handle.module);
                if count > 0 && ctx.types.len() == count && token.table() == Some(TableId::TypeDef) {
                    Ok(Gen::Named(handle, ctx.types.clone()))
                } else {
                    Ok(Gen::Named(handle, Rc::from(Vec::new())))
                }
            }
            _ => Err("token is not a type".into()),
        }
    }

    pub fn full_name(&self, exact: &Gen) -> String {
        match exact {
            Gen::Named(handle, arguments) => {
                let name = self.set.full_type_name(*handle).unwrap_or_default();
                if arguments.is_empty() {
                    name
                } else {
                    let inner: Vec<String> = arguments.iter().map(|a| self.full_name(a)).collect();
                    format!("{name}[{}]", inner.join(","))
                }
            }
            Gen::SzArray(inner) => format!("{}[]", self.full_name(inner)),
            Gen::Array(inner, rank) => {
                format!("{}[{}]", self.full_name(inner), ",".repeat(rank.saturating_sub(1) as usize))
            }
            Gen::Pointer(inner) => format!("{}*", self.full_name(inner)),
            Gen::ByRef(inner) => format!("{}&", self.full_name(inner)),
            Gen::FunctionPointer => "function pointer".into(),
            Gen::Var(..) | Gen::MethodVar(..) => self.parameter_name(exact),
        }
    }

    pub fn is_value_type(&self, exact: &Gen) -> bool {
        match exact {
            Gen::Named(handle, _) => self.set.is_value_type(*handle).unwrap_or(false),
            _ => false,
        }
    }

    pub fn base(&self, exact: &Gen) -> Outcome<Option<Gen>> {
        match exact {
            Gen::Named(handle, arguments) => {
                let metadata = &self.set.modules[handle.module].metadata;
                let Some(extends) = metadata
                    .coded(TableId::TypeDef, handle.row, 3)
                    .map_err(|e| e.to_string())?
                else {
                    return Ok(None);
                };
                let ctx = Ctx {
                    types: arguments.clone(),
                    methods: Rc::from(Vec::new()),
                };
                Ok(Some(self.resolve_token(handle.module, extends, &ctx)?))
            }
            Gen::SzArray(_) | Gen::Array(_, _) => Ok(Some(self.core("Array")?)),
            _ => Ok(None),
        }
    }

    pub fn interfaces(&self, exact: &Gen) -> Vec<Gen> {
        let Gen::Named(handle, arguments) = exact else {
            return Vec::new();
        };
        let tokens = {
            let mut cache = self.interfaces.borrow_mut();
            let index = cache.entry(handle.module).or_insert_with(|| {
                let metadata = &self.set.modules[handle.module].metadata;
                let mut index: HashMap<u32, Vec<Token>> = HashMap::new();
                for row in 1..=metadata.rows(TableId::InterfaceImpl) {
                    if let (Ok(class), Ok(Some(interface))) = (
                        metadata.cell(TableId::InterfaceImpl, row, 0),
                        metadata.coded(TableId::InterfaceImpl, row, 1),
                    ) {
                        index.entry(class).or_default().push(interface);
                    }
                }
                index
            });
            index.get(&handle.row).cloned().unwrap_or_default()
        };
        let ctx = Ctx {
            types: arguments.clone(),
            methods: Rc::from(Vec::new()),
        };
        tokens
            .into_iter()
            .filter_map(|token| self.resolve_token(handle.module, token, &ctx).ok())
            .collect()
    }

    pub fn ancestry(&self, exact: &Gen) -> Vec<Gen> {
        let mut chain = Vec::new();
        let mut current = Some(exact.clone());
        while let Some(found) = current {
            if chain.len() > 64 {
                break;
            }
            current = self.base(&found).ok().flatten();
            chain.push(found);
        }
        chain
    }

    pub fn all_interfaces(&self, exact: &Gen) -> Vec<Gen> {
        let mut found: Vec<Gen> = Vec::new();
        let mut pending: Vec<Gen> = self
            .ancestry(exact)
            .iter()
            .flat_map(|g| self.interfaces(g))
            .collect();
        while let Some(next) = pending.pop() {
            if found.contains(&next) || found.len() > 256 {
                continue;
            }
            pending.extend(self.interfaces(&next));
            found.push(next);
        }
        found
    }

    pub fn instance_of(&self, exact: &Gen, definition: TypeHandle) -> Option<Gen> {
        self.ancestry(exact)
            .into_iter()
            .chain(self.all_interfaces(exact))
            .find(|g| g.handle() == Some(definition))
    }

    pub fn layout_arguments(&self, arguments: &[Gen]) -> Outcome<Vec<(usize, TypeSig)>> {
        let mut list = Vec::with_capacity(arguments.len());
        let mut extras = Vec::new();
        for exact in arguments {
            let module = match exact {
                Gen::Named(handle, _) => handle.module,
                _ => self.core.ok_or("System.Private.CoreLib is not loaded")?,
            };
            let sig = self.layout_sig(exact, module, Some((&mut extras, arguments.len())))?;
            list.push((module, sig));
        }
        list.extend(extras);
        Ok(list)
    }

    fn layout_sig(
        &self,
        exact: &Gen,
        module: usize,
        mut extras: Option<(&mut Vec<(usize, TypeSig)>, usize)>,
    ) -> Outcome<TypeSig> {
        match exact {
            Gen::Named(handle, arguments) => {
                let name = self.set.full_type_name(*handle).map_err(|e| format!("{e:?}"))?;
                if let Some(sig) = primitive_sig(&name) {
                    return Ok(sig);
                }
                if !self.set.is_value_type(*handle).unwrap_or(false) {
                    return Ok(TypeSig::Object);
                }
                if self.set.is_enum(*handle).unwrap_or(false) {
                    let size = self
                        .set
                        .value_type_layout(*handle)
                        .map_err(|e| format!("{e:?}"))?
                        .size;
                    return Ok(match size {
                        1 => TypeSig::U1,
                        2 => TypeSig::U2,
                        4 => TypeSig::I4,
                        _ => TypeSig::I8,
                    });
                }
                if handle.module != module {
                    let Some((list, base)) = extras else {
                        return Err("a struct argument from another assembly is nested inside a generic struct".into());
                    };
                    let foreign = self.layout_sig(exact, handle.module, None)?;
                    list.push((handle.module, foreign));
                    return Ok(TypeSig::TypeVar((base + list.len() - 1) as u32));
                }
                let definition = Token::new(TableId::TypeDef, handle.row);
                if arguments.is_empty() {
                    return Ok(TypeSig::ValueType(definition));
                }
                Ok(TypeSig::GenericInst {
                    value_type: true,
                    definition,
                    arguments: arguments
                        .iter()
                        .map(|argument| {
                            self.layout_sig(argument, module, extras.as_mut().map(|(list, base)| (&mut **list, *base)))
                        })
                        .collect::<Outcome<Vec<_>>>()?,
                })
            }
            Gen::Pointer(_) | Gen::FunctionPointer => Ok(TypeSig::IntPtr),
            Gen::ByRef(_) => Err("byref fields".into()),
            Gen::SzArray(_) | Gen::Array(_, _) | Gen::Var(..) | Gen::MethodVar(..) => Ok(TypeSig::Object),
        }
    }
}

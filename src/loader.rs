use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::metadata::{Metadata, TableId, Token};
use crate::pe::PeImage;

const MAX_HOPS: u32 = 32;

pub struct Module {
    pub path: PathBuf,
    pub name: &'static str,
    pub metadata: Metadata<'static>,
    pub ready_to_run: bool,
    pub image: PeImage<'static>,
    top_level: HashMap<(&'static str, &'static str), u32>,
    nested: HashMap<(u32, &'static str), u32>,
    exported: HashMap<(&'static str, &'static str), u32>,
    enclosing: HashMap<u32, u32>,
    class_layout: HashMap<u32, u32>,
    field_layout: HashMap<u32, u32>,
    inline_arrays: std::sync::OnceLock<HashMap<u32, u32>>,
}

impl Module {
    fn load(path: &Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let image = PeImage::parse(data)?;
        let metadata = Metadata::parse(image.metadata()?)?;
        let ready_to_run = image.cli.is_ready_to_run();

        let name = if metadata.rows(TableId::Assembly) > 0 {
            metadata.string_cell(TableId::Assembly, 1, 7)?
        } else {
            metadata.string_cell(TableId::Module, 1, 1)?
        };

        let mut nested = HashMap::new();
        let mut enclosing = HashMap::new();
        let mut is_nested = vec![false; metadata.rows(TableId::TypeDef) as usize + 1];
        for row in 1..=metadata.rows(TableId::NestedClass) {
            let inner = metadata.cell(TableId::NestedClass, row, 0)?;
            let outer = metadata.cell(TableId::NestedClass, row, 1)?;
            if let Some(slot) = is_nested.get_mut(inner as usize) {
                *slot = true;
            }
            enclosing.insert(inner, outer);
            nested.insert(
                (outer, metadata.string_cell(TableId::TypeDef, inner, 1)?),
                inner,
            );
        }

        let mut top_level = HashMap::with_capacity(metadata.rows(TableId::TypeDef) as usize);
        for row in 1..=metadata.rows(TableId::TypeDef) {
            if !is_nested[row as usize] {
                let key = (
                    metadata.string_cell(TableId::TypeDef, row, 2)?,
                    metadata.string_cell(TableId::TypeDef, row, 1)?,
                );
                top_level.insert(key, row);
            }
        }

        let mut exported = HashMap::new();
        for row in 1..=metadata.rows(TableId::ExportedType) {
            let is_top = !matches!(metadata.coded(TableId::ExportedType, row, 4)?, Some(token) if token.table() == Some(TableId::ExportedType));
            if is_top {
                let key = (
                    metadata.string_cell(TableId::ExportedType, row, 3)?,
                    metadata.string_cell(TableId::ExportedType, row, 2)?,
                );
                exported.insert(key, row);
            }
        }

        let mut class_layout = HashMap::new();
        for row in 1..=metadata.rows(TableId::ClassLayout) {
            class_layout.insert(metadata.cell(TableId::ClassLayout, row, 2)?, row);
        }
        let mut field_layout = HashMap::new();
        for row in 1..=metadata.rows(TableId::FieldLayout) {
            field_layout.insert(
                metadata.cell(TableId::FieldLayout, row, 1)?,
                metadata.cell(TableId::FieldLayout, row, 0)?,
            );
        }

        Ok(Self {
            path: path.to_path_buf(),
            name,
            metadata,
            ready_to_run,
            image,
            top_level,
            nested,
            exported,
            enclosing,
            class_layout,
            field_layout,
            inline_arrays: std::sync::OnceLock::new(),
        })
    }
}

impl Module {
    pub fn version(&self) -> Result<[u32; 4]> {
        if self.metadata.rows(TableId::Assembly) == 0 {
            return Ok([0; 4]);
        }
        let mut version = [0u32; 4];
        for (slot, column) in version.iter_mut().zip(1..=4) {
            *slot = self.metadata.cell(TableId::Assembly, 1, column)?;
        }
        Ok(version)
    }
}

impl Module {
    pub fn enclosing_type(&self, row: u32) -> Option<u32> {
        self.enclosing.get(&row).copied()
    }

    pub fn class_layout(&self, row: u32) -> Result<Option<(u32, u32)>> {
        match self.class_layout.get(&row) {
            Some(&layout) => Ok(Some((
                self.metadata.cell(TableId::ClassLayout, layout, 0)?,
                self.metadata.cell(TableId::ClassLayout, layout, 1)?,
            ))),
            None => Ok(None),
        }
    }

    pub fn inline_array_length(&self, row: u32) -> Option<u32> {
        self.inline_arrays
            .get_or_init(|| self.scan_inline_arrays().unwrap_or_default())
            .get(&row)
            .copied()
    }

    fn scan_inline_arrays(&self) -> Result<HashMap<u32, u32>> {
        let metadata = &self.metadata;
        let mut found = HashMap::new();
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
                Some(TableId::MethodDef) => self
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
            if metadata.string_cell(table, owner.row(), 1)? != "InlineArrayAttribute"
                || metadata.string_cell(table, owner.row(), 2)? != "System.Runtime.CompilerServices"
            {
                continue;
            }
            let value = metadata.blob(metadata.cell(TableId::CustomAttribute, row, 2)?)?;
            if value.len() >= 6 && value[0] == 1 && value[1] == 0 {
                found.insert(
                    parent.row(),
                    u32::from_le_bytes([value[2], value[3], value[4], value[5]]),
                );
            }
        }
        Ok(found)
    }

    pub fn field_offset(&self, field: u32) -> Option<u32> {
        self.field_layout.get(&field).copied()
    }

    pub fn full_name(&self, row: u32) -> Result<String> {
        let name = self.metadata.string_cell(TableId::TypeDef, row, 1)?;
        match self.enclosing_type(row) {
            Some(outer) => Ok(format!("{}+{name}", self.full_name(outer)?)),
            None => {
                let namespace = self.metadata.string_cell(TableId::TypeDef, row, 2)?;
                Ok(if namespace.is_empty() {
                    name.to_owned()
                } else {
                    format!("{namespace}.{name}")
                })
            }
        }
    }

    pub fn method_owner(&self, method: u32) -> Result<Option<u32>> {
        self.list_owner(method, 5)
    }

    pub fn field_owner(&self, field: u32) -> Result<Option<u32>> {
        self.list_owner(field, 4)
    }

    fn list_owner(&self, method: u32, column: usize) -> Result<Option<u32>> {
        let types = self.metadata.rows(TableId::TypeDef);
        let (mut low, mut high) = (1u32, types);
        let mut found = None;
        while low <= high {
            let middle = low + (high - low) / 2;
            if self.metadata.cell(TableId::TypeDef, middle, column)? <= method {
                found = Some(middle);
                low = middle + 1;
            } else {
                if middle == 0 {
                    break;
                }
                high = middle - 1;
            }
        }
        Ok(found)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeHandle {
    pub module: usize,
    pub row: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    MissingAssembly(String),
    MissingType { assembly: String, name: String },
    Unsupported(&'static str),
    Malformed(String),
}

impl From<Error> for ResolveError {
    fn from(error: Error) -> Self {
        ResolveError::Malformed(error.to_string())
    }
}

#[derive(Default)]
pub struct AssemblySet {
    pub modules: Vec<Module>,
    by_name: HashMap<String, usize>,
}

impl AssemblySet {
    pub fn add(&mut self, path: &Path) -> Result<Option<usize>> {
        let module = match Module::load(path) {
            Ok(module) => module,
            Err(Error::NotManaged) => return Ok(None),
            Err(error) => return Err(error),
        };
        let key = module.name.to_ascii_lowercase();
        if let Some(&existing) = self.by_name.get(&key) {
            if module.version()? > self.modules[existing].version()? {
                self.modules[existing] = module;
            }
            return Ok(Some(existing));
        }
        let index = self.modules.len();
        self.by_name.insert(key, index);
        self.modules.push(module);
        Ok(Some(index))
    }

    pub fn find_type_by_name(
        &self,
        module: usize,
        namespace: &str,
        name: &str,
    ) -> Option<TypeHandle> {
        self.modules[module]
            .top_level
            .get(&(namespace, name))
            .map(|&row| TypeHandle { module, row })
    }

    pub fn find_assembly(&self, name: &str) -> Option<usize> {
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn type_name(&self, handle: TypeHandle) -> String {
        let metadata = &self.modules[handle.module].metadata;
        metadata
            .type_name(Token::new(TableId::TypeDef, handle.row))
            .unwrap_or_else(|_| format!("TypeDef#{}", handle.row))
    }

    pub fn resolve_type_ref(
        &self,
        module: usize,
        row: u32,
    ) -> std::result::Result<TypeHandle, ResolveError> {
        self.type_ref(module, row, 0)
    }

    fn type_ref(
        &self,
        module: usize,
        row: u32,
        hops: u32,
    ) -> std::result::Result<TypeHandle, ResolveError> {
        if hops > MAX_HOPS {
            return Err(ResolveError::Malformed(
                "type reference chain is cyclic".into(),
            ));
        }
        let metadata = &self.modules[module].metadata;
        let name = metadata.string_cell(TableId::TypeRef, row, 1)?;
        let namespace = metadata.string_cell(TableId::TypeRef, row, 2)?;
        match metadata.coded(TableId::TypeRef, row, 0)? {
            None => self.find(module, namespace, name, hops + 1),
            Some(scope) => match scope.table() {
                Some(TableId::Module) | Some(TableId::ModuleRef) => {
                    self.find(module, namespace, name, hops + 1)
                }
                Some(TableId::AssemblyRef) => {
                    let target = self.assembly_ref(module, scope.row())?;
                    self.find(target, namespace, name, hops + 1)
                }
                Some(TableId::TypeRef) => {
                    let outer = self.type_ref(module, scope.row(), hops + 1)?;
                    self.nested(outer, name)
                }
                _ => Err(ResolveError::Malformed(
                    "type reference has an invalid scope".into(),
                )),
            },
        }
    }

    pub fn find_named(&self, module: usize, namespace: &str, name: &str) -> Option<TypeHandle> {
        self.find(module, namespace, name, 0).ok()
    }

    pub fn find_nested(&self, outer: TypeHandle, name: &str) -> Option<TypeHandle> {
        self.nested(outer, name).ok()
    }

    fn assembly_ref(&self, module: usize, row: u32) -> std::result::Result<usize, ResolveError> {
        let name = self.modules[module]
            .metadata
            .string_cell(TableId::AssemblyRef, row, 6)?;
        self.find_assembly(name)
            .ok_or_else(|| ResolveError::MissingAssembly(name.to_owned()))
    }

    fn nested(
        &self,
        outer: TypeHandle,
        name: &str,
    ) -> std::result::Result<TypeHandle, ResolveError> {
        let module = &self.modules[outer.module];
        module
            .nested
            .get(&(outer.row, name))
            .map(|&row| TypeHandle {
                module: outer.module,
                row,
            })
            .ok_or_else(|| ResolveError::MissingType {
                assembly: module.name.to_owned(),
                name: format!("{}+{name}", self.type_name(outer)),
            })
    }

    fn find(
        &self,
        module: usize,
        namespace: &str,
        name: &str,
        hops: u32,
    ) -> std::result::Result<TypeHandle, ResolveError> {
        let target = &self.modules[module];
        if let Some(&row) = target.top_level.get(&(namespace, name)) {
            return Ok(TypeHandle { module, row });
        }
        if let Some(&row) = target.exported.get(&(namespace, name)) {
            return self.exported(module, row, hops + 1);
        }
        Err(ResolveError::MissingType {
            assembly: target.name.to_owned(),
            name: if namespace.is_empty() {
                name.to_owned()
            } else {
                format!("{namespace}.{name}")
            },
        })
    }

    fn exported(
        &self,
        module: usize,
        row: u32,
        hops: u32,
    ) -> std::result::Result<TypeHandle, ResolveError> {
        if hops > MAX_HOPS {
            return Err(ResolveError::Malformed(
                "type forwarding chain is cyclic".into(),
            ));
        }
        let metadata = &self.modules[module].metadata;
        let name = metadata.string_cell(TableId::ExportedType, row, 2)?;
        let namespace = metadata.string_cell(TableId::ExportedType, row, 3)?;
        let implementation = metadata
            .coded(TableId::ExportedType, row, 4)?
            .ok_or_else(|| ResolveError::Malformed("exported type has no implementation".into()))?;
        match implementation.table() {
            Some(TableId::AssemblyRef) => {
                let target = self.assembly_ref(module, implementation.row())?;
                self.find(target, namespace, name, hops + 1)
            }
            Some(TableId::ExportedType) => {
                let outer = self.exported(module, implementation.row(), hops + 1)?;
                self.nested(outer, name)
            }
            Some(TableId::File) => Err(ResolveError::Unsupported("multi file assemblies")),
            _ => Err(ResolveError::Malformed(
                "exported type has an invalid implementation".into(),
            )),
        }
    }
}

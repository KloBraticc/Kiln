use std::collections::{BTreeMap, HashMap, HashSet};

use crate::baml::{self, KnownTypes, TypeId};
use crate::il::{Argument, MethodBody};
use crate::loader::{AssemblySet, TypeHandle};
use crate::members::Member;
use crate::metadata::{TableId, Token};
use crate::resources::{self, Value};
use crate::signature::Signature;

const VIRTUAL: u32 = 0x40;
const SPECIAL_NAME: u32 = 0x800;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MethodRef {
    pub module: usize,
    pub row: u32,
}

#[derive(Debug, Clone, Copy)]
enum Item {
    Type(TypeHandle),
    Construct(TypeHandle),
    Dynamic(TypeHandle),
    Activate(TypeHandle),
    Method(MethodRef),
}

#[derive(Debug, Default)]
pub struct BamlSummary {
    pub resource: String,
    pub records: usize,
    pub elements: Vec<String>,
    pub unresolved: Vec<String>,
}

type Attributes = HashMap<u32, Vec<(Token, u32)>>;

pub struct Reachability<'s> {
    set: &'s AssemblySet,
    known: KnownTypes,
    pub types: HashSet<TypeHandle>,
    pub constructed: HashSet<TypeHandle>,
    pub dynamic: HashSet<TypeHandle>,
    pub methods: HashSet<MethodRef>,
    pub baml: Vec<BamlSummary>,
    pub baml_types: HashSet<TypeHandle>,
    pub open_reflection: HashSet<MethodRef>,
    pub unresolved: BTreeMap<String, u64>,
    queue: Vec<Item>,
    modules: HashSet<usize>,
    attributes: HashMap<usize, Attributes>,
    interfaces: HashMap<usize, HashMap<u32, Vec<Token>>>,
}

impl<'s> Reachability<'s> {
    pub fn new(set: &'s AssemblySet) -> Self {
        Self {
            set,
            known: KnownTypes::load(set),
            types: HashSet::new(),
            constructed: HashSet::new(),
            dynamic: HashSet::new(),
            methods: HashSet::new(),
            baml: Vec::new(),
            baml_types: HashSet::new(),
            open_reflection: HashSet::new(),
            unresolved: BTreeMap::new(),
            queue: Vec::new(),
            modules: HashSet::new(),
            attributes: HashMap::new(),
            interfaces: HashMap::new(),
        }
    }

    pub fn known_types(&self) -> usize {
        self.known.len()
    }

    pub fn add_application(&mut self, module: usize) {
        let entry = self.set.modules[module].image.cli.entry_point;
        let token = Token(entry);
        if token.table() == Some(TableId::MethodDef) {
            self.queue.push(Item::Method(MethodRef {
                module,
                row: token.row(),
            }));
        }
        self.enter_module(module);
    }

    fn enter_module(&mut self, module: usize) {
        if !self.modules.insert(module) {
            return;
        }
        if self.set.modules[module].metadata.rows(TableId::TypeDef) > 0 {
            let global = TypeHandle { module, row: 1 };
            let mixed =
                self.set.modules[module].image.cli.flags & crate::pe::CliHeader::IL_ONLY == 0;
            self.queue.push(if mixed {
                Item::Dynamic(global)
            } else {
                Item::Type(global)
            });
        }
        self.add_baml(module);
    }

    fn add_baml(&mut self, module: usize) {
        let Ok(manifest) = resources::manifest(&self.set.modules[module]) else {
            self.miss("unreadable manifest resources");
            return;
        };
        for (name, data) in manifest {
            if !name.ends_with(".g.resources") {
                continue;
            }
            let Ok(entries) = resources::entries(data) else {
                self.miss("unreadable resources file");
                continue;
            };
            for (entry, value) in entries {
                let Value::Data(bytes) = value else { continue };
                if !baml::is_baml(bytes) {
                    continue;
                }
                match baml::parse(bytes) {
                    Ok(document) => self.add_document(format!("{name}/{entry}"), &document),
                    Err(error) => self.miss(&format!("BAML could not be decoded: {error}")),
                }
            }
        }
    }

    fn add_document(&mut self, resource: String, document: &baml::Document) {
        let mut summary = BamlSummary {
            resource,
            records: document.records,
            ..Default::default()
        };
        let owners: Vec<TypeId> = document
            .properties
            .iter()
            .filter_map(|id| document.attributes.get(id).map(|(owner, _)| *owner))
            .collect();
        let uses = document
            .elements
            .iter()
            .map(|id| (*id, true))
            .chain(document.referenced.iter().map(|id| (*id, false)))
            .chain(owners.into_iter().map(|id| (id, false)));
        for (id, element) in uses {
            let resolved = baml::resolve_type(self.set, &self.known, document, id);
            match resolved.handle {
                Some(handle) => {
                    if element {
                        summary.elements.push(resolved.name);
                    }
                    self.baml_types.insert(handle);
                    self.queue.push(Item::Dynamic(handle));
                }
                None => summary.unresolved.push(resolved.name),
            }
        }
        self.baml.push(summary);
    }

    pub fn run(&mut self) {
        while let Some(item) = self.queue.pop() {
            match item {
                Item::Type(handle) => self.visit_type(handle),
                Item::Construct(handle) => self.visit_construct(handle),
                Item::Dynamic(handle) => self.visit_dynamic(handle),
                Item::Activate(handle) => self.visit_activate(handle),
                Item::Method(method) => self.visit_method(method),
            }
        }
    }

    fn miss(&mut self, reason: &str) {
        *self.unresolved.entry(reason.to_owned()).or_default() += 1;
    }

    fn method_flags(&self, method: MethodRef) -> u32 {
        self.set.modules[method.module]
            .metadata
            .cell(TableId::MethodDef, method.row, 2)
            .unwrap_or(0)
    }

    fn method_name(&self, method: MethodRef) -> &'s str {
        self.set.modules[method.module]
            .metadata
            .string_cell(TableId::MethodDef, method.row, 3)
            .unwrap_or("")
    }

    fn type_methods(&self, handle: TypeHandle) -> Vec<MethodRef> {
        self.set.modules[handle.module]
            .metadata
            .method_range(handle.row)
            .map(|range| {
                range
                    .map(|row| MethodRef {
                        module: handle.module,
                        row,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn visit_type(&mut self, handle: TypeHandle) {
        if !self.types.insert(handle) {
            return;
        }
        self.enter_module(handle.module);
        match self.set.base_type(handle) {
            Ok(Some(base)) => self.queue.push(Item::Type(base)),
            Ok(None) => {}
            Err(_) => self.miss("base type does not resolve"),
        }
        let module = &self.set.modules[handle.module];
        if let Some(outer) = module.enclosing_type(handle.row) {
            self.queue.push(Item::Type(TypeHandle {
                module: handle.module,
                row: outer,
            }));
        }
        for method in self.type_methods(handle) {
            if self.method_name(method) == ".cctor" {
                self.queue.push(Item::Method(method));
            }
        }
        for token in self.interface_tokens(handle) {
            for found in self.type_token(handle.module, token) {
                self.queue.push(Item::Type(found));
            }
        }
        self.visit_attributes(handle.module, Token::new(TableId::TypeDef, handle.row));
    }

    fn visit_construct(&mut self, handle: TypeHandle) {
        if !self.constructed.insert(handle) {
            return;
        }
        self.queue.push(Item::Type(handle));
        if let Ok(Some(base)) = self.set.base_type(handle) {
            self.queue.push(Item::Construct(base));
        }
        for method in self.type_methods(handle) {
            let flags = self.method_flags(method);
            if flags & VIRTUAL != 0 {
                self.queue.push(Item::Method(method));
            } else if flags & SPECIAL_NAME != 0 {
                let name = self.method_name(method);
                if name.starts_with("get_") || name.starts_with("set_") {
                    self.queue.push(Item::Method(method));
                }
            }
        }
    }

    fn visit_activate(&mut self, handle: TypeHandle) {
        self.queue.push(Item::Construct(handle));
        for method in self.type_methods(handle) {
            if self.method_name(method) == ".ctor" {
                self.queue.push(Item::Method(method));
            }
        }
    }

    fn visit_dynamic(&mut self, handle: TypeHandle) {
        if !self.dynamic.insert(handle) {
            return;
        }
        self.queue.push(Item::Construct(handle));
        for method in self.type_methods(handle) {
            self.queue.push(Item::Method(method));
        }
        let metadata = &self.set.modules[handle.module].metadata;
        if let Ok(fields) = metadata.field_range(handle.row) {
            for field in fields {
                self.visit_attributes(handle.module, Token::new(TableId::Field, field));
            }
        }
    }

    fn visit_method(&mut self, method: MethodRef) {
        if !self.methods.insert(method) {
            return;
        }
        let module = &self.set.modules[method.module];
        match module.method_owner(method.row) {
            Ok(Some(owner)) => {
                let owner = TypeHandle {
                    module: method.module,
                    row: owner,
                };
                self.queue.push(if self.method_name(method) == ".ctor" {
                    Item::Construct(owner)
                } else {
                    Item::Type(owner)
                });
            }
            _ => self.miss("method has no owner"),
        }
        self.visit_attributes(method.module, Token::new(TableId::MethodDef, method.row));
        let metadata = &module.metadata;
        let rva = metadata.cell(TableId::MethodDef, method.row, 0).unwrap_or(0);
        if rva == 0 {
            return;
        }
        let Ok(body) = module
            .image
            .tail_at_rva(rva)
            .and_then(|tail| MethodBody::parse(tail, rva))
        else {
            self.miss("method body does not parse");
            return;
        };
        let mut reflects = false;
        let mut type_tokens = Vec::new();
        let mut literals = Vec::new();
        for instruction in body.instructions() {
            let Ok(instruction) = instruction else {
                self.miss("method body does not decode");
                break;
            };
            let Argument::Token(token) = instruction.argument else {
                continue;
            };
            let opcode = instruction.opcode.value;
            match opcode {
                0x27 | 0x28 | 0x6F | 0x73 | 0xFE06 | 0xFE07 => {
                    let Some(target) = self.method_token(method.module, token) else {
                        continue;
                    };
                    if is_reflection(self.set, target) {
                        reflects = true;
                    }
                    self.queue.push(Item::Method(target));
                    if opcode == 0x73
                        && let Ok(Some(owner)) = self.set.modules[target.module].method_owner(target.row)
                    {
                        self.queue.push(Item::Construct(TypeHandle {
                            module: target.module,
                            row: owner,
                        }));
                    }
                }
                0x7B..=0x81 => {
                    if let Some(owner) = self.field_owner(method.module, token) {
                        self.queue.push(Item::Type(owner));
                    }
                }
                0x72 => {
                    if let Ok(text) = metadata.user_string(token.0 & 0x00FF_FFFF) {
                        literals.push(text);
                    }
                }
                _ => {
                    let constructs = matches!(opcode, 0x8C | 0x8D | 0xFE15);
                    let found = match token.table() {
                        Some(TableId::Field) | Some(TableId::MemberRef)
                        | Some(TableId::MethodDef) | Some(TableId::MethodSpec) => {
                            if let Some(target) = self.method_token(method.module, token) {
                                self.queue.push(Item::Method(target));
                            } else if let Some(owner) = self.field_owner(method.module, token) {
                                self.queue.push(Item::Type(owner));
                            }
                            continue;
                        }
                        _ => self.type_token(method.module, token),
                    };
                    for handle in found {
                        if opcode == 0xD0 {
                            type_tokens.push(handle);
                        }
                        self.queue.push(if constructs {
                            Item::Construct(handle)
                        } else {
                            Item::Type(handle)
                        });
                    }
                }
            }
        }
        let mut resolved_any = false;
        let named = literals
            .iter()
            .filter_map(|text| literal_type(self.set, text))
            .collect::<Vec<_>>();
        for handle in type_tokens.into_iter().chain(named) {
            resolved_any = true;
            self.queue.push(if reflects {
                Item::Dynamic(handle)
            } else {
                Item::Activate(handle)
            });
        }
        if reflects && !resolved_any {
            self.open_reflection.insert(method);
        }
    }

    fn visit_attributes(&mut self, module: usize, parent: Token) {
        let index = self.attributes.entry(module).or_insert_with(|| {
            let metadata = &self.set.modules[module].metadata;
            let mut index: Attributes = HashMap::new();
            for row in 1..=metadata.rows(TableId::CustomAttribute) {
                let (Ok(Some(owner)), Ok(Some(constructor)), Ok(blob)) = (
                    metadata.coded(TableId::CustomAttribute, row, 0),
                    metadata.coded(TableId::CustomAttribute, row, 1),
                    metadata.cell(TableId::CustomAttribute, row, 2),
                ) else {
                    continue;
                };
                index.entry(owner.0).or_default().push((constructor, blob));
            }
            index
        });
        let Some(entries) = index.get(&parent.0).cloned() else {
            return;
        };
        let metadata = &self.set.modules[module].metadata;
        for (constructor, blob) in entries {
            if let Some(target) = self.method_token(module, constructor) {
                self.queue.push(Item::Method(target));
                if let Ok(Some(owner)) = self.set.modules[target.module].method_owner(target.row) {
                    self.queue.push(Item::Construct(TypeHandle {
                        module: target.module,
                        row: owner,
                    }));
                }
            }
            if let Ok(value) = metadata.blob(blob) {
                for name in serialized_strings(value) {
                    if let Some(handle) = literal_type(self.set, &name) {
                        self.queue.push(Item::Dynamic(handle));
                    }
                }
            }
        }
    }

    fn interface_tokens(&mut self, handle: TypeHandle) -> Vec<Token> {
        let index = self.interfaces.entry(handle.module).or_insert_with(|| {
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
    }

    fn method_token(&mut self, module: usize, token: Token) -> Option<MethodRef> {
        match token.table()? {
            TableId::MethodDef => Some(MethodRef {
                module,
                row: token.row(),
            }),
            TableId::MemberRef => match self.set.resolve_member_ref(module, token.row()) {
                Ok(Member::Method(handle)) if handle.token.table() == Some(TableId::MethodDef) => {
                    Some(MethodRef {
                        module: handle.module,
                        row: handle.token.row(),
                    })
                }
                Ok(Member::Method(handle)) => self.method_token(handle.module, handle.token),
                Ok(_) => None,
                Err(_) => {
                    self.miss("method reference does not resolve");
                    None
                }
            },
            TableId::MethodSpec => {
                let metadata = &self.set.modules[module].metadata;
                let generic = metadata.coded(TableId::MethodSpec, token.row(), 0).ok()??;
                if let Ok(blob) = metadata.cell(TableId::MethodSpec, token.row(), 1)
                    && let Ok(bytes) = metadata.blob(blob)
                {
                    for handle in instantiation_types(self.set, module, bytes) {
                        self.queue.push(Item::Activate(handle));
                    }
                }
                self.method_token(module, generic)
            }
            _ => None,
        }
    }

    fn field_owner(&mut self, module: usize, token: Token) -> Option<TypeHandle> {
        match token.table()? {
            TableId::Field => self.set.modules[module]
                .field_owner(token.row())
                .ok()
                .flatten()
                .map(|row| TypeHandle { module, row }),
            TableId::MemberRef => match self.set.resolve_member_ref(module, token.row()) {
                Ok(Member::Field(handle)) => self.set.modules[handle.module]
                    .field_owner(handle.token.row())
                    .ok()
                    .flatten()
                    .map(|row| TypeHandle {
                        module: handle.module,
                        row,
                    }),
                Ok(_) => None,
                Err(_) => {
                    self.miss("field reference does not resolve");
                    None
                }
            },
            _ => None,
        }
    }

    fn type_token(&mut self, module: usize, token: Token) -> Vec<TypeHandle> {
        match token.table() {
            Some(TableId::TypeDef) | Some(TableId::TypeRef) => {
                match self.set.resolve_type_token(module, token) {
                    Ok(handle) => vec![handle],
                    Err(_) => {
                        self.miss("type reference does not resolve");
                        Vec::new()
                    }
                }
            }
            Some(TableId::TypeSpec) => {
                let metadata = &self.set.modules[module].metadata;
                let Ok(signature) = metadata
                    .cell(TableId::TypeSpec, token.row(), 0)
                    .and_then(|blob| metadata.blob(blob))
                    .and_then(Signature::parse_type)
                else {
                    self.miss("type specification does not parse");
                    return Vec::new();
                };
                let mut tokens = Vec::new();
                signature.tokens(&mut tokens);
                tokens
                    .into_iter()
                    .filter_map(|t| self.set.resolve_type_token(module, t).ok())
                    .collect()
            }
            _ => Vec::new(),
        }
    }
}

fn instantiation_types(set: &AssemblySet, module: usize, bytes: &[u8]) -> Vec<TypeHandle> {
    let mut tokens = Vec::new();
    if let Ok(signature @ Signature::MethodSpec(_)) = Signature::parse(bytes) {
        signature.tokens(&mut tokens);
    }
    tokens
        .into_iter()
        .filter_map(|t| set.resolve_type_token(module, t).ok())
        .collect()
}

fn is_reflection(set: &AssemblySet, method: MethodRef) -> bool {
    let module = &set.modules[method.module];
    let Ok(Some(owner)) = module.method_owner(method.row) else {
        return false;
    };
    let Ok(owner) = set.full_type_name(TypeHandle {
        module: method.module,
        row: owner,
    }) else {
        return false;
    };
    let name = module
        .metadata
        .string_cell(TableId::MethodDef, method.row, 3)
        .unwrap_or("");
    match owner.as_str() {
        "System.Type" => !matches!(
            name,
            "GetTypeFromHandle" | "op_Equality" | "op_Inequality" | "get_TypeHandle" | "Equals"
                | "GetHashCode" | "ToString" | "get_IsValueType" | "IsAssignableFrom"
                | "IsInstanceOfType" | "get_FullName" | "get_Name"
        ),
        "System.Activator" | "System.ComponentModel.TypeDescriptor" => true,
        other => other.starts_with("System.Reflection."),
    }
}

fn literal_type(set: &AssemblySet, text: &str) -> Option<TypeHandle> {
    let (name, assembly) = match text.split_once(',') {
        Some((name, rest)) => (name.trim(), rest.split(',').next().map(str::trim)),
        None => (text.trim(), None),
    };
    let shaped = name.len() <= 512
        && name.contains('.')
        && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '`' | '+'));
    if !shaped {
        return None;
    }
    let preferred = assembly.and_then(|a| set.find_assembly(a));
    baml::find_type(set, preferred, name)
}

fn serialized_strings(blob: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let mut position = 2;
    while position < blob.len() {
        let length = blob[position] as usize;
        if length > 0 && length < 0x80 && position + 1 + length <= blob.len() {
            let candidate = &blob[position + 1..position + 1 + length];
            if candidate.iter().all(|b| b.is_ascii_graphic() || *b == b' ')
                && let Ok(text) = std::str::from_utf8(candidate)
                && text.contains('.')
            {
                found.push(text.to_owned());
                position += 1 + length;
                continue;
            }
        }
        position += 1;
    }
    found
}

use std::cell::RefCell;
use std::collections::HashMap;

use crate::il::{Argument, ClauseKind, MethodBody};
use crate::loader::{AssemblySet, ResolveError, TypeHandle};
use crate::metadata::{TableId, Token};
use crate::signature::{MethodSig, Signature, TypeSig};

type Outcome<T> = std::result::Result<T, TranslateError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Int32,
    Int64,
    NativeInt,
    Float,
    Object,
    ManagedPointer,
    Struct,
    Generic,
}

impl Kind {
    fn merge(self, other: Kind) -> Option<Kind> {
        match (self, other) {
            (a, b) if a == b => Some(a),
            (Kind::Generic, _) | (_, Kind::Generic) => Some(Kind::Generic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslateError {
    StackUnderflow(u32),
    StackOverflow(u32),
    DepthMismatch(u32),
    StackNotEmptyAtReturn(u32),
    BadTarget(u32),
    Unresolved(u32, String),
    Malformed(String),
}

impl std::fmt::Display for TranslateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranslateError::StackUnderflow(at) => write!(f, "stack underflow at IL {at:#x}"),
            TranslateError::StackOverflow(at) => write!(f, "stack exceeds max stack at IL {at:#x}"),
            TranslateError::DepthMismatch(at) => {
                write!(f, "stack depth differs between paths into IL {at:#x}")
            }
            TranslateError::StackNotEmptyAtReturn(at) => {
                write!(f, "stack is not balanced at the return at IL {at:#x}")
            }
            TranslateError::BadTarget(at) => write!(f, "branch at IL {at:#x} leaves the method"),
            TranslateError::Unresolved(at, what) => write!(f, "IL {at:#x}: {what}"),
            TranslateError::Malformed(what) => f.write_str(what),
        }
    }
}

impl From<crate::error::Error> for TranslateError {
    fn from(error: crate::error::Error) -> Self {
        TranslateError::Malformed(error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub enum IrArgument {
    None,
    Int(i64),
    Float(f64),
    Variable(u16),
    Token(Token),
    Targets(Vec<usize>),
}

#[derive(Debug, Clone)]
pub struct IrInstruction {
    pub offset: u32,
    pub opcode: u16,
    pub argument: IrArgument,
    pub inputs: Vec<ValueId>,
    pub output: Option<(ValueId, Kind)>,
}

#[derive(Debug, Clone, Default)]
pub struct IrBlock {
    pub start: u32,
    pub parameters: Vec<(ValueId, Kind)>,
    pub instructions: Vec<IrInstruction>,
    pub successors: Vec<(usize, Vec<ValueId>)>,
    pub handler_entry: bool,
}

#[derive(Debug, Clone, Default)]
pub struct IrMethod {
    pub blocks: Vec<IrBlock>,
    pub values: u32,
    pub merge_conflicts: u32,
    pub unreachable_blocks: u32,
}

impl IrMethod {
    pub fn validate(&self) -> Result<(), String> {
        let mut defined = vec![false; self.values as usize];
        for (index, block) in self.blocks.iter().enumerate() {
            let mut live: std::collections::HashSet<ValueId> =
                block.parameters.iter().map(|p| p.0).collect();
            for (id, _) in &block.parameters {
                if std::mem::replace(&mut defined[id.0 as usize], true) {
                    return Err(format!("value {} is defined twice", id.0));
                }
            }
            for instruction in &block.instructions {
                for input in &instruction.inputs {
                    if !live.contains(input) {
                        return Err(format!(
                            "block {index} uses value {} before it is defined",
                            input.0
                        ));
                    }
                }
                if let Some((id, _)) = instruction.output {
                    if std::mem::replace(&mut defined[id.0 as usize], true) {
                        return Err(format!("value {} is defined twice", id.0));
                    }
                    live.insert(id);
                }
            }
            for (target, carried) in &block.successors {
                let target_block = self
                    .blocks
                    .get(*target)
                    .ok_or_else(|| format!("block {index} jumps to missing block {target}"))?;
                if target_block.parameters.len() != carried.len() {
                    return Err(format!(
                        "block {index} passes {} values to block {target} which takes {}",
                        carried.len(),
                        target_block.parameters.len()
                    ));
                }
                if let Some(missing) = carried.iter().find(|v| !live.contains(v)) {
                    return Err(format!(
                        "block {index} passes undefined value {}",
                        missing.0
                    ));
                }
            }
        }
        Ok(())
    }
}

struct Effect {
    pops: usize,
    push: Option<Kind>,
    ends_block: bool,
    clears_stack: bool,
}

const fn effect(pops: usize, push: Option<Kind>) -> Effect {
    Effect {
        pops,
        push,
        ends_block: false,
        clears_stack: false,
    }
}

pub struct Translator<'s> {
    set: &'s AssemblySet,
    kinds: RefCell<HashMap<(usize, Token), Kind>>,
    value_types: RefCell<HashMap<TypeHandle, Option<Kind>>>,
}

impl<'s> Translator<'s> {
    pub fn new(set: &'s AssemblySet) -> Self {
        Self {
            set,
            kinds: RefCell::new(HashMap::new()),
            value_types: RefCell::new(HashMap::new()),
        }
    }

    fn resolve_error(at: u32, error: ResolveError) -> TranslateError {
        TranslateError::Unresolved(at, format!("{error:?}"))
    }

    fn handle_kind(&self, handle: TypeHandle) -> Result<Option<Kind>, ResolveError> {
        if let Some(cached) = self.value_types.borrow().get(&handle) {
            return Ok(*cached);
        }
        let kind = if self.set.is_enum(handle)? {
            let layout = self.set.value_type_layout(handle)?;
            Some(match layout.size {
                8 => Kind::Int64,
                _ => Kind::Int32,
            })
        } else if self.set.is_value_type(handle)? {
            Some(match self.set.full_type_name(handle)?.as_str() {
                "System.IntPtr" | "System.UIntPtr" => Kind::NativeInt,
                "System.Int64" | "System.UInt64" => Kind::Int64,
                "System.Double" | "System.Single" => Kind::Float,
                "System.Int32" | "System.UInt32" | "System.Int16" | "System.UInt16"
                | "System.Byte" | "System.SByte" | "System.Boolean" | "System.Char" => Kind::Int32,
                _ => Kind::Struct,
            })
        } else {
            None
        };
        self.value_types.borrow_mut().insert(handle, kind);
        Ok(kind)
    }

    fn type_token_kind(&self, module: usize, token: Token) -> Result<Kind, ResolveError> {
        if let Some(kind) = self.kinds.borrow().get(&(module, token)) {
            return Ok(*kind);
        }
        let kind = match token.table() {
            Some(TableId::TypeDef) | Some(TableId::TypeRef) => {
                let handle = self.set.resolve_type_token(module, token)?;
                self.handle_kind(handle)?.unwrap_or(Kind::Object)
            }
            Some(TableId::TypeSpec) => {
                let metadata = &self.set.modules[module].metadata;
                let sig = Signature::parse_type(metadata.blob(metadata.cell(
                    TableId::TypeSpec,
                    token.row(),
                    0,
                )?)?)?;
                self.sig_kind(module, &sig)?
            }
            _ => {
                return Err(ResolveError::Malformed(format!(
                    "{:#010x} is not a type",
                    token.0
                )));
            }
        };
        self.kinds.borrow_mut().insert((module, token), kind);
        Ok(kind)
    }

    fn sig_kind(&self, module: usize, sig: &TypeSig) -> Result<Kind, ResolveError> {
        use TypeSig as T;
        Ok(match sig {
            T::Boolean | T::Char | T::I1 | T::U1 | T::I2 | T::U2 | T::I4 | T::U4 => Kind::Int32,
            T::I8 | T::U8 => Kind::Int64,
            T::R4 | T::R8 => Kind::Float,
            T::IntPtr | T::UIntPtr | T::Pointer(_) | T::FunctionPointer(_) => Kind::NativeInt,
            T::ByRef(_) => Kind::ManagedPointer,
            T::String | T::Object | T::Class(_) | T::SzArray(_) | T::Array(_) => Kind::Object,
            T::GenericInst {
                value_type: false, ..
            } => Kind::Object,
            T::GenericInst {
                value_type: true,
                definition,
                ..
            } => {
                let handle = self.set.resolve_type_token(module, *definition)?;
                self.handle_kind(handle)?.unwrap_or(Kind::Struct)
            }
            T::ValueType(token) => self.type_token_kind(module, *token)?,
            T::TypedReference => Kind::Struct,
            T::Modified { inner, .. } | T::Pinned(inner) => self.sig_kind(module, inner)?,
            T::TypeVar(_) | T::MethodVar(_) => Kind::Generic,
            T::Void | T::Sentinel => {
                return Err(ResolveError::Malformed("void has no stack kind".into()));
            }
        })
    }

    fn method_sig(&self, module: usize, token: Token) -> Result<MethodSig, ResolveError> {
        let metadata = &self.set.modules[module].metadata;
        let blob = match token.table() {
            Some(TableId::MethodDef) => metadata.cell(TableId::MethodDef, token.row(), 4)?,
            Some(TableId::MemberRef) => metadata.cell(TableId::MemberRef, token.row(), 2)?,
            Some(TableId::MethodSpec) => {
                let method = metadata
                    .coded(TableId::MethodSpec, token.row(), 0)?
                    .ok_or_else(|| {
                        ResolveError::Malformed("method instantiation has no method".into())
                    })?;
                return self.method_sig(module, method);
            }
            Some(TableId::StandAloneSig) => {
                metadata.cell(TableId::StandAloneSig, token.row(), 0)?
            }
            _ => {
                return Err(ResolveError::Malformed(format!(
                    "{:#010x} is not a method",
                    token.0
                )));
            }
        };
        match Signature::parse(metadata.blob(blob)?)? {
            Signature::Method(sig) => Ok(sig),
            _ => Err(ResolveError::Malformed(
                "token does not name a method".into(),
            )),
        }
    }

    fn method_owner_kind(&self, module: usize, token: Token) -> Result<Kind, ResolveError> {
        let metadata = &self.set.modules[module].metadata;
        match token.table() {
            Some(TableId::MethodDef) => {
                let owner = self.set.modules[module]
                    .method_owner(token.row())?
                    .ok_or_else(|| ResolveError::Malformed("method has no owner".into()))?;
                Ok(self
                    .handle_kind(TypeHandle { module, row: owner })?
                    .unwrap_or(Kind::Object))
            }
            Some(TableId::MemberRef) => {
                let parent = metadata
                    .coded(TableId::MemberRef, token.row(), 0)?
                    .ok_or_else(|| {
                        ResolveError::Malformed("member reference has no parent".into())
                    })?;
                match parent.table() {
                    Some(TableId::TypeSpec) => {
                        let sig = Signature::parse_type(metadata.blob(metadata.cell(
                            TableId::TypeSpec,
                            parent.row(),
                            0,
                        )?)?)?;
                        match sig {
                            TypeSig::Array(_) | TypeSig::SzArray(_) => Ok(Kind::Object),
                            other => self.sig_kind(module, &other),
                        }
                    }
                    Some(TableId::MethodDef) => self.method_owner_kind(module, parent),
                    _ => self.type_token_kind(module, parent),
                }
            }
            Some(TableId::MethodSpec) => {
                let method = metadata
                    .coded(TableId::MethodSpec, token.row(), 0)?
                    .ok_or_else(|| {
                        ResolveError::Malformed("method instantiation has no method".into())
                    })?;
                self.method_owner_kind(module, method)
            }
            _ => Err(ResolveError::Malformed(
                "constructor token is not a method".into(),
            )),
        }
    }

    fn field_kind(&self, module: usize, token: Token) -> Result<Kind, ResolveError> {
        if let Some(kind) = self.kinds.borrow().get(&(module, token)) {
            return Ok(*kind);
        }
        let metadata = &self.set.modules[module].metadata;
        let blob = match token.table() {
            Some(TableId::Field) => metadata.cell(TableId::Field, token.row(), 2)?,
            Some(TableId::MemberRef) => metadata.cell(TableId::MemberRef, token.row(), 2)?,
            _ => {
                return Err(ResolveError::Malformed(format!(
                    "{:#010x} is not a field",
                    token.0
                )));
            }
        };
        let kind = match Signature::parse(metadata.blob(blob)?)? {
            Signature::Field(sig) => self.sig_kind(module, &sig)?,
            _ => {
                return Err(ResolveError::Malformed(
                    "token does not name a field".into(),
                ));
            }
        };
        self.kinds.borrow_mut().insert((module, token), kind);
        Ok(kind)
    }

    fn value_kind(&self, module: usize, sig: &TypeSig) -> Result<Option<Kind>, ResolveError> {
        if is_void(sig) {
            return Ok(None);
        }
        Ok(Some(self.sig_kind(module, sig)?))
    }

    pub fn translate(&self, module: usize, method_row: u32) -> Outcome<Option<IrMethod>> {
        let module_ref = &self.set.modules[module];
        let metadata = &module_ref.metadata;
        let rva = metadata.cell(TableId::MethodDef, method_row, 0)?;
        if rva == 0 || metadata.cell(TableId::MethodDef, method_row, 1)? & 0x3 != 0 {
            return Ok(None);
        }
        let body = MethodBody::parse(module_ref.image.tail_at_rva(rva)?, rva)?;
        let Signature::Method(signature) = Signature::parse(metadata.blob(metadata.cell(
            TableId::MethodDef,
            method_row,
            4,
        )?)?)?
        else {
            return Err(TranslateError::Malformed(
                "method signature is not a method".into(),
            ));
        };

        let mut arguments = Vec::new();
        if signature.has_this && !signature.explicit_this {
            let owner = self
                .method_owner_kind(module, Token::new(TableId::MethodDef, method_row))
                .map_err(|e| Self::resolve_error(0, e))?;
            arguments.push(if owner == Kind::Object {
                Kind::Object
            } else {
                Kind::ManagedPointer
            });
        }
        for parameter in &signature.parameters {
            arguments.push(
                self.sig_kind(module, parameter)
                    .map_err(|e| Self::resolve_error(0, e))?,
            );
        }
        let returns_value = !is_void(&signature.return_type);

        let mut locals = Vec::new();
        if !body.local_signature.is_null() {
            let blob = metadata.blob(metadata.cell(
                TableId::StandAloneSig,
                body.local_signature.row(),
                0,
            )?)?;
            if let Signature::Locals(types) = Signature::parse(blob)? {
                for local in &types {
                    locals.push(
                        self.sig_kind(module, local)
                            .map_err(|e| Self::resolve_error(0, e))?,
                    );
                }
            }
        }

        let mut instructions = Vec::new();
        for instruction in body.instructions() {
            instructions.push(instruction?);
        }
        if instructions.is_empty() {
            return Err(TranslateError::Malformed(
                "method body has no instructions".into(),
            ));
        }
        let index_of: HashMap<u32, usize> = instructions
            .iter()
            .enumerate()
            .map(|(i, x)| (x.offset, i))
            .collect();
        let target_index = |at: u32, target: i64| -> Outcome<usize> {
            u32::try_from(target)
                .ok()
                .and_then(|t| index_of.get(&t).copied())
                .ok_or(TranslateError::BadTarget(at))
        };

        let mut leaders = vec![false; instructions.len()];
        leaders[0] = true;
        let mut entry_states: HashMap<usize, Vec<Kind>> = HashMap::new();
        let mut handler_starts = Vec::new();
        for clause in &body.clauses {
            let try_start = target_index(clause.try_offset, i64::from(clause.try_offset))?;
            leaders[try_start] = true;
            if let Some(&after) = index_of.get(&(clause.try_offset + clause.try_length)) {
                leaders[after] = true;
            }
            let handler = target_index(clause.handler_offset, i64::from(clause.handler_offset))?;
            leaders[handler] = true;
            if let Some(&after) = index_of.get(&(clause.handler_offset + clause.handler_length)) {
                leaders[after] = true;
            }
            let exception_stack = match clause.kind {
                ClauseKind::Catch | ClauseKind::Filter => vec![Kind::Object],
                ClauseKind::Finally | ClauseKind::Fault => Vec::new(),
            };
            entry_states.insert(handler, exception_stack.clone());
            handler_starts.push(handler);
            if clause.kind == ClauseKind::Filter {
                let filter =
                    target_index(clause.class_or_filter, i64::from(clause.class_or_filter))?;
                leaders[filter] = true;
                entry_states.insert(filter, vec![Kind::Object]);
                handler_starts.push(filter);
            }
        }
        for (i, instruction) in instructions.iter().enumerate() {
            let mut mark = |target: i64| -> Outcome<()> {
                leaders[target_index(instruction.offset, target)?] = true;
                Ok(())
            };
            match instruction.argument {
                Argument::Target(target) => mark(target)?,
                Argument::Switch(switch) => {
                    for target in switch.iter() {
                        mark(target)?;
                    }
                }
                _ => {}
            }
            if (is_branch(instruction.opcode.value) || ends_flow(instruction.opcode.value))
                && i + 1 < instructions.len()
            {
                leaders[i + 1] = true;
            }
        }

        let block_starts: Vec<usize> = (0..instructions.len()).filter(|&i| leaders[i]).collect();
        let block_of: HashMap<usize, usize> = block_starts
            .iter()
            .enumerate()
            .map(|(b, &i)| (i, b))
            .collect();
        let block_count = block_starts.len();
        let block_end = |b: usize| {
            if b + 1 < block_count {
                block_starts[b + 1]
            } else {
                instructions.len()
            }
        };

        let mut states: Vec<Option<Vec<Kind>>> = vec![None; block_count];
        states[0] = Some(Vec::new());
        let mut handler_blocks = vec![false; block_count];
        for (&index, stack) in &entry_states {
            let block = block_of[&index];
            states[block] = Some(stack.clone());
            handler_blocks[block] = true;
        }
        let mut merge_conflicts = 0u32;
        let max_stack = body.max_stack as usize;

        let mut worklist: Vec<usize> = (0..block_count).filter(|&b| states[b].is_some()).collect();
        let mut visited = vec![false; block_count];
        let mut successors_of: Vec<Vec<usize>> = vec![Vec::new(); block_count];
        while let Some(block) = worklist.pop() {
            visited[block] = true;
            let mut stack = states[block].clone().unwrap_or_default();
            let mut falls_through = true;
            let mut targets = Vec::new();
            for instruction in &instructions[block_starts[block]..block_end(block)] {
                let at = instruction.offset;
                let effect = self
                    .effect(module, instruction, &arguments, &locals, returns_value)
                    .map_err(|e| Self::resolve_error(at, e))?;
                if stack.len() < effect.pops {
                    return Err(TranslateError::StackUnderflow(at));
                }
                let popped: Vec<Kind> = stack.split_off(stack.len() - effect.pops);
                if instruction.opcode.value == 0x2A && !stack.is_empty() {
                    return Err(TranslateError::StackNotEmptyAtReturn(at));
                }
                if effect.clears_stack {
                    stack.clear();
                }
                if let Some(kind) = effect.push {
                    let kind = match instruction.opcode.value {
                        0x25 => popped[0],
                        0x58..=0x5E | 0xD6..=0xDB => arithmetic(popped[0], popped[1]),
                        0x5F..=0x61 => arithmetic(popped[0], popped[1]),
                        0x62..=0x66 => popped[0],
                        _ => kind,
                    };
                    stack.push(kind);
                    if instruction.opcode.value == 0x25 {
                        stack.push(kind);
                    }
                }
                if stack.len() > max_stack {
                    return Err(TranslateError::StackOverflow(at));
                }
                match instruction.argument {
                    Argument::Target(target) => targets.push(block_of[&target_index(at, target)?]),
                    Argument::Switch(switch) => {
                        for target in switch.iter() {
                            targets.push(block_of[&target_index(at, target)?]);
                        }
                    }
                    _ => {}
                }
                if effect.ends_block {
                    falls_through = false;
                }
            }
            if falls_through && block + 1 < block_count {
                targets.push(block + 1);
            }
            let leaves = instructions[block_end(block) - 1].opcode.value;
            if leaves == 0xDD || leaves == 0xDE {
                stack.clear();
            }
            targets.sort_unstable();
            targets.dedup();
            successors_of[block] = targets.clone();
            for target in targets {
                match &states[target] {
                    None => {
                        states[target] = Some(stack.clone());
                        worklist.push(target);
                    }
                    Some(existing) => {
                        if existing.len() != stack.len() {
                            return Err(TranslateError::DepthMismatch(
                                instructions[block_starts[target]].offset,
                            ));
                        }
                        let mut merged = existing.clone();
                        let mut changed = false;
                        for (slot, incoming) in merged.iter_mut().zip(&stack) {
                            match slot.merge(*incoming) {
                                Some(kind) if kind != *slot => {
                                    *slot = kind;
                                    changed = true;
                                }
                                Some(_) => {}
                                None => {
                                    if *slot != Kind::Generic {
                                        merge_conflicts += 1;
                                        *slot = Kind::Generic;
                                        changed = true;
                                    }
                                }
                            }
                        }
                        if changed {
                            states[target] = Some(merged);
                            worklist.push(target);
                        } else if !visited[target] {
                            worklist.push(target);
                        }
                    }
                }
            }
        }

        let mut method = IrMethod {
            merge_conflicts,
            ..Default::default()
        };
        let mut next_value = 0u32;
        let mut fresh = |kind: Kind, list: &mut Vec<(ValueId, Kind)>| {
            let id = ValueId(next_value);
            next_value += 1;
            list.push((id, kind));
            id
        };
        for block in 0..block_count {
            let Some(entry) = &states[block] else {
                method.unreachable_blocks += 1;
                method.blocks.push(IrBlock {
                    start: instructions[block_starts[block]].offset,
                    ..Default::default()
                });
                continue;
            };
            let mut ir_block = IrBlock {
                start: instructions[block_starts[block]].offset,
                handler_entry: handler_blocks[block],
                ..Default::default()
            };
            let mut stack: Vec<(ValueId, Kind)> = Vec::new();
            for &kind in entry {
                let mut parameters = std::mem::take(&mut ir_block.parameters);
                let id = fresh(kind, &mut parameters);
                ir_block.parameters = parameters;
                stack.push((id, kind));
            }
            for instruction in &instructions[block_starts[block]..block_end(block)] {
                let effect = self
                    .effect(module, instruction, &arguments, &locals, returns_value)
                    .map_err(|e| Self::resolve_error(instruction.offset, e))?;
                let inputs: Vec<(ValueId, Kind)> = stack.split_off(stack.len() - effect.pops);
                if effect.clears_stack {
                    stack.clear();
                }
                let mut output = None;
                if let Some(kind) = effect.push {
                    let kind = match instruction.opcode.value {
                        0x25 => inputs[0].1,
                        0x58..=0x61 | 0xD6..=0xDB => arithmetic(inputs[0].1, inputs[1].1),
                        0x62..=0x66 => inputs[0].1,
                        _ => kind,
                    };
                    if instruction.opcode.value == 0x25 {
                        stack.push(inputs[0]);
                        stack.push(inputs[0]);
                    } else {
                        let mut produced = Vec::new();
                        let id = fresh(kind, &mut produced);
                        stack.push((id, kind));
                        output = Some((id, kind));
                    }
                }
                let argument = match instruction.argument {
                    Argument::None => IrArgument::None,
                    Argument::Int(value) => IrArgument::Int(value),
                    Argument::Float(value) => IrArgument::Float(value),
                    Argument::Variable(index) => IrArgument::Variable(index),
                    Argument::Token(token) => IrArgument::Token(token),
                    Argument::Target(target) => IrArgument::Targets(vec![
                        block_of[&target_index(instruction.offset, target)?],
                    ]),
                    Argument::Switch(switch) => IrArgument::Targets(
                        switch
                            .iter()
                            .map(|t| target_index(instruction.offset, t).map(|i| block_of[&i]))
                            .collect::<Outcome<_>>()?,
                    ),
                };
                ir_block.instructions.push(IrInstruction {
                    offset: instruction.offset,
                    opcode: instruction.opcode.value,
                    argument,
                    inputs: inputs.iter().map(|v| v.0).collect(),
                    output,
                });
            }
            let leaves = instructions[block_end(block) - 1].opcode.value;
            if leaves == 0xDD || leaves == 0xDE {
                stack.clear();
            }
            let carried: Vec<ValueId> = stack.iter().map(|v| v.0).collect();
            ir_block.successors = successors_of[block]
                .iter()
                .map(|&s| (s, carried.clone()))
                .collect();
            method.blocks.push(ir_block);
        }
        method.values = next_value;
        Ok(Some(method))
    }

    fn effect(
        &self,
        module: usize,
        instruction: &crate::il::Instruction<'_>,
        arguments: &[Kind],
        locals: &[Kind],
        returns_value: bool,
    ) -> Result<Effect, ResolveError> {
        use Kind::*;
        let token = match instruction.argument {
            Argument::Token(token) => token,
            _ => Token(0),
        };
        let variable = match instruction.argument {
            Argument::Variable(index) => index as usize,
            _ => 0,
        };
        let argument = |index: usize| {
            arguments
                .get(index)
                .copied()
                .ok_or_else(|| ResolveError::Malformed(format!("argument {index} does not exist")))
        };
        let local = |index: usize| {
            locals
                .get(index)
                .copied()
                .ok_or_else(|| ResolveError::Malformed(format!("local {index} does not exist")))
        };
        let ends = |mut e: Effect| {
            e.ends_block = true;
            e
        };
        let clears = |mut e: Effect| {
            e.clears_stack = true;
            e.ends_block = true;
            e
        };
        Ok(match instruction.opcode.value {
            0x00 | 0x01 => effect(0, None),
            0x02..=0x05 => effect(
                0,
                Some(argument((instruction.opcode.value - 0x02) as usize)?),
            ),
            0x06..=0x09 => effect(0, Some(local((instruction.opcode.value - 0x06) as usize)?)),
            0x0A..=0x0D => {
                local((instruction.opcode.value - 0x0A) as usize)?;
                effect(1, None)
            }
            0x0E | 0xFE09 => effect(0, Some(argument(variable)?)),
            0x0F | 0xFE0A => effect(0, Some(ManagedPointer)),
            0x10 | 0xFE0B => effect(1, None),
            0x11 | 0xFE0C => effect(0, Some(local(variable)?)),
            0x12 | 0xFE0D => effect(0, Some(ManagedPointer)),
            0x13 | 0xFE0E => effect(1, None),
            0x14 => effect(0, Some(Object)),
            0x15..=0x20 => effect(0, Some(Int32)),
            0x21 => effect(0, Some(Int64)),
            0x22 | 0x23 => effect(0, Some(Float)),
            0x25 => effect(1, Some(Generic)),
            0x26 => effect(1, None),
            0x27 => ends(effect(0, None)),
            0x28 | 0x6F | 0x29 => {
                let sig = self.method_sig(module, token)?;
                let mut pops =
                    sig.parameters.len() + usize::from(sig.has_this && !sig.explicit_this);
                if instruction.opcode.value == 0x29 {
                    pops += 1;
                }
                effect(pops, self.value_kind(module, &sig.return_type)?)
            }
            0x2A => ends(effect(usize::from(returns_value), None)),
            0x2B | 0x38 => ends(effect(0, None)),
            0x2C | 0x2D | 0x39 | 0x3A => effect(1, None),
            0x2E..=0x37 | 0x3B..=0x44 => effect(2, None),
            0x45 => effect(1, None),
            0x46..=0x4B => effect(1, Some(Int32)),
            0x4C => effect(1, Some(Int64)),
            0x4D => effect(1, Some(NativeInt)),
            0x4E | 0x4F => effect(1, Some(Float)),
            0x50 => effect(1, Some(Object)),
            0x51..=0x57 | 0xDF => effect(2, None),
            0x58..=0x64 | 0xD6..=0xDB => effect(2, Some(Generic)),
            0x65 | 0x66 => effect(1, Some(Generic)),
            0x67..=0x69 | 0x6D | 0x82..=0x84 | 0x86..=0x88 | 0xB3..=0xB8 | 0xD1 | 0xD2 => {
                effect(1, Some(Int32))
            }
            0x6A | 0x6E | 0x85 | 0x89 | 0xB9 | 0xBA => effect(1, Some(Int64)),
            0x6B | 0x6C | 0x76 | 0xC3 => effect(1, Some(Float)),
            0x8A | 0x8B | 0xD3..=0xD5 | 0xE0 => effect(1, Some(NativeInt)),
            0x70 => effect(2, None),
            0x71 => effect(1, Some(self.type_token_kind(module, token)?)),
            0x72 => effect(0, Some(Object)),
            0x73 => {
                let sig = self.method_sig(module, token)?;
                effect(
                    sig.parameters.len(),
                    Some(self.method_owner_kind(module, token)?),
                )
            }
            0x74 | 0x75 => effect(1, Some(Object)),
            0x79 => effect(1, Some(ManagedPointer)),
            0x7A => clears(effect(1, None)),
            0x7B => effect(1, Some(self.field_kind(module, token)?)),
            0x7C => effect(1, Some(ManagedPointer)),
            0x7D => effect(2, None),
            0x7E => effect(0, Some(self.field_kind(module, token)?)),
            0x7F => effect(0, Some(ManagedPointer)),
            0x80 => effect(1, None),
            0x81 => effect(2, None),
            0x8C => effect(1, Some(Object)),
            0x8D => effect(1, Some(Object)),
            0x8E => effect(1, Some(NativeInt)),
            0x8F => effect(2, Some(ManagedPointer)),
            0x90..=0x95 => effect(2, Some(Int32)),
            0x96 => effect(2, Some(Int64)),
            0x97 => effect(2, Some(NativeInt)),
            0x98 | 0x99 => effect(2, Some(Float)),
            0x9A => effect(2, Some(Object)),
            0x9B..=0xA2 => effect(3, None),
            0xA3 => effect(2, Some(self.type_token_kind(module, token)?)),
            0xA4 => effect(3, None),
            0xA5 => effect(1, Some(self.type_token_kind(module, token)?)),
            0xC2 => effect(1, Some(ManagedPointer)),
            0xC6 => effect(1, Some(Struct)),
            0xD0 => effect(0, Some(Struct)),
            0xDC => clears(effect(0, None)),
            0xDD | 0xDE => ends(effect(0, None)),
            0xFE00 => effect(0, Some(Struct)),
            0xFE01..=0xFE05 => effect(2, Some(Int32)),
            0xFE06 => effect(0, Some(NativeInt)),
            0xFE07 => effect(1, Some(NativeInt)),
            0xFE0F => effect(1, Some(NativeInt)),
            0xFE11 => clears(effect(1, None)),
            0xFE12 | 0xFE13 | 0xFE14 | 0xFE16 | 0xFE19 | 0xFE1E => effect(0, None),
            0xFE15 => effect(1, None),
            0xFE17 | 0xFE18 => effect(3, None),
            0xFE1A => clears(effect(0, None)),
            0xFE1C => effect(0, Some(Int32)),
            0xFE1D => effect(1, Some(Struct)),
            other => {
                return Err(ResolveError::Malformed(format!(
                    "opcode {other:#06x} has no stack effect"
                )));
            }
        })
    }
}

fn arithmetic(a: Kind, b: Kind) -> Kind {
    use Kind::*;
    match (a, b) {
        (Generic, _) | (_, Generic) => Generic,
        (ManagedPointer, ManagedPointer) => NativeInt,
        (ManagedPointer, _) | (_, ManagedPointer) => ManagedPointer,
        (Float, _) | (_, Float) => Float,
        (Int64, _) | (_, Int64) => Int64,
        (NativeInt, _) | (_, NativeInt) => NativeInt,
        _ => Int32,
    }
}

fn is_branch(opcode: u16) -> bool {
    matches!(opcode, 0x2B..=0x45 | 0xDD | 0xDE)
}

fn ends_flow(opcode: u16) -> bool {
    matches!(opcode, 0x27 | 0x2A | 0x7A | 0xDC | 0xFE11 | 0xFE1A)
}

fn is_void(sig: &TypeSig) -> bool {
    match sig {
        TypeSig::Void => true,
        TypeSig::Modified { inner, .. } => is_void(inner),
        _ => false,
    }
}

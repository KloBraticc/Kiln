pub mod asm;

use std::collections::HashMap;

use asm::{
    ARGUMENT_REGISTERS, Absolute, AbsoluteTarget, Alu, Assembler, Cond, Label, RAX, RBP, RCX, RDX,
    RSP, Shift, Sse, Unary,
};

use crate::ir::{IrArgument, IrInstruction, IrMethod, TranslateError, Translator, ValueId};
use crate::loader::AssemblySet;
use crate::members::Member;
use crate::metadata::{TableId, Token};
use crate::signature::{CallingConvention, MethodSig, Signature, TypeSig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VarType {
    I8,
    U8,
    I16,
    U16,
    I32,
    I64,
    F32,
    F64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Width {
    Int32,
    Int64,
    Float32,
    Float64,
}

impl Width {
    fn is_float(self) -> bool {
        matches!(self, Width::Float32 | Width::Float64)
    }

    fn wide(self) -> bool {
        matches!(self, Width::Int64 | Width::Float64)
    }
}

impl VarType {
    fn width(self) -> Width {
        match self {
            VarType::I64 => Width::Int64,
            VarType::F32 => Width::Float32,
            VarType::F64 => Width::Float64,
            _ => Width::Int32,
        }
    }

    pub fn from_sig(sig: &TypeSig) -> Option<VarType> {
        use TypeSig as T;
        Some(match sig {
            T::Boolean | T::U1 => VarType::U8,
            T::I1 => VarType::I8,
            T::I2 => VarType::I16,
            T::U2 | T::Char => VarType::U16,
            T::I4 | T::U4 => VarType::I32,
            T::I8 | T::U8 | T::IntPtr | T::UIntPtr => VarType::I64,
            T::R4 => VarType::F32,
            T::R8 => VarType::F64,
            T::Modified { inner, .. } => return VarType::from_sig(inner),
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodKey {
    pub module: usize,
    pub row: u32,
}

#[derive(Debug, Clone)]
pub struct CompiledMethod {
    pub code: Vec<u8>,
    pub absolutes: Vec<Absolute>,
    pub callees: Vec<MethodKey>,
    pub parameters: Vec<VarType>,
    pub returns: Option<VarType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    Unsupported(String),
    Translate(String),
}

impl From<TranslateError> for CompileError {
    fn from(error: TranslateError) -> Self {
        CompileError::Translate(error.to_string())
    }
}

impl From<crate::error::Error> for CompileError {
    fn from(error: crate::error::Error) -> Self {
        CompileError::Translate(error.to_string())
    }
}

fn unsupported<T>(what: impl Into<String>) -> Result<T, CompileError> {
    Err(CompileError::Unsupported(what.into()))
}

pub struct Helpers {
    pub float_to_int: [u64; 8],
    pub uint64_to_double: u64,
    pub remainder32: u64,
    pub remainder64: u64,
}

pub fn method_types(
    set: &AssemblySet,
    key: &MethodKey,
) -> Result<(Vec<VarType>, Option<VarType>), CompileError> {
    let metadata = &set.modules[key.module].metadata;
    let Signature::Method(sig) =
        Signature::parse(metadata.blob(metadata.cell(TableId::MethodDef, key.row, 4)?)?)?
    else {
        return unsupported("signature is not a method");
    };
    check_signature(&sig)?;
    let parameters = sig
        .parameters
        .iter()
        .map(|p| {
            VarType::from_sig(p)
                .ok_or_else(|| CompileError::Unsupported(format!("parameter type {p:?}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let returns = match &sig.return_type {
        TypeSig::Void => None,
        TypeSig::Modified { inner, .. } if **inner == TypeSig::Void => None,
        other => Some(
            VarType::from_sig(other)
                .ok_or_else(|| CompileError::Unsupported(format!("return type {other:?}")))?,
        ),
    };
    Ok((parameters, returns))
}

fn check_signature(sig: &MethodSig) -> Result<(), CompileError> {
    if sig.has_this {
        return unsupported("instance methods need the object model");
    }
    if sig.generic_parameters != 0 {
        return unsupported("generic methods");
    }
    if sig.convention != CallingConvention::Default {
        return unsupported("non default calling convention");
    }
    Ok(())
}

struct Frame {
    values: usize,
    arguments: usize,
    locals: usize,
}

impl Frame {
    fn value(&self, id: ValueId) -> i32 {
        -8 * (id.0 as i32 + 1)
    }

    fn argument(&self, index: usize) -> i32 {
        -8 * ((self.values + index) as i32 + 1)
    }

    fn local(&self, index: usize) -> i32 {
        -8 * ((self.values + self.arguments + index) as i32 + 1)
    }

    fn temporary(&self, index: usize) -> i32 {
        -8 * ((self.values + self.arguments + self.locals + index) as i32 + 1)
    }
}

type IntrinsicRows = (
    std::collections::HashSet<u32>,
    std::collections::HashSet<u32>,
);

pub struct Compiler<'s> {
    set: &'s AssemblySet,
    translator: Translator<'s>,
    helpers: &'s Helpers,
    intrinsics: std::cell::RefCell<HashMap<usize, IntrinsicRows>>,
}

struct Lowering<'a> {
    asm: Assembler,
    frame: Frame,
    widths: Vec<Option<Width>>,
    parameters: Vec<VarType>,
    locals: Vec<VarType>,
    returns: Option<VarType>,
    callee_types: Vec<(Vec<VarType>, Option<VarType>)>,
    helpers: &'a Helpers,
}

impl<'s> Compiler<'s> {
    pub fn new(set: &'s AssemblySet, helpers: &'s Helpers) -> Self {
        Self {
            set,
            translator: Translator::new(set),
            helpers,
            intrinsics: std::cell::RefCell::new(HashMap::new()),
        }
    }

    fn resolve_call(&self, module: usize, token: Token) -> Result<MethodKey, CompileError> {
        match token.table() {
            Some(TableId::MethodDef) => Ok(MethodKey {
                module,
                row: token.row(),
            }),
            Some(TableId::MemberRef) => match self.set.resolve_member_ref(module, token.row()) {
                Ok(Member::Method(handle)) if handle.token.table() == Some(TableId::MethodDef) => {
                    Ok(MethodKey {
                        module: handle.module,
                        row: handle.token.row(),
                    })
                }
                Ok(_) => unsupported("call to a non method member"),
                Err(error) => unsupported(format!("call target does not resolve: {error:?}")),
            },
            _ => unsupported("generic method instantiation calls"),
        }
    }

    fn is_intrinsic(&self, key: &MethodKey) -> bool {
        let mut cache = self.intrinsics.borrow_mut();
        let (methods, types) = cache.entry(key.module).or_insert_with(|| {
            let scan = |table| {
                self.set
                    .rows_with_attribute(
                        key.module,
                        table,
                        "System.Runtime.CompilerServices",
                        "IntrinsicAttribute",
                    )
                    .unwrap_or_default()
            };
            (scan(TableId::MethodDef), scan(TableId::TypeDef))
        });
        if methods.contains(&key.row) {
            return true;
        }
        let module = &self.set.modules[key.module];
        let mut owner = module.method_owner(key.row).ok().flatten();
        while let Some(row) = owner {
            if types.contains(&row) {
                return true;
            }
            owner = module.enclosing_type(row);
        }
        false
    }

    pub fn compile(&self, key: &MethodKey) -> Result<CompiledMethod, CompileError> {
        let metadata = &self.set.modules[key.module].metadata;
        let (parameters, returns) = method_types(self.set, key)?;
        let Some(ir) = self.translator.translate(key.module, key.row)? else {
            return unsupported("method has no IL body");
        };
        let rva = metadata.cell(TableId::MethodDef, key.row, 0)?;
        let body = crate::il::MethodBody::parse(
            self.set.modules[key.module].image.tail_at_rva(rva)?,
            rva,
        )?;
        if !body.clauses.is_empty() {
            return unsupported("exception handling");
        }
        let mut locals = Vec::new();
        if !body.local_signature.is_null() {
            let blob = metadata.blob(metadata.cell(
                TableId::StandAloneSig,
                body.local_signature.row(),
                0,
            )?)?;
            if let Signature::Locals(types) = Signature::parse(blob)? {
                for local in &types {
                    locals.push(VarType::from_sig(local).ok_or_else(|| {
                        CompileError::Unsupported(format!("local type {local:?}"))
                    })?);
                }
            }
        }

        let mut callees = Vec::new();
        let mut callee_types = Vec::new();
        let mut call_index: HashMap<Token, usize> = HashMap::new();
        for block in &ir.blocks {
            for instruction in &block.instructions {
                if instruction.opcode == 0x28 {
                    let IrArgument::Token(token) = instruction.argument else {
                        continue;
                    };
                    if call_index.contains_key(&token) {
                        continue;
                    }
                    let callee = self.resolve_call(key.module, token)?;
                    if callee == *key && self.is_intrinsic(key) {
                        return unsupported("JIT intrinsic");
                    }
                    let types = method_types(self.set, &callee)?;
                    call_index.insert(token, callees.len());
                    callees.push(callee);
                    callee_types.push(types);
                }
            }
        }

        let widths = infer_widths(&ir, &parameters, &locals, &call_index, &callee_types)?;
        let max_parameters = ir
            .blocks
            .iter()
            .map(|b| b.parameters.len())
            .max()
            .unwrap_or(0);
        let max_outgoing = callee_types
            .iter()
            .map(|(p, _)| p.len().saturating_sub(4))
            .max()
            .unwrap_or(0);
        let frame = Frame {
            values: ir.values as usize,
            arguments: parameters.len(),
            locals: locals.len(),
        };
        let slots = frame.values + frame.arguments + frame.locals + max_parameters;
        let frame_size = (8 * slots as u32 + 32 + 8 * max_outgoing as u32).next_multiple_of(16);

        let mut lowering = Lowering {
            asm: Assembler::default(),
            frame,
            widths,
            parameters: parameters.clone(),
            locals,
            returns,
            callee_types,
            helpers: self.helpers,
        };
        lowering.emit(&ir, frame_size, &call_index)?;
        let (code, absolutes) = lowering.asm.finish().map_err(CompileError::Unsupported)?;
        Ok(CompiledMethod {
            code,
            absolutes,
            callees,
            parameters,
            returns,
        })
    }
}

fn output_width(
    instruction: &IrInstruction,
    inputs: &[Width],
    parameters: &[VarType],
    locals: &[VarType],
    call_index: &HashMap<Token, usize>,
    callee_types: &[(Vec<VarType>, Option<VarType>)],
) -> Result<Option<Width>, CompileError> {
    let variable = match instruction.argument {
        IrArgument::Variable(index) => index as usize,
        _ => 0,
    };
    let parameter = |index: usize| {
        parameters
            .get(index)
            .map(|v| v.width())
            .ok_or_else(|| CompileError::Unsupported("argument index".into()))
    };
    let local = |index: usize| {
        locals
            .get(index)
            .map(|v| v.width())
            .ok_or_else(|| CompileError::Unsupported("local index".into()))
    };
    let integer = |a: Width, b: Width| -> Result<Width, CompileError> {
        if a.is_float() || b.is_float() {
            return unsupported("mixed integer and float arithmetic");
        }
        Ok(if a == Width::Int64 || b == Width::Int64 {
            Width::Int64
        } else {
            Width::Int32
        })
    };
    Ok(Some(match instruction.opcode {
        0x02..=0x05 => parameter((instruction.opcode - 0x02) as usize)?,
        0x0E | 0xFE09 => parameter(variable)?,
        0x06..=0x09 => local((instruction.opcode - 0x06) as usize)?,
        0x11 | 0xFE0C => local(variable)?,
        0x15..=0x20 => Width::Int32,
        0x21 => Width::Int64,
        0x22 => Width::Float32,
        0x23 => Width::Float64,
        0x58..=0x5E | 0xD6..=0xDB => {
            let (a, b) = (inputs[0], inputs[1]);
            if a.is_float() && b.is_float() {
                if a == Width::Float32 && b == Width::Float32 {
                    Width::Float32
                } else {
                    Width::Float64
                }
            } else {
                integer(a, b)?
            }
        }
        0x5F..=0x61 => integer(inputs[0], inputs[1])?,
        0x62..=0x64 => {
            if inputs[0].is_float() || inputs[1].is_float() {
                return unsupported("float shift");
            }
            inputs[0]
        }
        0x65 => inputs[0],
        0x66 => {
            if inputs[0].is_float() {
                return unsupported("float not");
            }
            inputs[0]
        }
        0x67..=0x69 | 0x6D | 0x82..=0x84 | 0x86..=0x88 | 0xB3..=0xB8 | 0xD1 | 0xD2 => Width::Int32,
        0x6A | 0x6E | 0x85 | 0x89 | 0x8A | 0x8B | 0xB9 | 0xBA | 0xD3..=0xD5 | 0xE0 => Width::Int64,
        0x6B => Width::Float32,
        0x6C | 0x76 => Width::Float64,
        0xFE01..=0xFE05 => Width::Int32,
        0x28 => {
            let IrArgument::Token(token) = instruction.argument else {
                return unsupported("call without token");
            };
            match callee_types[call_index[&token]].1 {
                Some(returned) => returned.width(),
                None => return Ok(None),
            }
        }
        _ => return Ok(None),
    }))
}

fn infer_widths(
    ir: &IrMethod,
    parameters: &[VarType],
    locals: &[VarType],
    call_index: &HashMap<Token, usize>,
    callee_types: &[(Vec<VarType>, Option<VarType>)],
) -> Result<Vec<Option<Width>>, CompileError> {
    let mut widths: Vec<Option<Width>> = vec![None; ir.values as usize];
    for _ in 0..ir.blocks.len() + 2 {
        let mut changed = false;
        for block in &ir.blocks {
            if block
                .parameters
                .iter()
                .any(|(id, _)| widths[id.0 as usize].is_none())
            {
                continue;
            }
            for instruction in &block.instructions {
                let Some(inputs) = instruction
                    .inputs
                    .iter()
                    .map(|v| widths[v.0 as usize])
                    .collect::<Option<Vec<_>>>()
                else {
                    break;
                };
                if let Some((id, _)) = instruction.output
                    && widths[id.0 as usize].is_none()
                    && let Some(width) = output_width(
                        instruction,
                        &inputs,
                        parameters,
                        locals,
                        call_index,
                        callee_types,
                    )?
                {
                    widths[id.0 as usize] = Some(width);
                    changed = true;
                }
            }
            for (target, carried) in &block.successors {
                for (slot, value) in ir.blocks[*target].parameters.iter().zip(carried) {
                    if widths[slot.0.0 as usize].is_none()
                        && let Some(width) = widths[value.0 as usize]
                    {
                        widths[slot.0.0 as usize] = Some(width);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    Ok(widths)
}

impl<'a> Lowering<'a> {
    fn width_of(&self, value: ValueId) -> Result<Width, CompileError> {
        self.widths[value.0 as usize]
            .ok_or_else(|| CompileError::Unsupported("value width could not be inferred".into()))
    }

    fn load_int(&mut self, reg: u8, value: ValueId, wide: bool) -> Result<(), CompileError> {
        let width = self.width_of(value)?;
        if width.is_float() {
            return unsupported("float used as integer");
        }
        let slot = self.frame.value(value);
        if wide && width == Width::Int32 {
            self.asm.load_sign_extend_32(reg, RBP, slot);
        } else {
            self.asm.load(wide, reg, RBP, slot);
        }
        Ok(())
    }

    fn load_float(&mut self, xmm: u8, value: ValueId, double: bool) -> Result<(), CompileError> {
        let width = self.width_of(value)?;
        if !width.is_float() {
            return unsupported("integer used as float");
        }
        let slot = self.frame.value(value);
        self.asm.sse_load(width == Width::Float64, xmm, RBP, slot);
        match (width, double) {
            (Width::Float32, true) => self.asm.float_to_double(xmm, xmm),
            (Width::Float64, false) => self.asm.double_to_float(xmm, xmm),
            _ => {}
        }
        Ok(())
    }

    fn store_result(&mut self, value: ValueId, reg_or_xmm: u8) -> Result<(), CompileError> {
        let width = self.width_of(value)?;
        let slot = self.frame.value(value);
        if width.is_float() {
            self.asm
                .sse_store(width == Width::Float64, RBP, slot, reg_or_xmm);
        } else {
            self.asm.store(width.wide(), RBP, slot, reg_or_xmm);
        }
        Ok(())
    }

    fn normalize(&mut self, reg: u8, var: VarType) {
        match var {
            VarType::I8 => self.asm.sign_extend_byte(reg, reg),
            VarType::U8 => self.asm.zero_extend_byte(reg, reg),
            VarType::I16 => self.asm.sign_extend_word(reg, reg),
            VarType::U16 => self.asm.zero_extend_word(reg, reg),
            _ => {}
        }
    }

    fn load_variable(
        &mut self,
        var: VarType,
        slot: i32,
        output: ValueId,
    ) -> Result<(), CompileError> {
        match var {
            VarType::F32 | VarType::F64 => {
                self.asm.sse_load(var == VarType::F64, 0, RBP, slot);
                self.store_result(output, 0)
            }
            VarType::I64 => {
                self.asm.load(true, RAX, RBP, slot);
                self.store_result(output, RAX)
            }
            _ => {
                self.asm.load(false, RAX, RBP, slot);
                self.normalize(RAX, var);
                self.store_result(output, RAX)
            }
        }
    }

    fn convert_into_register(
        &mut self,
        value: ValueId,
        var: VarType,
        reg_or_xmm: u8,
    ) -> Result<(), CompileError> {
        match var {
            VarType::F32 | VarType::F64 => self.load_float(reg_or_xmm, value, var == VarType::F64),
            VarType::I64 => self.load_int(reg_or_xmm, value, true),
            _ => {
                self.load_int(reg_or_xmm, value, false)?;
                self.normalize(reg_or_xmm, var);
                Ok(())
            }
        }
    }

    fn store_variable(
        &mut self,
        value: ValueId,
        var: VarType,
        slot: i32,
    ) -> Result<(), CompileError> {
        self.convert_into_register(value, var, 0)?;
        match var {
            VarType::F32 | VarType::F64 => self.asm.sse_store(var == VarType::F64, RBP, slot, 0),
            VarType::I64 => self.asm.store(true, RBP, slot, RAX),
            _ => self.asm.store(false, RBP, slot, RAX),
        }
        Ok(())
    }

    fn call_address(&mut self, target: AbsoluteTarget) {
        self.asm.move_absolute(RAX, target);
        self.asm.call_register(RAX);
    }

    fn edge(
        &mut self,
        ir: &IrMethod,
        from: usize,
        target: usize,
        labels: &[Label],
    ) -> Result<(), CompileError> {
        let parameters = ir.blocks[target].parameters.clone();
        let carried = ir.blocks[from]
            .successors
            .iter()
            .find(|(t, _)| *t == target)
            .map(|(_, c)| c.clone())
            .unwrap_or_default();
        if parameters.len() != carried.len() {
            return unsupported("edge value count mismatch");
        }
        for (index, value) in carried.iter().enumerate() {
            let slot = self.frame.value(*value);
            self.asm.load(true, RAX, RBP, slot);
            let temporary = self.frame.temporary(index);
            self.asm.store(true, RBP, temporary, RAX);
        }
        for (index, (parameter, _)) in parameters.iter().enumerate() {
            let temporary = self.frame.temporary(index);
            self.asm.load(true, RAX, RBP, temporary);
            let slot = self.frame.value(*parameter);
            self.asm.store(true, RBP, slot, RAX);
        }
        self.asm.jump(labels[target]);
        Ok(())
    }

    fn emit(
        &mut self,
        ir: &IrMethod,
        frame_size: u32,
        call_index: &HashMap<Token, usize>,
    ) -> Result<(), CompileError> {
        self.asm.prologue(frame_size);
        let parameters = self.parameters.clone();
        for (index, var) in parameters.iter().enumerate() {
            let slot = self.frame.argument(index);
            if index < 4 {
                if matches!(var, VarType::F32 | VarType::F64) {
                    self.asm
                        .sse_store(*var == VarType::F64, RBP, slot, index as u8);
                } else {
                    self.asm.store(true, RBP, slot, ARGUMENT_REGISTERS[index]);
                }
            } else {
                self.asm.load(true, RAX, RBP, 48 + 8 * (index as i32 - 4));
                self.asm.store(true, RBP, slot, RAX);
            }
        }
        let locals = self.locals.clone();
        for index in 0..locals.len() {
            self.asm.alu(Alu::Xor, true, RAX, RAX);
            let slot = self.frame.local(index);
            self.asm.store(true, RBP, slot, RAX);
        }

        let labels: Vec<Label> = (0..ir.blocks.len()).map(|_| self.asm.label()).collect();
        let reachable: Vec<bool> = ir
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| i == 0 || !b.instructions.is_empty())
            .collect();
        for (index, block) in ir.blocks.iter().enumerate() {
            if !reachable[index] {
                continue;
            }
            if block.handler_entry {
                return unsupported("exception handlers");
            }
            self.asm.bind(labels[index]);
            let mut terminated = false;
            for instruction in &block.instructions {
                terminated = self.instruction(ir, index, instruction, &labels, call_index)?;
            }
            if !terminated {
                if index + 1 >= ir.blocks.len() {
                    return unsupported("falls off the end of the method");
                }
                self.edge(ir, index, index + 1, &labels)?;
            }
        }
        Ok(())
    }

    fn compare_float(&mut self, a: ValueId, b: ValueId, swap: bool) -> Result<(), CompileError> {
        let double = self.width_of(a)? == Width::Float64 || self.width_of(b)? == Width::Float64;
        self.load_float(0, a, double)?;
        self.load_float(1, b, double)?;
        if swap {
            self.asm.sse_compare(double, 1, 0);
        } else {
            self.asm.sse_compare(double, 0, 1);
        }
        Ok(())
    }

    fn compare_int(&mut self, a: ValueId, b: ValueId) -> Result<(), CompileError> {
        let wide = self.width_of(a)? == Width::Int64 || self.width_of(b)? == Width::Int64;
        self.load_int(RAX, a, wide)?;
        self.load_int(RCX, b, wide)?;
        self.asm.alu(Alu::Cmp, wide, RAX, RCX);
        Ok(())
    }

    fn instruction(
        &mut self,
        ir: &IrMethod,
        block: usize,
        instruction: &IrInstruction,
        labels: &[Label],
        call_index: &HashMap<Token, usize>,
    ) -> Result<bool, CompileError> {
        let inputs = &instruction.inputs;
        let output = instruction.output.map(|o| o.0);
        let out = || output.ok_or_else(|| CompileError::Unsupported("missing output".into()));
        let variable = match instruction.argument {
            IrArgument::Variable(index) => index as usize,
            _ => 0,
        };
        match instruction.opcode {
            0x00 | 0x25 | 0x26 => {}
            0x02..=0x05 | 0x0E | 0xFE09 => {
                let index = if instruction.opcode <= 0x05 {
                    (instruction.opcode - 0x02) as usize
                } else {
                    variable
                };
                let var = self.parameters[index];
                let slot = self.frame.argument(index);
                self.load_variable(var, slot, out()?)?;
            }
            0x06..=0x09 | 0x11 | 0xFE0C => {
                let index = if instruction.opcode <= 0x09 {
                    (instruction.opcode - 0x06) as usize
                } else {
                    variable
                };
                let var = *self
                    .locals
                    .get(index)
                    .ok_or_else(|| CompileError::Unsupported("local index".into()))?;
                let slot = self.frame.local(index);
                self.load_variable(var, slot, out()?)?;
            }
            0x0A..=0x0D | 0x13 | 0xFE0E => {
                let index = if instruction.opcode <= 0x0D {
                    (instruction.opcode - 0x0A) as usize
                } else {
                    variable
                };
                let var = *self
                    .locals
                    .get(index)
                    .ok_or_else(|| CompileError::Unsupported("local index".into()))?;
                let slot = self.frame.local(index);
                self.store_variable(inputs[0], var, slot)?;
            }
            0x10 | 0xFE0B => {
                let var = self.parameters[variable];
                let slot = self.frame.argument(variable);
                self.store_variable(inputs[0], var, slot)?;
            }
            0x15..=0x21 => {
                let IrArgument::Int(value) = (match instruction.opcode {
                    0x15..=0x1E => IrArgument::Int(instruction.opcode as i64 - 0x16),
                    _ => instruction.argument.clone(),
                }) else {
                    return unsupported("constant without value");
                };
                self.asm.move_immediate(RAX, value as u64);
                self.store_result(out()?, RAX)?;
            }
            0x22 | 0x23 => {
                let IrArgument::Float(value) = instruction.argument else {
                    return unsupported("float constant without value");
                };
                if instruction.opcode == 0x22 {
                    self.asm
                        .move_immediate(RAX, u64::from((value as f32).to_bits()));
                    self.asm.xmm_from_gpr(false, 0, RAX);
                } else {
                    self.asm.move_immediate(RAX, value.to_bits());
                    self.asm.xmm_from_gpr(true, 0, RAX);
                }
                self.store_result(out()?, 0)?;
            }
            0x58..=0x5E | 0xD6..=0xDB => {
                self.arithmetic(instruction.opcode, inputs[0], inputs[1], out()?)?
            }
            0x5F..=0x61 => {
                let output = out()?;
                let wide = self.width_of(output)? == Width::Int64;
                self.load_int(RAX, inputs[0], wide)?;
                self.load_int(RCX, inputs[1], wide)?;
                let operation = match instruction.opcode {
                    0x5F => Alu::And,
                    0x60 => Alu::Or,
                    _ => Alu::Xor,
                };
                self.asm.alu(operation, wide, RAX, RCX);
                self.store_result(output, RAX)?;
            }
            0x62..=0x64 => {
                let output = out()?;
                let wide = self.width_of(output)? == Width::Int64;
                self.load_int(RAX, inputs[0], wide)?;
                self.load_int(RCX, inputs[1], false)?;
                let operation = match instruction.opcode {
                    0x62 => Shift::Left,
                    0x63 => Shift::RightArithmetic,
                    _ => Shift::RightLogical,
                };
                self.asm.shift(operation, wide, RAX);
                self.store_result(output, RAX)?;
            }
            0x65 => {
                let output = out()?;
                let width = self.width_of(output)?;
                if width.is_float() {
                    let double = width == Width::Float64;
                    self.load_float(0, inputs[0], double)?;
                    let mask = if double {
                        0x8000_0000_0000_0000
                    } else {
                        0x8000_0000
                    };
                    self.asm.move_immediate(RAX, mask);
                    self.asm.xmm_from_gpr(double, 1, RAX);
                    self.asm.xor_packed(double, 0, 1);
                    self.store_result(output, 0)?;
                } else {
                    self.load_int(RAX, inputs[0], width.wide())?;
                    self.asm.unary(Unary::Neg, width.wide(), RAX);
                    self.store_result(output, RAX)?;
                }
            }
            0x66 => {
                let output = out()?;
                let wide = self.width_of(output)?.wide();
                self.load_int(RAX, inputs[0], wide)?;
                self.asm.unary(Unary::Not, wide, RAX);
                self.store_result(output, RAX)?;
            }
            0x67..=0x6E | 0x76 | 0x82..=0x8B | 0xB3..=0xBA | 0xD1..=0xD5 | 0xE0 => {
                self.convert(instruction.opcode, inputs[0], out()?)?
            }
            0xFE01..=0xFE05 => {
                let output = out()?;
                let (a, b) = (inputs[0], inputs[1]);
                if self.width_of(a)?.is_float() {
                    match instruction.opcode {
                        0xFE01 => {
                            self.compare_float(a, b, false)?;
                            self.asm.set(Cond::Equal, RAX);
                            self.asm.set(Cond::NoParity, RCX);
                            self.asm.alu(Alu::And, false, RAX, RCX);
                        }
                        0xFE02 => {
                            self.compare_float(a, b, false)?;
                            self.asm.set(Cond::Above, RAX);
                        }
                        0xFE03 => {
                            self.compare_float(a, b, true)?;
                            self.asm.set(Cond::Below, RAX);
                        }
                        0xFE04 => {
                            self.compare_float(a, b, true)?;
                            self.asm.set(Cond::Above, RAX);
                        }
                        _ => {
                            self.compare_float(a, b, false)?;
                            self.asm.set(Cond::Below, RAX);
                        }
                    }
                } else {
                    self.compare_int(a, b)?;
                    let condition = match instruction.opcode {
                        0xFE01 => Cond::Equal,
                        0xFE02 => Cond::Greater,
                        0xFE03 => Cond::Above,
                        0xFE04 => Cond::Less,
                        _ => Cond::Below,
                    };
                    self.asm.set(condition, RAX);
                }
                self.asm.zero_extend_byte(RAX, RAX);
                self.store_result(output, RAX)?;
            }
            0x2B | 0x38 => {
                let target = single_target(instruction)?;
                self.edge(ir, block, target, labels)?;
                return Ok(true);
            }
            0x2C | 0x2D | 0x39 | 0x3A => {
                let target = single_target(instruction)?;
                let wide = self.width_of(inputs[0])?.wide();
                self.load_int(RAX, inputs[0], wide)?;
                self.asm.test(wide, RAX, RAX);
                let taken = self.asm.label();
                let condition = if matches!(instruction.opcode, 0x2C | 0x39) {
                    Cond::Equal
                } else {
                    Cond::NotEqual
                };
                self.asm.jump_if(condition, taken);
                self.edge(ir, block, block + 1, labels)?;
                self.asm.bind(taken);
                self.edge(ir, block, target, labels)?;
                return Ok(true);
            }
            0x2E..=0x37 | 0x3B..=0x44 => {
                let target = single_target(instruction)?;
                let kind = if instruction.opcode >= 0x3B {
                    instruction.opcode - 0x3B
                } else {
                    instruction.opcode - 0x2E
                };
                let taken = self.asm.label();
                let (a, b) = (inputs[0], inputs[1]);
                if self.width_of(a)?.is_float() {
                    let skip = self.asm.label();
                    match kind {
                        0 => {
                            self.compare_float(a, b, false)?;
                            self.asm.jump_if(Cond::Parity, skip);
                            self.asm.jump_if(Cond::Equal, taken);
                        }
                        1 => {
                            self.compare_float(a, b, false)?;
                            self.asm.jump_if(Cond::AboveOrEqual, taken);
                        }
                        2 => {
                            self.compare_float(a, b, false)?;
                            self.asm.jump_if(Cond::Above, taken);
                        }
                        3 => {
                            self.compare_float(a, b, true)?;
                            self.asm.jump_if(Cond::AboveOrEqual, taken);
                        }
                        4 => {
                            self.compare_float(a, b, true)?;
                            self.asm.jump_if(Cond::Above, taken);
                        }
                        5 => {
                            self.compare_float(a, b, false)?;
                            self.asm.jump_if(Cond::Parity, taken);
                            self.asm.jump_if(Cond::NotEqual, taken);
                        }
                        6 => {
                            self.compare_float(a, b, true)?;
                            self.asm.jump_if(Cond::BelowOrEqual, taken);
                        }
                        7 => {
                            self.compare_float(a, b, true)?;
                            self.asm.jump_if(Cond::Below, taken);
                        }
                        8 => {
                            self.compare_float(a, b, false)?;
                            self.asm.jump_if(Cond::BelowOrEqual, taken);
                        }
                        _ => {
                            self.compare_float(a, b, false)?;
                            self.asm.jump_if(Cond::Below, taken);
                        }
                    }
                    self.asm.bind(skip);
                } else {
                    self.compare_int(a, b)?;
                    let condition = match kind {
                        0 => Cond::Equal,
                        1 => Cond::GreaterOrEqual,
                        2 => Cond::Greater,
                        3 => Cond::LessOrEqual,
                        4 => Cond::Less,
                        5 => Cond::NotEqual,
                        6 => Cond::AboveOrEqual,
                        7 => Cond::Above,
                        8 => Cond::BelowOrEqual,
                        _ => Cond::Below,
                    };
                    self.asm.jump_if(condition, taken);
                }
                self.edge(ir, block, block + 1, labels)?;
                self.asm.bind(taken);
                self.edge(ir, block, target, labels)?;
                return Ok(true);
            }
            0x45 => {
                let IrArgument::Targets(targets) = &instruction.argument else {
                    return unsupported("switch without targets");
                };
                self.load_int(RAX, inputs[0], false)?;
                let stubs: Vec<Label> = targets.iter().map(|_| self.asm.label()).collect();
                for (case, stub) in stubs.iter().enumerate() {
                    self.asm.move_immediate(RCX, case as u64);
                    self.asm.alu(Alu::Cmp, false, RAX, RCX);
                    self.asm.jump_if(Cond::Equal, *stub);
                }
                self.edge(ir, block, block + 1, labels)?;
                for (stub, target) in stubs.iter().zip(targets) {
                    self.asm.bind(*stub);
                    self.edge(ir, block, *target, labels)?;
                }
                return Ok(true);
            }
            0x2A => {
                if let Some(returns) = self.returns {
                    self.convert_into_register(inputs[0], returns, 0)?;
                }
                self.asm.epilogue();
                return Ok(true);
            }
            0x28 => {
                let IrArgument::Token(token) = instruction.argument else {
                    return unsupported("call without token");
                };
                let index = call_index[&token];
                let (parameters, returns) = self.callee_types[index].clone();
                for (position, (value, var)) in inputs.iter().zip(&parameters).enumerate().skip(4) {
                    let is_float = matches!(var, VarType::F32 | VarType::F64);
                    self.convert_into_register(*value, *var, 0)?;
                    let displacement = 32 + 8 * (position as i32 - 4);
                    if is_float {
                        self.asm
                            .sse_store(*var == VarType::F64, RSP, displacement, 0);
                    } else {
                        self.asm.store(true, RSP, displacement, RAX);
                    }
                }
                for (position, (value, var)) in inputs.iter().zip(&parameters).enumerate().take(4) {
                    if matches!(var, VarType::F32 | VarType::F64) {
                        self.convert_into_register(*value, *var, position as u8)?;
                    } else {
                        self.convert_into_register(*value, *var, ARGUMENT_REGISTERS[position])?;
                    }
                }
                self.call_address(AbsoluteTarget::Method(index));
                if let (Some(returned), Some(output)) = (returns, output) {
                    let register = if matches!(returned, VarType::F32 | VarType::F64) {
                        0
                    } else {
                        RAX
                    };
                    self.store_result(output, register)?;
                }
            }
            other => return unsupported(format!("opcode {other:#06x}")),
        }
        Ok(false)
    }

    fn arithmetic(
        &mut self,
        opcode: u16,
        a: ValueId,
        b: ValueId,
        output: ValueId,
    ) -> Result<(), CompileError> {
        let width = self.width_of(output)?;
        if width.is_float() {
            let double = width == Width::Float64;
            self.load_float(0, a, double)?;
            self.load_float(1, b, double)?;
            let operation = match opcode {
                0x58 | 0xD6 => Sse::Add,
                0x59 | 0xDA => Sse::Sub,
                0x5A | 0xD8 => Sse::Mul,
                0x5B => Sse::Div,
                0x5D => {
                    let helper = if double {
                        self.helpers.remainder64
                    } else {
                        self.helpers.remainder32
                    };
                    self.call_address(AbsoluteTarget::Address(helper));
                    return self.store_result(output, 0);
                }
                _ => return unsupported("unsigned float arithmetic"),
            };
            self.asm.sse_arithmetic(operation, double, 0, 1);
            return self.store_result(output, 0);
        }
        let wide = width.wide();
        self.load_int(RAX, a, wide)?;
        self.load_int(RCX, b, wide)?;
        match opcode {
            0x58 | 0xD6 | 0xD7 => self.asm.alu(Alu::Add, wide, RAX, RCX),
            0x59 | 0xDA | 0xDB => self.asm.alu(Alu::Sub, wide, RAX, RCX),
            0x5A | 0xD8 | 0xD9 => self.asm.imul(wide, RAX, RCX),
            0x5B | 0x5D => {
                self.asm.sign_extend_accumulator(wide);
                self.asm.unary(Unary::Idiv, wide, RCX);
                if opcode == 0x5D {
                    self.asm.move_register(wide, RAX, RDX);
                }
            }
            0x5C | 0x5E => {
                self.asm.alu(Alu::Xor, false, RDX, RDX);
                self.asm.unary(Unary::Div, wide, RCX);
                if opcode == 0x5E {
                    self.asm.move_register(wide, RAX, RDX);
                }
            }
            _ => return unsupported("arithmetic opcode"),
        }
        self.store_result(output, RAX)
    }

    fn convert(
        &mut self,
        opcode: u16,
        input: ValueId,
        output: ValueId,
    ) -> Result<(), CompileError> {
        let from = self.width_of(input)?;
        let target: VarType = match opcode {
            0x67 | 0x82 | 0xB3 => VarType::I8,
            0xD2 | 0x86 | 0xB4 => VarType::U8,
            0x68 | 0x83 | 0xB5 => VarType::I16,
            0xD1 | 0x87 | 0xB6 => VarType::U16,
            0x69 | 0x84 | 0xB7 | 0x6D | 0x88 | 0xB8 => VarType::I32,
            0x6B => VarType::F32,
            0x6C | 0x76 => VarType::F64,
            _ => VarType::I64,
        };
        let unsigned_target = matches!(
            opcode,
            0x6D | 0x88 | 0xB8 | 0x6E | 0x89 | 0xBA | 0xE0 | 0x8B | 0xD5
        );
        let unsigned_source = matches!(opcode, 0x76 | 0x82..=0x8B);
        if from.is_float() {
            match target {
                VarType::F32 | VarType::F64 => {
                    self.load_float(0, input, target == VarType::F64)?;
                    return self.store_result(output, 0);
                }
                _ => {
                    self.load_float(0, input, true)?;
                    let helper = match (target, unsigned_target) {
                        (VarType::I8, _) => self.helpers.float_to_int[0],
                        (VarType::U8, _) => self.helpers.float_to_int[1],
                        (VarType::I16, _) => self.helpers.float_to_int[2],
                        (VarType::U16, _) => self.helpers.float_to_int[3],
                        (VarType::I32, false) => self.helpers.float_to_int[4],
                        (VarType::I32, true) => self.helpers.float_to_int[5],
                        (_, false) => self.helpers.float_to_int[6],
                        (_, true) => self.helpers.float_to_int[7],
                    };
                    self.call_address(AbsoluteTarget::Address(helper));
                    self.normalize(RAX, target);
                    return self.store_result(output, RAX);
                }
            }
        }
        match target {
            VarType::F32 | VarType::F64 => {
                let double = target == VarType::F64;
                if unsigned_source && from == Width::Int64 {
                    self.load_int(RCX, input, true)?;
                    self.call_address(AbsoluteTarget::Address(self.helpers.uint64_to_double));
                    if !double {
                        self.asm.double_to_float(0, 0);
                    }
                } else if unsigned_source {
                    self.load_int(RAX, input, false)?;
                    self.asm.int_to_float(double, true, 0, RAX);
                } else {
                    let wide = from == Width::Int64;
                    self.load_int(RAX, input, wide)?;
                    self.asm.int_to_float(double, wide, 0, RAX);
                }
                self.store_result(output, 0)
            }
            VarType::I64 => {
                let zero_extend = from == Width::Int32
                    && (unsigned_source || (unsigned_target && !matches!(opcode, 0xBA | 0xD5)));
                self.load_int(RAX, input, !zero_extend)?;
                self.store_result(output, RAX)
            }
            _ => {
                self.load_int(RAX, input, false)?;
                self.normalize(RAX, target);
                self.store_result(output, RAX)
            }
        }
    }
}

fn single_target(instruction: &IrInstruction) -> Result<usize, CompileError> {
    match &instruction.argument {
        IrArgument::Targets(targets) if targets.len() == 1 => Ok(targets[0]),
        _ => unsupported("branch without a target"),
    }
}

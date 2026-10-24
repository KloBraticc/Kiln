pub const RAX: u8 = 0;
pub const RCX: u8 = 1;
pub const RDX: u8 = 2;
pub const RSP: u8 = 4;
pub const RBP: u8 = 5;
pub const R8: u8 = 8;
pub const R9: u8 = 9;
pub const R10: u8 = 10;

pub const ARGUMENT_REGISTERS: [u8; 4] = [RCX, RDX, R8, R9];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    Overflow = 0x0,
    Below = 0x2,
    AboveOrEqual = 0x3,
    Equal = 0x4,
    NotEqual = 0x5,
    BelowOrEqual = 0x6,
    Above = 0x7,
    Parity = 0xA,
    NoParity = 0xB,
    Less = 0xC,
    GreaterOrEqual = 0xD,
    LessOrEqual = 0xE,
    Greater = 0xF,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alu {
    Add = 0x01,
    Or = 0x09,
    And = 0x21,
    Sub = 0x29,
    Xor = 0x31,
    Cmp = 0x39,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unary {
    Not = 2,
    Neg = 3,
    Div = 6,
    Idiv = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shift {
    Left = 4,
    RightLogical = 5,
    RightArithmetic = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sse {
    Add = 0x58,
    Mul = 0x59,
    Sub = 0x5C,
    Div = 0x5E,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label(pub usize);

#[derive(Debug, Clone, Copy)]
pub struct Absolute {
    pub at: usize,
    pub target: AbsoluteTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsoluteTarget {
    Method(usize),
    Address(u64),
}

#[derive(Default)]
pub struct Assembler {
    pub code: Vec<u8>,
    labels: Vec<Option<usize>>,
    fixups: Vec<(usize, Label)>,
    pub absolutes: Vec<Absolute>,
}

impl Assembler {
    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() - 1)
    }

    pub fn bind(&mut self, label: Label) {
        self.labels[label.0] = Some(self.code.len());
    }

    pub fn finish(mut self) -> Result<(Vec<u8>, Vec<Absolute>), String> {
        for (at, label) in std::mem::take(&mut self.fixups) {
            let target = self.labels[label.0].ok_or("a jump target was never placed")?;
            let relative = target as i64 - (at as i64 + 4);
            let relative = i32::try_from(relative).map_err(|_| "a jump is out of range")?;
            self.code[at..at + 4].copy_from_slice(&relative.to_le_bytes());
        }
        Ok((self.code, self.absolutes))
    }

    fn byte(&mut self, value: u8) {
        self.code.push(value);
    }

    fn bytes(&mut self, values: &[u8]) {
        self.code.extend_from_slice(values);
    }

    fn rex(&mut self, wide: bool, reg: u8, base: u8, force: bool) {
        let value = 0x40 | (u8::from(wide) << 3) | ((reg >> 3) << 2) | (base >> 3);
        if value != 0x40 || force {
            self.byte(value);
        }
    }

    fn memory(&mut self, reg: u8, base: u8, displacement: i32) {
        self.byte(0x80 | ((reg & 7) << 3) | (base & 7));
        if base & 7 == RSP {
            self.byte(0x24);
        }
        self.bytes(&displacement.to_le_bytes());
    }

    fn direct(&mut self, reg: u8, rm: u8) {
        self.byte(0xC0 | ((reg & 7) << 3) | (rm & 7));
    }

    fn op_memory(
        &mut self,
        prefix: Option<u8>,
        wide: bool,
        opcode: &[u8],
        reg: u8,
        base: u8,
        displacement: i32,
    ) {
        if let Some(prefix) = prefix {
            self.byte(prefix);
        }
        self.rex(wide, reg, base, false);
        self.bytes(opcode);
        self.memory(reg, base, displacement);
    }

    fn op_direct(&mut self, prefix: Option<u8>, wide: bool, opcode: &[u8], reg: u8, rm: u8) {
        if let Some(prefix) = prefix {
            self.byte(prefix);
        }
        self.rex(wide, reg, rm, false);
        self.bytes(opcode);
        self.direct(reg, rm);
    }

    pub fn load(&mut self, wide: bool, reg: u8, base: u8, displacement: i32) {
        self.op_memory(None, wide, &[0x8B], reg, base, displacement);
    }

    pub fn store(&mut self, wide: bool, base: u8, displacement: i32, reg: u8) {
        self.op_memory(None, wide, &[0x89], reg, base, displacement);
    }

    pub fn load_sign_extend_32(&mut self, reg: u8, base: u8, displacement: i32) {
        self.op_memory(None, true, &[0x63], reg, base, displacement);
    }

    pub fn move_register(&mut self, wide: bool, destination: u8, source: u8) {
        self.op_direct(None, wide, &[0x89], source, destination);
    }

    pub fn move_immediate(&mut self, reg: u8, value: u64) {
        self.rex(true, 0, reg, false);
        self.byte(0xB8 + (reg & 7));
        self.bytes(&value.to_le_bytes());
    }

    pub fn move_absolute(&mut self, reg: u8, target: AbsoluteTarget) {
        self.rex(true, 0, reg, false);
        self.byte(0xB8 + (reg & 7));
        self.absolutes.push(Absolute {
            at: self.code.len(),
            target,
        });
        self.bytes(&0u64.to_le_bytes());
    }

    pub fn alu(&mut self, operation: Alu, wide: bool, destination: u8, source: u8) {
        self.op_direct(None, wide, &[operation as u8], source, destination);
    }

    pub fn imul(&mut self, wide: bool, destination: u8, source: u8) {
        self.op_direct(None, wide, &[0x0F, 0xAF], destination, source);
    }

    pub fn unary(&mut self, operation: Unary, wide: bool, reg: u8) {
        self.op_direct(None, wide, &[0xF7], operation as u8, reg);
    }

    pub fn shift(&mut self, operation: Shift, wide: bool, reg: u8) {
        self.op_direct(None, wide, &[0xD3], operation as u8, reg);
    }

    pub fn sign_extend_accumulator(&mut self, wide: bool) {
        if wide {
            self.bytes(&[0x48, 0x99]);
        } else {
            self.byte(0x99);
        }
    }

    pub fn test(&mut self, wide: bool, a: u8, b: u8) {
        self.op_direct(None, wide, &[0x85], b, a);
    }

    pub fn set(&mut self, condition: Cond, reg: u8) {
        self.rex(false, 0, reg, reg >= 4);
        self.bytes(&[0x0F, 0x90 + condition as u8]);
        self.direct(0, reg);
    }

    pub fn zero_extend_byte(&mut self, destination: u8, source: u8) {
        self.rex(false, destination, source, source >= 4);
        self.bytes(&[0x0F, 0xB6]);
        self.direct(destination, source);
    }

    pub fn sign_extend_byte(&mut self, destination: u8, source: u8) {
        self.rex(false, destination, source, source >= 4);
        self.bytes(&[0x0F, 0xBE]);
        self.direct(destination, source);
    }

    pub fn zero_extend_word(&mut self, destination: u8, source: u8) {
        self.op_direct(None, false, &[0x0F, 0xB7], destination, source);
    }

    pub fn sign_extend_word(&mut self, destination: u8, source: u8) {
        self.op_direct(None, false, &[0x0F, 0xBF], destination, source);
    }

    pub fn sign_extend_dword(&mut self, destination: u8, source: u8) {
        self.op_direct(None, true, &[0x63], destination, source);
    }

    pub fn jump(&mut self, label: Label) {
        self.byte(0xE9);
        self.fixups.push((self.code.len(), label));
        self.bytes(&[0; 4]);
    }

    pub fn jump_if(&mut self, condition: Cond, label: Label) {
        self.bytes(&[0x0F, 0x80 + condition as u8]);
        self.fixups.push((self.code.len(), label));
        self.bytes(&[0; 4]);
    }

    pub fn call_register(&mut self, reg: u8) {
        self.rex(false, 0, reg, false);
        self.byte(0xFF);
        self.direct(2, reg);
    }

    pub fn prologue(&mut self, frame: u32) {
        self.byte(0x55);
        self.bytes(&[0x48, 0x89, 0xE5]);
        self.bytes(&[0x48, 0x81, 0xEC]);
        self.bytes(&frame.to_le_bytes());
    }

    pub fn epilogue(&mut self) {
        self.bytes(&[0xC9, 0xC3]);
    }

    pub fn sse_load(&mut self, double: bool, xmm: u8, base: u8, displacement: i32) {
        self.op_memory(
            Some(if double { 0xF2 } else { 0xF3 }),
            false,
            &[0x0F, 0x10],
            xmm,
            base,
            displacement,
        );
    }

    pub fn sse_store(&mut self, double: bool, base: u8, displacement: i32, xmm: u8) {
        self.op_memory(
            Some(if double { 0xF2 } else { 0xF3 }),
            false,
            &[0x0F, 0x11],
            xmm,
            base,
            displacement,
        );
    }

    pub fn sse_arithmetic(&mut self, operation: Sse, double: bool, destination: u8, source: u8) {
        self.op_direct(
            Some(if double { 0xF2 } else { 0xF3 }),
            false,
            &[0x0F, operation as u8],
            destination,
            source,
        );
    }

    pub fn sse_compare(&mut self, double: bool, a: u8, b: u8) {
        if double {
            self.op_direct(Some(0x66), false, &[0x0F, 0x2E], a, b);
        } else {
            self.op_direct(None, false, &[0x0F, 0x2E], a, b);
        }
    }

    pub fn int_to_float(&mut self, double: bool, wide: bool, xmm: u8, reg: u8) {
        self.op_direct(
            Some(if double { 0xF2 } else { 0xF3 }),
            wide,
            &[0x0F, 0x2A],
            xmm,
            reg,
        );
    }

    pub fn float_to_double(&mut self, destination: u8, source: u8) {
        self.op_direct(Some(0xF3), false, &[0x0F, 0x5A], destination, source);
    }

    pub fn double_to_float(&mut self, destination: u8, source: u8) {
        self.op_direct(Some(0xF2), false, &[0x0F, 0x5A], destination, source);
    }

    pub fn xmm_from_gpr(&mut self, wide: bool, xmm: u8, reg: u8) {
        self.op_direct(Some(0x66), wide, &[0x0F, 0x6E], xmm, reg);
    }

    pub fn gpr_from_xmm(&mut self, wide: bool, reg: u8, xmm: u8) {
        self.op_direct(Some(0x66), wide, &[0x0F, 0x7E], xmm, reg);
    }

    pub fn xor_packed(&mut self, double: bool, destination: u8, source: u8) {
        if double {
            self.op_direct(Some(0x66), false, &[0x0F, 0x57], destination, source);
        } else {
            self.op_direct(None, false, &[0x0F, 0x57], destination, source);
        }
    }
}

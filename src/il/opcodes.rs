#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    None,
    I8,
    U8,
    U16,
    I32,
    I64,
    F32,
    F64,
    Token,
    Branch8,
    Branch32,
    Switch,
}

#[derive(Debug, Clone, Copy)]
pub struct OpCode {
    pub name: &'static str,
    pub value: u16,
    pub operand: Operand,
}

const fn op(name: &'static str, value: u16, operand: Operand) -> Option<OpCode> {
    Some(OpCode {
        name,
        value,
        operand,
    })
}

use Operand::*;

pub static ONE_BYTE: [Option<OpCode>; 256] = {
    let mut t: [Option<OpCode>; 256] = [Option::None; 256];
    t[0x00] = op("nop", 0x00, None);
    t[0x01] = op("break", 0x01, None);
    t[0x02] = op("ldarg.0", 0x02, None);
    t[0x03] = op("ldarg.1", 0x03, None);
    t[0x04] = op("ldarg.2", 0x04, None);
    t[0x05] = op("ldarg.3", 0x05, None);
    t[0x06] = op("ldloc.0", 0x06, None);
    t[0x07] = op("ldloc.1", 0x07, None);
    t[0x08] = op("ldloc.2", 0x08, None);
    t[0x09] = op("ldloc.3", 0x09, None);
    t[0x0A] = op("stloc.0", 0x0A, None);
    t[0x0B] = op("stloc.1", 0x0B, None);
    t[0x0C] = op("stloc.2", 0x0C, None);
    t[0x0D] = op("stloc.3", 0x0D, None);
    t[0x0E] = op("ldarg.s", 0x0E, U8);
    t[0x0F] = op("ldarga.s", 0x0F, U8);
    t[0x10] = op("starg.s", 0x10, U8);
    t[0x11] = op("ldloc.s", 0x11, U8);
    t[0x12] = op("ldloca.s", 0x12, U8);
    t[0x13] = op("stloc.s", 0x13, U8);
    t[0x14] = op("ldnull", 0x14, None);
    t[0x15] = op("ldc.i4.m1", 0x15, None);
    t[0x16] = op("ldc.i4.0", 0x16, None);
    t[0x17] = op("ldc.i4.1", 0x17, None);
    t[0x18] = op("ldc.i4.2", 0x18, None);
    t[0x19] = op("ldc.i4.3", 0x19, None);
    t[0x1A] = op("ldc.i4.4", 0x1A, None);
    t[0x1B] = op("ldc.i4.5", 0x1B, None);
    t[0x1C] = op("ldc.i4.6", 0x1C, None);
    t[0x1D] = op("ldc.i4.7", 0x1D, None);
    t[0x1E] = op("ldc.i4.8", 0x1E, None);
    t[0x1F] = op("ldc.i4.s", 0x1F, I8);
    t[0x20] = op("ldc.i4", 0x20, I32);
    t[0x21] = op("ldc.i8", 0x21, I64);
    t[0x22] = op("ldc.r4", 0x22, F32);
    t[0x23] = op("ldc.r8", 0x23, F64);
    t[0x25] = op("dup", 0x25, None);
    t[0x26] = op("pop", 0x26, None);
    t[0x27] = op("jmp", 0x27, Token);
    t[0x28] = op("call", 0x28, Token);
    t[0x29] = op("calli", 0x29, Token);
    t[0x2A] = op("ret", 0x2A, None);
    t[0x2B] = op("br.s", 0x2B, Branch8);
    t[0x2C] = op("brfalse.s", 0x2C, Branch8);
    t[0x2D] = op("brtrue.s", 0x2D, Branch8);
    t[0x2E] = op("beq.s", 0x2E, Branch8);
    t[0x2F] = op("bge.s", 0x2F, Branch8);
    t[0x30] = op("bgt.s", 0x30, Branch8);
    t[0x31] = op("ble.s", 0x31, Branch8);
    t[0x32] = op("blt.s", 0x32, Branch8);
    t[0x33] = op("bne.un.s", 0x33, Branch8);
    t[0x34] = op("bge.un.s", 0x34, Branch8);
    t[0x35] = op("bgt.un.s", 0x35, Branch8);
    t[0x36] = op("ble.un.s", 0x36, Branch8);
    t[0x37] = op("blt.un.s", 0x37, Branch8);
    t[0x38] = op("br", 0x38, Branch32);
    t[0x39] = op("brfalse", 0x39, Branch32);
    t[0x3A] = op("brtrue", 0x3A, Branch32);
    t[0x3B] = op("beq", 0x3B, Branch32);
    t[0x3C] = op("bge", 0x3C, Branch32);
    t[0x3D] = op("bgt", 0x3D, Branch32);
    t[0x3E] = op("ble", 0x3E, Branch32);
    t[0x3F] = op("blt", 0x3F, Branch32);
    t[0x40] = op("bne.un", 0x40, Branch32);
    t[0x41] = op("bge.un", 0x41, Branch32);
    t[0x42] = op("bgt.un", 0x42, Branch32);
    t[0x43] = op("ble.un", 0x43, Branch32);
    t[0x44] = op("blt.un", 0x44, Branch32);
    t[0x45] = op("switch", 0x45, Switch);
    t[0x46] = op("ldind.i1", 0x46, None);
    t[0x47] = op("ldind.u1", 0x47, None);
    t[0x48] = op("ldind.i2", 0x48, None);
    t[0x49] = op("ldind.u2", 0x49, None);
    t[0x4A] = op("ldind.i4", 0x4A, None);
    t[0x4B] = op("ldind.u4", 0x4B, None);
    t[0x4C] = op("ldind.i8", 0x4C, None);
    t[0x4D] = op("ldind.i", 0x4D, None);
    t[0x4E] = op("ldind.r4", 0x4E, None);
    t[0x4F] = op("ldind.r8", 0x4F, None);
    t[0x50] = op("ldind.ref", 0x50, None);
    t[0x51] = op("stind.ref", 0x51, None);
    t[0x52] = op("stind.i1", 0x52, None);
    t[0x53] = op("stind.i2", 0x53, None);
    t[0x54] = op("stind.i4", 0x54, None);
    t[0x55] = op("stind.i8", 0x55, None);
    t[0x56] = op("stind.r4", 0x56, None);
    t[0x57] = op("stind.r8", 0x57, None);
    t[0x58] = op("add", 0x58, None);
    t[0x59] = op("sub", 0x59, None);
    t[0x5A] = op("mul", 0x5A, None);
    t[0x5B] = op("div", 0x5B, None);
    t[0x5C] = op("div.un", 0x5C, None);
    t[0x5D] = op("rem", 0x5D, None);
    t[0x5E] = op("rem.un", 0x5E, None);
    t[0x5F] = op("and", 0x5F, None);
    t[0x60] = op("or", 0x60, None);
    t[0x61] = op("xor", 0x61, None);
    t[0x62] = op("shl", 0x62, None);
    t[0x63] = op("shr", 0x63, None);
    t[0x64] = op("shr.un", 0x64, None);
    t[0x65] = op("neg", 0x65, None);
    t[0x66] = op("not", 0x66, None);
    t[0x67] = op("conv.i1", 0x67, None);
    t[0x68] = op("conv.i2", 0x68, None);
    t[0x69] = op("conv.i4", 0x69, None);
    t[0x6A] = op("conv.i8", 0x6A, None);
    t[0x6B] = op("conv.r4", 0x6B, None);
    t[0x6C] = op("conv.r8", 0x6C, None);
    t[0x6D] = op("conv.u4", 0x6D, None);
    t[0x6E] = op("conv.u8", 0x6E, None);
    t[0x6F] = op("callvirt", 0x6F, Token);
    t[0x70] = op("cpobj", 0x70, Token);
    t[0x71] = op("ldobj", 0x71, Token);
    t[0x72] = op("ldstr", 0x72, Token);
    t[0x73] = op("newobj", 0x73, Token);
    t[0x74] = op("castclass", 0x74, Token);
    t[0x75] = op("isinst", 0x75, Token);
    t[0x76] = op("conv.r.un", 0x76, None);
    t[0x79] = op("unbox", 0x79, Token);
    t[0x7A] = op("throw", 0x7A, None);
    t[0x7B] = op("ldfld", 0x7B, Token);
    t[0x7C] = op("ldflda", 0x7C, Token);
    t[0x7D] = op("stfld", 0x7D, Token);
    t[0x7E] = op("ldsfld", 0x7E, Token);
    t[0x7F] = op("ldsflda", 0x7F, Token);
    t[0x80] = op("stsfld", 0x80, Token);
    t[0x81] = op("stobj", 0x81, Token);
    t[0x82] = op("conv.ovf.i1.un", 0x82, None);
    t[0x83] = op("conv.ovf.i2.un", 0x83, None);
    t[0x84] = op("conv.ovf.i4.un", 0x84, None);
    t[0x85] = op("conv.ovf.i8.un", 0x85, None);
    t[0x86] = op("conv.ovf.u1.un", 0x86, None);
    t[0x87] = op("conv.ovf.u2.un", 0x87, None);
    t[0x88] = op("conv.ovf.u4.un", 0x88, None);
    t[0x89] = op("conv.ovf.u8.un", 0x89, None);
    t[0x8A] = op("conv.ovf.i.un", 0x8A, None);
    t[0x8B] = op("conv.ovf.u.un", 0x8B, None);
    t[0x8C] = op("box", 0x8C, Token);
    t[0x8D] = op("newarr", 0x8D, Token);
    t[0x8E] = op("ldlen", 0x8E, None);
    t[0x8F] = op("ldelema", 0x8F, Token);
    t[0x90] = op("ldelem.i1", 0x90, None);
    t[0x91] = op("ldelem.u1", 0x91, None);
    t[0x92] = op("ldelem.i2", 0x92, None);
    t[0x93] = op("ldelem.u2", 0x93, None);
    t[0x94] = op("ldelem.i4", 0x94, None);
    t[0x95] = op("ldelem.u4", 0x95, None);
    t[0x96] = op("ldelem.i8", 0x96, None);
    t[0x97] = op("ldelem.i", 0x97, None);
    t[0x98] = op("ldelem.r4", 0x98, None);
    t[0x99] = op("ldelem.r8", 0x99, None);
    t[0x9A] = op("ldelem.ref", 0x9A, None);
    t[0x9B] = op("stelem.i", 0x9B, None);
    t[0x9C] = op("stelem.i1", 0x9C, None);
    t[0x9D] = op("stelem.i2", 0x9D, None);
    t[0x9E] = op("stelem.i4", 0x9E, None);
    t[0x9F] = op("stelem.i8", 0x9F, None);
    t[0xA0] = op("stelem.r4", 0xA0, None);
    t[0xA1] = op("stelem.r8", 0xA1, None);
    t[0xA2] = op("stelem.ref", 0xA2, None);
    t[0xA3] = op("ldelem", 0xA3, Token);
    t[0xA4] = op("stelem", 0xA4, Token);
    t[0xA5] = op("unbox.any", 0xA5, Token);
    t[0xB3] = op("conv.ovf.i1", 0xB3, None);
    t[0xB4] = op("conv.ovf.u1", 0xB4, None);
    t[0xB5] = op("conv.ovf.i2", 0xB5, None);
    t[0xB6] = op("conv.ovf.u2", 0xB6, None);
    t[0xB7] = op("conv.ovf.i4", 0xB7, None);
    t[0xB8] = op("conv.ovf.u4", 0xB8, None);
    t[0xB9] = op("conv.ovf.i8", 0xB9, None);
    t[0xBA] = op("conv.ovf.u8", 0xBA, None);
    t[0xC2] = op("refanyval", 0xC2, Token);
    t[0xC3] = op("ckfinite", 0xC3, None);
    t[0xC6] = op("mkrefany", 0xC6, Token);
    t[0xD0] = op("ldtoken", 0xD0, Token);
    t[0xD1] = op("conv.u2", 0xD1, None);
    t[0xD2] = op("conv.u1", 0xD2, None);
    t[0xD3] = op("conv.i", 0xD3, None);
    t[0xD4] = op("conv.ovf.i", 0xD4, None);
    t[0xD5] = op("conv.ovf.u", 0xD5, None);
    t[0xD6] = op("add.ovf", 0xD6, None);
    t[0xD7] = op("add.ovf.un", 0xD7, None);
    t[0xD8] = op("mul.ovf", 0xD8, None);
    t[0xD9] = op("mul.ovf.un", 0xD9, None);
    t[0xDA] = op("sub.ovf", 0xDA, None);
    t[0xDB] = op("sub.ovf.un", 0xDB, None);
    t[0xDC] = op("endfinally", 0xDC, None);
    t[0xDD] = op("leave", 0xDD, Branch32);
    t[0xDE] = op("leave.s", 0xDE, Branch8);
    t[0xDF] = op("stind.i", 0xDF, None);
    t[0xE0] = op("conv.u", 0xE0, None);
    t
};

pub static TWO_BYTE: [Option<OpCode>; 0x1F] = [
    op("arglist", 0xFE00, None),
    op("ceq", 0xFE01, None),
    op("cgt", 0xFE02, None),
    op("cgt.un", 0xFE03, None),
    op("clt", 0xFE04, None),
    op("clt.un", 0xFE05, None),
    op("ldftn", 0xFE06, Token),
    op("ldvirtftn", 0xFE07, Token),
    Option::None,
    op("ldarg", 0xFE09, U16),
    op("ldarga", 0xFE0A, U16),
    op("starg", 0xFE0B, U16),
    op("ldloc", 0xFE0C, U16),
    op("ldloca", 0xFE0D, U16),
    op("stloc", 0xFE0E, U16),
    op("localloc", 0xFE0F, None),
    Option::None,
    op("endfilter", 0xFE11, None),
    op("unaligned.", 0xFE12, U8),
    op("volatile.", 0xFE13, None),
    op("tail.", 0xFE14, None),
    op("initobj", 0xFE15, Token),
    op("constrained.", 0xFE16, Token),
    op("cpblk", 0xFE17, None),
    op("initblk", 0xFE18, None),
    op("no.", 0xFE19, U8),
    op("rethrow", 0xFE1A, None),
    Option::None,
    op("sizeof", 0xFE1C, Token),
    op("refanytype", 0xFE1D, None),
    op("readonly.", 0xFE1E, None),
];

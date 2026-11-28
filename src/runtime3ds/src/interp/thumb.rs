//! the Thumb-1 decoder: the 16-bit formats through the conditional branch, the state-switching
//! BX and BLX pairs, push/pop, the sp-relative and pc-relative loads.

use super::*;

/// one Thumb instruction (two aligned halfwords when BL/BLX needs them)
pub fn step(ctx: &mut Context, mem: &Memory, address: u32, log: &mut dyn MissLog, svc: SvcSink) -> Step {
    let hw = mem.read16(address);
    match hw >> 12 {
        // 000xx: shifts (format 1) and add/subtract (format 2, bits[12:11] == 11)
        0x0 | 0x1 => {
            if hw >> 11 & 0x7 == 0x3 {
                add_subtract(ctx, address, hw)
            } else {
                shift_imm(ctx, address, hw)
            }
        }
        // 001 op rd imm8: mov/cmp/add/sub immediate
        0x2 | 0x3 => immediate_op(ctx, address, hw),
        // 01000x: alu and hi-register operations; 01001: the pc-relative load
        0x4 => {
            if hw >> 11 & 1 == 1 {
                // format 6: ldr rd, [pc, imm8*4]
                let rd = (hw >> 8 & 0x7) as usize;
                ctx.r[rd] = mem.read32(((address + 4) & !3) + u32::from(hw & 0xFF) * 4);
                ctx.r[15] = address + 2;
                Step::Next
            } else if hw >> 10 & 1 == 0 {
                alu(ctx, address, hw)
            } else {
                hi_register(ctx, address, hw, log)
            }
        }
        // 0101: load/store with a register offset, word/byte and the signed halfword forms
        0x5 => reg_offset(ctx, mem, address, hw, log),
        // 011x: load/store word and byte with an immediate offset
        0x6 | 0x7 => {
            let load = hw >> 11 & 1 != 0;
            let byte = hw >> 12 & 1 != 0;
            let offset = u32::from(hw >> 6 & 0x1F) * if byte { 1 } else { 4 };
            let rd = (hw >> 3 & 0x7) as usize;
            let rb = (hw & 0x7) as usize;
            let at = ctx.r[rb] + offset;
            if load {
                ctx.r[rd] = if byte { mem.read8(at) as u32 } else { mem.read32(at) };
            } else if byte {
                mem.write8(at, ctx.r[rd] as u8);
            } else {
                mem.write32(at, ctx.r[rd]);
            }
            ctx.r[15] = address + 2;
            Step::Next
        }
        // 1000: halfword load/store, immediate offset
        0x8 => {
            let load = hw >> 11 & 1 != 0;
            let rd = (hw >> 3 & 0x7) as usize;
            let rb = (hw & 0x7) as usize;
            let at = ctx.r[rb] + u32::from(hw >> 6 & 0x1F) * 2;
            if load {
                ctx.r[rd] = mem.read16(at) as u32;
            } else {
                mem.write16(at, ctx.r[rd] as u16);
            }
            ctx.r[15] = address + 2;
            Step::Next
        }
        // 1001: sp-relative load/store
        0x9 => {
            let load = hw >> 11 & 1 != 0;
            let rd = (hw >> 8 & 0x7) as usize;
            let at = ctx.r[13] + u32::from(hw & 0xFF) * 4;
            if load {
                ctx.r[rd] = mem.read32(at);
            } else {
                mem.write32(at, ctx.r[rd]);
            }
            ctx.r[15] = address + 2;
            Step::Next
        }
        // 1010: load address (pc or sp relative)
        0xA => {
            let rd = (hw >> 8 & 0x7) as usize;
            let base = if hw >> 11 & 1 != 0 { ctx.r[13] } else { (address + 4) & !3 };
            ctx.r[rd] = base + u32::from(hw & 0xFF) * 4;
            ctx.r[15] = address + 2;
            Step::Next
        }
        // 1011: add sp immediate, push/pop
        0xB => match hw >> 8 & 0xF {
            0x0 => {
                let amount = u32::from(hw & 0x7F) * 4;
                ctx.r[13] = if hw >> 7 & 1 != 0 { ctx.r[13].wrapping_sub(amount) } else { ctx.r[13].wrapping_add(amount) };
                ctx.r[15] = address + 2;
                Step::Next
            }
            kind @ (0x4 | 0x5) => push(ctx, mem, address, hw, kind == 0x5),
            kind @ (0xC | 0xD) => pop(ctx, mem, address, hw, kind == 0xD, log),
            _ => {
                log.miss(address, hw as u32, "a 1011 thumb form this host does not run");
                Step::Unsupported
            }
        },
        // 1100: load/store multiple
        0xC => multiple(ctx, mem, address, hw, log),
        // 1101: conditional branch, or the svc when the field is 1111
        0xD => {
            let field = hw >> 8 & 0xF;
            if field == 0xF {
                let number = (hw & 0xFF) as u32;
                ctx.r[15] = address + 2;
                svc(ctx, number);
                Step::Svc(number)
            } else if cond_holds(ctx, field as u32) {
                let offset = ((u32::from(hw & 0xFF) << 24) as i32) >> 23;
                ctx.r[15] = (address as i32 + 4 + offset) as u32 & !1;
                Step::Taken
            } else {
                ctx.r[15] = address + 2;
                Step::Next
            }
        }
        // 1110: unconditional branch
        0xE => {
            let offset = ((u32::from(hw & 0x7FF) << 21) as i32) >> 20;
            ctx.r[15] = (address as i32 + 4 + offset) as u32 & !1;
            Step::Taken
        }
        // 1111 + partner: BL (11H1) or BLX (11H0)
        _ => {
            let second = mem.read16(address + 2);
            if second >> 14 != 0x3 {
                log.miss(address, (hw as u32) << 16 | second as u32, "a 32-bit thumb form this host does not run");
                return Step::Unsupported;
            }
            let bl = second >> 12 & 1 != 0;
            // the v5T form: S:imm10 (first) and imm11 (second); the offset is imm10<<12 |
            // imm11<<1, sign-extended from bit 22
            let raw = ((hw as u32 & 0x7FF) << 12) | ((second as u32 & 0x7FF) << 1);
            let offset = ((raw << 9) as i32) >> 9;
            ctx.r[14] = address + 4 | if bl { 1 } else { 0 };
            let target = (address as i32 + 4 + offset) as u32;
            if bl {
                ctx.r[15] = target & !1;
            } else {
                ctx.thumb = 0;
                ctx.r[15] = target & !3;
            }
            Step::Taken
        }
    }
}

/// format 1: lsl/lsr/asr by an immediate (imm5 == 0 names lsr/asr 32, lsl 0 is a plain move)
fn shift_imm(ctx: &mut Context, address: u32, hw: u16) -> Step {
    let op = hw >> 11 & 0x3;
    let amount = u32::from(hw >> 6 & 0x1F);
    let rm = (hw >> 3 & 0x7) as usize;
    let rd = (hw & 0x7) as usize;
    let value = ctx.r[rm];
    let (result, carry) = match op {
        0 if amount == 0 => (value, ctx.c != 0),
        0 => shift_value(value, &Shift { kind: ShiftKind::Lsl, amount, by_register: false }, ctx.c != 0, amount),
        1 => shift_value(value, &Shift { kind: ShiftKind::Lsr, amount: amount.max(1), by_register: false }, ctx.c != 0, amount.max(1)),
        _ => shift_value(value, &Shift { kind: ShiftKind::Asr, amount: amount.max(1), by_register: false }, ctx.c != 0, amount.max(1)),
    };
    ctx.r[rd] = result;
    ctx.n = (result >> 31 != 0) as u8;
    ctx.z = (result == 0) as u8;
    ctx.c = carry as u8;
    ctx.r[15] = address + 2;
    Step::Next
}

/// format 2: add/subtract a register or a 3-bit immediate
fn add_subtract(ctx: &mut Context, address: u32, hw: u16) -> Step {
    let immediate = hw >> 10 & 1 != 0;
    let subtract = hw >> 9 & 1 != 0;
    let operand = if immediate { u32::from(hw >> 6 & 0x7) } else { ctx.r[(hw >> 6 & 0x7) as usize] };
    let rs = (hw >> 3 & 0x7) as usize;
    let rd = (hw & 0x7) as usize;
    let (result, carry, overflow) = if subtract {
        sub_flags(ctx.r[rs], operand, false)
    } else {
        add_flags(ctx.r[rs], operand, false)
    };
    ctx.r[rd] = result;
    ctx.n = (result >> 31 != 0) as u8;
    ctx.z = (result == 0) as u8;
    ctx.c = carry as u8;
    ctx.v = overflow as u8;
    ctx.r[15] = address + 2;
    Step::Next
}

/// format 3: mov/cmp/add/sub an 8-bit immediate into a low register
fn immediate_op(ctx: &mut Context, address: u32, hw: u16) -> Step {
    let op = hw >> 11 & 0x3;
    let rd = (hw >> 8 & 0x7) as usize;
    let imm = u32::from(hw & 0xFF);
    match op {
        0 => {
            ctx.r[rd] = imm;
            ctx.n = 0;
            ctx.z = (imm == 0) as u8;
        }
        1 => {
            let (value, carry, overflow) = sub_flags(ctx.r[rd], imm, false);
            ctx.n = (value >> 31 != 0) as u8;
            ctx.z = (value == 0) as u8;
            ctx.c = carry as u8;
            ctx.v = overflow as u8;
        }
        _ => {
            let (result, carry, overflow) = if op == 2 {
                add_flags(ctx.r[rd], imm, false)
            } else {
                sub_flags(ctx.r[rd], imm, false)
            };
            ctx.r[rd] = result;
            ctx.n = (result >> 31 != 0) as u8;
            ctx.z = (result == 0) as u8;
            ctx.c = carry as u8;
            ctx.v = overflow as u8;
        }
    }
    ctx.r[15] = address + 2;
    Step::Next
}

/// format 4: the alu operations between low registers
fn alu(ctx: &mut Context, address: u32, hw: u16) -> Step {
    let op = hw >> 6 & 0xF;
    let rm = (hw & 0x7) as usize;
    let rd = (hw >> 3 & 0x7) as usize;
    match op {
        0x0 => {
            ctx.r[rd] &= ctx.r[rm];
            flags_nz_c(ctx, ctx.r[rd], ctx.c != 0);
        }
        0x1 => {
            ctx.r[rd] ^= ctx.r[rm];
            flags_nz_c(ctx, ctx.r[rd], ctx.c != 0);
        }
        0x2 => {
            let shift = Shift { kind: ShiftKind::Lsl, amount: 0, by_register: true };
            let (value, carry) = shift_value(ctx.r[rd], &shift, ctx.c != 0, ctx.r[rm] & 0xFF);
            ctx.r[rd] = value;
            flags_nz_c(ctx, value, carry);
        }
        0x3 => {
            let shift = Shift { kind: ShiftKind::Lsr, amount: 0, by_register: true };
            let (value, carry) = shift_value(ctx.r[rd], &shift, ctx.c != 0, ctx.r[rm] & 0xFF);
            ctx.r[rd] = value;
            flags_nz_c(ctx, value, carry);
        }
        0x4 => {
            let shift = Shift { kind: ShiftKind::Asr, amount: 0, by_register: true };
            let (value, carry) = shift_value(ctx.r[rd], &shift, ctx.c != 0, ctx.r[rm] & 0xFF);
            ctx.r[rd] = value;
            flags_nz_c(ctx, value, carry);
        }
        0x5 => {
            let (value, carry, overflow) = add_flags(ctx.r[rd], ctx.r[rm], ctx.c != 0);
            ctx.r[rd] = value;
            flags_nz_cv(ctx, value, carry, overflow);
        }
        0x6 => {
            let (value, carry, overflow) = sub_flags(ctx.r[rd], ctx.r[rm], ctx.c == 0);
            ctx.r[rd] = value;
            flags_nz_cv(ctx, value, carry, overflow);
        }
        0x7 => {
            let shift = Shift { kind: ShiftKind::Ror, amount: 0, by_register: true };
            let (value, carry) = shift_value(ctx.r[rd], &shift, ctx.c != 0, ctx.r[rm] & 0xFF);
            ctx.r[rd] = value;
            flags_nz_c(ctx, value, carry);
        }
        0x8 => {
            let value = ctx.r[rd] & ctx.r[rm];
            flags_nz_c(ctx, value, ctx.c != 0);
        }
        0x9 => {
            let (value, carry, overflow) = sub_flags(0, ctx.r[rm], false);
            ctx.r[rd] = value;
            flags_nz_cv(ctx, value, carry, overflow);
        }
        0xA => {
            let (value, carry, overflow) = sub_flags(ctx.r[rd], ctx.r[rm], false);
            flags_nz_cv(ctx, value, carry, overflow);
        }
        0xB => {
            let (value, carry, overflow) = add_flags(ctx.r[rd], ctx.r[rm], false);
            flags_nz_cv(ctx, value, carry, overflow);
        }
        0xC => {
            ctx.r[rd] |= ctx.r[rm];
            flags_nz_c(ctx, ctx.r[rd], ctx.c != 0);
        }
        0xD => {
            ctx.r[rd] = ctx.r[rd].wrapping_mul(ctx.r[rm]);
            flags_nz_c(ctx, ctx.r[rd], ctx.c != 0);
        }
        0xE => {
            ctx.r[rd] &= !ctx.r[rm];
            flags_nz_c(ctx, ctx.r[rd], ctx.c != 0);
        }
        _ => {
            ctx.r[rd] = !ctx.r[rm];
            flags_nz_c(ctx, ctx.r[rd], ctx.c != 0);
        }
    }
    ctx.r[15] = address + 2;
    Step::Next
}

fn flags_nz_c(ctx: &mut Context, value: u32, carry: bool) {
    ctx.n = (value >> 31 != 0) as u8;
    ctx.z = (value == 0) as u8;
    ctx.c = carry as u8;
}

fn flags_nz_cv(ctx: &mut Context, value: u32, carry: bool, overflow: bool) {
    flags_nz_c(ctx, value, carry);
    ctx.v = overflow as u8;
}

/// format 5: add/cmp/mov on the high registers, then bx/blx through a register
/// the fields: the destination (or cmp's first operand) is bit7:bits[2:0], the source is
/// bit6:bits[5:3]; bx/blx name the target in bits[6:3] with bit7 as the link
fn hi_register(ctx: &mut Context, address: u32, hw: u16, _log: &mut dyn MissLog) -> Step {
    let op = hw >> 8 & 0x3;
    let first = (((hw >> 7 & 1) as usize) << 3) | ((hw & 0x7) as usize);
    let second = (((hw >> 6 & 1) as usize) << 3) | ((hw >> 3 & 0x7) as usize);
    match op {
        0 => {
            let (value, carry, overflow) = add_flags(reg_read(ctx, first, address, true), reg_read(ctx, second, address, true), false);
            if first == 15 {
                ctx.r[15] = value & !1;
                return Step::Taken;
            }
            ctx.r[first] = value;
            ctx.n = (value >> 31 != 0) as u8;
            ctx.z = (value == 0) as u8;
            ctx.c = carry as u8;
            ctx.v = overflow as u8;
            ctx.r[15] = address + 2;
            Step::Next
        }
        1 => {
            let (value, carry, overflow) = sub_flags(reg_read(ctx, first, address, true), reg_read(ctx, second, address, true), false);
            ctx.n = (value >> 31 != 0) as u8;
            ctx.z = (value == 0) as u8;
            ctx.c = carry as u8;
            ctx.v = overflow as u8;
            ctx.r[15] = address + 2;
            Step::Next
        }
        2 => {
            let value = reg_read(ctx, second, address, true);
            if first == 15 {
                // mov pc from the hi form: state per the low bit
                jump_arm(ctx, value);
                return Step::Taken;
            }
            ctx.r[first] = value;
            ctx.r[15] = address + 2;
            Step::Next
        }
        _ => {
            // bx lr, blx r3: the register in bits[6:3], bit7 links (blx: return in Thumb, go ARM)
            let target = reg_read(ctx, (((hw >> 6 & 1) as usize) << 3) | ((hw >> 3 & 0x7) as usize), address, true);
            if hw >> 7 & 1 == 1 {
                ctx.r[14] = address + 4 | 1;
                ctx.thumb = 0;
                ctx.r[15] = target & !3;
            } else {
                jump_arm(ctx, target);
            }
            Step::Taken
        }
    }
}

/// formats 7/8: load/store with a register offset. bits[11:9] name the form:
/// 0 STR, 1 STRH, 2 STRB, 3 LDRSB, 4 LDR, 5 LDRH, 6 LDRB, 7 LDRSH
fn reg_offset(ctx: &mut Context, mem: &Memory, address: u32, hw: u16, log: &mut dyn MissLog) -> Step {
    let op = hw >> 9 & 0x7;
    let load = op & 0b100 != 0 || op == 0b011;
    let signed = matches!(op, 0b011 | 0b111);
    let half = op & 1 == 1;
    let byte = matches!(op, 0b010 | 0b011 | 0b110);
    let rm = (hw >> 6 & 0x7) as usize;
    let rn = (hw >> 3 & 0x7) as usize;
    let rd = (hw & 0x7) as usize;
    let at = ctx.r[rn].wrapping_add(ctx.r[rm]);
    if load {
        ctx.r[rd] = if signed && byte {
            mem.read8(at) as i8 as i32 as u32
        } else if signed {
            mem.read16(at) as i16 as i32 as u32
        } else if half {
            mem.read16(at) as u32
        } else if byte {
            mem.read8(at) as u32
        } else {
            mem.read32(at)
        };
    } else if half {
        mem.write16(at, ctx.r[rd] as u16);
    } else if byte {
        mem.write8(at, ctx.r[rd] as u8);
    } else {
        mem.write32(at, ctx.r[rd]);
    }
    let _ = log;
    ctx.r[15] = address + 2;
    Step::Next
}

/// push: 1011 010M list, descending from sp, M names lr
fn push(ctx: &mut Context, mem: &Memory, address: u32, hw: u16, with_lr: bool) -> Step {
    let list = hw & 0xFF;
    let count = list.count_ones() + with_lr as u32;
    let base = ctx.r[13];
    let mut at = base - 4 * count;
    for register in 0..8usize {
        if list >> register & 1 == 0 {
            continue;
        }
        mem.write32(at, ctx.r[register]);
        at += 4;
    }
    if with_lr {
        mem.write32(base - 4, ctx.r[14]);
    }
    ctx.r[13] = base - 4 * count;
    ctx.r[15] = address + 2;
    Step::Next
}

/// pop: 1011 011M list, ascending, M names pc (its low bit picks the state)
fn pop(ctx: &mut Context, mem: &Memory, address: u32, hw: u16, with_pc: bool, log: &mut dyn MissLog) -> Step {
    let list = hw & 0xFF;
    if list == 0 && !with_pc {
        log.miss(address, hw as u32, "an empty pop list");
        return Step::Unsupported;
    }
    let count = list.count_ones() + with_pc as u32;
    let base = ctx.r[13];
    let mut at = base;
    for register in 0..8usize {
        if list >> register & 1 == 0 {
            continue;
        }
        ctx.r[register] = mem.read32(at);
        at += 4;
    }
    ctx.r[13] = base + 4 * count;
    if with_pc {
        let target = mem.read32(base + 4 * (count - 1));
        jump_arm(ctx, target);
        return Step::Taken;
    }
    ctx.r[15] = address + 2;
    Step::Next
}

/// format 15: ldmia/stmia rn!, list
fn multiple(ctx: &mut Context, mem: &Memory, address: u32, hw: u16, log: &mut dyn MissLog) -> Step {
    let load = hw >> 11 & 1 != 0;
    let rn = (hw >> 8 & 0x7) as usize;
    let list = hw & 0xFF;
    if list == 0 {
        log.miss(address, hw as u32, "an empty thumb register list");
        return Step::Unsupported;
    }
    let mut at = ctx.r[rn];
    for register in 0..8usize {
        if list >> register & 1 == 0 {
            continue;
        }
        if load {
            ctx.r[register] = mem.read32(at);
        } else {
            mem.write32(at, ctx.r[register]);
        }
        at += 4;
    }
    ctx.r[rn] = at;
    ctx.r[15] = address + 2;
    Step::Next
}

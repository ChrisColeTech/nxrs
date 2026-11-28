//! the ARM decoder: ARMv6-K, the classes the recompiler runs and the ones it leaves to
//! `interpret` (msr/mrs forms, exception returns, user-bank multiples, pair loads, odd
//! coprocessors). Everything this host has no semantics for stops the step with a miss logged
//! — the boot's gap discipline, at instruction grain.

use super::*;

/// one ARM instruction
pub fn step(ctx: &mut Context, mem: &Memory, address: u32, log: &mut dyn MissLog, svc: SvcSink) -> Step {
    let opcode = mem.read32(address);
    let cond = opcode >> 28;
    if cond == 0xF {
        return unconditional(ctx, address, opcode, log);
    }
    if !cond_holds(ctx, cond) {
        ctx.r[15] = address + 4;
        return Step::Next;
    }
    match opcode >> 25 {
        0 | 1 => special_or_data(ctx, mem, address, opcode, log, svc),
        2 => transfer_word_byte(ctx, mem, address, opcode, log),
        3 => media_or_extend(ctx, mem, address, opcode, log),
        4 => multiple_transfer(ctx, mem, address, opcode, log),
        5 => {
            // B/BL: cond 101L imm24
            let link = opcode >> 24 & 1 != 0;
            let offset = (((opcode & 0x00FF_FFFF) << 8) as i32) >> 6;
            if link {
                ctx.r[14] = address + 4;
            }
            ctx.r[15] = (address as i32 + 8 + offset) as u32;
            Step::Taken
        }
        6 => vfp::step(ctx, mem, address, opcode, log),
        7 => coprocessor_or_svc(ctx, mem, address, opcode, log, svc),
        _ => unreachable!(),
    }
}

/// the cond = 1111 space: BLX(1), PLD, CPS, SETEND and the 2-form coprocessors
fn unconditional(ctx: &mut Context, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    match opcode >> 25 {
        // BLX(1): 1111 101H imm24
        5 if opcode >> 24 & 1 == 1 => {
            let offset = (((opcode & 0x00FF_FFFF) << 8) as i32) >> 7 | ((opcode >> 24) as i32 & 1) << 1;
            ctx.r[14] = address + 4;
            ctx.thumb = 1;
            ctx.r[15] = (address as i32 + 8 + offset) as u32 & !1;
            Step::Taken
        }
        // PLD: a prefetch hint, nothing to do
        5 => {
            ctx.r[15] = address + 4;
            Step::Next
        }
        // SETEND changes no byte order this host keeps
        7 if opcode >> 20 & 0xF7 == 0x10 => {
            ctx.r[15] = address + 4;
            Step::Next
        }
        // CPS: mode changes have no meaning on this flat host
        0 => {
            ctx.r[15] = address + 4;
            Step::Next
        }
        _ => {
            log.miss(address, opcode, "unconditional form this host does not run");
            Step::Unsupported
        }
    }
}

/// the groups that decode below the data-processing ones
fn special_or_data(ctx: &mut Context, mem: &Memory, address: u32, opcode: u32, log: &mut dyn MissLog, _svc: SvcSink) -> Step {
    if opcode >> 4 & 0xF == 0x9 {
        return exchange_group(ctx, mem, address, opcode, log);
    }
    // MRS rd, cpsr: cond 0001 0000 rd 0000 0000 1111 0000
    if opcode >> 20 & 0xFF == 0x10 && opcode >> 4 & 0xFFF == 0x0F0 {
        let rd = (opcode >> 12 & 0xF) as usize;
        let value = (ctx.n as u32) << 31
            | (ctx.z as u32) << 30
            | (ctx.c as u32) << 29
            | (ctx.v as u32) << 28
            | (ctx.q as u32) << 27
            | u32::from(ctx.ge & 0xF) << 16
            | (ctx.thumb as u32) << 5
            | 0x10;
        reg_write(ctx, rd, value);
        ctx.r[15] = address + 4;
        return Step::Next;
    }
    // BX / BLX(2): cond 0001 0010 1111 1111 1111 00x1 rm
    if opcode >> 20 & 0xFF == 0x12 && opcode >> 12 & 0xF == 0xF && opcode >> 4 & 0x3 == 0x1 {
        let target = reg_read(ctx, (opcode & 0xF) as usize, address, false);
        if opcode >> 4 & 0x3 == 0x3 {
            ctx.r[14] = address + 4;
        }
        jump_arm(ctx, target);
        return Step::Taken;
    }
    // BKPT: cond 0001 0010 imm12 0111 imm8
    if opcode >> 20 & 0xFF == 0x12 && opcode >> 4 & 0xF == 0x7 {
        log.miss(address, opcode, "bkpt");
        return Step::Unsupported;
    }
    // MSR cpsr, register: cond 0001 0R10 mask 1111 0000 0000 rm
    if matches!(opcode >> 20 & 0xFF, 0x12 | 0x16) && opcode >> 12 & 0xF == 0xF && opcode >> 4 & 0xFFF == 0 {
        let value = reg_read(ctx, (opcode & 0xF) as usize, address, false);
        apply_msr(ctx, opcode >> 16 & 0xF, value, log, address, opcode);
        ctx.r[15] = address + 4;
        return Step::Next;
    }
    data_processing(ctx, address, opcode, log)
}

/// multiplies (bit24 clear) and swaps/exclusives (bit24 set): bits[7:4] == 1001
fn exchange_group(ctx: &mut Context, mem: &Memory, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    let rd = (opcode >> 12 & 0xF) as usize;
    let rn = (opcode >> 16 & 0xF) as usize;
    let rs = (opcode >> 8 & 0xF) as usize;
    let rm = (opcode & 0xF) as usize;
    let set_flags = opcode >> 20 & 1 != 0;
    if opcode >> 24 & 1 == 0 {
        match opcode >> 20 & 0xF {
            // MUL/MLA
            kind @ (0x0 | 0x1) => {
                let product = ctx.r[rm].wrapping_mul(ctx.r[rs]);
                let value = if kind == 0x1 { product.wrapping_add(ctx.r[rn]) } else { product };
                if rd == 15 {
                    log.miss(address, opcode, "a multiply into pc");
                    return Step::Unsupported;
                }
                ctx.r[rd] = value;
                if set_flags {
                    ctx.n = (value >> 31 != 0) as u8;
                    ctx.z = (value == 0) as u8;
                }
                ctx.r[15] = address + 4;
                Step::Next
            }
            // UMAAL: RdHi:RdLo = Rn*Rm + RdHi + RdLo
            0x2 => {
                let sum = (ctx.r[rm] as u64) * (ctx.r[rs] as u64) + ctx.r[rn] as u64 + ctx.r[rd] as u64;
                ctx.r[rn] = (sum & 0xFFFF_FFFF) as u32;
                ctx.r[rd] = (sum >> 32) as u32;
                ctx.r[15] = address + 4;
                Step::Next
            }
            // UMULL/UMLAL/SMULL/SMLAL (+S)
            kind @ 0x8..=0xF => {
                let mut product = match kind & 0xC {
                    0x8 => (ctx.r[rm] as u64) * (ctx.r[rs] as u64),
                    _ => ((ctx.r[rm] as i32 as i64) * (ctx.r[rs] as i32 as i64)) as u64,
                };
                if kind & 0x2 != 0 {
                    product = product.wrapping_add(ctx.r[rn] as u64 | (ctx.r[rd] as u64) << 32);
                }
                ctx.r[rn] = (product & 0xFFFF_FFFF) as u32;
                ctx.r[rd] = (product >> 32) as u32;
                if set_flags {
                    ctx.n = (product >> 63 != 0) as u8;
                    ctx.z = (product == 0) as u8;
                }
                ctx.r[15] = address + 4;
                Step::Next
            }
            _ => {
                log.miss(address, opcode, "a multiply form this host does not run");
                Step::Unsupported
            }
        }
    } else {
        match opcode >> 20 & 0xF {
            // SWP/SWPB
            0x4 => {
                let base = ctx.r[rn];
                let loaded = if opcode >> 22 & 1 != 0 { mem.read8(base) as u32 } else { mem.read32(base) };
                let stored = ctx.r[rm];
                if opcode >> 22 & 1 != 0 {
                    mem.write8(base, stored as u8);
                } else {
                    mem.write32(base, stored);
                }
                ctx.r[rd] = loaded;
                ctx.r[15] = address + 4;
                Step::Next
            }
            // STREX/LDREX and the v6K byte, halfword and double forms
            kind @ 0x8..=0xF => {
                let strex = kind & 1 == 0;
                let pairs = kind & 2 != 0;
                let size = match kind & 3 {
                    0 => 4usize,
                    2 => 1,
                    3 => 2,
                    _ => 8,
                };
                let at = ctx.r[rn];
                if strex {
                    let pass = ctx.exclusive != 0 && ctx.exclusive_address == at;
                    if pass {
                        match size {
                            1 => mem.write8(at, ctx.r[rm] as u8),
                            2 => mem.write16(at, ctx.r[rm] as u16),
                            4 => mem.write32(at, ctx.r[rm]),
                            _ => {
                                mem.write32(at, ctx.r[rm]);
                                mem.write32(at.wrapping_add(4), ctx.r[(rm + 1) & 0xF]);
                            }
                        }
                    }
                    ctx.exclusive = 0;
                    ctx.r[rd] = !pass as u32;
                } else {
                    let value = match size {
                        1 => mem.read8(at) as u32,
                        2 => mem.read16(at) as u32,
                        4 => mem.read32(at),
                        _ => {
                            let low = mem.read32(at);
                            ctx.r[(rd + 1) & 0xF] = mem.read32(at.wrapping_add(4));
                            low
                        }
                    };
                    ctx.exclusive = 1;
                    ctx.exclusive_address = at;
                    ctx.r[rd] = value;
                }
                let _ = pairs;
                ctx.r[15] = address + 4;
                Step::Next
            }
            _ => {
                log.miss(address, opcode, "an exchange form this host does not run");
                Step::Unsupported
            }
        }
    }
}

/// data processing, immediate and register-shifted operands
fn data_processing(ctx: &mut Context, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    let opcode_name = opcode >> 21 & 0xF;
    let set_flags = opcode >> 20 & 1 != 0;
    let rd = (opcode >> 12 & 0xF) as usize;
    let rn = (opcode >> 16 & 0xF) as usize;
    let immediate = opcode >> 25 & 1 != 0;
    let carry_in = ctx.c != 0;

    // the operand, shifted, with its carry out
    let (operand, carry_out) = if immediate {
        let rotate = (opcode >> 8 & 0xF) * 2;
        shift_value(opcode & 0xFF, &Shift { kind: ShiftKind::Ror, amount: rotate, by_register: false }, carry_in, rotate)
    } else {
        let rm = (opcode & 0xF) as usize;
        let value = reg_read(ctx, rm, address, false);
        if rm == 15 && opcode >> 4 & 1 != 0 {
            log.miss(address, opcode, "a register-controlled shift reading r15");
            return Step::Unsupported;
        }
        let shift = if opcode >> 4 & 1 != 0 {
            let amount = reg_read(ctx, (opcode >> 8 & 0xF) as usize, address, false) & 0xFF;
            match opcode >> 5 & 3 {
                0 => Shift { kind: ShiftKind::Lsl, amount, by_register: true },
                1 => Shift { kind: ShiftKind::Lsr, amount, by_register: true },
                2 => Shift { kind: ShiftKind::Asr, amount, by_register: true },
                _ => Shift { kind: ShiftKind::Ror, amount, by_register: true },
            }
        } else {
            let amount = opcode >> 7 & 0x1F;
            match opcode >> 5 & 3 {
                0 => Shift { kind: ShiftKind::Lsl, amount, by_register: false },
                1 => Shift { kind: ShiftKind::Lsr, amount: amount.max(1), by_register: false },
                2 => Shift { kind: ShiftKind::Asr, amount: amount.max(1), by_register: false },
                _ if amount == 0 => Shift { kind: ShiftKind::Rrx, amount: 0, by_register: false },
                _ => Shift { kind: ShiftKind::Ror, amount, by_register: false },
            }
        };
        shift_value(value, &shift, carry_in, shift_amount(&shift, value))
    };

    // a flagged write to pc is an exception return, the host has no spsr
    if set_flags && rd == 15 && opcode_name < 0x8 {
        log.miss(address, opcode, "an exception return");
        return Step::Unsupported;
    }

    let first = reg_read(ctx, rn, address, false);
    let result;
    match opcode_name {
        0x0 | 0x8 => {
            // AND, TST
            result = first & operand;
            if set_flags || opcode_name == 0x8 {
                ctx.n = (result >> 31 != 0) as u8;
                ctx.z = (result == 0) as u8;
                ctx.c = carry_out as u8;
            }
        }
        0x1 | 0x9 => {
            // EOR, TEQ
            result = first ^ operand;
            if set_flags || opcode_name == 0x9 {
                ctx.n = (result >> 31 != 0) as u8;
                ctx.z = (result == 0) as u8;
                ctx.c = carry_out as u8;
            }
        }
        0x2 => {
            let (value, carry, overflow) = sub_flags(first, operand, false);
            result = value;
            if set_flags {
                ctx.n = (value >> 31 != 0) as u8;
                ctx.z = (value == 0) as u8;
                ctx.c = carry as u8;
                ctx.v = overflow as u8;
            }
        }
        0x3 => {
            let (value, carry, overflow) = sub_flags(operand, first, false);
            result = value;
            if set_flags {
                ctx.n = (value >> 31 != 0) as u8;
                ctx.z = (value == 0) as u8;
                ctx.c = carry as u8;
                ctx.v = overflow as u8;
            }
        }
        0x4 => {
            let (value, carry, overflow) = add_flags(first, operand, false);
            result = value;
            if set_flags {
                ctx.n = (value >> 31 != 0) as u8;
                ctx.z = (value == 0) as u8;
                ctx.c = carry as u8;
                ctx.v = overflow as u8;
            }
        }
        0x5 => {
            let (value, carry, overflow) = add_flags(first, operand, carry_in);
            result = value;
            if set_flags {
                ctx.n = (value >> 31 != 0) as u8;
                ctx.z = (value == 0) as u8;
                ctx.c = carry as u8;
                ctx.v = overflow as u8;
            }
        }
        0x6 => {
            let (value, carry, overflow) = sub_flags(first, operand, !carry_in);
            result = value;
            if set_flags {
                ctx.n = (value >> 31 != 0) as u8;
                ctx.z = (value == 0) as u8;
                ctx.c = carry as u8;
                ctx.v = overflow as u8;
            }
        }
        0x7 => {
            let (value, carry, overflow) = sub_flags(operand, first, !carry_in);
            result = value;
            if set_flags {
                ctx.n = (value >> 31 != 0) as u8;
                ctx.z = (value == 0) as u8;
                ctx.c = carry as u8;
                ctx.v = overflow as u8;
            }
        }
        0xA => {
            // CMP
            let (value, carry, overflow) = sub_flags(first, operand, false);
            ctx.n = (value >> 31 != 0) as u8;
            ctx.z = (value == 0) as u8;
            ctx.c = carry as u8;
            ctx.v = overflow as u8;
            ctx.r[15] = address + 4;
            return Step::Next;
        }
        0xB => {
            // CMN
            let (value, carry, overflow) = add_flags(first, operand, false);
            ctx.n = (value >> 31 != 0) as u8;
            ctx.z = (value == 0) as u8;
            ctx.c = carry as u8;
            ctx.v = overflow as u8;
            ctx.r[15] = address + 4;
            return Step::Next;
        }
        0xC => {
            result = first | operand;
            if set_flags {
                ctx.n = (result >> 31 != 0) as u8;
                ctx.z = (result == 0) as u8;
                ctx.c = carry_out as u8;
            }
        }
        0xD => {
            result = operand;
            if set_flags {
                ctx.n = (result >> 31 != 0) as u8;
                ctx.z = (result == 0) as u8;
                ctx.c = carry_out as u8;
            }
        }
        0xE => {
            result = first & !operand;
            if set_flags {
                ctx.n = (result >> 31 != 0) as u8;
                ctx.z = (result == 0) as u8;
                ctx.c = carry_out as u8;
            }
        }
        _ => {
            result = !operand;
            if set_flags {
                ctx.n = (result >> 31 != 0) as u8;
                ctx.z = (result == 0) as u8;
                ctx.c = carry_out as u8;
            }
        }
    }
    if rd == 15 {
        // a plain write to pc stays in ARM state, word aligned
        ctx.r[15] = result & !3;
        Step::Taken
    } else {
        ctx.r[rd] = result;
        ctx.r[15] = address + 4;
        Step::Next
    }
}

/// MSR: the fields this flat host has are the flags and the state bit; control fields drop with
/// one logged line (the codegen leaves every msr to this interpreter, so the two sides match)
fn apply_msr(ctx: &mut Context, mask: u32, value: u32, log: &mut dyn MissLog, address: u32, opcode: u32) {
    if mask & 0b1000 != 0 {
        ctx.n = (value >> 31 & 1) as u8;
        ctx.z = (value >> 30 & 1) as u8;
        ctx.c = (value >> 29 & 1) as u8;
        ctx.v = (value >> 28 & 1) as u8;
        ctx.q = (value >> 27 & 1) as u8;
    }
    if mask & 0b0100 != 0 {
        ctx.ge = (value >> 16 & 0xF) as u8;
    }
    if mask & 0b0010 != 0 || mask & 0b0001 != 0 {
        ctx.thumb = (value >> 5 & 1) as u8;
    }
    if mask & 0b0011 != 0 {
        // control fields (mode, interrupt masks, endianness) have no meaning on this host; a
        // thumb flip is the one control bit that does, handled above
        log.miss(address, opcode, "msr control fields (mode/interrupts) dropped");
    }
}

/// LDR/STR word and byte, all of P/U/B/W/L, pc-relative literal loads included
fn transfer_word_byte(ctx: &mut Context, mem: &Memory, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    let pre_index = opcode >> 24 & 1 != 0;
    let up = opcode >> 23 & 1 != 0;
    let byte = opcode >> 22 & 1 != 0;
    let writeback = opcode >> 21 & 1 != 0;
    let load = opcode >> 20 & 1 != 0;
    let rn = (opcode >> 16 & 0xF) as usize;
    let rd = (opcode >> 12 & 0xF) as usize;
    let offset = if opcode >> 25 & 1 != 0 {
        opcode & 0xFFF
    } else {
        let rm = (opcode & 0xF) as usize;
        let value = reg_read(ctx, rm, address, false);
        let amount = opcode >> 7 & 0x1F;
        let shift = match opcode >> 5 & 3 {
            0 => Shift { kind: ShiftKind::Lsl, amount, by_register: false },
            1 => Shift { kind: ShiftKind::Lsr, amount: amount.max(1), by_register: false },
            2 => Shift { kind: ShiftKind::Asr, amount: amount.max(1), by_register: false },
            _ if amount == 0 => Shift { kind: ShiftKind::Rrx, amount: 0, by_register: false },
            _ => Shift { kind: ShiftKind::Ror, amount, by_register: false },
        };
        shift_value(value, &shift, ctx.c != 0, shift_amount(&shift, value)).0
    };
    let delta = if up { offset as i32 } else { -(offset as i32) };
    let base = reg_read(ctx, rn, address, false);
    let target = (base as i32).wrapping_add(delta) as u32;

    if load {
        if rd == 15 {
            log.miss(address, opcode, "a load into pc");
            return Step::Unsupported;
        }
        ctx.r[rd] = if byte { mem.read8(target) as u32 } else { mem.read32(target) };
    } else if byte {
        mem.write8(target, ctx.r[rd] as u8);
    } else {
        mem.write32(target, ctx.r[rd]);
    }
    if writeback || !pre_index {
        ctx.r[rn] = target;
    }
    ctx.r[15] = address + 4;
    Step::Next
}

/// the halfword transfers, the pair transfers, and the extend/reverse group
fn media_or_extend(ctx: &mut Context, mem: &Memory, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    // LDRD/STRD: bit22 clear, 1111 in bits[11:8], bits[7:4] 1101 (load) or 1111 (store)
    if opcode >> 22 & 1 == 0 && opcode >> 8 & 0xF == 0xF && matches!(opcode >> 4 & 0xF, 0xD | 0xF) {
        return pair_transfer(ctx, mem, address, opcode, log);
    }
    let bits74 = opcode >> 4 & 0xF;
    // halfword / signed byte transfers: bits[7:4] = 1SH1 with SH naming the form
    if bits74 & 0b1001 == 0b1001 && bits74 & 0b0110 != 0 {
        let signed_byte = bits74 >> 5 & 3 == 0b10;
        let signed_half = bits74 >> 5 & 3 == 0b11;
        let pre_index = opcode >> 24 & 1 != 0;
        let up = opcode >> 23 & 1 != 0;
        let writeback = opcode >> 21 & 1 != 0;
        let load = opcode >> 20 & 1 != 0;
        let rn = (opcode >> 16 & 0xF) as usize;
        let rd = (opcode >> 12 & 0xF) as usize;
        let offset = if opcode >> 22 & 1 != 0 {
            opcode >> 4 & 0xF0 | opcode & 0xF
        } else {
            reg_read(ctx, (opcode & 0xF) as usize, address, false)
        };
        let delta = if up { offset as i32 } else { -(offset as i32) };
        let base = reg_read(ctx, rn, address, false);
        let target = (base as i32).wrapping_add(delta) as u32;
        if load {
            ctx.r[rd] = if signed_byte {
                mem.read8(target) as i8 as i32 as u32
            } else if signed_half {
                mem.read16(target) as i16 as i32 as u32
            } else {
                mem.read16(target) as u32
            };
        } else {
            mem.write16(target, ctx.r[rd] as u16);
        }
        if writeback || !pre_index {
            ctx.r[rn] = target;
        }
        ctx.r[15] = address + 4;
        return Step::Next;
    }

    let rd = (opcode >> 12 & 0xF) as usize;
    // the extends: 0x6A/0x6B/0x6E, 1111 in bits[15:12], bits[7:4] == 0111, rotate in bits[11:8]
    if matches!(opcode >> 20 & 0xFF, 0x6A | 0x6B | 0x6E) && opcode >> 12 & 0xF == 0xF && bits74 == 0x7 {
        let rm = (opcode & 0xF) as usize;
        let value = ctx.r[rm].rotate_right((opcode >> 10 & 0x3) * 8);
        ctx.r[rd] = match opcode >> 20 & 0xF {
            0xA => value as u8 as u32,
            0xB => value as u16 as i16 as i32 as u32,
            _ => value & 0xFF,
        };
        ctx.r[15] = address + 4;
        return Step::Next;
    }
    // UXTH: 0x6F, 1111 in bits[15:12], rotate not 1111 (that pattern is REVSH)
    if opcode >> 20 & 0xFF == 0x6F && opcode >> 12 & 0xF == 0xF && bits74 == 0x7 && opcode >> 8 & 0xF != 0xF {
        let rm = (opcode & 0xF) as usize;
        ctx.r[rd] = ctx.r[rm].rotate_right((opcode >> 10 & 0x3) * 8) & 0xFFFF;
        ctx.r[15] = address + 4;
        return Step::Next;
    }
    // REV / REV16 / REVSH: 0x6F, 1111 in bits[15:12], 1111 in bits[11:8]
    if opcode >> 20 & 0xFF == 0x6F && opcode >> 12 & 0xF == 0xF && opcode >> 8 & 0xF == 0xF {
        let rm = (opcode & 0xF) as usize;
        let value = ctx.r[rm];
        ctx.r[rd] = match bits74 {
            0x3 => value.swap_bytes(),
            0xB => ((value & 0x00FF_00FF) << 8) | ((value >> 8) & 0x00FF_00FF),
            _ => (value as u16).swap_bytes() as u16 as i16 as i32 as u32,
        };
        ctx.r[15] = address + 4;
        return Step::Next;
    }
    log.miss(address, opcode, "a media/extend form this host does not run");
    Step::Unsupported
}

/// LDRD/STRD: two registers, one address
fn pair_transfer(ctx: &mut Context, mem: &Memory, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    let pre_index = opcode >> 24 & 1 != 0;
    let up = opcode >> 23 & 1 != 0;
    let writeback = opcode >> 21 & 1 != 0;
    let load = opcode >> 20 & 1 != 0;
    let rn = (opcode >> 16 & 0xF) as usize;
    let rd = (opcode >> 12 & 0xF) as usize;
    let offset = opcode & 0xF | ((opcode >> 8 & 0xF) << 4);
    let delta = if up { offset as i32 } else { -(offset as i32) };
    let base = reg_read(ctx, rn, address, false);
    let target = (base as i32).wrapping_add(delta) as u32;
    if load {
        if rd & 1 != 0 || rd >= 14 {
            log.miss(address, opcode, "a pair load into a register this host refuses");
            return Step::Unsupported;
        }
        ctx.r[rd] = mem.read32(target);
        ctx.r[rd + 1] = mem.read32(target.wrapping_add(4));
    } else {
        if rd & 1 != 0 {
            log.miss(address, opcode, "a pair store from an odd register");
            return Step::Unsupported;
        }
        mem.write32(target, ctx.r[rd]);
        mem.write32(target.wrapping_add(4), ctx.r[rd + 1]);
    }
    if writeback || !pre_index {
        ctx.r[rn] = target;
    }
    ctx.r[15] = address + 4;
    Step::Next
}

/// LDM/STM
fn multiple_transfer(ctx: &mut Context, mem: &Memory, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    let load = opcode >> 20 & 1 != 0;
    let rn = (opcode >> 16 & 0xF) as usize;
    if opcode >> 22 & 1 != 0 {
        log.miss(address, opcode, "a user-bank multiple transfer");
        return Step::Unsupported;
    }
    if rn == 15 {
        log.miss(address, opcode, "a multiple transfer based on pc");
        return Step::Unsupported;
    }
    let pre_index = opcode >> 24 & 1 != 0;
    let up = opcode >> 23 & 1 != 0;
    let writeback = opcode >> 21 & 1 != 0;
    let list = opcode & 0xFFFF;
    let count = list.count_ones();
    if count == 0 {
        log.miss(address, opcode, "an empty register list");
        return Step::Unsupported;
    }
    let base = ctx.r[rn];
    let mut at = if up { base + if pre_index { 4 } else { 0 } } else { base - 4 * count };
    let mut loaded_pc = false;
    for register in 0..16usize {
        if list >> register & 1 == 0 {
            continue;
        }
        if load {
            if register == 15 {
                ctx.r[15] = mem.read32(at) & !3;
                loaded_pc = true;
            } else {
                ctx.r[register] = mem.read32(at);
            }
        } else {
            mem.write32(at, ctx.r[register]);
        }
        at += 4;
    }
    if writeback || !up {
        ctx.r[rn] = if up { base + 4 * count } else { base - 4 * count };
    }
    if loaded_pc {
        Step::Taken
    } else {
        Step::Next
    }
}

/// the svc, then MCR/MRC (cp15 answered: the TLS register, the barrier/cache sites, id reads)
fn coprocessor_or_svc(ctx: &mut Context, mem: &Memory, address: u32, opcode: u32, log: &mut dyn MissLog, svc: SvcSink) -> Step {
    // SVC: cond 1110 1111 imm24
    if opcode >> 24 & 0xF == 0xF {
        let number = opcode & 0x00FF_FFFF;
        ctx.r[15] = address + 4;
        svc(ctx, number);
        return Step::Svc(number);
    }
    let coproc = opcode >> 8 & 0xF;
    let load = opcode >> 20 & 1 != 0;
    let rd = (opcode >> 12 & 0xF) as usize;
    let crn = opcode >> 16 & 0xF;
    let crm = opcode & 0xF;
    let opc1 = opcode >> 21 & 0x7;
    let opc2 = opcode >> 5 & 0x7;
    if opcode >> 4 & 1 == 0 {
        if super::vfp::owns(opcode) {
            return super::vfp::data_processing(ctx, address, opcode, log);
        }
        log.miss(address, opcode, "a coprocessor data op");
        return Step::Unsupported;
    }
    if super::vfp::owns(opcode) {
        return super::vfp::register_transfer(ctx, mem, address, opcode, log);
    }
    if coproc == 15 {
        if load && opc1 == 0 && crn == 13 && opc2 == 3 && crm == 0 {
            // mrc p15, 0, rd, c13, c0, 3: the thread's TLS address
            ctx.r[rd] = ctx.tls;
            ctx.r[15] = address + 4;
            return Step::Next;
        }
        if !load && matches!(crn, 7 | 8 | 10) {
            // cache and TLB maintenance and barriers: nothing diverges on this host
            ctx.r[15] = address + 4;
            return Step::Next;
        }
        if load && crn == 0 && crm == 0 {
            ctx.r[rd] = 0;
            ctx.r[15] = address + 4;
            return Step::Next;
        }
        log.miss(address, opcode, "a cp15 form this host does not answer");
        return Step::Unsupported;
    }
    let _ = (load, opc1, opc2);
    log.miss(address, opcode, "a coprocessor this host does not have");
    Step::Unsupported
}

//! the VFP2 executor: singles as f32 and doubles as f64 rounding once, results flushed to zero
//! when fpscr asks, compares into the fpscr flags, conversions, and the short vectors the fpscr
//! length and stride shape (the same semantics recomp.h's helpers give the generated code).

use super::*;
use crate::context::Context;

/// one VFP instruction: the cdp space (the math), the mrc/mcr space (moves, fpscr), and the
/// ldc/stc space (vldr/vstr/vldm/vstm and the core-pair moves)
pub fn step(ctx: &mut Context, mem: &Memory, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    match opcode >> 25 & 0x7 {
        // vldr/vstr/vldm/vstm and the two-core-register moves
        6 => load_store(ctx, mem, address, opcode, log),
        // cdp: the arithmetic; mcr/mrc: the register moves
        7 => {
            if opcode >> 4 & 1 == 0 {
                data_processing(ctx, address, opcode, log)
            } else {
                register_transfer(ctx, mem, address, opcode, log)
            }
        }
        _ => {
            log.miss(address, opcode, "a vfp form this host does not run");
            Step::Unsupported
        }
    }
}

/// is this instruction VFP's at all (coprocessor 10 or 11); the dispatcher routes first
pub fn owns(opcode: u32) -> bool {
    matches!(opcode >> 8 & 0xF, 10 | 11)
}

fn regs(opcode: u32, double: bool) -> (usize, usize, usize) {
    let (vd, vn, vm) = ((opcode >> 12) & 0xF, (opcode >> 16) & 0xF, opcode & 0xF);
    let (d_bit, n_bit, m_bit) = (opcode & (1 << 22) != 0, opcode & (1 << 7) != 0, opcode & (1 << 5) != 0);
    if double {
        ((u32::from(d_bit) << 4 | vd) as usize & 15, (u32::from(n_bit) << 4 | vn) as usize & 15, (u32::from(m_bit) << 4 | vm) as usize & 15)
    } else {
        (((vd << 1) | d_bit as u32) as usize & 31, ((vn << 1) | n_bit as u32) as usize & 31, ((vm << 1) | m_bit as u32) as usize & 31)
    }
}

fn single_bits(ctx: &Context, r: usize) -> u32 {
    // safe: vfp points at the thread's 32 words, owned by the guest thread running this step
    unsafe { *ctx.vfp.add(r) }
}

fn set_single_bits(ctx: &mut Context, r: usize, bits: u32) {
    unsafe { *ctx.vfp.add(r) = bits }
}

/// a single's bits, flushed to zero when fpscr asks (recomp.h's vfp_to_s)
fn to_single(fpscr: u32, value: f32) -> u32 {
    let bits = value.to_bits();
    if fpscr & (1 << 24) != 0 && bits & 0x7F80_0000 == 0 {
        bits & 0x8000_0000
    } else {
        bits
    }
}

/// a double's bits, flushed the same way
fn to_double(fpscr: u32, value: f64) -> (u32, u32) {
    let bits = value.to_bits();
    let (low, high) = (bits as u32, (bits >> 32) as u32);
    if fpscr & (1 << 24) != 0 && bits & 0x7FF0_0000_0000_0000 == 0 {
        (0, high & 0x8000_0000)
    } else {
        (low, high)
    }
}

fn get_s(ctx: &Context, r: usize) -> f32 {
    f32::from_bits(single_bits(ctx, r))
}

fn set_s(ctx: &mut Context, r: usize, value: f32) {
    let fpscr = unsafe { *ctx.fpscr };
    set_single_bits(ctx, r, to_single(fpscr, value));
}

fn get_d(ctx: &Context, d: usize) -> f64 {
    f64::from_bits((single_bits(ctx, 2 * d) as u64) | (single_bits(ctx, 2 * d + 1) as u64) << 32)
}

fn set_d(ctx: &mut Context, d: usize, value: f64) {
    let fpscr = unsafe { *ctx.fpscr };
    let (low, high) = to_double(fpscr, value);
    set_single_bits(ctx, 2 * d, low);
    set_single_bits(ctx, 2 * d + 1, high);
}

/// a result's math, numbered like vfp_apply numbers them
fn apply_s(op: u32, a: f32, b: f32, acc: f32) -> f32 {
    match op {
        0 => acc + a * b,
        1 => acc - a * b,
        2 => -acc + a * b,
        3 => -acc - a * b,
        4 => a * b,
        5 => -(a * b),
        6 => a + b,
        7 => a - b,
        8 => a / b,
        _ => b.sqrt(),
    }
}

fn apply_d(op: u32, a: f64, b: f64, acc: f64) -> f64 {
    match op {
        0 => acc + a * b,
        1 => acc - a * b,
        2 => -acc + a * b,
        3 => -acc - a * b,
        4 => a * b,
        5 => -(a * b),
        6 => a + b,
        7 => a - b,
        8 => a / b,
        9 => b,
        10 => b.abs(),
        11 => -b,
        _ => b.sqrt(),
    }
}

/// the compare into the fpscr flags, nzcv style (recomp.h's vfp_compared)
fn compare(ctx: &mut Context, a: f64, b: f64) {
    let flags = if a != a || b != b {
        0x3
    } else if a == b {
        0x6
    } else if a < b {
        0x8
    } else {
        0x2
    };
    let fpscr = unsafe { *ctx.fpscr };
    unsafe { *ctx.fpscr = (fpscr & 0x0FFF_FFFF) | flags << 28 };
}

/// to int, toward zero, saturating (recomp.h's vfp_to_s32 / vfp_to_u32)
fn to_s32(value: f64) -> u32 {
    if value != value {
        return 0;
    }
    if value >= 2147483647.0 {
        return 0x7FFF_FFFF;
    }
    if value <= -2147483648.0 {
        return 0x8000_0000;
    }
    value as i32 as u32
}

fn to_u32(value: f64) -> u32 {
    if value != value {
        return 0;
    }
    if value >= 4294967295.0 {
        return 0xFFFF_FFFF;
    }
    if value <= 0.0 {
        return 0;
    }
    value as u32
}

fn to_int_round(value: f64, mode: u32, signed: bool) -> u32 {
    let rounded = match mode {
        0 => value.round(),                    // nearest, ties to even
        1 => value.ceil(),                     // toward +inf
        2 => value.floor(),                    // toward -inf
        _ => value,                            // toward zero
    };
    if signed {
        to_s32(rounded)
    } else {
        to_u32(rounded)
    }
}

/// an op on a short vector shaped by fpscr's len and stride (recomp.h's vfp_vector)
fn vector(ctx: &mut Context, op: u32, wide: bool, d: usize, n: usize, m: usize) {
    let fpscr = unsafe { *ctx.fpscr };
    let length = ((fpscr >> 16) & 7) as usize + 1;
    let stride = if (fpscr >> 20) & 3 == 3 { 2 } else { 1 };
    let bank = if wide { 4 } else { 8 };
    for i in 0..length {
        let dd = step_reg(d, bank, i, stride);
        let nn = step_reg(n, bank, i, stride);
        let mm = if m < bank { m } else { step_reg(m, bank, i, stride) };
        if wide && (9..=11).contains(&op) {
            let low = single_bits(ctx, 2 * (mm & 15));
            let high = single_bits(ctx, 2 * (mm & 15) + 1);
            set_single_bits(ctx, 2 * (dd & 15), low);
            set_single_bits(ctx, 2 * (dd & 15) + 1, if op == 9 { high } else if op == 10 { high & 0x7FFF_FFFF } else { high ^ 0x8000_0000 });
        } else if wide {
            set_d(ctx, dd & 15, apply_d(op, get_d(ctx, nn & 15), get_d(ctx, mm & 15), get_d(ctx, dd & 15)));
        } else if (9..=11).contains(&op) {
            let bits = single_bits(ctx, mm);
            set_single_bits(ctx, dd, if op == 9 { bits } else if op == 10 { bits & 0x7FFF_FFFF } else { bits ^ 0x8000_0000 });
        } else {
            set_s(ctx, dd, apply_s(op, get_s(ctx, nn), get_s(ctx, mm), get_s(ctx, dd)));
        }
    }
}

/// where a reg is on step i of a short vector, wrapping around its bank (recomp.h's vfp_step)
fn step_reg(reg: usize, bank: usize, i: usize, stride: usize) -> usize {
    (reg & !(bank - 1)) | ((reg + i * stride) & (bank - 1))
}

/// the arithmetic: same decode the generated C uses, ((op >> 20) & 0b1011, bit6) names the math
pub(crate) fn data_processing(ctx: &mut Context, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    let coprocessor = opcode >> 8 & 0xF;
    if !owns(opcode) {
        log.miss(address, opcode, "a coprocessor data op this host does not have");
        return Step::Unsupported;
    }
    let double = coprocessor == 11;
    let (rd, rn, rm) = regs(opcode, double);
    let negate = opcode & (1 << 6) != 0;
    let op = match ((opcode >> 20) & 0b1011, negate) {
        (0b0000, false) => 0,
        (0b0000, true) => 1,
        (0b0001, false) => 2,
        (0b0001, true) => 3,
        (0b0010, false) => 4,
        (0b0010, true) => 5,
        (0b0011, false) => 6,
        (0b0011, true) => 7,
        (0b1000, false) => 8,
        (0b1011, true) => return extension(ctx, address, opcode, double, rd, rm, log),
        _ => {
            log.miss(address, opcode, "a vfp data form this host does not run");
            return Step::Unsupported;
        }
    };
    // only a dest past the first bank can be a short vector, like the generated code decides
    let bank = if double { 4 } else { 8 };
    if rd < bank {
        if double {
            set_d(ctx, rd, apply_d(op, get_d(ctx, rn), get_d(ctx, rm), get_d(ctx, rd)));
        } else {
            set_s(ctx, rd, apply_s(op, get_s(ctx, rn), get_s(ctx, rm), get_s(ctx, rd)));
        }
    } else {
        vector(ctx, op, double, rd, rn, rm);
    }
    ctx.r[15] = address + 4;
    Step::Next
}

/// vmov, vabs, vneg, vsqrt, vcmp and the vcvt forms
fn extension(ctx: &mut Context, address: u32, opcode: u32, double: bool, rd: usize, rm: usize, log: &mut dyn MissLog) -> Step {
    let top = opcode & (1 << 7) != 0;
    let vn = opcode >> 16 & 0xF;
    match (vn, top) {
        // vmov, vabs, vneg and vsqrt: moves and sign ops leave bits untouched, doubles included
        (0b0000 | 0b0001, _) => {
            let second = vn & 1 != 0;
            let op = 9 + ((second as u32) << 1 | top as u32);
            if double {
                if op == 12 {
                    let value = get_d(ctx, rm).sqrt();
                    set_d(ctx, rd, value);
                } else {
                    let (low, high) = (single_bits(ctx, 2 * rm), single_bits(ctx, 2 * rm + 1));
                    let high = match (second, top) {
                        (false, false) => high,
                        (false, true) => high & 0x7FFF_FFFF,
                        (true, false) => high ^ 0x8000_0000,
                        _ => unreachable!("op 12 handled above"),
                    };
                    set_single_bits(ctx, 2 * rd, low);
                    set_single_bits(ctx, 2 * rd + 1, high);
                }
            } else if op == 12 {
                let value = get_s(ctx, rm).sqrt();
                set_s(ctx, rd, value);
            } else {
                let bits = single_bits(ctx, rm);
                set_single_bits(ctx, rd, if op == 9 { bits } else if op == 10 { bits & 0x7FFF_FFFF } else { bits ^ 0x8000_0000 });
            }
            // a vector of the sign ops runs the same loop the generated code uses
            let bank = if double { 4 } else { 8 };
            if rd >= bank && op >= 9 {
                vector(ctx, op, double, rd, rd, rm);
            }
        }
        // vcmp against a register
        (0b0100, _) => {
            if double {
                let (a, b) = (get_d(ctx, rd), get_d(ctx, rm));
                compare(ctx, a, b);
            } else {
                let (a, b) = (get_s(ctx, rd) as f64, get_s(ctx, rm) as f64);
                compare(ctx, a, b);
            }
        }
        // vcmp against zero
        (0b0101, _) => {
            if double {
                let a = get_d(ctx, rd);
                compare(ctx, a, 0.0);
            } else {
                let a = get_s(ctx, rd) as f64;
                compare(ctx, a, 0.0);
            }
        }
        // vcvt between single and double
        (0b0111, true) => {
            if double {
                let value = get_d(ctx, rm);
                let vd = ((opcode >> 12 & 0xF) << 1 | (opcode >> 22 & 1) as u32) as usize & 31;
                set_s(ctx, vd, value as f32);
            } else {
                let value = get_s(ctx, ((opcode & 0xF) << 1 | (opcode >> 5 & 1) as u32) as usize & 31);
                let vd = ((u32::from(opcode >> 22 & 1 != 0) << 4) | opcode >> 12 & 0xF) as usize & 15;
                set_d(ctx, vd, value as f64);
            }
        }
        // vcvt from an int, bit 7 set means signed
        (0b1000, _) => {
            let source = ((opcode & 0xF) << 1 | (opcode >> 5 & 1) as u32) as usize & 31;
            let raw = single_bits(ctx, source);
            let value = if top { (raw as i32) as f64 } else { raw as f64 };
            if double {
                set_d(ctx, rd, value);
            } else {
                set_s(ctx, rd, value as f32);
            }
        }
        // vcvt to an int: toward zero is what the codegen lowers, the other roundings run here
        (0b1000..=0b1101, true) if vn & 0b1100 == 0b1100 || vn & 0b1000 == 0b1000 => {
            if matches!(vn, 0b1100 | 0b1101) {
                // toward zero, exactly the generated code's conversion
                let value = if double { get_d(ctx, rm) } else { get_s(ctx, rm) as f64 };
                let result = if vn & 1 != 0 { to_s32(value) } else { to_u32(value) };
                let vd = ((opcode >> 12 & 0xF) << 1 | (opcode >> 22 & 1) as u32) as usize & 31;
                set_single_bits(ctx, vd, result);
            } else {
                // the rounding modes the codegen leaves here
                let value = if double { get_d(ctx, rm) } else { get_s(ctx, rm) as f64 };
                let mode = vn >> 1 & 0x3;
                let signed = vn & 1 != 0;
                let result = to_int_round(value, mode, signed);
                let vd = ((opcode >> 12 & 0xF) << 1 | (opcode >> 22 & 1) as u32) as usize & 31;
                set_single_bits(ctx, vd, result);
            }
        }
        _ => {
            log.miss(address, opcode, "a vfp extension form this host does not run");
            return Step::Unsupported;
        }
    }
    ctx.r[15] = address + 4;
    Step::Next
}

/// vmrs/vmsr on fpscr, and vmov between a core register and a single
pub(crate) fn register_transfer(ctx: &mut Context, mem: &Memory, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    let coprocessor = opcode >> 8 & 0xF;
    let load = opcode & (1 << 20) != 0;
    let rt = (opcode >> 12 & 0xF) as usize;
    let opc1 = opcode >> 21 & 0x7;
    if !owns(opcode) {
        log.miss(address, opcode, "a coprocessor register transfer this host does not have");
        return Step::Unsupported;
    }
    match (coprocessor, opc1) {
        // vmrs and vmsr on fpscr (the codegen folds a vmsr constant in through its own literal)
        (_, 7) if opcode >> 16 & 0xF == 1 => {
            if !load {
                let value = if rt == 15 { mem.read32(address + 8) } else { ctx.r[rt] };
                unsafe { *ctx.fpscr = value };
            } else if rt == 15 {
                let f = unsafe { *ctx.fpscr };
                ctx.n = (f >> 31 & 1) as u8;
                ctx.z = (f >> 30 & 1) as u8;
                ctx.c = (f >> 29 & 1) as u8;
                ctx.v = (f >> 28 & 1) as u8;
            } else {
                ctx.r[rt] = unsafe { *ctx.fpscr };
            }
            ctx.r[15] = address + 4;
            Step::Next
        }
        // vmov between a core reg and a single
        (_, 0) if rt != 15 => {
            let sn = ((opcode >> 16 & 0xF) << 1 | (opcode >> 7 & 1) as u32) as usize & 31;
            if load {
                ctx.r[rt] = single_bits(ctx, sn);
            } else {
                set_single_bits(ctx, sn, ctx.r[rt]);
            }
            ctx.r[15] = address + 4;
            Step::Next
        }
        _ => {
            log.miss(address, opcode, "a vfp register transfer this host does not run");
            Step::Unsupported
        }
    }
}

/// vldr/vstr/vldm/vstm, and vmov between two core registers and a double or two singles
fn load_store(ctx: &mut Context, mem: &Memory, address: u32, opcode: u32, log: &mut dyn MissLog) -> Step {
    let coprocessor = opcode >> 8 & 0xF;
    if !owns(opcode) {
        log.miss(address, opcode, "a coprocessor load/store this host does not have");
        return Step::Unsupported;
    }
    let double = coprocessor == 11;
    // vmov rt, rt2, dm (or the reverse): cond 1100 010L rt2 rt 101x M1 vm
    if opcode >> 21 & 0x7F == 0b110_0010 {
        let load = opcode & (1 << 20) != 0;
        let rt = (opcode >> 12 & 0xF) as usize;
        let rt2 = (opcode >> 16 & 0xF) as usize;
        if rt == 15 || rt2 == 15 {
            log.miss(address, opcode, "a core-pair move naming pc");
            return Step::Unsupported;
        }
        let (vm, m_bit) = (opcode & 0xF, opcode & (1 << 5) != 0);
        let (low, high) = if double {
            let dm = ((u32::from(m_bit) << 4) | vm) as usize & 15;
            ((dm * 2) & 31, (dm * 2 + 1) & 31)
        } else {
            let sm = ((vm << 1) | m_bit as u32) as usize & 31;
            (sm, (sm + 1) & 31)
        };
        if load {
            ctx.r[rt] = single_bits(ctx, low);
            ctx.r[rt2] = single_bits(ctx, high);
        } else {
            set_single_bits(ctx, low, ctx.r[rt]);
            set_single_bits(ctx, high, ctx.r[rt2]);
        }
        ctx.r[15] = address + 4;
        return Step::Next;
    }
    let load = opcode & (1 << 20) != 0;
    let pre = opcode & (1 << 24) != 0;
    let up = opcode & (1 << 23) != 0;
    let writeback = opcode & (1 << 21) != 0;
    let rn = (opcode >> 16 & 0xF) as usize;
    let offset = (opcode & 0xFF) * 4;
    let (vd, d_bit) = ((opcode >> 12 & 0xF) as usize, opcode & (1 << 22) != 0);
    let first = if double { (u32::from(d_bit) << 4 | vd as u32) as usize & 15 } else { ((vd as u32) << 1 | d_bit as u32) as usize & 31 };
    let count = if pre && !writeback {
        1
    } else if double {
        (opcode & 0xFF) as usize / 2
    } else {
        (opcode & 0xFF) as usize
    };
    let base = if rn == 15 { address + 8 } else { ctx.r[rn] };
    let start = match (up, pre && !writeback) {
        (true, true) => base + offset,
        (true, false) => base,
        (false, _) => base - offset,
    };
    // the registers as the words they occupy: a double is two singles at 2N and 2N + 1
    let mut words: Vec<usize> = Vec::new();
    for i in 0..count.max(1) {
        if double {
            words.push(((first + i) * 2) & 31);
            words.push(((first + i) * 2 + 1) & 31);
        } else {
            words.push((first + i) & 31);
        }
    }
    let mut at = start;
    for r in &words {
        if load {
            let bits = mem.read32(at);
            set_single_bits(ctx, *r, bits);
        } else {
            mem.write32(at, single_bits(ctx, *r));
        }
        at += 4;
    }
    if writeback {
        ctx.r[rn] = if up { base + offset } else { base - offset };
    }
    ctx.r[15] = address + 4;
    Step::Next
}

//! the ARM11 interpreter: single-instruction steps for the recompiler's `interpret` callback and
//! block mode for code no entry covers (plan §4: new code, the ARM64 interpreter does not
//! transfer). The instruction semantics follow recomp.h's own helpers where the generated code
//! uses them (shifts, saturates, VFP apply/flush/vectors), so the two sides of the differential
//! share one definition of every operation; the decoders follow the ARMv6-K architecture.

mod arm;
mod thumb;
mod vfp;

use crate::context::Context;
use crate::mem::Memory;

/// what one instruction did
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// execution goes on at r15 (already advanced)
    Next,
    /// control moved: r15 says where, the caller takes over
    Taken,
    /// an svc ran; the number, the host handles it (the interpreter never answers in place)
    Svc(u32),
    /// something this host does not run (an unimplemented coprocessor, an exception return);
    /// the guest stopped where it was and the reason is in the miss log
    Unsupported,
}

/// who the interpreter asks when the guest runs an svc
pub type SvcSink<'a> = &'a mut dyn FnMut(&mut Context, u32);

/// where misses get counted (the hot-set recorder and the logs)
pub trait MissLog {
    fn miss(&mut self, address: u32, opcode: u32, why: &str);
}

/// the no-op log (golden tests use it)
pub struct NoLog;
impl MissLog for NoLog {
    fn miss(&mut self, _address: u32, _opcode: u32, _why: &str) {}
}

pub struct Flags {
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
}

/// condition codes (recomp.h's C_* macros)
pub fn cond_holds(ctx: &Context, cond: u32) -> bool {
    match cond {
        0x0 => ctx.z != 0,
        0x1 => ctx.z == 0,
        0x2 => ctx.c != 0,
        0x3 => ctx.c == 0,
        0x4 => ctx.n != 0,
        0x5 => ctx.n == 0,
        0x6 => ctx.v != 0,
        0x7 => ctx.v == 0,
        0x8 => ctx.c != 0 && ctx.z == 0,
        0x9 => ctx.c == 0 || ctx.z != 0,
        0xA => (ctx.n != 0) == (ctx.v != 0),
        0xB => (ctx.n != 0) != (ctx.v != 0),
        0xC => ctx.z == 0 && (ctx.n != 0) == (ctx.v != 0),
        0xD => ctx.z != 0 || (ctx.n != 0) != (ctx.v != 0),
        _ => true,
    }
}

/// how an operand is shifted
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftKind {
    Lsl,
    Lsr,
    Asr,
    Ror,
    /// RRX: rotate right one through the carry
    Rrx,
}

/// a shift: the amount is immediate, or read from a register's bottom byte at run time
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shift {
    pub kind: ShiftKind,
    pub amount: u32,
    pub by_register: bool,
}

/// applies a shift to a value, carry out included (recomp.h's shift_* helpers)
pub fn shift_value(value: u32, shift: &Shift, carry_in: bool, amount: u32) -> (u32, bool) {
    match shift.kind {
        ShiftKind::Lsl => match amount {
            0 => (value, carry_in),
            n @ 1..32 => (value << n, (value >> (32 - n)) & 1 != 0),
            32 => (0, value & 1 != 0),
            _ => (0, false),
        },
        ShiftKind::Lsr => match amount {
            0 => (value, carry_in),
            n @ 1..32 => (value >> n, (value >> (n - 1)) & 1 != 0),
            32 => (0, value >> 31 != 0),
            _ => (0, false),
        },
        ShiftKind::Asr => match amount {
            0 => (value, carry_in),
            n @ 1..32 => (((value as i32) >> n) as u32, ((value as i32) >> (n - 1)) & 1 != 0),
            _ => ((value as i32 >> 31) as u32, value >> 31 != 0),
        },
        ShiftKind::Ror => match amount & 31 {
            0 if amount == 0 => (value, carry_in),
            0 => (value, value >> 31 != 0),
            n => (value.rotate_right(n), (value >> (n - 1)) & 1 != 0),
        },
        ShiftKind::Rrx => (((value >> 1) | ((carry_in as u32) << 31)), value & 1 != 0),
    }
}

/// the shift's run-time amount: the immediate, or the register's bottom byte
fn shift_amount(shift: &Shift, register_value: u32) -> u32 {
    if shift.by_register {
        register_value & 0xFF
    } else {
        shift.amount
    }
}

/// the add carry and overflow, cin folded in (overflow: both operands share a sign the sum lost)
fn add_flags(a: u32, b: u32, carry_in: bool) -> (u32, bool, bool) {
    let (s1, c1) = a.overflowing_add(b);
    let (sum, c2) = s1.overflowing_add(carry_in as u32);
    let overflow = ((a ^ sum) & (b ^ sum)) >> 31 != 0;
    (sum, c1 || c2, overflow)
}

/// the sub carry (not borrow: a - b - borrow >= 0) and overflow
fn sub_flags(a: u32, b: u32, borrow_in: bool) -> (u32, bool, bool) {
    let borrow = borrow_in as u32;
    let (d1, borrow1) = a.overflowing_sub(b);
    let (diff, borrow2) = d1.overflowing_sub(borrow);
    let carry = !(borrow1 || borrow2);
    let overflow = ((a ^ b) & (a ^ diff)) >> 31 != 0;
    (diff, carry, overflow)
}

/// a data-processing result into n and z
#[allow(dead_code)]
fn set_nz(ctx: &mut Context, value: u32) {
    ctx.n = (value >> 31 != 0) as u8;
    ctx.z = (value == 0) as u8;
}

/// the value of register i where an instruction reads it: r15 holds the next instruction's
/// address, an operand read sees the current one plus 8 in ARM and plus 4 in Thumb
fn reg_read(ctx: &Context, index: usize, address: u32, thumb: bool) -> u32 {
    if index == 15 {
        address + if thumb { 4 } else { 8 }
    } else {
        ctx.r[index]
    }
}

/// writes r15 for a jump in ARM state (word aligned, state unchanged) or anywhere else
fn reg_write(ctx: &mut Context, index: usize, value: u32) {
    if index == 15 {
        ctx.r[15] = value & !3;
    } else {
        ctx.r[index] = value;
    }
}

/// an ARM jump: pc to the target, state per the low bit
fn jump_arm(ctx: &mut Context, target: u32) {
    ctx.thumb = (target & 1) as u8;
    ctx.r[15] = target & if target & 1 != 0 { !1 } else { !3 };
}

/// a Thumb jump: stays Thumb unless the target says otherwise (BX)
#[allow(dead_code)]
fn jump_thumb(ctx: &mut Context, target: u32, switch: bool) {
    if switch {
        jump_arm(ctx, target);
    } else {
        ctx.r[15] = target & !1;
    }
}

/// one instruction, at address, in the state ctx already carries (thumb in ctx.thumb)
/// the budget is the caller's: block mode charges per instruction, single steps are already
/// counted in the block the recompiled code charged
pub fn step(ctx: &mut Context, mem: &Memory, address: u32, log: &mut dyn MissLog, svc: SvcSink) -> Step {
    if ctx.thumb != 0 {
        thumb::step(ctx, mem, address, log, svc)
    } else {
        arm::step(ctx, mem, address, log, svc)
    }
}

/// block mode: runs from address until the budget runs out, an svc needs the host, an
/// unsupported instruction shows up, or the code runs into an entry (the caller's lookup says so)
/// returns how many instructions it ran
pub fn run_block(
    ctx: &mut Context,
    mem: &Memory,
    lookup: &dyn Fn(u32) -> Option<crate::context::Code>,
    log: &mut dyn MissLog,
    svc: SvcSink,
    cap: u32,
) -> u32 {
    let mut ran = 0;
    while ctx.budget > 0 && ran < cap {
        let address = ctx.r[15];
        let thumb = ctx.thumb != 0;
        let covered = if thumb { lookup(address & !1 | 1) } else { lookup(address & !3) };
        if covered.is_some() {
            break;
        }
        let stepped = step(ctx, mem, address, log, svc);
        ran += 1;
        ctx.budget -= 1;
        if !matches!(stepped, Step::Next) {
            break;
        }
    }
    ran
}

/// the tick a host running one instruction per tick would report (the differential's svc answer)
pub fn tick_of(budget: i32, end: u64) -> u64 {
    end.wrapping_sub(budget as i64 as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the shift helpers match recomp.h's, carry included
    #[test]
    fn shifts_match_the_headers_helpers() {
        let imm = |kind: ShiftKind, amount: u32| Shift { kind, amount, by_register: false };
        let (value, carry) = shift_value(0x8000_0001, &imm(ShiftKind::Lsl, 1), false, 1);
        assert_eq!((value, carry), (0x2, true));
        let (value, carry) = shift_value(0x8000_0001, &imm(ShiftKind::Lsr, 1), false, 1);
        assert_eq!((value, carry), (0x4000_0000, true));
        let (value, carry) = shift_value(0x8000_0000, &imm(ShiftKind::Asr, 31), false, 31);
        assert_eq!((value, carry), (0xFFFF_FFFF, false), "the carry is the last bit shifted out");
        let (value, carry) = shift_value(0x8000_0001, &imm(ShiftKind::Ror, 1), false, 1);
        assert_eq!((value, carry), (0xC000_0000, true));
        let (value, carry) = shift_value(0x8000_0000, &imm(ShiftKind::Rrx, 0), true, 0);
        assert_eq!((value, carry), (0xC000_0000, false), "the carry out is the value's bit 0");
        // by-register amounts read the bottom byte, 32 and 0 differ
        assert_eq!(shift_value(5, &imm(ShiftKind::Lsl, 32), false, 32).0, 0);
        assert_eq!(shift_value(5, &imm(ShiftKind::Lsl, 0), false, 0).0, 5);
        assert_eq!(shift_value(0x8000_0000, &imm(ShiftKind::Lsr, 32), false, 32), (0, true));
        assert_eq!(shift_value(1, &imm(ShiftKind::Asr, 32), false, 32), (0, false));
        assert_eq!(shift_value(0xFFFF_FFFF, &imm(ShiftKind::Asr, 40), false, 40), (0xFFFF_FFFF, true));
        // ror by 32 leaves the value, carry from the top
        assert_eq!(shift_value(0x8000_0000, &imm(ShiftKind::Ror, 32), false, 32), (0x8000_0000, true));
        // a register-controlled shift's amount is the run-time one
        let by_reg = Shift { kind: ShiftKind::Lsl, amount: 7, by_register: true };
        assert_eq!(shift_value(3, &by_reg, false, 2), (12, false));
    }

    /// the add and sub flags, carries folded in
    #[test]
    fn add_and_sub_flags_carry_in() {
        let (sum, carry, overflow) = add_flags(0xFFFF_FFFF, 1, false);
        assert_eq!((sum, carry, overflow), (0, true, false));
        let (sum, carry, overflow) = add_flags(0x7FFF_FFFF, 1, false);
        assert_eq!((sum, carry, overflow), (0x8000_0000, false, true));
        let (_, carry, _) = add_flags(0xFFFF_FFFF, 0, true);
        assert!(carry, "the carry-in alone can carry out");
        let (diff, carry, overflow) = sub_flags(0, 1, false);
        assert_eq!((diff, carry, overflow), (0xFFFF_FFFF, false, false));
        let (_, carry, _) = sub_flags(0, 0, true);
        assert!(!carry, "borrowing from zero");
        let (_, _, overflow) = sub_flags(0x8000_0000, 1, false);
        assert!(overflow);
    }

    /// the condition codes, exactly recomp.h's C_* macros
    #[test]
    fn conditions_match_the_macros() {
        let mut ctx = boxed_context();
        ctx.n = 0;
        ctx.z = 1;
        ctx.c = 0;
        ctx.v = 0;
        assert!(cond_holds(&ctx, 0x0)); // EQ
        assert!(!cond_holds(&ctx, 0x1)); // NE
        assert!(!cond_holds(&ctx, 0x2)); // CS
        assert!(cond_holds(&ctx, 0x3)); // CC
        assert!(!cond_holds(&ctx, 0x8)); // HI
        assert!(cond_holds(&ctx, 0x9)); // LS
        assert!(cond_holds(&ctx, 0xA)); // GE
        assert!(cond_holds(&ctx, 0xD)); // LE (z is set)
        ctx.n = 1;
        ctx.v = 1;
        assert!(cond_holds(&ctx, 0xA)); // GE
        assert!(!cond_holds(&ctx, 0xB)); // LT needs the signs to differ
        ctx.z = 0;
        assert!(cond_holds(&ctx, 0xC)); // GT
        assert!(cond_holds(&ctx, 0xE)); // AL
        assert!(cond_holds(&ctx, 0xF)); // the never cond never reaches the check
    }

    pub(crate) fn boxed_context() -> Box<Context> {
        Box::new(Context {
            r: [0; 16],
            n: 0,
            z: 0,
            c: 0,
            v: 0,
            q: 0,
            thumb: 0,
            ge: 0,
            exclusive: 0,
            budget: 0,
            exit: 0,
            svc: 0,
            depth: 0,
            exclusive_address: 0,
            tls: 0,
            read_pages: std::ptr::null(),
            write_pages: std::ptr::null(),
            vfp: std::ptr::null_mut(),
            fpscr: std::ptr::null_mut(),
            host: std::ptr::null(),
            user: std::ptr::null_mut(),
            stack_limit: 0,
            tick_end: 0,
            tick_last: 0,
            tick_floor: 0,
            tick_cost: 0,
            tick_at: 0,
            ticks: 0,
            wait_at: 0,
            release_at: 0,
            held: 0,
            holding: 0,
            held_floor: 0,
            done: 0,
            trail: [0; crate::context::TRAIL],
        })
    }
}

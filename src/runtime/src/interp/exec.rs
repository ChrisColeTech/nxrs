//! One AArch64 instruction, executed: the base instruction set (data processing, branches, system, loads and stores,
//! exclusives and atomics), with the same decode and the same semantics as the translator's lifter
//! (`src/translator/src/xlate/lifter.rs`): an encoding the lifter refuses (an `rt_udf` trap in lifted code) is refused
//! here too, and everything it lowers behaves as its C does. FP / SIMD is `interp::simd`.

use crate::cpu::Cpu;
use crate::interp::mem::{self, ld, st};
use crate::interp::xutil::{bits, decode_bit_masks, ones, sext};
use core::sync::atomic::Ordering;

/// What an instruction does to the flow of control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flow {
    /// On to the next instruction.
    Next,
    /// A jump (B, a taken conditional branch, BR, a context restore's RET).
    Jump(u64),
    /// A call (BL, BLR): x30 is set already.
    Call(u64),
    /// RET: back to the caller (the lifted code's host-stack call model: a function returns to whoever called it).
    Ret,
    /// SVC #imm.
    Svc(u32),
    /// BRK #imm.
    Brk(u32),
    /// An encoding the lifter does not lower either (lifted code traps with `rt_udf`).
    Udf(String),
}

pub type R<T> = Result<T, String>;

pub fn unsup<T>(m: impl Into<String>) -> R<T> {
    Err(m.into())
}

// ---- registers (cpu.h / util.rs `x`, `xs`, `w`, `reg`, `set_reg`)

#[inline]
pub fn x(c: &Cpu, r: u32) -> u64 {
    if r == 31 {
        0
    } else {
        c.x[r as usize]
    }
}
#[inline]
pub fn xs(c: &Cpu, r: u32) -> u64 {
    if r == 31 {
        c.sp
    } else {
        c.x[r as usize]
    }
}
#[inline]
pub fn wv(c: &Cpu, r: u32) -> u64 {
    x(c, r) as u32 as u64
}
#[inline]
pub fn reg(c: &Cpu, sf: u32, r: u32, sp: bool) -> u64 {
    let v = if sp { xs(c, r) } else { x(c, r) };
    if sf != 0 {
        v
    } else {
        v as u32 as u64
    }
}
/// Writes Rd (zero-extending a 32-bit result); register 31 is SP when `sp`, else the write is dropped.
#[inline]
pub fn set_reg(c: &mut Cpu, sf: u32, r: u32, v: u64, sp: bool) {
    let v = if sf != 0 { v } else { v as u32 as u64 };
    if r == 31 {
        if sp {
            c.sp = v;
        }
        return;
    }
    c.x[r as usize] = v;
}
fn set_base(c: &mut Cpu, rn: u32, v: u64) {
    if rn == 31 {
        c.sp = v
    } else {
        c.x[rn as usize] = v
    }
}

// ---- flags (cpu.h)

pub fn mkflags(n: bool, z: bool, cy: bool, v: bool) -> u64 {
    (n as u64) << 31 | (z as u64) << 30 | (cy as u64) << 29 | (v as u64) << 28
}
pub fn cond(c: &Cpu, cc: u32) -> bool {
    let f = c.nzcv;
    let (n, z, cy, v) = ((f >> 31) & 1 != 0, (f >> 30) & 1 != 0, (f >> 29) & 1 != 0, (f >> 28) & 1 != 0);
    match cc {
        0 => z,
        1 => !z,
        2 => cy,
        3 => !cy,
        4 => n,
        5 => !n,
        6 => v,
        7 => !v,
        8 => cy && !z,
        9 => !(cy && !z),
        10 => n == v,
        11 => n != v,
        12 => !z && n == v,
        13 => !(!z && n == v),
        _ => true,
    }
}
fn size_bits(sf: u32) -> u32 {
    if sf != 0 {
        64
    } else {
        32
    }
}
/// adds / subs of `size` bits: the result, NZCV set.
pub fn addsub_flags(c: &mut Cpu, sf: u32, a: u64, b: u64, sub: bool) -> u64 {
    let n = size_bits(sf);
    let m = ones(n);
    let (a, b) = (a & m, b & m);
    let top = n - 1;
    if sub {
        let r = a.wrapping_sub(b) & m;
        c.nzcv = mkflags(r >> top & 1 != 0, r == 0, a >= b, ((a ^ b) & (a ^ r)) >> top & 1 != 0);
        r
    } else {
        let r = a.wrapping_add(b) & m;
        c.nzcv = mkflags(r >> top & 1 != 0, r == 0, r < a, ((a ^ r) & (b ^ r)) >> top & 1 != 0);
        r
    }
}
fn logic_flags(c: &mut Cpu, sf: u32, r: u64) -> u64 {
    let n = size_bits(sf);
    let r = r & ones(n);
    c.nzcv = mkflags(r >> (n - 1) & 1 != 0, r == 0, false, false);
    r
}
fn carry(c: &Cpu) -> u64 {
    (c.nzcv >> 29) & 1
}

fn ror(v: u64, r: u32, size: u32) -> u64 {
    let v = v & ones(size);
    let r = r % size;
    if r == 0 {
        v
    } else {
        ((v >> r) | (v << (size - r))) & ones(size)
    }
}

/// util.rs `shift_expr` on a value of `size` bits.
fn shift(sf: u32, v: u64, typ: u32, amt: u32) -> u64 {
    let size = size_bits(sf);
    if amt == 0 && typ != 3 {
        return v;
    }
    let m = ones(size);
    match typ {
        0 => v.wrapping_shl(amt) & m,
        1 => (v & m).wrapping_shr(amt),
        2 => {
            if sf != 0 {
                ((v as i64).wrapping_shr(amt)) as u64
            } else {
                ((v as u32 as i32).wrapping_shr(amt)) as u32 as u64
            }
        }
        _ => ror(v, amt, size),
    }
}

/// util.rs `extend_expr`.
pub fn extend(c: &Cpu, r: u32, option: u32, sh: u32) -> u64 {
    let b = x(c, r);
    let e = match option {
        0 => b as u8 as u64,
        1 => b as u16 as u64,
        2 => b as u32 as u64,
        4 => b as u8 as i8 as i64 as u64,
        5 => b as u16 as i16 as i64 as u64,
        6 => b as u32 as i32 as i64 as u64,
        _ => b,
    };
    e.wrapping_shl(sh)
}

fn pc_rel(pc: u64, off: i64) -> u64 {
    pc.wrapping_add(off as u64)
}

/// The rt_pacga code (cpu.h): a fixed mix of the value and the modifier.
fn pacga(v: u64, m: u64) -> u32 {
    let h = (v ^ m.wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    ((h ^ (h >> 29)) >> 32) as u32
}

/// Executes the instruction `word` at `pc` on `c` (the lifter's `lift` decode).
///
/// # Safety
/// `c` is the calling thread's guest state; guest memory is host memory.
pub unsafe fn step(c: &mut Cpu, word: u32, pc: u64) -> Flow {
    let op0 = bits(word, 25, 4);
    let r = if (op0 & 0b1110) == 0b1000 {
        dp_imm(c, word, pc).map(|_| Flow::Next)
    } else if (op0 & 0b1110) == 0b1010 {
        branch_sys(c, word, pc)
    } else if (op0 & 0b0101) == 0b0100 {
        ldst(c, word, pc).map(|_| Flow::Next)
    } else if (op0 & 0b0111) == 0b0101 {
        dp_reg(c, word).map(|_| Flow::Next)
    } else if (op0 & 0b0111) == 0b0111 {
        crate::interp::simd::dp(c, word).map(|_| Flow::Next)
    } else {
        Err(format!("unallocated {word:08x}"))
    };
    r.unwrap_or_else(Flow::Udf)
}

// ------------------------------------------------------------ data processing (imm)

fn dp_imm(c: &mut Cpu, w: u32, pc: u64) -> R<()> {
    let op = bits(w, 23, 3);
    let sf = bits(w, 31, 1);
    let (rd, rn) = (bits(w, 0, 5), bits(w, 5, 5));
    match op {
        0 | 1 => {
            let imm = sext(((bits(w, 5, 19) as u64) << 2) | bits(w, 29, 2) as u64, 21);
            let val = if bits(w, 31, 1) != 0 { (pc & !0xFFF).wrapping_add((imm << 12) as u64) } else { pc_rel(pc, imm) };
            set_reg(c, 1, rd, val, false);
        }
        2 => {
            let (opc, s, sh) = (bits(w, 30, 1), bits(w, 29, 1), bits(w, 22, 1));
            let imm = (bits(w, 10, 12) as u64) << if sh != 0 { 12 } else { 0 };
            let a = reg(c, sf, rn, true);
            if s != 0 {
                let r = addsub_flags(c, sf, a, imm, opc != 0);
                set_reg(c, sf, rd, r, false);
            } else {
                let r = if opc != 0 { a.wrapping_sub(imm) } else { a.wrapping_add(imm) };
                set_reg(c, sf, rd, r, true);
            }
        }
        4 => {
            let (opc, n) = (bits(w, 29, 2), bits(w, 22, 1));
            if sf == 0 && n != 0 {
                return unsup("logical imm N=1 32-bit");
            }
            let size = size_bits(sf);
            let (imm, _) = decode_bit_masks(n, bits(w, 10, 6), bits(w, 16, 6), size, true).map_err(|e| e.0)?;
            let a = reg(c, sf, rn, false);
            match opc {
                0 => set_reg(c, sf, rd, a & imm, true),
                1 => set_reg(c, sf, rd, a | imm, true),
                2 => set_reg(c, sf, rd, a ^ imm, true),
                _ => {
                    let r = logic_flags(c, sf, a & imm);
                    set_reg(c, sf, rd, r, false)
                }
            }
        }
        5 => {
            let (opc, hw) = (bits(w, 29, 2), bits(w, 21, 2));
            if sf == 0 && hw > 1 {
                return unsup("movw hw");
            }
            let imm = (bits(w, 5, 16) as u64) << (hw * 16);
            match opc {
                0 => set_reg(c, sf, rd, !imm & ones(size_bits(sf)), false),
                2 => set_reg(c, sf, rd, imm, false),
                3 => {
                    let mask = !(0xFFFFu64 << (hw * 16));
                    let v = (x(c, rd) & mask) | imm;
                    set_reg(c, sf, rd, v, false)
                }
                _ => return unsup("movw opc"),
            }
        }
        6 => {
            let (opc, n) = (bits(w, 29, 2), bits(w, 22, 1));
            let (immr, imms) = (bits(w, 16, 6), bits(w, 10, 6));
            let size = size_bits(sf);
            if n != sf {
                return unsup("bitfield N");
            }
            let (wmask, tmask) = decode_bit_masks(n, imms, immr, size, false).map_err(|e| e.0)?;
            let src = reg(c, sf, rn, false);
            let rot = if immr != 0 { ror(src, immr, size) } else { src };
            let all = ones(size);
            let v = match opc {
                2 => rot & (wmask & tmask),
                0 => {
                    let top = if (src >> imms) & 1 != 0 { all } else { 0 };
                    (top & (!tmask & all)) | (rot & (wmask & tmask))
                }
                1 => {
                    let dst = reg(c, sf, rd, false);
                    let bot = (dst & (!wmask & all)) | (rot & wmask);
                    (dst & (!tmask & all)) | (bot & tmask)
                }
                _ => return unsup("bitfield opc"),
            };
            set_reg(c, sf, rd, v, false);
        }
        7 => {
            let (rm, lsb) = (bits(w, 16, 5), bits(w, 10, 6));
            let size = size_bits(sf);
            if lsb == 0 {
                let v = reg(c, sf, rm, false);
                set_reg(c, sf, rd, v, false);
                return Ok(());
            }
            if lsb >= size {
                return unsup("extract lsb");
            }
            let v = (reg(c, sf, rm, false) >> lsb) | (reg(c, sf, rn, false) << (size - lsb));
            set_reg(c, sf, rd, v, false);
        }
        _ => return unsup(format!("dp_imm {w:08x}")),
    }
    Ok(())
}

// ------------------------------------------------------------ branches / system

/// True when the RET at `pc` ends a context restore (libunwind's jumpto): in the straight-line code before it x30 is
/// loaded through a base other than sp and sp is set from a register other than x29. The translator's emitter
/// (`aot::emit::is_context_restore`) lowers such a RET as a jump through x30, so the interpreter does as well.
unsafe fn is_context_restore(pc: u64) -> bool {
    let (mut sp_set, mut lr_loaded) = (false, false);
    let lo = crate::dispatch::table().and_then(|t| t.module_at(pc)).map_or(pc, |m| m.base);
    let mut a = pc;
    for _ in 0..96 {
        if a < lo + 4 {
            break;
        }
        a -= 4;
        let w = core::ptr::read_unaligned(a as *const u32);
        if (w & 0x7C00_0000) == 0x1400_0000
            || (w & 0xFF00_0010) == 0x5400_0000
            || (w & 0x7E00_0000) == 0x3400_0000
            || (w & 0x7E00_0000) == 0x3600_0000
            || (w & 0xFE00_0000) == 0xD600_0000
        {
            break;
        }
        let rn = (w >> 5) & 31;
        if (w & 0xFFFF_FC1F) == 0x9100_001F && rn != 31 && rn != 29 {
            sp_set = true;
        }
        let ldr = (w & 0xFFC0_001F) == 0xF940_001E;
        let ldp = (w & 0xFFC0_0000) == 0xA940_0000 && ((w & 31) == 30 || ((w >> 10) & 31) == 30);
        if (ldr || ldp) && rn != 31 {
            lr_loaded = true;
        }
    }
    sp_set && lr_loaded
}

unsafe fn branch_sys(c: &mut Cpu, w: u32, pc: u64) -> R<Flow> {
    if bits(w, 26, 5) == 0b00101 {
        let t = pc_rel(pc, sext(bits(w, 0, 26) as u64, 26) * 4);
        if bits(w, 31, 1) != 0 {
            c.x[30] = pc.wrapping_add(4);
            return Ok(Flow::Call(t));
        }
        return Ok(Flow::Jump(t));
    }
    if bits(w, 25, 7) == 0b0101010 && bits(w, 4, 1) == 0 {
        let t = pc_rel(pc, sext(bits(w, 5, 19) as u64, 19) * 4);
        return Ok(if cond(c, bits(w, 0, 4)) { Flow::Jump(t) } else { Flow::Next });
    }
    if bits(w, 25, 6) == 0b011010 {
        let (sf, nz, rt) = (bits(w, 31, 1), bits(w, 24, 1), bits(w, 0, 5));
        let t = pc_rel(pc, sext(bits(w, 5, 19) as u64, 19) * 4);
        let zero = reg(c, sf, rt, false) == 0;
        return Ok(if zero != (nz != 0) { Flow::Jump(t) } else { Flow::Next });
    }
    if bits(w, 25, 6) == 0b011011 {
        let b = (bits(w, 31, 1) << 5) | bits(w, 19, 5);
        let (nz, rt) = (bits(w, 24, 1), bits(w, 0, 5));
        let t = pc_rel(pc, sext(bits(w, 5, 14) as u64, 14) * 4);
        let set = (x(c, rt) >> b) & 1 != 0;
        return Ok(if set == (nz != 0) { Flow::Jump(t) } else { Flow::Next });
    }
    if bits(w, 25, 7) == 0b1101011 {
        let (opc, rn) = (bits(w, 21, 4), bits(w, 5, 5));
        let ret = || if is_context_restore(pc) { Flow::Jump(x(c, 30)) } else { Flow::Ret };
        if opc == 2 && matches!(bits(w, 10, 6), 2 | 3) && rn == 31 && bits(w, 0, 5) == 31 && bits(w, 16, 5) == 31 {
            return Ok(ret());
        }
        if bits(w, 16, 5) != 31 || bits(w, 10, 6) != 0 || bits(w, 0, 5) != 0 {
            return unsup("branch reg variant");
        }
        return match opc {
            0 => Ok(Flow::Jump(x(c, rn))),
            1 => {
                let t = x(c, rn);
                c.x[30] = pc.wrapping_add(4);
                Ok(Flow::Call(t))
            }
            2 => Ok(ret()),
            _ => unsup("branch reg opc"),
        };
    }
    if bits(w, 24, 8) == 0b11010100 {
        let (opc, ll, imm) = (bits(w, 21, 3), bits(w, 0, 5), bits(w, 5, 16));
        if opc == 0 && ll == 1 {
            return Ok(Flow::Svc(imm));
        }
        if opc == 1 && ll == 0 {
            return Ok(Flow::Brk(imm));
        }
        return unsup("exception");
    }
    if bits(w, 22, 10) == 0b1101010100 {
        system(c, w)?;
        return Ok(Flow::Next);
    }
    unsup(format!("branch_sys {w:08x}"))
}

unsafe fn system(c: &mut Cpu, w: u32) -> R<()> {
    let l = bits(w, 21, 1);
    let (op0, op1, crn, crm, op2, rt) = (bits(w, 19, 2), bits(w, 16, 3), bits(w, 12, 4), bits(w, 8, 4), bits(w, 5, 3), bits(w, 0, 5));
    if op0 == 0 && l == 0 {
        if crn == 2 && op1 == 3 && rt == 31 {
            return Ok(()); // hint
        }
        if crn == 3 && op1 == 3 && rt == 31 {
            if op2 == 2 {
                c.excl_addr = !0;
            } else {
                core::sync::atomic::fence(Ordering::SeqCst);
            }
            return Ok(());
        }
        return unsup(format!("pstate/system {w:08x}"));
    }
    if op0 == 1 {
        if l == 0 && op1 == 3 && crn == 7 && crm == 4 && op2 == 1 {
            core::ptr::write_bytes((x(c, rt) & !63) as *mut u8, 0, 64); // DC ZVA
        }
        return Ok(()); // cache maintenance
    }
    let key = (op0, op1, crn, crm, op2);
    if l != 0 {
        let v = match key {
            (3, 3, 13, 0, 3) => c.tpidrro,
            (3, 3, 13, 0, 2) => c.tpidr,
            (3, 3, 14, 0, 0) => crate::rt::tick_hz() as u64,
            (3, 3, 14, 0, 1) | (3, 3, 14, 0, 2) => crate::rt::rt_ticks(),
            (3, 3, 4, 2, 0) => c.nzcv,
            (3, 3, 4, 4, 0) => c.fpcr,
            (3, 3, 4, 4, 1) => c.fpsr,
            (3, 3, 0, 0, 7) => 4,
            (3, 3, 0, 0, 1) => 0x8444_c004,
            _ => return unsup(format!("mrs {key:?}")),
        };
        set_reg(c, 1, rt, v, false);
        return Ok(());
    }
    let v = x(c, rt);
    match key {
        (3, 3, 13, 0, 2) => c.tpidr = v,
        (3, 3, 4, 2, 0) => c.nzcv = v & 0xf000_0000,
        (3, 3, 4, 4, 0) => {
            c.fpcr = v & 0x7c0_0000;
            c.sync_mxcsr();
        }
        (3, 3, 4, 4, 1) => c.fpsr = v & 0xf800_009f,
        _ => return unsup(format!("msr {key:?}")),
    }
    Ok(())
}

// ------------------------------------------------------------ loads / stores

unsafe fn ldst(c: &mut Cpu, w: u32, pc: u64) -> R<()> {
    if bits(w, 26, 1) != 0 {
        return crate::interp::simd::ldst(c, w, pc);
    }
    if bits(w, 24, 6) == 0b001000 {
        return ldst_excl(c, w);
    }
    if bits(w, 27, 3) == 0b011 && bits(w, 24, 2) == 0 {
        let (opc, rt) = (bits(w, 30, 2), bits(w, 0, 5));
        let a = pc_rel(pc, sext(bits(w, 5, 19) as u64, 19) * 4);
        match opc {
            0 => set_reg(c, 0, rt, ld(a, 2), false),
            1 => set_reg(c, 1, rt, ld(a, 3), false),
            2 => set_reg(c, 1, rt, ld(a, 2) as u32 as i32 as i64 as u64, false),
            _ => {} // prfm
        }
        return Ok(());
    }
    if bits(w, 27, 3) == 0b101 {
        return ldst_pair(c, w);
    }
    if bits(w, 27, 3) == 0b111 {
        return ldst_reg(c, w);
    }
    if (w & 0xFFE0_0000) == 0xD920_0000 && bits(w, 10, 2) != 0 {
        // STG (MTE): only the writeback forms change state
        let (mode, rn) = (bits(w, 10, 2), bits(w, 5, 5));
        if mode != 2 {
            let off = sext(bits(w, 12, 9) as u64, 9) << 4;
            let v = xs(c, rn).wrapping_add(off as u64);
            set_base(c, rn, v);
        }
        return Ok(());
    }
    unsup(format!("ldst {w:08x}"))
}

unsafe fn ldst_excl(c: &mut Cpu, w: u32) -> R<()> {
    let (size, o2, l, o1, rs, rn, rt) =
        (bits(w, 30, 2), bits(w, 23, 1), bits(w, 22, 1), bits(w, 21, 1), bits(w, 16, 5), bits(w, 5, 5), bits(w, 0, 5));
    let a = xs(c, rn);
    if o1 != 0 {
        if o2 != 0 {
            // CAS
            let m = ones(8 << size);
            let e = mem::cas(size, a, x(c, rs) & m, x(c, rt) & m);
            set_reg(c, (size == 3) as u32, rs, e, false);
            return Ok(());
        }
        if bits(w, 31, 1) != 0 {
            excl_pair(c, bits(w, 30, 1), l, rs, bits(w, 10, 5), rt, a);
            return Ok(());
        }
        // CASP
        if rs & 1 != 0 || rt & 1 != 0 {
            return unsup("casp odd register");
        }
        if bits(w, 30, 1) != 0 {
            let e = (x(c, rs + 1) as u128) << 64 | x(c, rs) as u128;
            let v = (x(c, rt + 1) as u128) << 64 | x(c, rt) as u128;
            let got = mem::cas128(a, e, v).1;
            set_reg(c, 1, rs, got as u64, false);
            set_reg(c, 1, rs + 1, (got >> 64) as u64, false);
        } else {
            let e = wv(c, rs) | wv(c, rs + 1) << 32;
            let v = wv(c, rt) | wv(c, rt + 1) << 32;
            let got = mem::cas(3, a, e, v);
            set_reg(c, 0, rs, got, false);
            set_reg(c, 0, rs + 1, got >> 32, false);
        }
        return Ok(());
    }
    if o2 == 0 {
        if l != 0 {
            let v = mem::ldx(c, size, a);
            set_reg(c, 1, rt, v, false);
        } else {
            let st = mem::stx(c, size, a, x(c, rt));
            set_reg(c, 0, rs, st as u64, false);
        }
        return Ok(());
    }
    if l != 0 {
        let v = mem::lda(size, a);
        set_reg(c, 1, rt, v, false);
    } else {
        mem::stl(size, a, x(c, rt));
    }
    Ok(())
}

unsafe fn excl_pair(c: &mut Cpu, sz: u32, l: u32, rs: u32, rt2: u32, rt: u32, a: u64) {
    if l != 0 {
        if sz != 0 {
            let p = mem::ldxp128(c, a);
            set_reg(c, 1, rt, p as u64, false);
            set_reg(c, 1, rt2, (p >> 64) as u64, false);
        } else {
            let p = mem::ldx(c, 3, a);
            set_reg(c, 0, rt, p, false);
            set_reg(c, 0, rt2, p >> 32, false);
        }
        return;
    }
    let st = if sz != 0 {
        mem::stxp128(c, a, x(c, rt), x(c, rt2))
    } else {
        let v = wv(c, rt) | (wv(c, rt2) << 32);
        mem::stx(c, 3, a, v)
    };
    set_reg(c, 0, rs, st as u64, false);
}

unsafe fn atomic(c: &mut Cpu, w: u32) -> R<()> {
    let (size, a, r, rs, o3, opc, rn, rt) =
        (bits(w, 30, 2), bits(w, 23, 1), bits(w, 22, 1), bits(w, 16, 5), bits(w, 15, 1), bits(w, 12, 3), bits(w, 5, 5), bits(w, 0, 5));
    let order = [Ordering::Relaxed, Ordering::Release, Ordering::Acquire, Ordering::AcqRel][((a << 1) | r) as usize];
    let sf = (size == 3) as u32;
    let p = xs(c, rn);
    let v = x(c, rs) & ones(8 << size);
    let old = if o3 != 0 {
        if opc == 4 && rs == 31 && a != 0 && r == 0 {
            let v = mem::lda(size, p);
            set_reg(c, sf, rt, v, false);
            return Ok(());
        }
        if opc != 0 {
            return unsup("ldapr");
        }
        mem::atomic_rmw(size, 8, p, v, order)
    } else {
        mem::atomic_rmw(size, opc, p, v, order)
    };
    set_reg(c, sf, rt, old, false);
    Ok(())
}

unsafe fn ldst_pair(c: &mut Cpu, w: u32) -> R<()> {
    let (opc, mode, l) = (bits(w, 30, 2), bits(w, 23, 2), bits(w, 22, 1));
    let (imm7, rt2, rn, rt) = (sext(bits(w, 15, 7) as u64, 7), bits(w, 10, 5), bits(w, 5, 5), bits(w, 0, 5));
    if opc == 3 {
        return unsup("pair opc 3");
    }
    if opc == 1 && (l == 0 || mode == 0) {
        return unsup("pair opc 1 (STGP / unallocated)");
    }
    let scale = if opc == 2 { 3 } else { 2 };
    let off = imm7 << scale;
    let base = xs(c, rn);
    let post = mode == 1;
    let wb = mode == 1 || mode == 3;
    let ea = if post { base } else { base.wrapping_add(off as u64) };
    let (mut va, mut vb) = (0, 0);
    if l != 0 {
        match opc {
            2 => {
                va = ld(ea, 3);
                vb = ld(ea.wrapping_add(8), 3);
            }
            1 => {
                va = ld(ea, 2) as u32 as i32 as i64 as u64;
                vb = ld(ea.wrapping_add(4), 2) as u32 as i32 as i64 as u64;
            }
            _ => {
                va = ld(ea, 2);
                vb = ld(ea.wrapping_add(4), 2);
            }
        }
    } else if opc == 2 {
        st(ea, 3, x(c, rt));
        st(ea.wrapping_add(8), 3, x(c, rt2));
    } else {
        st(ea, 2, wv(c, rt));
        st(ea.wrapping_add(4), 2, wv(c, rt2));
    }
    if wb {
        set_base(c, rn, if post { ea.wrapping_add(off as u64) } else { ea });
    }
    if l != 0 {
        set_reg(c, 1, rt, va, false);
        set_reg(c, 1, rt2, vb, false);
    }
    Ok(())
}

unsafe fn ldst_reg(c: &mut Cpu, w: u32) -> R<()> {
    let (size, opc, rn, rt) = (bits(w, 30, 2), bits(w, 22, 2), bits(w, 5, 5), bits(w, 0, 5));
    let base = xs(c, rn);
    let mut wb: Option<u64> = None;
    let ea;
    if bits(w, 24, 2) == 1 {
        ea = base.wrapping_add((bits(w, 10, 12) as u64) << size);
    } else {
        let kind = bits(w, 10, 2);
        if bits(w, 21, 1) != 0 {
            if kind == 0 {
                return atomic(c, w);
            }
            if kind != 2 {
                return unsup("ldst reg pac");
            }
            let (rm, option, s) = (bits(w, 16, 5), bits(w, 13, 3), bits(w, 12, 1));
            if !matches!(option, 2 | 3 | 6 | 7) {
                return unsup("ldst reg option");
            }
            ea = base.wrapping_add(extend(c, rm, option, if s != 0 { size } else { 0 }));
        } else {
            let imm9 = sext(bits(w, 12, 9) as u64, 9) as u64;
            match kind {
                0 | 2 => ea = base.wrapping_add(imm9),
                1 => {
                    ea = base;
                    wb = Some(base.wrapping_add(imm9));
                }
                _ => {
                    ea = base.wrapping_add(imm9);
                    wb = Some(ea);
                }
            }
        }
    }
    let load: Option<(u32, u64)>;
    if opc == 0 {
        st(ea, size, if size == 3 { x(c, rt) } else { wv(c, rt) });
        load = None;
    } else if opc == 1 {
        load = Some((1, ld(ea, size)));
    } else if size == 3 {
        return Ok(()); // prfm
    } else if size == 2 && opc == 3 {
        return unsup("ldst size2 opc3");
    } else {
        let nb = 8 << size;
        let v = ld(ea, size);
        let v = ((v << (64 - nb)) as i64 >> (64 - nb)) as u64;
        load = Some((if opc == 2 { 1 } else { 0 }, v));
    }
    if let Some(v) = wb {
        set_base(c, rn, v);
    }
    if let Some((sf, v)) = load {
        set_reg(c, sf, rt, v, false);
    }
    Ok(())
}

// ------------------------------------------------------------ data processing (reg)

unsafe fn dp_reg(c: &mut Cpu, w: u32) -> R<()> {
    let sf = bits(w, 31, 1);
    let (rd, rn, rm) = (bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5));
    let size = size_bits(sf);
    let m = ones(size);
    let r = |c: &Cpu, n: u32| reg(c, sf, n, false);
    if bits(w, 28, 1) == 0 {
        if bits(w, 24, 4) == 0b1010 {
            let (opc, sh, n, amt) = (bits(w, 29, 2), bits(w, 22, 2), bits(w, 21, 1), bits(w, 10, 6));
            if sf == 0 && amt >= 32 {
                return unsup("logical shift amount");
            }
            let mut b = shift(sf, r(c, rm), sh, amt);
            if n != 0 {
                b = !b & m;
            }
            let a = r(c, rn);
            let v = match opc {
                1 => a | b,
                2 => a ^ b,
                _ => a & b,
            };
            let v = if opc == 3 { logic_flags(c, sf, v) } else { v };
            set_reg(c, sf, rd, v, false);
            return Ok(());
        }
        if bits(w, 24, 4) == 0b1011 {
            let (op, s) = (bits(w, 30, 1), bits(w, 29, 1));
            let (a, b, sp_ok);
            if bits(w, 21, 1) == 0 {
                let (sh, amt) = (bits(w, 22, 2), bits(w, 10, 6));
                if sh == 3 {
                    return unsup("add shift ror");
                }
                a = r(c, rn);
                b = shift(sf, r(c, rm), sh, amt);
                sp_ok = false;
            } else {
                if bits(w, 22, 2) != 0 {
                    return unsup("add ext opt");
                }
                let (option, amt) = (bits(w, 13, 3), bits(w, 10, 3));
                a = reg(c, sf, rn, true);
                let e = extend(c, rm, option, amt);
                b = if sf == 0 { e as u32 as u64 } else { e };
                sp_ok = s == 0;
            }
            if s != 0 {
                let v = addsub_flags(c, sf, a, b, op != 0);
                set_reg(c, sf, rd, v, false);
            } else {
                let v = if op != 0 { a.wrapping_sub(b) } else { a.wrapping_add(b) };
                set_reg(c, sf, rd, v, sp_ok);
            }
            return Ok(());
        }
        return unsup(format!("dp_reg {w:08x}"));
    }
    let op2 = bits(w, 21, 4);
    if bits(w, 24, 4) == 0b1011 {
        let (op54, op31, o0, ra) = (bits(w, 29, 2), bits(w, 21, 3), bits(w, 15, 1), bits(w, 10, 5));
        if op54 != 0 {
            return unsup("dp3 op54");
        }
        let addsub = |acc: u64, p: u64| if o0 != 0 { acc.wrapping_sub(p) } else { acc.wrapping_add(p) };
        let v = match op31 {
            0 => {
                let p = r(c, rn).wrapping_mul(r(c, rm)) & m;
                set_reg(c, sf, rd, addsub(r(c, ra), p), false);
                return Ok(());
            }
            1 => addsub(x(c, ra), ((w_(c, rn) as i32 as i64).wrapping_mul(w_(c, rm) as i32 as i64)) as u64),
            5 => addsub(x(c, ra), wv(c, rn).wrapping_mul(wv(c, rm))),
            2 => ((x(c, rn) as i64 as i128 * x(c, rm) as i64 as i128) >> 64) as u64,
            6 => ((x(c, rn) as u128 * x(c, rm) as u128) >> 64) as u64,
            _ => return unsup("dp3"),
        };
        set_reg(c, 1, rd, v, false);
        return Ok(());
    }
    if op2 == 0 {
        let (op, s) = (bits(w, 30, 1), bits(w, 29, 1));
        let (a, b) = (r(c, rn), r(c, rm));
        let b = if op != 0 { !b & m } else { b };
        let cin = carry(c);
        let v = a.wrapping_add(b).wrapping_add(cin) & m;
        if s != 0 {
            let top = size - 1;
            let cy = if cin != 0 { v <= a } else { v < a };
            c.nzcv = mkflags(v >> top & 1 != 0, v == 0, cy, ((a ^ v) & (b ^ v)) >> top & 1 != 0);
        }
        set_reg(c, sf, rd, v, false);
        return Ok(());
    }
    if op2 == 2 {
        let (op, imm_form, cnd, nzcv) = (bits(w, 30, 1), bits(w, 11, 1), bits(w, 12, 4), bits(w, 0, 4));
        let b = if imm_form != 0 { rm as u64 } else { r(c, rm) };
        if cond(c, cnd) {
            let a = r(c, rn);
            addsub_flags(c, sf, a, b, op != 0);
        } else {
            c.nzcv = (nzcv as u64) << 28;
        }
        return Ok(());
    }
    if op2 == 4 {
        let (op, o2, cnd) = (bits(w, 30, 1), bits(w, 10, 2), bits(w, 12, 4));
        let b = r(c, rm);
        let alt = match (op, o2) {
            (0, 0) => b,
            (0, 1) => b.wrapping_add(1),
            (1, 0) => !b,
            (1, 1) => b.wrapping_neg(),
            _ => return unsup("csel variant"),
        } & m;
        let v = if cond(c, cnd) { r(c, rn) } else { alt };
        set_reg(c, sf, rd, v, false);
        return Ok(());
    }
    if op2 == 6 {
        let opcode = bits(w, 10, 6);
        if bits(w, 30, 1) != 0 {
            if bits(w, 16, 5) == 1 && sf != 0 && opcode <= 0x11 {
                return Ok(()); // pac
            }
            if bits(w, 16, 5) != 0 {
                return unsup("dp1 opcode2");
            }
            let a = r(c, rn);
            let v = match opcode {
                0 => {
                    if sf != 0 {
                        a.reverse_bits()
                    } else {
                        (a as u32).reverse_bits() as u64
                    }
                }
                1 => {
                    if sf != 0 {
                        ((a & 0x00ff_00ff_00ff_00ff) << 8) | ((a >> 8) & 0x00ff_00ff_00ff_00ff)
                    } else {
                        ((a & 0x00ff_00ff) << 8) | ((a >> 8) & 0x00ff_00ff)
                    }
                }
                2 => {
                    if sf == 0 {
                        (a as u32).swap_bytes() as u64
                    } else {
                        ((a as u32).swap_bytes() as u64) | (((a >> 32) as u32).swap_bytes() as u64) << 32
                    }
                }
                3 if sf != 0 => a.swap_bytes(),
                4 => {
                    if sf != 0 {
                        a.leading_zeros() as u64
                    } else {
                        (a as u32).leading_zeros() as u64
                    }
                }
                5 => {
                    if sf != 0 {
                        (a ^ ((a as i64 >> 1) as u64)).leading_zeros() as u64 - 1
                    } else {
                        let a = a as u32;
                        (a ^ ((a as i32 >> 1) as u32)).leading_zeros() as u64 - 1
                    }
                }
                _ => return unsup("dp1"),
            };
            set_reg(c, sf, rd, v, false);
            return Ok(());
        }
        if (16..=23).contains(&opcode) {
            // CRC32 / CRC32C
            let (cbit, sz) = (bits(w, 12, 1), bits(w, 10, 2));
            if (sz == 3) != (sf != 0) {
                return unsup("crc size");
            }
            let val = if sz == 3 { x(c, rm) } else { wv(c, rm) };
            let v = crate::interp::lanes::crc(cbit != 0, wv(c, rn) as u32, val, 1 << sz);
            set_reg(c, 0, rd, v as u64, false);
            return Ok(());
        }
        if opcode == 12 && sf != 0 {
            let v = (pacga(x(c, rn), xs(c, rm)) as u64) << 32;
            set_reg(c, 1, rd, v, false);
            return Ok(());
        }
        let (a, b) = (r(c, rn), r(c, rm));
        let v = match opcode {
            2 => {
                if b == 0 {
                    0
                } else {
                    a / b
                }
            }
            3 => {
                if b == 0 {
                    0
                } else if sf != 0 {
                    (a as i64).wrapping_div(b as i64) as u64
                } else {
                    (a as u32 as i32).wrapping_div(b as u32 as i32) as u32 as u64
                }
            }
            8 => a << (b & (size as u64 - 1)),
            9 => a >> (b & (size as u64 - 1)),
            10 => {
                if sf != 0 {
                    ((a as i64) >> (b & 63)) as u64
                } else {
                    ((a as u32 as i32) >> (b & 31)) as u32 as u64
                }
            }
            11 => ror(a, (b & (size as u64 - 1)) as u32, size),
            _ => return unsup("dp2"),
        };
        set_reg(c, sf, rd, v, false);
        return Ok(());
    }
    unsup(format!("dp_reg {w:08x}"))
}

fn w_(c: &Cpu, r: u32) -> u32 {
    x(c, r) as u32
}

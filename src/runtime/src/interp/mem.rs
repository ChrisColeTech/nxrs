//! Guest memory for the interpreter: guest addresses are host addresses, as for the lifted code (cpu.h `LD*` / `ST*`),
//! with cpu.h's exclusive monitor (`ldx*` / `stx*` and the global bucket table `rt_excl_mon`) and its atomics.

use crate::cpu::Cpu;
use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};

#[inline]
pub unsafe fn ld(a: u64, sz: u32) -> u64 {
    match sz {
        0 => core::ptr::read_volatile(a as *const u8) as u64,
        1 => core::ptr::read_unaligned(a as *const u16) as u64,
        2 => core::ptr::read_unaligned(a as *const u32) as u64,
        _ => core::ptr::read_unaligned(a as *const u64),
    }
}
#[inline]
pub unsafe fn st(a: u64, sz: u32, v: u64) {
    match sz {
        0 => core::ptr::write_volatile(a as *mut u8, v as u8),
        1 => core::ptr::write_unaligned(a as *mut u16, v as u16),
        2 => core::ptr::write_unaligned(a as *mut u32, v as u32),
        _ => core::ptr::write_unaligned(a as *mut u64, v),
    }
}
#[inline]
pub unsafe fn ld128(a: u64) -> u128 {
    core::ptr::read_unaligned(a as *const u128)
}
#[inline]
pub unsafe fn st128(a: u64, v: u128) {
    core::ptr::write_unaligned(a as *mut u128, v)
}
/// Loads of 0..4 = 8, 16, 32, 64, 128 bits.
pub unsafe fn ld_v(a: u64, scale: u32) -> u128 {
    if scale == 4 {
        ld128(a)
    } else {
        ld(a, scale) as u128
    }
}
pub unsafe fn st_v(a: u64, scale: u32, v: u128) {
    if scale == 4 {
        st128(a, v)
    } else {
        st(a, scale, v as u64)
    }
}

// ---- the exclusive monitor (cpu.h EXCL)

fn xm(a: u64) -> Option<&'static AtomicU32> {
    // SAFETY: a pointer read; the table is static and lives for the process
    let t = unsafe { core::ptr::addr_of!(crate::cpu::rt_excl_mon).read_volatile() };
    if t.is_null() {
        return None;
    }
    let i = (((a >> 4) as u32).wrapping_mul(2_654_435_761)) >> (32 - 16);
    // SAFETY: the table has 2^16 entries, used only atomically
    Some(unsafe { AtomicU32::from_ptr(t.add(i as usize)) })
}
fn xm_mark(c: &mut Cpu, a: u64) {
    if let Some(m) = xm(a) {
        let mut q;
        loop {
            q = m.load(Ordering::Acquire);
            if q & 1 == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        c.excl_seq = q;
    }
}
fn xm_begin(c: &Cpu, a: u64) -> bool {
    match xm(a) {
        None => true,
        Some(m) => m
            .compare_exchange(
                c.excl_seq,
                c.excl_seq.wrapping_add(1),
                Ordering::SeqCst,
                Ordering::Relaxed,
            )
            .is_ok(),
    }
}
fn xm_end(c: &Cpu, a: u64, stored: bool) {
    if let Some(m) = xm(a) {
        m.store(
            if stored {
                c.excl_seq.wrapping_add(2)
            } else {
                c.excl_seq
            },
            Ordering::Release,
        );
    }
}

macro_rules! with_atomic {
    ($sz:expr, $a:expr, |$x:ident| $body:expr) => {
        match $sz {
            0 => {
                let $x = AtomicU8::from_ptr($a as *mut u8);
                ($body) as u64
            }
            1 => {
                let $x = AtomicU16::from_ptr($a as *mut u16);
                ($body) as u64
            }
            2 => {
                let $x = AtomicU32::from_ptr($a as *mut u32);
                ($body) as u64
            }
            _ => {
                let $x = AtomicU64::from_ptr($a as *mut u64);
                ($body) as u64
            }
        }
    };
}

/// LDXR / LDAXR.
pub unsafe fn ldx(c: &mut Cpu, sz: u32, a: u64) -> u64 {
    xm_mark(c, a);
    let v = with_atomic!(sz, a, |x| x.load(Ordering::Acquire));
    c.excl_addr = a;
    c.excl_val = v;
    v
}
/// STXR / STLXR: 0 stored, 1 failed.
pub unsafe fn stx(c: &mut Cpu, sz: u32, a: u64, v: u64) -> u32 {
    if c.excl_addr != a {
        return 1;
    }
    c.excl_addr = !0;
    if !xm_begin(c, a) {
        return 1;
    }
    let e = c.excl_val;
    let ok = with_atomic!(sz, a, |x| x
        .compare_exchange(e as _, v as _, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok())
        != 0;
    xm_end(c, a, ok);
    !ok as u32
}
/// LDAR / LDAPR.
pub unsafe fn lda(sz: u32, a: u64) -> u64 {
    with_atomic!(sz, a, |x| x.load(Ordering::Acquire))
}
/// STLR.
pub unsafe fn stl(sz: u32, a: u64, v: u64) {
    let _ = with_atomic!(sz, a, |x| {
        x.store(v as _, Ordering::SeqCst);
        0u8
    });
}

/// lock cmpxchg16b: (success, the value found).
#[cfg(target_arch = "x86_64")]
pub unsafe fn cas128(a: u64, expected: u128, new: u128) -> (bool, u128) {
    let (mut lo, mut hi) = (expected as u64, (expected >> 64) as u64);
    let ok: u8;
    core::arch::asm!(
        "xchg {nlo}, rbx",
        "lock cmpxchg16b [{p}]",
        "sete {ok}",
        "mov rbx, {nlo}",
        p = in(reg) a,
        nlo = inout(reg) new as u64 => _,
        ok = out(reg_byte) ok,
        inout("rax") lo,
        inout("rdx") hi,
        in("rcx") (new >> 64) as u64,
        options(nostack),
    );
    (ok != 0, (hi as u128) << 64 | lo as u128)
}

/// AArch64: an LDAXP / STLXP loop (no LSE needed): (success, the value found). A failed compare stores the value read
/// back, so the 16 bytes were read in one atomic access either way.
#[cfg(target_arch = "aarch64")]
pub unsafe fn cas128(a: u64, expected: u128, new: u128) -> (bool, u128) {
    let (lo, hi): (u64, u64);
    let ok: u64;
    core::arch::asm!(
        "2:",
        "ldaxp {lo}, {hi}, [{p}]",
        "cmp {lo}, {elo}",
        "ccmp {hi}, {ehi}, #0, eq",
        "b.ne 3f",
        "stlxp {st:w}, {nlo}, {nhi}, [{p}]",
        "cbnz {st:w}, 2b",
        "mov {ok}, #1",
        "b 4f",
        "3:",
        "stlxp {st:w}, {lo}, {hi}, [{p}]",
        "cbnz {st:w}, 2b",
        "mov {ok}, #0",
        "4:",
        p = in(reg) a,
        elo = in(reg) expected as u64,
        ehi = in(reg) (expected >> 64) as u64,
        nlo = in(reg) new as u64,
        nhi = in(reg) (new >> 64) as u64,
        lo = out(reg) lo,
        hi = out(reg) hi,
        st = out(reg) _,
        ok = out(reg) ok,
        options(nostack),
    );
    (ok != 0, (hi as u128) << 64 | lo as u128)
}

/// LDXP / LDAXP of two X registers: one 16-byte atomic access.
pub unsafe fn ldxp128(c: &mut Cpu, a: u64) -> u128 {
    xm_mark(c, a);
    let v = cas128(a, 0, 0).1;
    c.excl_addr = a;
    c.excl_val = v as u64;
    c.excl_val_hi = (v >> 64) as u64;
    v
}
pub unsafe fn stxp128(c: &mut Cpu, a: u64, lo: u64, hi: u64) -> u32 {
    if c.excl_addr != a {
        return 1;
    }
    c.excl_addr = !0;
    if !xm_begin(c, a) {
        return 1;
    }
    let e = ((c.excl_val_hi as u128) << 64) | c.excl_val as u128;
    let ok = cas128(a, e, ((hi as u128) << 64) | lo as u128).0;
    xm_end(c, a, ok);
    !ok as u32
}

/// CAS: returns the value found (the new Rs).
pub unsafe fn cas(sz: u32, a: u64, expected: u64, new: u64) -> u64 {
    with_atomic!(sz, a, |x| match x.compare_exchange(
        expected as _,
        new as _,
        Ordering::SeqCst,
        Ordering::SeqCst
    ) {
        Ok(v) | Err(v) => v,
    })
}

/// LSE: LDADD, LDCLR, LDEOR, LDSET, LDSMAX, LDSMIN, LDUMAX, LDUMIN (opc 0..7) and SWP (opc 8); the old value.
pub unsafe fn atomic_rmw(sz: u32, opc: u32, a: u64, v: u64, order: Ordering) -> u64 {
    let bits = 8u32 << sz;
    let sx = |x: u64| ((x << (64 - bits)) as i64) >> (64 - bits);
    let f = move |o: u64| -> u64 {
        match opc {
            4 => {
                if sx(o) > sx(v) {
                    o
                } else {
                    v
                }
            }
            5 => {
                if sx(o) < sx(v) {
                    o
                } else {
                    v
                }
            }
            6 => {
                if o > v {
                    o
                } else {
                    v
                }
            }
            _ => {
                if o < v {
                    o
                } else {
                    v
                }
            }
        }
    };
    match opc {
        0 => with_atomic!(sz, a, |x| x.fetch_add(v as _, order)),
        1 => with_atomic!(sz, a, |x| x.fetch_and(!v as _, order)),
        2 => with_atomic!(sz, a, |x| x.fetch_xor(v as _, order)),
        3 => with_atomic!(sz, a, |x| x.fetch_or(v as _, order)),
        8 => with_atomic!(sz, a, |x| x.swap(v as _, order)),
        // min / max: a CAS loop (cpu.h ATOMIC_MINMAX, sequentially consistent)
        _ => with_atomic!(sz, a, |x| {
            let mut o = x.load(Ordering::Relaxed);
            loop {
                match x.compare_exchange_weak(
                    o,
                    f(o as u64) as _,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                ) {
                    Ok(v) => break v,
                    Err(v) => o = v,
                }
            }
        }),
    }
}

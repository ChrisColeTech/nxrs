//! Floating point for the interpreter: the semantics of the generated code's `simd.h` / `simd_fp.h` (the ARM
//! pseudocode: NaN propagation, FPCR.DN default NaN, FPCR.FZ flushing with tininess detected before rounding, FPCR.RMode,
//! saturating conversions, fused multiply-add, the ARM estimate tables), always on their exact ("slow") path: the
//! interpreter is the rare path, so it never takes the host shortcuts the lifted code takes.
//!
//! Host assumptions as for the lifted code: IEEE binary32 / binary64, MXCSR FTZ|DAZ equal to FPCR.FZ (the runtime
//! keeps it so while guest code runs) and RC = nearest outside the operations that switch it here.

use crate::cpu::Cpu;

pub const FPCR_AHP: u64 = 1 << 26;
pub const FPCR_DN: u64 = 1 << 25;
pub const FPCR_FZ: u64 = 1 << 24;
pub const FPCR_RM: u64 = 3 << 22;

pub const FOP_ADD: u32 = 0;
pub const FOP_SUB: u32 = 1;
pub const FOP_MUL: u32 = 2;
pub const FOP_MULX: u32 = 3;
pub const FOP_NMUL: u32 = 4;
pub const FOP_DIV: u32 = 5;
pub const FOP_ABD: u32 = 6;
pub const FOP_SQRT: u32 = 7;
pub const FOP_MADD: u32 = 8;
pub const FOP_RECPS: u32 = 9;
pub const FOP_RSQRTS: u32 = 10;
pub const FOP_MAX: u32 = 11;
pub const FOP_MIN: u32 = 12;
pub const FOP_MAXNM: u32 = 13;
pub const FOP_MINNM: u32 = 14;

// ---- the host rounding mode (MXCSR.RC)

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn csr_get() -> u32 {
    let mut v: u32 = 0;
    // SAFETY: stores MXCSR into a local
    unsafe { core::arch::asm!("stmxcsr [{}]", in(reg) &mut v, options(nostack)) };
    v
}
#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn csr_set(v: u32) {
    // SAFETY: loads MXCSR from a local holding a valid value (only the RC bits differ from the current one)
    unsafe { core::arch::asm!("ldmxcsr [{}]", in(reg) &v, options(nostack)) };
}

// AArch64: the host's own FPCR, whose RMode field (bits 22-23) is the guest's encoding
#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn csr_get() -> u32 {
    let v: u64;
    // SAFETY: reads FPCR
    unsafe { core::arch::asm!("mrs {}, fpcr", out(reg) v, options(nomem, nostack)) };
    v as u32
}
#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn csr_set(v: u32) {
    // SAFETY: writes FPCR with only the RMode bits changed from the current value
    unsafe { core::arch::asm!("msr fpcr, {}", in(reg) v as u64, options(nomem, nostack)) };
}

/// The host's rounding-mode field and its value per FPCR.RMode (RN 00, RP 01, RM 10, RZ 11): MXCSR.RC on x86-64
/// (RN 00, RP 10, RM 01, RZ 11 there), FPCR.RMode itself on AArch64.
#[cfg(target_arch = "x86_64")]
const RC_MASK: u32 = 0x6000;
#[cfg(target_arch = "x86_64")]
const RC: [u32; 4] = [0x0000, 0x4000, 0x2000, 0x6000];
#[cfg(target_arch = "aarch64")]
const RC_MASK: u32 = 3 << 22;
#[cfg(target_arch = "aarch64")]
const RC: [u32; 4] = [0, 1 << 22, 2 << 22, 3 << 22];

/// Sets the host rounding mode from FPCR.RMode; returns the previous control word.
pub fn rm_enter(fpcr: u64) -> u32 {
    let cur = csr_get();
    let want = (cur & !RC_MASK) | RC[((fpcr >> 22) & 3) as usize];
    if want != cur {
        csr_set(want);
    }
    cur
}
/// Forces toward zero.
pub fn rm_rz() {
    let cur = csr_get();
    if cur & RC_MASK != RC[3] {
        csr_set((cur & !RC_MASK) | RC[3]);
    }
}
/// Restores the rounding mode saved by `rm_enter`.
pub fn rm_leave(old: u32) {
    let cur = csr_get();
    if (cur & RC_MASK) != (old & RC_MASK) {
        csr_set((cur & !RC_MASK) | (old & RC_MASK));
    }
}

#[inline(always)]
fn bb<T>(x: T) -> T {
    core::hint::black_box(x)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
unsafe fn fma_f32(a: f32, b: f32, c: f32) -> f32 {
    use core::arch::x86_64::*;
    _mm_cvtss_f32(_mm_fmadd_ss(_mm_set_ss(a), _mm_set_ss(b), _mm_set_ss(c)))
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
unsafe fn fma_f64(a: f64, b: f64, c: f64) -> f64 {
    use core::arch::x86_64::*;
    _mm_cvtsd_f64(_mm_fmadd_sd(_mm_set_sd(a), _mm_set_sd(b), _mm_set_sd(c)))
}

/// a * b + c, one rounding in the current mode (the hardware FMA the game code is built for; x86-64-v3).
pub fn fma32(a: f32, b: f32, c: f32) -> f32 {
    // AArch64: mul_add is the fused instruction (FMADD)
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("fma") {
        // SAFETY: the CPU has FMA
        return unsafe { fma_f32(a, b, c) };
    }
    a.mul_add(b, c)
}
pub fn fma64(a: f64, b: f64, c: f64) -> f64 {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("fma") {
        // SAFETY: the CPU has FMA
        return unsafe { fma_f64(a, b, c) };
    }
    a.mul_add(b, c)
}

fn is_minmax(op: u32) -> bool {
    (FOP_MAX..=FOP_MINNM).contains(&op)
}

macro_rules! fp_impl {
    ($m:ident, $T:ty, $U:ty, $NB:expr, $EB:expr, $MB:expr, $fma:ident) => {
        pub mod $m {
            use super::*;
            pub type T = $T;
            pub type U = $U;
            pub const NB: u32 = $NB;
            pub const EB: u32 = $EB;
            pub const MB: u32 = $MB;
            pub const SGN: U = 1 << (NB - 1);
            pub const EXPM: U = (((1 as U) << EB) - 1) << MB;
            pub const QB: U = 1 << (MB - 1);
            pub const DNAN: U = EXPM | QB;
            pub const MINN: U = 1 << MB;

            /// The bits of x, opaque to the optimiser: LLVM turns bit tests of a float's bits into FP class compares
            /// otherwise, and with MXCSR.DAZ set (FPCR.FZ) the host compare sees a denormal as zero (simd_fp.h hides
            /// the origin with an empty asm for the same reason).
            #[inline]
            pub fn fb(x: T) -> U {
                bb(x.to_bits())
            }
            #[inline]
            pub fn fv(u: U) -> T {
                T::from_bits(u)
            }
            pub fn isnan(u: U) -> bool {
                (u & !SGN) > EXPM
            }
            pub fn issnan(u: U) -> bool {
                isnan(u) && u & QB == 0
            }
            pub fn isqnan(u: U) -> bool {
                isnan(u) && u & QB != 0
            }
            pub fn isinf(u: U) -> bool {
                (u & !SGN) == EXPM
            }
            pub fn iszero(u: U) -> bool {
                u & !SGN == 0
            }
            pub fn isden(u: U) -> bool {
                u & EXPM == 0 && u & !SGN != 0
            }
            pub fn flush(x: T) -> T {
                let u = fb(x);
                fv(if isden(u) { u & SGN } else { u })
            }
            pub fn fz(c: &Cpu, x: T) -> T {
                if c.fpcr & FPCR_FZ != 0 {
                    flush(x)
                } else {
                    x
                }
            }
            pub fn neg(x: T) -> T {
                fv(fb(x) ^ SGN)
            }
            fn abs(x: T) -> T {
                fv(fb(x) & !SGN)
            }

            // FPProcessNaN(s)
            pub fn nan1(c: &Cpu, a: T) -> T {
                let x = fb(a);
                if c.fpcr & FPCR_DN == 0 && isnan(x) {
                    return fv(x | QB);
                }
                fv(DNAN)
            }
            pub fn nan2(c: &Cpu, a: T, b: T) -> T {
                let (x, y) = (fb(a), fb(b));
                if c.fpcr & FPCR_DN == 0 {
                    if issnan(x) {
                        return fv(x | QB);
                    }
                    if issnan(y) {
                        return fv(y | QB);
                    }
                    if isnan(x) {
                        return a;
                    }
                    if isnan(y) {
                        return b;
                    }
                }
                fv(DNAN)
            }
            pub fn nan3(c: &Cpu, a: T, b: T, d: T) -> T {
                let (x, y, z) = (fb(a), fb(b), fb(d));
                if c.fpcr & FPCR_DN == 0 {
                    if issnan(x) {
                        return fv(x | QB);
                    }
                    if issnan(y) {
                        return fv(y | QB);
                    }
                    if issnan(z) {
                        return fv(z | QB);
                    }
                    if isnan(x) {
                        return a;
                    }
                    if isnan(y) {
                        return b;
                    }
                    if isnan(z) {
                        return d;
                    }
                }
                fv(DNAN)
            }

            /// The arithmetic in the current host rounding mode. MADD: a + b*d; RECPS / RSQRTS get their first
            /// operand negated already.
            #[inline(never)]
            fn calc(op: u32, a: T, b: T, d: T) -> T {
                let (a, b, d) = (bb(a), bb(b), bb(d));
                let r = match op {
                    FOP_ADD => a + b,
                    FOP_SUB => a - b,
                    FOP_MUL | FOP_MULX => a * b,
                    FOP_NMUL => -bb(a * b), // the product rounded, then negated (not (-a) * b: directed rounding)
                    FOP_DIV => a / b,
                    FOP_ABD => abs(a - b),
                    FOP_SQRT => a.sqrt(),
                    FOP_MADD => $fma(b, d, a),
                    FOP_RECPS => $fma(a, b, 2.0),
                    FOP_RSQRTS => $fma(a, b, 3.0) * 0.5,
                    _ => a,
                };
                bb(r)
            }

            fn bad(op: u32, r: T) -> bool {
                if op == FOP_RSQRTS {
                    r.is_nan() || abs(r) == T::INFINITY
                } else {
                    r.is_nan()
                }
            }

            /// (3 + a*b) / 2 with one rounding (a negated already).
            #[inline(never)]
            fn rsqrts(a: T, b: T) -> T {
                let (a, b) = (bb(a), bb(b));
                let r = if abs(a) >= fv(2 * MINN) {
                    $fma(a * 0.5, b, 1.5)
                } else if abs(b) >= fv(2 * MINN) {
                    $fma(a, b * 0.5, 1.5)
                } else {
                    $fma(a, b, 3.0) * 0.5
                };
                bb(r)
            }

            fn fix(c: &Cpu, op: u32, a: T, b: T, d: T) -> T {
                let (x, y, z) = (fb(a), fb(b), fb(d));
                let nan = isnan(x) || isnan(y);
                match op {
                    FOP_SQRT => return nan1(c, a),
                    FOP_MADD => {
                        if isnan(x) || isnan(y) || isnan(z) {
                            if isqnan(x) && ((isinf(y) && iszero(z)) || (iszero(y) && isinf(z))) {
                                return fv(DNAN);
                            }
                            return nan3(c, a, b, d);
                        }
                        return fv(DNAN);
                    }
                    FOP_MULX if !nan => return fv(((x ^ y) & SGN) | fb(2.0)),
                    FOP_RECPS if !nan => return 2.0,
                    FOP_RSQRTS if !nan => {
                        if (isinf(x) && iszero(y)) || (iszero(x) && isinf(y)) {
                            return 1.5;
                        }
                        return rsqrts(a, b);
                    }
                    FOP_NMUL => return neg(nan2(c, a, b)),
                    FOP_ABD => return fv(fb(nan2(c, a, b)) & !SGN),
                    _ => {}
                }
                nan2(c, a, b)
            }

            pub fn minmax(c: &Cpu, op: u32, a: T, b: T) -> T {
                let (x, y) = (fb(a), fb(b));
                if isnan(x) || isnan(y) {
                    if op == FOP_MAXNM || op == FOP_MINNM {
                        if isqnan(x) && !isnan(y) {
                            return b;
                        }
                        if isqnan(y) && !isnan(x) {
                            return a;
                        }
                    }
                    return nan2(c, a, b);
                }
                if op == FOP_MAX || op == FOP_MAXNM {
                    return if a == b {
                        fv(x & y)
                    } else if a > b {
                        a
                    } else {
                        b
                    };
                }
                if a == b {
                    fv(x | y)
                } else if a < b {
                    a
                } else {
                    b
                }
            }

            /// One FP operation as the guest computes it (simd_fp.h `op` through its slow path).
            pub fn op(c: &Cpu, op: u32, mut a: T, mut b: T, mut d: T) -> T {
                let fpcr = c.fpcr;
                if fpcr & FPCR_FZ != 0 {
                    a = flush(a);
                    b = flush(b);
                    d = flush(d);
                }
                if is_minmax(op) {
                    return minmax(c, op, a, b);
                }
                let old = rm_enter(fpcr);
                let mut r = if op == FOP_RSQRTS {
                    rsqrts(a, b)
                } else {
                    calc(op, a, b, d)
                };
                if bad(op, r) {
                    r = fix(c, op, a, b, d);
                }
                let m = fb(r) & !SGN;
                if fpcr & FPCR_FZ != 0 && m <= MINN && m != 0 {
                    if m == MINN {
                        rm_rz();
                        let z = calc(op, a, b, d);
                        if fb(z) & !SGN < MINN {
                            r = fv(fb(r) & SGN);
                        }
                    } else {
                        r = fv(fb(r) & SGN);
                    }
                }
                rm_leave(old);
                r
            }

            /// FCMP's NZCV.
            pub fn cmp(c: &Cpu, a: T, b: T) -> u64 {
                let (a, b) = (fz(c, a), fz(c, b));
                if a == b {
                    0x6000_0000
                } else if a < b {
                    0x8000_0000
                } else if a > b {
                    0x2000_0000
                } else {
                    0x3000_0000
                }
            }
            /// FCMEQ / FCMGE / FCMGT / FACGE / FACGT (op 0..4) of one lane: all ones or zero.
            pub fn vcmp(c: &Cpu, op: u32, a: T, b: T) -> U {
                let (mut a, mut b) = (fz(c, a), fz(c, b));
                if op >= 3 {
                    a = abs(a);
                    b = abs(b);
                }
                let t = match op {
                    0 => a == b,
                    1 | 3 => a >= b,
                    _ => a > b,
                };
                if t {
                    !0
                } else {
                    0
                }
            }

            /// rm 0 N, 1 P, 2 M, 3 Z, 4 A.
            pub fn rnd(x: T, rm: u32) -> T {
                match rm {
                    0 => x.round_ties_even(),
                    1 => x.ceil(),
                    2 => x.floor(),
                    3 => x.trunc(),
                    _ => x.round(),
                }
            }
            /// FRINT*: rm 0..4 as `rnd`, 5 X / 6 I = the FPCR mode.
            pub fn frint(c: &Cpu, x: T, rm: u32) -> T {
                let x = fz(c, x);
                let rm = if rm >= 5 {
                    ((c.fpcr >> 22) & 3) as u32
                } else {
                    rm
                };
                let r = rnd(x, rm);
                if r.is_nan() {
                    return nan1(c, x);
                }
                r
            }

            /// FP -> integer (FCVT[NPMZA][SU], fixed point): saturating, NaN -> 0; ib = 32 / 64.
            pub fn toint(c: &Cpu, x: T, rm: u32, fbits: u32, ib: u32, uns: bool) -> u64 {
                let mut x = fz(c, x);
                if fbits != 0 {
                    x = bb(bb(x) * ((1u64 << (fbits - 1)) as T)) * 2.0;
                }
                let r = rnd(x, rm);
                if r.is_nan() {
                    return 0;
                }
                if uns {
                    if r <= 0.0 {
                        return 0;
                    }
                    if ib == 32 {
                        return if r >= 4294967296.0 {
                            0xffff_ffff
                        } else {
                            r as u32 as u64
                        };
                    }
                    return if r >= 18446744073709551616.0 {
                        !0
                    } else {
                        r as u64
                    };
                }
                if ib == 32 {
                    return if r >= 2147483648.0 {
                        0x7fff_ffff
                    } else if r < -2147483648.0 {
                        0x8000_0000
                    } else {
                        r as i32 as u32 as u64
                    };
                }
                if r >= 9223372036854775808.0 {
                    0x7fff_ffff_ffff_ffff
                } else if r < -9223372036854775808.0 {
                    0x8000_0000_0000_0000
                } else {
                    r as i64 as u64
                }
            }

            /// integer -> FP (SCVTF / UCVTF, fixed point), rounded in the FPCR mode.
            pub fn fromint(c: &Cpu, v: u64, ib: u32, uns: bool, fbits: u32) -> T {
                let old = rm_enter(c.fpcr);
                let v = bb(v);
                let r: T = bb(if uns {
                    if ib == 32 {
                        v as u32 as i64 as T
                    } else if (v as i64) >= 0 {
                        v as i64 as T
                    } else {
                        // the high bit set: halve with a sticky bit, convert, double (exact). Rust's own u64 -> float
                        // lowering subtracts magic numbers and gives -0 for 0 when rounding toward minus infinity
                        bb(((v >> 1) | (v & 1)) as i64 as T) * 2.0
                    }
                } else if ib == 32 {
                    v as i32 as T
                } else {
                    v as i64 as T
                });
                rm_leave(old);
                if fbits != 0 {
                    return bb(bb(r) / ((1u64 << (fbits - 1)) as T)) / 2.0;
                }
                r
            }

            pub fn recpe(c: &Cpu, a: T) -> T {
                let tab = &crate::interp::tables::get().recip;
                let x = fb(fz(c, a));
                let s = x & SGN;
                if isnan(x) {
                    return nan1(c, a);
                }
                if isinf(x) {
                    return fv(s);
                }
                if iszero(x) {
                    return fv(s | EXPM);
                }
                let mag = x & !SGN;
                if mag < (1 as U) << (MB - 2) {
                    let rm = (c.fpcr >> 22) & 3;
                    let inf = rm == 0 || (rm == 1 && s == 0) || (rm == 2 && s != 0);
                    return fv(s | if inf { EXPM } else { EXPM - 1 });
                }
                let mut e = (mag >> MB) as i64;
                let mut frac = mag & (((1 as U) << MB) - 1);
                if c.fpcr & FPCR_FZ != 0 && e >= ((1i64 << EB) - 3) {
                    return fv(s);
                }
                if e == 0 {
                    if frac >> (MB - 1) == 0 {
                        e = -1;
                        frac = (frac << 2) & (((1 as U) << MB) - 1);
                    } else {
                        frac = (frac << 1) & (((1 as U) << MB) - 1);
                    }
                }
                let scaled = 256 | (frac >> (MB - 8)) as usize;
                let mut re = ((1i64 << EB) - 3) - e;
                let est = tab[scaled - 256] as U;
                let mut f = est << (MB - 8);
                if re == 0 {
                    f = (f >> 1) | ((1 as U) << (MB - 1));
                } else if re == -1 {
                    f = (f >> 2) | ((1 as U) << (MB - 2));
                    re = 0;
                }
                fv(s | ((re as U) << MB) | f)
            }
            pub fn rsqrte(c: &Cpu, a: T) -> T {
                let tab = &crate::interp::tables::get().rsqrt;
                let x = fb(fz(c, a));
                let s = x & SGN;
                if isnan(x) {
                    return nan1(c, a);
                }
                if iszero(x) {
                    return fv(s | EXPM);
                }
                if s != 0 {
                    return fv(DNAN);
                }
                if isinf(x) {
                    return 0.0;
                }
                let mut e = (x >> MB) as i64;
                let mut frac = x & (((1 as U) << MB) - 1);
                if e == 0 {
                    while frac >> (MB - 1) == 0 {
                        frac <<= 1;
                        e -= 1;
                    }
                    frac = (frac << 1) & (((1 as U) << MB) - 1);
                }
                let scaled = if e & 1 != 0 {
                    128 | (frac >> (MB - 7)) as usize
                } else {
                    256 | (frac >> (MB - 8)) as usize
                };
                let re = ((3 * ((1i64 << (EB - 1)) - 1) - 1) - e) / 2;
                let est = tab[scaled - 128] as U;
                fv(((re as U) << MB) | (est << (MB - 8)))
            }
            pub fn recpx(c: &Cpu, a: T) -> T {
                let x = fb(fz(c, a));
                let s = x & SGN;
                if isnan(x) {
                    return nan1(c, a);
                }
                if x & EXPM == 0 {
                    return fv(s | (EXPM - MINN));
                }
                fv(s | (!x & EXPM))
            }
        }
    };
}

fp_impl!(fs, f32, u32, 32, 8, 23, fma32);
fp_impl!(fd, f64, u64, 64, 11, 52, fma64);

// ---- precision conversions (FCVT, FCVTL/N, FCVTXN); FZ flushes single / double inputs, never halves

pub fn s2d(c: &Cpu, x: f32) -> f64 {
    let u = fs::fb(fs::fz(c, x));
    if fs::isnan(u) {
        if c.fpcr & FPCR_DN != 0 {
            return fd::fv(0x7ff8_0000_0000_0000);
        }
        return fd::fv(
            ((u as u64 >> 31) << 63) | 0x7ff8_0000_0000_0000 | ((u as u64 & 0x3f_ffff) << 29),
        );
    }
    bb(fs::fv(u)) as f64
}

pub fn d2s(c: &Cpu, x: f64) -> f32 {
    let u = fd::fb(fd::fz(c, x));
    if fd::isnan(u) {
        if c.fpcr & FPCR_DN != 0 {
            return fs::fv(0x7fc0_0000);
        }
        return fs::fv(((u >> 63) as u32) << 31 | 0x7fc0_0000 | ((u >> 29) & 0x3f_ffff) as u32);
    }
    let x = fd::fv(u);
    if c.fpcr & FPCR_FZ != 0 && x.abs() < f64::from_bits(0x3810_0000_0000_0000) && x != 0.0 {
        return fs::fv(((u >> 63) as u32) << 31);
    }
    let old = rm_enter(c.fpcr);
    let r = bb(bb(x) as f32);
    rm_leave(old);
    r
}

/// Round to odd (FCVTXN).
pub fn d2s_odd(c: &Cpu, x: f64) -> f32 {
    let u = fd::fb(fd::fz(c, x));
    if fd::isnan(u) {
        return d2s(c, x);
    }
    let x = fd::fv(u);
    if c.fpcr & FPCR_FZ != 0 && x.abs() < f64::from_bits(0x3810_0000_0000_0000) && x != 0.0 {
        return fs::fv(((u >> 63) as u32) << 31);
    }
    let f = bb(bb(x) as f32);
    if f as f64 != x && !x.is_nan() {
        let mut b = fs::fb(f);
        if (f as f64).abs() > x.abs() {
            b -= 1;
        }
        return fs::fv(b | 1);
    }
    f
}

fn rnd_shift(m: u64, sh: i32, rm: u32, neg: bool) -> u64 {
    if sh >= 64 {
        return ((rm == 1 && !neg && m != 0) || (rm == 2 && neg && m != 0)) as u64;
    }
    let mut q = m >> sh;
    let rem = m & ((1u64 << sh) - 1);
    let half = 1u64 << (sh - 1);
    match rm {
        0 => q += (rem > half || (rem == half && q & 1 != 0)) as u64,
        1 => q += (rem != 0 && !neg) as u64,
        2 => q += (rem != 0 && neg) as u64,
        _ => {}
    }
    q
}

/// double bits (the exact image of the source, NaN payload left-aligned) -> half, FPCR rounding.
pub fn d2h(c: &Cpu, u: u64) -> u16 {
    let fpcr = c.fpcr;
    let ahp = fpcr & FPCR_AHP != 0;
    let rm = ((fpcr >> 22) & 3) as u32;
    let neg = u >> 63 != 0;
    let s: u16 = (neg as u16) << 15;
    let e = ((u >> 52) & 0x7ff) as i32;
    let f = u & ((1u64 << 52) - 1);
    if e == 0x7ff {
        if f != 0 {
            return if ahp {
                s
            } else if fpcr & FPCR_DN != 0 {
                0x7e00
            } else {
                s | 0x7e00 | (f >> 42) as u16
            };
        }
        return if ahp { s | 0x7fff } else { s | 0x7c00 };
    }
    if e == 0 && f == 0 {
        return s;
    }
    let m = if e != 0 { f | (1u64 << 52) } else { f };
    let (big_e, ue) = if e != 0 {
        (e - 1075, e - 1023)
    } else {
        (-1074, -1023)
    };
    let bits: i64 = if ue < -14 {
        rnd_shift(m, -24 - big_e, rm, neg) as i64
    } else {
        (((ue + 14) as i64) << 10) + rnd_shift(m, 42, rm, neg) as i64
    };
    if ahp {
        return s | if bits > 0x7fff { 0x7fff } else { bits as u16 };
    }
    if bits >= 0x7c00 {
        let inf = rm == 0 || (rm == 1 && !neg) || (rm == 2 && neg);
        return s | if inf { 0x7c00 } else { 0x7bff };
    }
    s | bits as u16
}

pub fn s2h(c: &Cpu, x: f32) -> u16 {
    let u = fs::fb(fs::fz(c, x));
    let d = if fs::isnan(u) {
        ((u as u64 >> 31) << 63) | 0x7ff0_0000_0000_0000 | ((u as u64 & 0x7f_ffff) << 29)
    } else {
        (fs::fv(u) as f64).to_bits()
    };
    d2h(c, d)
}
pub fn dd2h(c: &Cpu, x: f64) -> u16 {
    d2h(c, fd::fb(fd::fz(c, x)))
}
pub fn h2dbits(c: &Cpu, h: u16) -> u64 {
    let s = ((h >> 15) as u64) << 63;
    let e = ((h >> 10) & 31) as i64;
    let f = (h & 0x3ff) as u64;
    if e == 31 && c.fpcr & FPCR_AHP == 0 {
        if f == 0 {
            return s | 0x7ff0_0000_0000_0000;
        }
        if c.fpcr & FPCR_DN != 0 {
            return 0x7ff8_0000_0000_0000;
        }
        return s | 0x7ff8_0000_0000_0000 | (f << 42);
    }
    if e == 0 {
        return ((if s != 0 { -1.0f64 } else { 1.0 })
            * f as f64
            * f64::from_bits(0x3e70_0000_0000_0000))
        .to_bits();
    }
    s | (((e - 15 + 1023) as u64) << 52) | (f << 42)
}
pub fn h2d(c: &Cpu, h: u16) -> f64 {
    fd::fv(h2dbits(c, h))
}
pub fn h2s(c: &Cpu, h: u16) -> f32 {
    let d = h2dbits(c, h);
    if fd::isnan(d) {
        return fs::fv(((d >> 63) as u32) << 31 | 0x7fc0_0000 | ((d >> 29) & 0x3f_ffff) as u32);
    }
    fd::fv(d) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nan_rules_and_rounding() {
        let mut c = Cpu::new_boxed();
        // signalling NaN wins and is quietened
        let snan = f32::from_bits(0x7f80_0001);
        let qnan = f32::from_bits(0x7fc0_1234);
        assert_eq!(fs::op(&c, FOP_ADD, qnan, snan, 0.0).to_bits(), 0x7fc0_0001);
        // default NaN
        c.fpcr = FPCR_DN;
        assert_eq!(fs::op(&c, FOP_ADD, qnan, 1.0, 0.0).to_bits(), 0x7fc0_0000);
        c.fpcr = 0;
        // inf - inf: default NaN
        assert_eq!(
            fd::op(&c, FOP_SUB, f64::INFINITY, f64::INFINITY, 0.0).to_bits(),
            0x7ff8_0000_0000_0000
        );
        // round toward plus infinity
        c.fpcr = 1 << 22;
        let r = fs::op(&c, FOP_ADD, 1.0, f32::from_bits(1), 0.0);
        assert_eq!(r.to_bits(), 0x3f80_0001);
        c.fpcr = 0;
        assert_eq!(fs::op(&c, FOP_ADD, 1.0, f32::from_bits(1), 0.0), 1.0);
        // FMAXNM with a quiet NaN takes the number
        assert_eq!(fs::op(&c, FOP_MAXNM, qnan, 2.0, 0.0), 2.0);
        // max(-0, +0) = +0
        assert_eq!(fs::op(&c, FOP_MAX, -0.0, 0.0, 0.0).to_bits(), 0);
        // conversions saturate
        assert_eq!(fs::toint(&c, 3e9, 3, 0, 32, false), 0x7fff_ffff);
        assert_eq!(fs::toint(&c, -1.5, 3, 0, 32, true), 0);
        assert_eq!(fd::toint(&c, -2.5, 0, 0, 64, false), (-2i64) as u64);
        assert_eq!(fs::fromint(&c, 3, 32, false, 1), 1.5);
        // halves
        assert_eq!(s2h(&c, 1.0), 0x3c00);
        assert_eq!(h2s(&c, 0x3c00), 1.0);
        assert_eq!(d2s(&c, 1.0 / 3.0), 1.0f32 / 3.0);
    }
}

//! Integer lanes, structure loads / stores, table lookups, CRC and crypto for the interpreter: the semantics of the
//! generated code's `simd.h` helpers (`lop`, `sl_*`, `sv_*`), with the lane size, count and operation as values.

use crate::cpu::Cpu;
use crate::interp::mem::{ld, st};

pub const LOP_SQADD: u32 = 0;
pub const LOP_UQADD: u32 = 1;
pub const LOP_SQSUB: u32 = 2;
pub const LOP_UQSUB: u32 = 3;
pub const LOP_SUQADD: u32 = 4;
pub const LOP_USQADD: u32 = 5;
pub const LOP_SHL: u32 = 6;
pub const LOP_RSHL: u32 = 7;
pub const LOP_QSHL: u32 = 8;
pub const LOP_QRSHL: u32 = 9;
pub const LOP_SHLI: u32 = 10;
pub const LOP_QSHLI: u32 = 11;
pub const LOP_QSHLUI: u32 = 12;
pub const LOP_SHRI: u32 = 13;
pub const LOP_RSHRI: u32 = 14;
pub const LOP_SRAI: u32 = 15;
pub const LOP_RSRAI: u32 = 16;
pub const LOP_SRI: u32 = 17;
pub const LOP_SLI: u32 = 18;
pub const LOP_SQDMULH: u32 = 19;
pub const LOP_SQRDMULH: u32 = 20;
pub const LOP_PMUL: u32 = 21;
pub const LOP_SQABS: u32 = 22;
pub const LOP_SQNEG: u32 = 23;
pub const LOP_ABS: u32 = 24;
pub const LOP_NEG: u32 = 25;
pub const LOP_CLS: u32 = 26;
pub const LOP_CLZ: u32 = 27;
pub const LOP_CNT: u32 = 28;
pub const LOP_RBIT: u32 = 29;
pub const LOP_URECPE: u32 = 30;
pub const LOP_URSQRTE: u32 = 31;
pub const LOP_HADD: u32 = 32;
pub const LOP_RHADD: u32 = 33;
pub const LOP_HSUB: u32 = 34;
pub const LOP_ABD: u32 = 35;
pub const LOP_ABA: u32 = 36;
pub const LOP_ADDL: u32 = 37;
pub const LOP_SUBL: u32 = 38;
pub const LOP_ABDL: u32 = 39;
pub const LOP_ABAL: u32 = 40;
pub const LOP_MLAL: u32 = 41;
pub const LOP_MLSL: u32 = 42;
pub const LOP_MULL: u32 = 43;
pub const LOP_ADDW: u32 = 44;
pub const LOP_SUBW: u32 = 45;
pub const LOP_SQDMULL: u32 = 46;
pub const LOP_SQDMLAL: u32 = 47;
pub const LOP_SQDMLSL: u32 = 48;
pub const LOP_PMULL: u32 = 49;
pub const LOP_SHLL: u32 = 50;
pub const LOP_XTN: u32 = 51;
pub const LOP_SQXTN: u32 = 52;
pub const LOP_UQXTN: u32 = 53;
pub const LOP_SQXTUN: u32 = 54;
pub const LOP_SHRN: u32 = 55;
pub const LOP_RSHRN: u32 = 56;
pub const LOP_QSHRN: u32 = 57;
pub const LOP_QRSHRN: u32 = 58;
pub const LOP_QSHRUN: u32 = 59;
pub const LOP_QRSHRUN: u32 = 60;
pub const LOP_ADDHN: u32 = 61;
pub const LOP_RADDHN: u32 = 62;
pub const LOP_SUBHN: u32 = 63;
pub const LOP_RSUBHN: u32 = 64;
pub const LOP_SHLLI: u32 = 65;
/// The unsigned variant of an op.
pub const LOP_U: u32 = 256;

#[inline]
pub fn lmask(sz: u32) -> u64 {
    if sz >= 3 {
        !0
    } else {
        (1u64 << (8 << sz)) - 1
    }
}
#[inline]
pub fn lsx(v: u64, sz: u32) -> i64 {
    let s = 64 - (8 << sz);
    ((v << s) as i64) >> s
}
#[inline]
pub fn lget(v: u128, sz: u32, i: u32) -> u64 {
    ((v >> ((i << sz) * 8)) as u64) & lmask(sz)
}
#[inline]
pub fn lset(v: &mut u128, sz: u32, i: u32, x: u64) {
    let sh = (i << sz) * 8;
    let m = (lmask(sz) as u128) << sh;
    *v = (*v & !m) | (((x & lmask(sz)) as u128) << sh);
}

fn sat_s(v: i128, sz: u32) -> u64 {
    let hi = (1i128 << ((8 << sz) - 1)) - 1;
    let lo = -hi - 1;
    (v.clamp(lo, hi) as u64) & lmask(sz)
}
fn sat_u(v: i128, sz: u32) -> u64 {
    let hi = (1i128 << (8 << sz)) - 1;
    v.clamp(0, hi) as u64
}
fn sat_uu(v: u128, sz: u32) -> u64 {
    if v > lmask(sz) as u128 {
        lmask(sz)
    } else {
        v as u64
    }
}
fn shl128(v: i128, s: u32) -> i128 {
    ((v as u128) << s) as i128
}
fn lshr_s(v: i64, s: i32, rnd: bool) -> i128 {
    let s = s.min(100);
    if rnd {
        (v as i128 + (1i128 << (s - 1))) >> s
    } else {
        (v as i128) >> s
    }
}
fn lshr_u(v: u64, s: i32, rnd: bool) -> u128 {
    let s = s.min(100);
    if rnd {
        (v as u128 + (1u128 << (s - 1))) >> s
    } else {
        (v as u128) >> s
    }
}
/// Carry-less 64 x 64 -> (low, high).
pub fn pmul64(a: u64, b: u64) -> (u64, u64) {
    let (mut lo, mut h) = (0u64, 0u64);
    for i in 0..64 {
        if (b >> i) & 1 != 0 {
            lo ^= a << i;
            if i != 0 {
                h ^= a >> (64 - i);
            }
        }
    }
    (lo, h)
}

/// One lane of an integer op (simd.h `lop`).
pub fn lop(op: u32, sz: u32, a: u64, b: u64, d: u64) -> u64 {
    let u = op >> 8 != 0;
    let es = 8u32 << sz;
    let op = op & 255;
    let (sa, sb) = (lsx(a, sz), lsx(b, sz));
    let (xa, xb): (i128, i128) = if u {
        (a as i128, b as i128)
    } else {
        (sa as i128, sb as i128)
    };
    let sh = b as u8 as i8 as i32;
    let t = crate::interp::tables::get();
    let abd = |x: i128, y: i128| if x > y { x - y } else { y - x };
    match op {
        LOP_SQADD => sat_s(sa as i128 + sb as i128, sz),
        LOP_UQADD => sat_u(a as i128 + b as i128, sz),
        LOP_SQSUB => sat_s(sa as i128 - sb as i128, sz),
        LOP_UQSUB => sat_u(a as i128 - b as i128, sz),
        LOP_SUQADD => sat_s(lsx(d, sz) as i128 + a as i128, sz),
        LOP_USQADD => sat_u(d as i128 + sa as i128, sz),
        LOP_SHL | LOP_RSHL | LOP_QSHL | LOP_QRSHL => {
            let sat = op == LOP_QSHL || op == LOP_QRSHL;
            let rnd = op == LOP_RSHL || op == LOP_QRSHL;
            if sh >= 0 {
                let s = sh.min(64) as u32;
                if sat {
                    return if u {
                        sat_uu((a as u128) << s, sz)
                    } else {
                        sat_s(shl128(sa as i128, s), sz)
                    };
                }
                return if sh as u32 >= es { 0 } else { a << sh };
            }
            if u {
                lshr_u(a, -sh, rnd) as u64
            } else {
                lshr_s(sa, -sh, rnd) as u64
            }
        }
        LOP_SHLI => a.wrapping_shl(b as u32),
        LOP_QSHLI => {
            if u {
                sat_uu((a as u128) << b, sz)
            } else {
                sat_s(shl128(sa as i128, b as u32), sz)
            }
        }
        LOP_QSHLUI => sat_u(shl128(sa as i128, b as u32), sz),
        LOP_SHRI => {
            if u {
                lshr_u(a, b as i32, false) as u64
            } else {
                lshr_s(sa, b as i32, false) as u64
            }
        }
        LOP_RSHRI => {
            if u {
                lshr_u(a, b as i32, true) as u64
            } else {
                lshr_s(sa, b as i32, true) as u64
            }
        }
        LOP_SRAI => d.wrapping_add(if u {
            lshr_u(a, b as i32, false) as u64
        } else {
            lshr_s(sa, b as i32, false) as u64
        }),
        LOP_RSRAI => d.wrapping_add(if u {
            lshr_u(a, b as i32, true) as u64
        } else {
            lshr_s(sa, b as i32, true) as u64
        }),
        LOP_SRI => {
            let m = if b as u32 >= es { 0 } else { lmask(sz) >> b };
            (d & !m) | if b as u32 >= es { 0 } else { a >> b }
        }
        LOP_SLI => {
            let m = (lmask(sz) << b) & lmask(sz);
            (d & !m) | ((a << b) & m)
        }
        LOP_SQDMULH => sat_s((sa as i128 * sb as i128 * 2) >> es, sz),
        LOP_SQRDMULH => sat_s(
            (sa as i128 * sb as i128 * 2 + (1i128 << (es - 1))) >> es,
            sz,
        ),
        LOP_PMUL | LOP_PMULL => pmul64(a, b).0,
        LOP_SQABS => sat_s(if sa < 0 { -(sa as i128) } else { sa as i128 }, sz),
        LOP_SQNEG => sat_s(-(sa as i128), sz),
        LOP_ABS => {
            if sa < 0 {
                sa.wrapping_neg() as u64
            } else {
                sa as u64
            }
        }
        LOP_NEG => a.wrapping_neg(),
        LOP_CLS => {
            let x = ((sa ^ (sa >> 1)) as u64) & lmask(sz);
            (if x != 0 {
                (x << (64 - es)).leading_zeros() as u64
            } else {
                es as u64
            })
            .wrapping_sub(1)
        }
        LOP_CLZ => {
            if a != 0 {
                (a << (64 - es)).leading_zeros() as u64
            } else {
                es as u64
            }
        }
        LOP_CNT => a.count_ones() as u64,
        LOP_RBIT => (a as u8).reverse_bits() as u64,
        LOP_URECPE => {
            if a >> 31 != 0 {
                ((256 | t.recip[((a >> 23) & 255) as usize] as u64) << 23) & 0xffff_ffff
            } else {
                0xffff_ffff
            }
        }
        LOP_URSQRTE => {
            if a >> 30 != 0 {
                ((256 | t.rsqrt[(((a >> 23) & 511) - 128) as usize] as u64) << 23) & 0xffff_ffff
            } else {
                0xffff_ffff
            }
        }
        LOP_HADD => ((xa + xb) >> 1) as u64,
        LOP_RHADD => ((xa + xb + 1) >> 1) as u64,
        LOP_HSUB => ((xa - xb) >> 1) as u64,
        LOP_ABD | LOP_ABDL => abd(xa, xb) as u64,
        LOP_ABA | LOP_ABAL => d.wrapping_add(abd(xa, xb) as u64),
        LOP_ADDL => (xa + xb) as u64,
        LOP_SUBL => (xa - xb) as u64,
        LOP_MLAL => d.wrapping_add((xa * xb) as u64),
        LOP_MLSL => d.wrapping_sub((xa * xb) as u64),
        LOP_MULL => (xa * xb) as u64,
        LOP_ADDW => a.wrapping_add(xb as u64),
        LOP_SUBW => a.wrapping_sub(xb as u64),
        LOP_SQDMULL => sat_s(sa as i128 * sb as i128 * 2, sz + 1),
        LOP_SQDMLAL => sat_s(
            lsx(d, sz + 1) as i128
                + lsx(sat_s(sa as i128 * sb as i128 * 2, sz + 1), sz + 1) as i128,
            sz + 1,
        ),
        LOP_SQDMLSL => sat_s(
            lsx(d, sz + 1) as i128
                - lsx(sat_s(sa as i128 * sb as i128 * 2, sz + 1), sz + 1) as i128,
            sz + 1,
        ),
        LOP_SHLL => a << es,
        LOP_XTN => a,
        LOP_SQXTN => sat_s(lsx(a, sz + 1) as i128, sz),
        LOP_UQXTN => sat_u(a as i128, sz),
        LOP_SQXTUN => sat_u(lsx(a, sz + 1) as i128, sz),
        LOP_SHRN => a.wrapping_shr(b as u32),
        LOP_RSHRN => lshr_u(a, b as i32, true) as u64,
        LOP_QSHRN | LOP_QRSHRN => {
            let rnd = op == LOP_QRSHRN;
            if u {
                sat_u(lshr_u(a, b as i32, rnd) as i128, sz)
            } else {
                sat_s(lshr_s(lsx(a, sz + 1), b as i32, rnd), sz)
            }
        }
        LOP_QSHRUN => sat_u(lshr_s(lsx(a, sz + 1), b as i32, false), sz),
        LOP_QRSHRUN => sat_u(lshr_s(lsx(a, sz + 1), b as i32, true), sz),
        LOP_ADDHN => a.wrapping_add(b) >> es,
        LOP_RADDHN => a.wrapping_add(b).wrapping_add(1u64 << (es - 1)) >> es,
        LOP_SUBHN => a.wrapping_sub(b) >> es,
        LOP_RSUBHN => a.wrapping_sub(b).wrapping_add(1u64 << (es - 1)) >> es,
        LOP_SHLLI => (xa as u64) << b,
        _ => 0,
    }
}

/// The generic lane loop (simd.h `sl_gen`): `ne` lanes; result lane i (size szd) goes to lane offd+i of d (the other
/// lanes kept when `keep`, else zero); a = n's lane offn+i (size szn), b = m's lane offm+i (size szm), or lane `bidx`
/// of m when bidx >= 0, or the constant `bimm` when m < 0; the accumulator is d's old lane offd+i.
#[allow(clippy::too_many_arguments)]
pub fn gen(
    c: &mut Cpu,
    op: u32,
    sz: u32,
    ne: u32,
    d: u32,
    szd: u32,
    offd: u32,
    keep: bool,
    n: u32,
    szn: u32,
    offn: u32,
    m: i32,
    szm: u32,
    offm: u32,
    bidx: i32,
    bimm: u64,
) {
    let a = c.v[n as usize];
    let b = if m >= 0 { c.v[m as usize] } else { 0 };
    let o = c.v[d as usize];
    let mut r = if keep { o } else { 0 };
    for i in 0..ne {
        let y = if m < 0 {
            bimm
        } else {
            lget(b, szm, if bidx >= 0 { bidx as u32 } else { offm + i })
        };
        lset(
            &mut r,
            szd,
            offd + i,
            lop(op, sz, lget(a, szn, offn + i), y, lget(o, szd, offd + i)),
        );
    }
    c.v[d as usize] = r;
}

/// Lane-wise op, every operand the same size (simd.rs `same`).
#[allow(clippy::too_many_arguments)]
pub fn same(c: &mut Cpu, op: u32, sz: u32, ne: u32, d: u32, n: u32, m: i32, bimm: u64, bidx: i32) {
    gen(
        c, op, sz, ne, d, sz, 0, false, n, sz, 0, m, sz, 0, bidx, bimm,
    )
}

/// SADDLP / UADDLP / SADALP / UADALP.
pub fn addlp(c: &mut Cpu, u: bool, acc: bool, q: u32, sz: u32, d: u32, n: u32) {
    let (a, o) = (c.v[n as usize], c.v[d as usize]);
    let mut r = 0u128;
    for i in 0..((4 << q) >> sz) {
        let (x, y) = (lget(a, sz, 2 * i), lget(a, sz, 2 * i + 1));
        let s = if u {
            x.wrapping_add(y)
        } else {
            (lsx(x, sz) + lsx(y, sz)) as u64
        };
        lset(
            &mut r,
            sz + 1,
            i,
            s.wrapping_add(if acc { lget(o, sz + 1, i) } else { 0 }),
        );
    }
    c.v[d as usize] = r;
}

/// ADDV, SADDLV / UADDLV, SMAXV / UMAXV, SMINV / UMINV (op 0..3).
pub fn across(c: &mut Cpu, op: u32, u: bool, q: u32, sz: u32, d: u32, n: u32) {
    let a = c.v[n as usize];
    let ne = (8 << q) >> sz;
    let get = |i| {
        if u {
            lget(a, sz, i) as i64
        } else {
            lsx(lget(a, sz, i), sz)
        }
    };
    let mut acc = get(0);
    for i in 1..ne {
        let x = get(i);
        acc = match op {
            0 | 1 => acc.wrapping_add(x),
            2 => {
                if u {
                    if x as u64 > acc as u64 {
                        x
                    } else {
                        acc
                    }
                } else if x > acc {
                    x
                } else {
                    acc
                }
            }
            _ => {
                if u {
                    if (x as u64) < acc as u64 {
                        x
                    } else {
                        acc
                    }
                } else if x < acc {
                    x
                } else {
                    acc
                }
            }
        };
    }
    c.v[d as usize] = (acc as u64 & lmask(if op == 1 { sz + 1 } else { sz })) as u128;
}

/// PMULL / PMULL2 1Q.
pub fn pmull64(c: &mut Cpu, part: u32, d: u32, n: u32, m: u32) {
    let (lo, hi) = pmul64(
        (c.v[n as usize] >> (64 * part)) as u64,
        (c.v[m as usize] >> (64 * part)) as u64,
    );
    c.v[d as usize] = ((hi as u128) << 64) | lo as u128;
}

/// TBL / TBX with len+1 table registers.
pub fn tbl(c: &mut Cpu, q: u32, len: u32, tbx: bool, d: u32, n: u32, m: u32) {
    let mut tab = [0u8; 64];
    for i in 0..=len {
        tab[16 * i as usize..16 * i as usize + 16]
            .copy_from_slice(&c.v[((n + i) & 31) as usize].to_le_bytes());
    }
    let (idx, o) = (c.v[m as usize], c.v[d as usize]);
    let mut r = 0u128;
    for i in 0..(8 << q) {
        let k = lget(idx, 0, i) as usize;
        let v = if k < 16 * (len as usize + 1) {
            tab[k] as u64
        } else if tbx {
            lget(o, 0, i)
        } else {
            0
        };
        lset(&mut r, 0, i, v);
    }
    c.v[d as usize] = r;
}

/// CRC32 / CRC32C over `bytes` bytes of v.
pub fn crc(castagnoli: bool, acc: u32, v: u64, bytes: u32) -> u32 {
    let t = crate::interp::tables::get();
    let tab = if castagnoli { &t.crc32c } else { &t.crc32 };
    let mut acc = acc;
    for i in 0..bytes {
        acc = tab[((acc ^ (v >> (8 * i)) as u8 as u32) & 0xff) as usize] ^ (acc >> 8);
    }
    acc
}

fn aes_xt(x: u8) -> u8 {
    (x << 1) ^ ((x >> 7) * 0x1b)
}
fn aes_mul(x: u8, k: u32) -> u8 {
    let (x2, x4, x8) = (aes_xt(x), aes_xt(aes_xt(x)), aes_xt(aes_xt(aes_xt(x))));
    x8 ^ if k & 4 != 0 { x4 } else { 0 }
        ^ if k & 2 != 0 { x2 } else { 0 }
        ^ if k & 1 != 0 { x } else { 0 }
}

/// 0 AESE, 1 AESD, 2 AESMC, 3 AESIMC.
pub fn aes(c: &mut Cpu, op: u32, d: u32, n: u32) {
    let t = crate::interp::tables::get();
    let v = if op < 2 {
        c.v[d as usize] ^ c.v[n as usize]
    } else {
        c.v[n as usize]
    };
    let s = v.to_le_bytes();
    let mut r = [0u8; 16];
    match op {
        0 => (0..16).for_each(|i| r[i] = t.aes_sbox[s[(i + 4 * (i & 3)) & 15] as usize]),
        1 => {
            (0..16).for_each(|i| r[i] = t.aes_isbox[s[(i.wrapping_sub(4 * (i & 3))) & 15] as usize])
        }
        _ => {
            for col in 0..4 {
                let a = &s[4 * col..4 * col + 4];
                for i in 0..4 {
                    let (a0, a1, a2, a3) = (a[i], a[(i + 1) & 3], a[(i + 2) & 3], a[(i + 3) & 3]);
                    r[4 * col + i] = if op == 2 {
                        aes_xt(a0) ^ aes_xt(a1) ^ a1 ^ a2 ^ a3
                    } else {
                        aes_mul(a0, 14) ^ aes_mul(a1, 11) ^ aes_mul(a2, 13) ^ aes_mul(a3, 9)
                    };
                }
            }
        }
    }
    c.v[d as usize] = u128::from_le_bytes(r);
}

fn words(v: u128) -> [u32; 4] {
    [
        v as u32,
        (v >> 32) as u32,
        (v >> 64) as u32,
        (v >> 96) as u32,
    ]
}
fn from_words(w: [u32; 4]) -> u128 {
    w[0] as u128 | (w[1] as u128) << 32 | (w[2] as u128) << 64 | (w[3] as u128) << 96
}

/// 0 SHA1C, 1 SHA1P, 2 SHA1M.
pub fn sha1hash(c: &mut Cpu, op: u32, d: u32, n: u32, m: u32) {
    let mut x = words(c.v[d as usize]);
    let w = words(c.v[m as usize]);
    let mut y = c.v[n as usize] as u32;
    for &we in &w {
        let t = match op {
            0 => ((x[2] ^ x[3]) & x[1]) ^ x[3],
            1 => x[1] ^ x[2] ^ x[3],
            _ => (x[1] & x[2]) | ((x[1] | x[2]) & x[3]),
        };
        y = y
            .wrapping_add(x[0].rotate_left(5))
            .wrapping_add(t)
            .wrapping_add(we);
        x[1] = x[1].rotate_left(30);
        let top = x[3];
        x[3] = x[2];
        x[2] = x[1];
        x[1] = x[0];
        x[0] = y;
        y = top;
    }
    c.v[d as usize] = from_words(x);
}

/// SHA256H (part1) / SHA256H2.
pub fn sha256hash(c: &mut Cpu, part1: bool, d: u32, n: u32, m: u32) {
    let (mut x, mut y) = if part1 {
        (words(c.v[d as usize]), words(c.v[n as usize]))
    } else {
        (words(c.v[n as usize]), words(c.v[d as usize]))
    };
    let w = words(c.v[m as usize]);
    for &we in &w {
        let chs = ((y[1] ^ y[2]) & y[0]) ^ y[2];
        let maj = (x[0] & x[1]) | ((x[0] | x[1]) & x[2]);
        let t1 = y[3]
            .wrapping_add(y[0].rotate_right(6) ^ y[0].rotate_right(11) ^ y[0].rotate_right(25))
            .wrapping_add(chs)
            .wrapping_add(we);
        x[3] = t1.wrapping_add(x[3]);
        y[3] = t1
            .wrapping_add(x[0].rotate_right(2) ^ x[0].rotate_right(13) ^ x[0].rotate_right(22))
            .wrapping_add(maj);
        let t = y[3];
        y[3] = y[2];
        y[2] = y[1];
        y[1] = y[0];
        y[0] = x[3];
        x[3] = x[2];
        x[2] = x[1];
        x[1] = x[0];
        x[0] = t;
    }
    c.v[d as usize] = from_words(if part1 { x } else { y });
}

/// 0 SHA1SU0, 1 SHA1SU1, 2 SHA256SU0, 3 SHA256SU1, 4 SHA1H.
pub fn sha_misc(c: &mut Cpu, op: u32, d: u32, n: u32, m: u32) {
    let (a, b, k) = (
        words(c.v[d as usize]),
        words(c.v[n as usize]),
        words(c.v[m as usize]),
    );
    let mut r = [0u32; 4];
    match op {
        0 => {
            let t = [a[2], a[3], b[0], b[1]];
            (0..4).for_each(|i| r[i] = t[i] ^ a[i] ^ k[i]);
        }
        1 => {
            let t: [u32; 4] = core::array::from_fn(|i| a[i] ^ if i < 3 { b[i + 1] } else { 0 });
            (0..4).for_each(|i| r[i] = t[i].rotate_left(1));
            r[3] ^= t[0].rotate_left(2);
        }
        2 => {
            let t = [a[1], a[2], a[3], b[0]];
            (0..4).for_each(|i| {
                r[i] =
                    (t[i].rotate_right(7) ^ t[i].rotate_right(18) ^ (t[i] >> 3)).wrapping_add(a[i])
            });
        }
        3 => {
            let t0 = [b[1], b[2], b[3], k[0]];
            for e in 0..4 {
                let x = if e < 2 { k[e + 2] } else { r[e - 2] };
                r[e] = (x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10))
                    .wrapping_add(a[e])
                    .wrapping_add(t0[e]);
            }
        }
        _ => {
            c.v[d as usize] = b[0].rotate_left(30) as u128;
            return;
        }
    }
    c.v[d as usize] = from_words(r);
}

// ---- loads / stores of lanes

/// LDn / STn (multiple structures): `selem` interleaved registers, `rpt` repeats (LD1 with 2-4 registers).
pub unsafe fn ldm(c: &mut Cpu, q: u32, sz: u32, selem: u32, rpt: u32, t: u32, mut ea: u64) {
    let mut r = [0u128; 4];
    let ne = (8 << q) >> sz;
    for k in 0..rpt {
        for e in 0..ne {
            for s in 0..selem {
                lset(&mut r[(k + s) as usize], sz, e, ld(ea, sz));
                ea = ea.wrapping_add(1 << sz);
            }
        }
    }
    for i in 0..rpt + selem - 1 {
        c.v[((t + i) & 31) as usize] = r[i as usize];
    }
}
pub unsafe fn stm(c: &Cpu, q: u32, sz: u32, selem: u32, rpt: u32, t: u32, mut ea: u64) {
    let r: [u128; 4] = core::array::from_fn(|i| c.v[((t + i as u32) & 31) as usize]);
    let ne = (8 << q) >> sz;
    for k in 0..rpt {
        for e in 0..ne {
            for s in 0..selem {
                st(ea, sz, lget(r[(k + s) as usize], sz, e));
                ea = ea.wrapping_add(1 << sz);
            }
        }
    }
}
/// Single structure: lane `idx` of `selem` registers.
pub unsafe fn lds(c: &mut Cpu, sz: u32, selem: u32, idx: u32, t: u32, ea: u64) {
    for s in 0..selem {
        let r = ((t + s) & 31) as usize;
        let mut v = c.v[r];
        lset(&mut v, sz, idx, ld(ea.wrapping_add((s << sz) as u64), sz));
        c.v[r] = v;
    }
}
pub unsafe fn sts(c: &Cpu, sz: u32, selem: u32, idx: u32, t: u32, ea: u64) {
    for s in 0..selem {
        st(
            ea.wrapping_add((s << sz) as u64),
            sz,
            lget(c.v[((t + s) & 31) as usize], sz, idx),
        );
    }
}
/// LDnR: replicate into every lane.
pub unsafe fn ldr(c: &mut Cpu, q: u32, sz: u32, selem: u32, t: u32, ea: u64) {
    let mut x = [0u64; 4];
    for s in 0..selem {
        x[s as usize] = ld(ea.wrapping_add((s << sz) as u64), sz);
    }
    for s in 0..selem {
        let mut v = 0u128;
        for e in 0..((8 << q) >> sz) {
            lset(&mut v, sz, e, x[s as usize]);
        }
        c.v[((t + s) & 31) as usize] = v;
    }
}

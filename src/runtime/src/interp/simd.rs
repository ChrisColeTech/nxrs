//! FP, Advanced SIMD, crypto and their loads / stores, executed: the decode of the translator's
//! `src/translator/src/xlate/simd.rs` (same encodings accepted, same refused), with the semantics of the C it emits
//! (`simd.h`, `simd_fp.h`; here `interp::fp` and `interp::lanes`).

use crate::cpu::Cpu;
use crate::interp::exec::{cond, set_reg, unsup, wv as wr, x as xr, xs, R};
use crate::interp::fp::{self, fd, fs, *};
use crate::interp::lanes::{self as ln, lget, lmask, lset, *};
use crate::interp::mem::{ld_v, st_v};
use crate::interp::xutil::{bits, sext};

// ---------------------------------------------------------------- register views

fn v(c: &Cpu, n: u32) -> u128 {
    c.v[n as usize]
}
fn set_v(c: &mut Cpu, d: u32, val: u128) {
    c.v[d as usize] = val;
}
/// VW: the upper half cleared when Q = 0.
fn vw(c: &mut Cpu, q: u32, d: u32, val: u128) {
    c.v[d as usize] = if q != 0 { val } else { val as u64 as u128 };
}
fn lanes(q: u32, sz: u32) -> u32 {
    (8 << q) >> sz
}
fn vs32(c: &Cpu, n: u32) -> u64 {
    c.v[n as usize] as u32 as u64
}
fn vs64(c: &Cpu, n: u32) -> u64 {
    c.v[n as usize] as u64
}
fn vhi64(c: &Cpu, n: u32) -> u64 {
    (c.v[n as usize] >> 64) as u64
}
fn sfs(c: &Cpu, n: u32) -> f32 {
    f32::from_bits(c.v[n as usize] as u32)
}
fn sfd(c: &Cpu, n: u32) -> f64 {
    f64::from_bits(c.v[n as usize] as u64)
}
fn sign_ext(v: u64, sz: u32) -> i64 {
    ln::lsx(v, sz)
}
/// Lane-wise map of `ne` lanes of size `sz` (the rest zero).
fn map1(a: u128, sz: u32, ne: u32, f: impl Fn(u64) -> u64) -> u128 {
    let mut r = 0u128;
    for i in 0..ne {
        lset(&mut r, sz, i, f(lget(a, sz, i)));
    }
    r
}
fn map2(a: u128, b: u128, sz: u32, ne: u32, f: impl Fn(u64, u64) -> u64) -> u128 {
    let mut r = 0u128;
    for i in 0..ne {
        lset(&mut r, sz, i, f(lget(a, sz, i), lget(b, sz, i)));
    }
    r
}
fn mask(t: bool, sz: u32) -> u64 {
    if t {
        lmask(sz)
    } else {
        0
    }
}

/// One FP lane of type t (0 single, 1 double), as raw bits.
fn fop(c: &Cpu, t: u32, op: u32, a: u64, b: u64, d: u64) -> u64 {
    if t == 0 {
        fs::op(
            c,
            op,
            f32::from_bits(a as u32),
            f32::from_bits(b as u32),
            f32::from_bits(d as u32),
        )
        .to_bits() as u64
    } else {
        fd::op(
            c,
            op,
            f64::from_bits(a),
            f64::from_bits(b),
            f64::from_bits(d),
        )
        .to_bits()
    }
}
fn fneg(t: u32, a: u64) -> u64 {
    if t == 0 {
        a ^ 0x8000_0000
    } else {
        a ^ 0x8000_0000_0000_0000
    }
}
fn fcmp_lane(c: &Cpu, t: u32, op: u32, a: u64, b: u64) -> u64 {
    if t == 0 {
        fs::vcmp(c, op, f32::from_bits(a as u32), f32::from_bits(b as u32)) as u64
    } else {
        fd::vcmp(c, op, f64::from_bits(a), f64::from_bits(b))
    }
}
fn toint_lane(c: &Cpu, t: u32, a: u64, rm: u32, fbits: u32, ib: u32, uns: bool) -> u64 {
    if t == 0 {
        fs::toint(c, f32::from_bits(a as u32), rm, fbits, ib, uns)
    } else {
        fd::toint(c, f64::from_bits(a), rm, fbits, ib, uns)
    }
}
fn fromint_lane(c: &Cpu, t: u32, a: u64, ib: u32, uns: bool, fbits: u32) -> u64 {
    if t == 0 {
        fs::fromint(c, a, ib, uns, fbits).to_bits() as u64
    } else {
        fd::fromint(c, a, ib, uns, fbits).to_bits()
    }
}
/// FP_SETS / FP_SETD: the whole register.
fn fset(c: &mut Cpu, t: u32, d: u32, bits: u64) {
    c.v[d as usize] = if t == 0 {
        bits as u32 as u128
    } else {
        bits as u128
    };
}
/// The scalar of type t in register n (VSF32 / VSF64 as bits).
fn sget(c: &Cpu, t: u32, n: u32) -> u64 {
    if t == 0 {
        vs32(c, n)
    } else {
        vs64(c, n)
    }
}
/// VQ for FP vectors: lanes of the 64-bit form computed on a zero upper half.
fn vq(c: &Cpu, q: u32, n: u32) -> u128 {
    if q != 0 {
        c.v[n as usize]
    } else {
        c.v[n as usize] as u64 as u128
    }
}

fn vfp_expand_imm(imm8: u32, n: u32) -> u64 {
    let e: u32 = if n == 32 { 8 } else { 11 };
    let f = n - e - 1;
    let imm8 = imm8 as u64;
    let sign = imm8 >> 7;
    let b6 = (imm8 >> 6) & 1;
    let mid = if b6 != 0 { (1u64 << (e - 3)) - 1 } else { 0 };
    let exp = ((b6 ^ 1) << (e - 1)) | (mid << 2) | ((imm8 >> 4) & 3);
    let frac = (imm8 & 15) << (f - 4);
    (sign << (n - 1)) | (exp << f) | frac
}

fn simd_expand_imm(op: u32, cmode: u32, imm8: u32) -> u64 {
    let i = imm8 as u64;
    match cmode >> 1 {
        0 => (i << 32) | i,
        1 => ((i << 8) << 32) | (i << 8),
        2 => ((i << 16) << 32) | (i << 16),
        3 => ((i << 24) << 32) | (i << 24),
        4 => (0..64).step_by(16).map(|s| i << s).sum(),
        5 => (0..64).step_by(16).map(|s| (i << 8) << s).sum(),
        6 => {
            let v = if cmode & 1 == 0 {
                (i << 8) | 0xFF
            } else {
                (i << 16) | 0xFFFF
            };
            (v << 32) | v
        }
        _ => {
            if cmode & 1 == 0 && op == 0 {
                (0..64).step_by(8).map(|s| i << s).sum()
            } else if cmode & 1 == 0 {
                let mut imm = 0u64;
                for k in 0..8 {
                    if (i >> k) & 1 != 0 {
                        imm |= 0xFF << (8 * k);
                    }
                }
                imm
            } else if op == 0 {
                let v = vfp_expand_imm(imm8, 32);
                (v << 32) | v
            } else {
                vfp_expand_imm(imm8, 64)
            }
        }
    }
}

fn fop3(key: (u32, u32, u32)) -> Option<u32> {
    Some(match key {
        (0, 0, 0b11000) => FOP_MAXNM,
        (0, 0, 0b11010) => FOP_ADD,
        (0, 0, 0b11011) => FOP_MULX,
        (0, 0, 0b11110) => FOP_MAX,
        (0, 0, 0b11111) => FOP_RECPS,
        (0, 1, 0b11000) => FOP_MINNM,
        (0, 1, 0b11010) => FOP_SUB,
        (0, 1, 0b11110) => FOP_MIN,
        (0, 1, 0b11111) => FOP_RSQRTS,
        (1, 0, 0b11011) => FOP_MUL,
        (1, 0, 0b11111) => FOP_DIV,
        (1, 1, 0b11010) => FOP_ABD,
        _ => return None,
    })
}
fn fpairv(key: (u32, u32, u32)) -> Option<u32> {
    Some(match key {
        (1, 0, 0b11000) => FOP_MAXNM,
        (1, 1, 0b11000) => FOP_MINNM,
        (1, 0, 0b11010) => FOP_ADD,
        (1, 0, 0b11110) => FOP_MAX,
        (1, 1, 0b11110) => FOP_MIN,
        _ => return None,
    })
}
fn fcmp3(key: (u32, u32, u32)) -> Option<u32> {
    Some(match key {
        (0, 0, 0b11100) => 0,
        (1, 0, 0b11100) => 1,
        (1, 1, 0b11100) => 2,
        (1, 0, 0b11101) => 3,
        (1, 1, 0b11101) => 4,
        _ => return None,
    })
}

// ---------------------------------------------------------------- top level

/// AdvSIMD / FP / crypto data processing (simd.rs `dp`).
pub fn dp(c: &mut Cpu, w: u32) -> R<()> {
    let (op0, op1, op2, op3) = (
        bits(w, 28, 4),
        bits(w, 23, 2),
        bits(w, 19, 4),
        bits(w, 10, 9),
    );
    let o1b = op1 & 2 != 0;
    if op0 == 0b0100 && !o1b && (op2 & 7) == 5 && (op3 & 0b110000011) == 0b10 {
        return crypto_aes(c, w);
    }
    if op0 == 0b0101 && !o1b && op2 & 4 == 0 && (op3 & 0b000100011) == 0 {
        return crypto_sha3(c, w);
    }
    if op0 == 0b0101 && !o1b && (op2 & 7) == 5 && (op3 & 0b110000011) == 0b10 {
        return crypto_sha2(c, w);
    }
    if (op0 & 0b1101) == 0b0101 {
        if op1 == 0 && op2 & 0b1100 == 0 && (op3 & 0b000100001) == 1 {
            return scalar_copy(c, w);
        }
        if !o1b && (op2 & 7) == 4 && (op3 & 0b110000011) == 0b10 {
            return misc2(c, w, true);
        }
        if !o1b && (op2 & 7) == 6 && (op3 & 0b110000011) == 0b10 {
            return scalar_pairwise(c, w);
        }
        if !o1b && op2 & 4 != 0 && (op3 & 3) == 0 {
            return three_diff(c, w, true);
        }
        if !o1b && op2 & 4 != 0 && op3 & 1 != 0 {
            return three_same(c, w, true);
        }
        if op1 == 2 && op3 & 1 != 0 {
            return shift_imm(c, w, true);
        }
        if o1b && op3 & 1 == 0 {
            return indexed(c, w, true);
        }
        return unsup(format!("simd scalar {w:08x}"));
    }
    if (op0 & 0b1011) == 0 && !o1b && op2 & 4 == 0 && (op3 & 0b000100011) == 0 {
        return tbl(c, w);
    }
    if (op0 & 0b1011) == 0 && !o1b && op2 & 4 == 0 && (op3 & 0b000100011) == 2 {
        return permute(c, w);
    }
    if (op0 & 0b1011) == 0b0010 && !o1b && op2 & 4 == 0 && (op3 & 0b000100001) == 0 {
        return ext(c, w);
    }
    if (op0 & 0b1001) == 0 {
        if op1 == 0 && op2 & 0b1100 == 0 && (op3 & 0b000100001) == 1 {
            return copy(c, w);
        }
        if !o1b && op2 & 4 != 0 && op3 & 1 != 0 {
            return three_same(c, w, false);
        }
        if !o1b && op2 & 4 != 0 && (op3 & 3) == 0 {
            return three_diff(c, w, false);
        }
        if !o1b && (op2 & 7) == 4 && (op3 & 0b110000011) == 0b10 {
            return misc2(c, w, false);
        }
        if op1 == 1 && op2 == 0b1111 && (op3 & 0b110000011) == 0b10 {
            return fp16_misc2(c, w);
        }
        if !o1b && (op2 & 7) == 6 && (op3 & 0b110000011) == 0b10 {
            return across(c, w);
        }
        if op1 == 2 && op2 == 0 && op3 & 1 != 0 {
            return modimm(c, w);
        }
        if op1 == 2 && op2 != 0 && op3 & 1 != 0 {
            return shift_imm(c, w, false);
        }
        if o1b && op3 & 1 == 0 {
            return indexed(c, w, false);
        }
        return unsup(format!("simd vector {w:08x}"));
    }
    if (op0 & 0b0101) == 0b0001 {
        if bits(w, 29, 1) != 0
            || (bits(w, 31, 1) != 0 && (o1b || (op2 & 4 != 0 && op3 & 0b111111 != 0)))
        {
            return unsup("fp M/S");
        }
        if o1b {
            return fp_dp3(c, w);
        }
        if op2 & 4 == 0 {
            return fp_fixed(c, w);
        }
        if (op3 & 0b111111) == 0 {
            return fp_int(c, w);
        }
        if (op3 & 0b11111) == 0b10000 {
            return fp_dp1(c, w);
        }
        if (op3 & 0b1111) == 0b1000 {
            return fp_cmp(c, w);
        }
        if (op3 & 0b111) == 0b100 {
            return fp_imm(c, w);
        }
        return match op3 & 3 {
            1 => fp_ccmp(c, w),
            2 => fp_dp2(c, w),
            3 => fp_csel(c, w),
            _ => unsup("fp op3"),
        };
    }
    unsup(format!("fp/simd {w:08x}"))
}

fn fp16_misc2(c: &mut Cpu, w: u32) -> R<()> {
    let (q, u, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 12, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if opc != 0b01111 {
        return unsup("fp16 misc");
    }
    let x = vq(c, q, n);
    let m = 0x8000_8000_8000_8000_8000_8000_8000_8000u128;
    vw(c, q, d, if u != 0 { x ^ m } else { x & !m });
    Ok(())
}

// ---------------------------------------------------------------- crypto

fn crypto_aes(c: &mut Cpu, w: u32) -> R<()> {
    if bits(w, 22, 2) != 0 {
        return unsup("aes size");
    }
    let opc = bits(w, 12, 5);
    if !(4..=7).contains(&opc) {
        return unsup("aes opcode");
    }
    ln::aes(c, opc - 4, bits(w, 0, 5), bits(w, 5, 5));
    Ok(())
}

fn crypto_sha3(c: &mut Cpu, w: u32) -> R<()> {
    if bits(w, 22, 2) != 0 {
        return unsup("sha size");
    }
    let (opc, d, n, m) = (bits(w, 12, 3), bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5));
    match opc {
        0..=2 => ln::sha1hash(c, opc, d, n, m),
        3 => ln::sha_misc(c, 0, d, n, m),
        4 | 5 => ln::sha256hash(c, opc == 4, d, n, m),
        6 => ln::sha_misc(c, 3, d, n, m),
        _ => return unsup("sha3 opcode"),
    }
    Ok(())
}

fn crypto_sha2(c: &mut Cpu, w: u32) -> R<()> {
    if bits(w, 22, 2) != 0 {
        return unsup("sha size");
    }
    let (opc, d, n) = (bits(w, 12, 5), bits(w, 0, 5), bits(w, 5, 5));
    let op = match opc {
        0 => 4,
        1 => 1,
        2 => 2,
        _ => return unsup("sha2 opcode"),
    };
    ln::sha_misc(c, op, d, n, 0);
    Ok(())
}

// ---------------------------------------------------------------- copy / permute / table

fn imm5_dec(imm5: u32) -> R<(u32, u32)> {
    if imm5 & 15 == 0 {
        return unsup("copy imm5");
    }
    let sz = imm5.trailing_zeros();
    Ok((sz, imm5 >> (sz + 1)))
}

fn scalar_copy(c: &mut Cpu, w: u32) -> R<()> {
    if bits(w, 29, 1) != 0 || bits(w, 11, 4) != 0 {
        return unsup("scalar copy");
    }
    let (sz, idx) = imm5_dec(bits(w, 16, 5))?;
    if sz > 3 {
        return unsup("scalar dup size");
    }
    let (d, n) = (bits(w, 0, 5), bits(w, 5, 5));
    let e = lget(v(c, n), sz, idx);
    set_v(c, d, e as u128);
    Ok(())
}

fn splat(sz: u32, x: u64) -> u128 {
    let mut r = 0u128;
    for i in 0..(16 >> sz) {
        lset(&mut r, sz, i, x);
    }
    r
}

fn copy(c: &mut Cpu, w: u32) -> R<()> {
    let (q, op, imm5, imm4) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 16, 5),
        bits(w, 11, 4),
    );
    let (d, n) = (bits(w, 0, 5), bits(w, 5, 5));
    let (sz, idx) = imm5_dec(imm5)?;
    if sz > 3 {
        return unsup("copy size");
    }
    if op != 0 {
        if q == 0 {
            return unsup("ins elem Q");
        }
        let idx2 = imm4 >> sz;
        let e = lget(v(c, n), sz, idx2);
        let mut r = v(c, d);
        lset(&mut r, sz, idx, e);
        set_v(c, d, r);
        return Ok(());
    }
    match imm4 {
        0 => {
            if sz == 3 && q == 0 {
                return unsup("dup 1d");
            }
            let e = lget(v(c, n), sz, idx);
            vw(c, q, d, splat(sz, e));
        }
        1 => {
            if sz == 3 && q == 0 {
                return unsup("dup 1d");
            }
            let e = xr(c, n);
            vw(c, q, d, splat(sz, e));
        }
        3 => {
            if q == 0 {
                return unsup("ins Q");
            }
            let mut r = v(c, d);
            lset(&mut r, sz, idx, xr(c, n));
            set_v(c, d, r);
        }
        5 => {
            if sz >= 2 + q {
                return unsup("smov size");
            }
            let e = sign_ext(lget(v(c, n), sz, idx), sz) as u64;
            set_reg(c, q, d, e, false);
        }
        7 => {
            if (q != 0 && sz != 3) || (q == 0 && sz == 3) {
                return unsup("umov size");
            }
            let e = lget(v(c, n), sz, idx);
            set_reg(c, q, d, e, false);
        }
        _ => return unsup("copy imm4"),
    }
    Ok(())
}

/// SHUF(a, b, idx) over lanes of size sz (index < nl: a's lane, else b's lane index - nl), `count` result lanes.
fn shuf(a: u128, b: u128, sz: u32, idx: &[u32]) -> u128 {
    let nl = 16 >> sz;
    let mut r = 0u128;
    for (i, &k) in idx.iter().enumerate() {
        let e = if k < nl {
            lget(a, sz, k)
        } else {
            lget(b, sz, k - nl)
        };
        lset(&mut r, sz, i as u32, e);
    }
    r
}

fn permute(c: &mut Cpu, w: u32) -> R<()> {
    let (q, sz, m, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 22, 2),
        bits(w, 16, 5),
        bits(w, 12, 3),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if opc == 0 || opc == 4 || (sz == 3 && q == 0) {
        return unsup("permute");
    }
    let (ne, nl) = (lanes(q, sz), 16 >> sz);
    let part = opc >> 2;
    let kind = opc & 3;
    let mut idx = Vec::new();
    for i in 0..ne {
        if kind == 1 {
            let j = 2 * i + part;
            idx.push(if j < ne { j } else { nl + j - ne });
        } else if kind == 2 {
            let p = i & !1;
            idx.push(if i & 1 == 0 { p + part } else { nl + p + part });
        } else {
            let base = part * ne / 2;
            idx.push(if i & 1 == 0 {
                base + i / 2
            } else {
                nl + base + i / 2
            });
        }
    }
    let r = shuf(v(c, n), v(c, m), sz, &idx);
    vw(c, q, d, r);
    Ok(())
}

fn ext(c: &mut Cpu, w: u32) -> R<()> {
    let (q, m, imm4, n, d) = (
        bits(w, 30, 1),
        bits(w, 16, 5),
        bits(w, 11, 4),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if bits(w, 22, 2) != 0 || (q == 0 && imm4 >= 8) {
        return unsup("ext");
    }
    let idx: Vec<u32> = if q != 0 {
        (0..16).map(|i| imm4 + i).collect()
    } else {
        (0..8)
            .map(|i| {
                if imm4 + i < 8 {
                    imm4 + i
                } else {
                    16 + imm4 + i - 8
                }
            })
            .collect()
    };
    let r = shuf(v(c, n), v(c, m), 0, &idx);
    vw(c, q, d, r);
    Ok(())
}

fn tbl(c: &mut Cpu, w: u32) -> R<()> {
    let (q, m, len, op, n, d) = (
        bits(w, 30, 1),
        bits(w, 16, 5),
        bits(w, 13, 2),
        bits(w, 12, 1),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if bits(w, 22, 2) != 0 {
        return unsup("tbl op2");
    }
    ln::tbl(c, q, len, op != 0, d, n, m);
    Ok(())
}

// ---------------------------------------------------------------- three same

fn lopu(op: u32, u: u32) -> u32 {
    if u != 0 {
        op | LOP_U
    } else {
        op
    }
}

fn three_same(c: &mut Cpu, w: u32, scalar: bool) -> R<()> {
    let (q, u, sz, m, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 22, 2),
        bits(w, 16, 5),
        bits(w, 11, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if opc >= 0b11000 {
        return three_same_fp(c, w, scalar);
    }
    if scalar {
        return three_same_scalar(c, u, sz, opc, d, n, m);
    }
    if sz == 3 && q == 0 && opc != 3 {
        return unsup("3same 1d");
    }
    let ne = lanes(q, sz);
    let (a, b, o) = (v(c, n), v(c, m), v(c, d));
    let us = u != 0;
    // signed or unsigned view of a lane, as i128
    let val = |x: u64| -> i128 {
        if us {
            x as i128
        } else {
            sign_ext(x, sz) as i128
        }
    };
    if [0, 2, 4, 14, 15, 12, 13].contains(&opc) && sz == 3 {
        return unsup("3same size");
    }
    let lm = lmask(sz) as i128;
    let (lo, hi) = if us {
        (0i128, lm)
    } else {
        (-(lm + 1) / 2, lm / 2)
    };
    match opc {
        0 => ln::same(c, lopu(LOP_HADD, u), sz, ne, d, n, m as i32, 0, -1),
        1 => vw(
            c,
            q,
            d,
            map2(a, b, sz, ne, |x, y| (val(x) + val(y)).clamp(lo, hi) as u64),
        ),
        2 => ln::same(c, lopu(LOP_RHADD, u), sz, ne, d, n, m as i32, 0, -1),
        3 => {
            let (a64, b64, o64) = (a, b, o);
            let e = match (u, sz) {
                (0, 0) => a64 & b64,
                (0, 1) => a64 & !b64,
                (0, 2) => a64 | b64,
                (0, _) => a64 | !b64,
                (_, 0) => a64 ^ b64,
                (_, 1) => (o64 & a64) | (!o64 & b64),
                (_, 2) => (o64 & !b64) | (a64 & b64),
                (_, _) => (o64 & b64) | (a64 & !b64),
            };
            vw(c, q, d, e);
        }
        4 => ln::same(c, lopu(LOP_HSUB, u), sz, ne, d, n, m as i32, 0, -1),
        5 => vw(
            c,
            q,
            d,
            map2(a, b, sz, ne, |x, y| (val(x) - val(y)).clamp(lo, hi) as u64),
        ),
        6 => vw(
            c,
            q,
            d,
            map2(a, b, sz, ne, |x, y| mask(val(x) > val(y), sz)),
        ),
        7 => vw(
            c,
            q,
            d,
            map2(a, b, sz, ne, |x, y| mask(val(x) >= val(y), sz)),
        ),
        8..=11 => ln::same(
            c,
            lopu(
                [LOP_SHL, LOP_QSHL, LOP_RSHL, LOP_QRSHL][(opc - 8) as usize],
                u,
            ),
            sz,
            ne,
            d,
            n,
            m as i32,
            0,
            -1,
        ),
        12 => vw(
            c,
            q,
            d,
            map2(a, b, sz, ne, |x, y| val(x).max(val(y)) as u64),
        ),
        13 => vw(
            c,
            q,
            d,
            map2(a, b, sz, ne, |x, y| val(x).min(val(y)) as u64),
        ),
        14 | 15 => {
            let r = map2(a, b, sz, ne, |x, y| {
                (val(x).max(val(y)) as u64).wrapping_sub(val(x).min(val(y)) as u64)
            });
            let r = if opc == 15 {
                map2(o, r, sz, ne, |x, y| x.wrapping_add(y))
            } else {
                r
            };
            vw(c, q, d, r);
        }
        16 => vw(
            c,
            q,
            d,
            map2(a, b, sz, ne, |x, y| {
                if us {
                    x.wrapping_sub(y)
                } else {
                    x.wrapping_add(y)
                }
            }),
        ),
        17 => vw(
            c,
            q,
            d,
            map2(a, b, sz, ne, |x, y| {
                mask(if us { x == y } else { x & y != 0 }, sz)
            }),
        ),
        _ => {
            if sz == 3 && opc != 23 {
                return unsup("3same size");
            }
            match opc {
                18 => {
                    let p = map2(a, b, sz, ne, |x, y| x.wrapping_mul(y));
                    vw(
                        c,
                        q,
                        d,
                        map2(o, p, sz, ne, |x, y| {
                            if us {
                                x.wrapping_sub(y)
                            } else {
                                x.wrapping_add(y)
                            }
                        }),
                    );
                }
                19 => {
                    if us {
                        if sz != 0 {
                            return unsup("pmul size");
                        }
                        ln::same(c, LOP_PMUL, 0, ne, d, n, m as i32, 0, -1);
                    } else {
                        vw(c, q, d, map2(a, b, sz, ne, |x, y| x.wrapping_mul(y)));
                    }
                }
                20 | 21 | 23 => {
                    if opc == 23 && us {
                        return unsup("addp U");
                    }
                    let src: Vec<u64> = (0..ne)
                        .map(|i| lget(a, sz, i))
                        .chain((0..ne).map(|i| lget(b, sz, i)))
                        .collect();
                    let mut r = 0u128;
                    for k in 0..ne {
                        let (x, y) = (src[2 * k as usize], src[2 * k as usize + 1]);
                        let e = match opc {
                            23 => x.wrapping_add(y),
                            20 => val(x).max(val(y)) as u64,
                            _ => val(x).min(val(y)) as u64,
                        };
                        lset(&mut r, sz, k, e);
                    }
                    vw(c, q, d, r);
                }
                22 => {
                    if sz != 1 && sz != 2 {
                        return unsup("sqdmulh size");
                    }
                    ln::same(
                        c,
                        if us { LOP_SQRDMULH } else { LOP_SQDMULH },
                        sz,
                        ne,
                        d,
                        n,
                        m as i32,
                        0,
                        -1,
                    );
                }
                _ => return unsup("3same opcode"),
            }
        }
    }
    Ok(())
}

fn three_same_scalar(c: &mut Cpu, u: u32, sz: u32, opc: u32, d: u32, n: u32, m: u32) -> R<()> {
    let mi = m as i32;
    if opc == 1 || opc == 5 {
        let op = if opc == 1 {
            if u != 0 {
                LOP_UQADD
            } else {
                LOP_SQADD
            }
        } else if u != 0 {
            LOP_UQSUB
        } else {
            LOP_SQSUB
        };
        ln::same(c, op, sz, 1, d, n, mi, 0, -1);
        return Ok(());
    }
    if opc == 9 || opc == 11 {
        ln::same(
            c,
            lopu(if opc == 9 { LOP_QSHL } else { LOP_QRSHL }, u),
            sz,
            1,
            d,
            n,
            mi,
            0,
            -1,
        );
        return Ok(());
    }
    if opc == 22 {
        if sz != 1 && sz != 2 {
            return unsup("sqdmulh size");
        }
        ln::same(
            c,
            if u != 0 { LOP_SQRDMULH } else { LOP_SQDMULH },
            sz,
            1,
            d,
            n,
            mi,
            0,
            -1,
        );
        return Ok(());
    }
    if sz != 3 {
        return unsup("scalar 3same size");
    }
    let (a, b) = (vs64(c, n), vs64(c, m));
    let gt = if u != 0 {
        a > b
    } else {
        (a as i64) > (b as i64)
    };
    let ge = if u != 0 {
        a >= b
    } else {
        (a as i64) >= (b as i64)
    };
    let r = match opc {
        6 => mask(gt, 3),
        7 => mask(ge, 3),
        8 | 10 => {
            ln::same(
                c,
                lopu(if opc == 8 { LOP_SHL } else { LOP_RSHL }, u),
                3,
                1,
                d,
                n,
                mi,
                0,
                -1,
            );
            return Ok(());
        }
        16 => {
            if u != 0 {
                a.wrapping_sub(b)
            } else {
                a.wrapping_add(b)
            }
        }
        17 => mask(if u != 0 { a == b } else { a & b != 0 }, 3),
        _ => return unsup("scalar 3same opcode"),
    };
    set_v(c, d, r as u128);
    Ok(())
}

fn three_same_fp(c: &mut Cpu, w: u32, scalar: bool) -> R<()> {
    let (q, u, a_, sz, m, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 23, 1),
        bits(w, 22, 1),
        bits(w, 16, 5),
        bits(w, 11, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if !scalar && sz != 0 && q == 0 {
        return unsup("fp 2d Q");
    }
    let t = sz;
    let fsz = 2 + sz;
    let key = (u, a_, opc);
    if let Some(cop) = fcmp3(key) {
        if scalar {
            let r = fcmp_lane(c, t, cop, sget(c, t, n), sget(c, t, m));
            set_v(c, d, r as u128);
            return Ok(());
        }
        let ne = lanes(1, fsz);
        let r = map2(vq(c, q, n), vq(c, q, m), fsz, ne, |x, y| {
            fcmp_lane(c, t, cop, x, y)
        });
        vw(c, q, d, r);
        return Ok(());
    }
    if scalar {
        if !matches!(
            key,
            (0, 0, 0b11011) | (0, 0, 0b11111) | (0, 1, 0b11111) | (1, 1, 0b11010)
        ) {
            return unsup("scalar fp 3same");
        }
        let op = fop3(key).unwrap();
        let x = if op == FOP_RECPS || op == FOP_RSQRTS {
            fneg(t, sget(c, t, n))
        } else {
            sget(c, t, n)
        };
        let r = fop(c, t, op, x, sget(c, t, m), 0);
        fset(c, t, d, r);
        return Ok(());
    }
    let ne = lanes(1, fsz); // lanes of the 128-bit form; the 64-bit form's upper lanes are zero and cleared after
    let (a, b) = (vq(c, q, n), vq(c, q, m));
    if let Some(p) = fpairv(key) {
        let nq = lanes(q, fsz);
        let src: Vec<u64> = (0..nq)
            .map(|i| lget(a, fsz, i))
            .chain((0..nq).map(|i| lget(b, fsz, i)))
            .collect();
        let mut r = 0u128;
        for k in 0..nq {
            lset(
                &mut r,
                fsz,
                k,
                fop(c, t, p, src[2 * k as usize], src[2 * k as usize + 1], 0),
            );
        }
        vw(c, q, d, r);
        return Ok(());
    }
    if u == 0 && opc == 0b11001 {
        let acc = vq(c, q, d);
        let mm = if a_ != 0 {
            map1(a, fsz, ne, |x| fneg(t, x))
        } else {
            a
        };
        let mut r = 0u128;
        for i in 0..lanes(q, fsz) {
            lset(
                &mut r,
                fsz,
                i,
                fop(
                    c,
                    t,
                    FOP_MADD,
                    lget(acc, fsz, i),
                    lget(mm, fsz, i),
                    lget(b, fsz, i),
                ),
            );
        }
        vw(c, q, d, r);
        return Ok(());
    }
    let Some(op) = fop3(key) else {
        return unsup("fp 3same opcode");
    };
    let a = if op == FOP_RECPS || op == FOP_RSQRTS {
        map1(a, fsz, ne, |x| fneg(t, x))
    } else {
        a
    };
    let r = map2(a, b, fsz, lanes(q, fsz), |x, y| fop(c, t, op, x, y, 0));
    vw(c, q, d, r);
    Ok(())
}

// ---------------------------------------------------------------- three different

fn three_diff(c: &mut Cpu, w: u32, scalar: bool) -> R<()> {
    let (q, u, sz, m, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 22, 2),
        bits(w, 16, 5),
        bits(w, 12, 4),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    let mi = m as i32;
    if scalar {
        if u != 0 || ![9, 11, 13].contains(&opc) || (sz != 1 && sz != 2) {
            return unsup("scalar 3diff");
        }
        let op = match opc {
            9 => LOP_SQDMLAL,
            11 => LOP_SQDMLSL,
            _ => LOP_SQDMULL,
        };
        ln::gen(
            c,
            op,
            sz,
            1,
            d,
            sz + 1,
            0,
            false,
            n,
            sz,
            0,
            mi,
            sz,
            0,
            -1,
            0,
        );
        return Ok(());
    }
    if opc == 14 {
        if u != 0 {
            return unsup("pmull U");
        }
        if sz == 3 {
            ln::pmull64(c, q, d, n, m);
            return Ok(());
        }
        if sz != 0 {
            return unsup("pmull size");
        }
    }
    if sz == 3 || opc == 15 {
        return unsup("3diff size");
    }
    let ne = 8 >> sz;
    let off = q * ne;
    if opc == 4 || opc == 6 {
        let op =
            [LOP_ADDHN, LOP_RADDHN, LOP_SUBHN, LOP_RSUBHN][((opc == 6) as u32 * 2 + u) as usize];
        ln::gen(
            c,
            op,
            sz,
            ne,
            d,
            sz,
            off,
            q != 0,
            n,
            sz + 1,
            0,
            mi,
            sz + 1,
            0,
            -1,
            0,
        );
        return Ok(());
    }
    if opc == 1 || opc == 3 {
        let op = if opc == 1 { LOP_ADDW } else { LOP_SUBW };
        ln::gen(
            c,
            lopu(op, u),
            sz,
            ne,
            d,
            sz + 1,
            0,
            false,
            n,
            sz + 1,
            0,
            mi,
            sz,
            off,
            -1,
            0,
        );
        return Ok(());
    }
    let op = match opc {
        0 => LOP_ADDL,
        2 => LOP_SUBL,
        5 => LOP_ABAL,
        7 => LOP_ABDL,
        8 => LOP_MLAL,
        10 => LOP_MLSL,
        12 => LOP_MULL,
        9 => LOP_SQDMLAL,
        11 => LOP_SQDMLSL,
        13 => LOP_SQDMULL,
        _ => LOP_PMULL,
    };
    let sqd = matches!(op, LOP_SQDMLAL | LOP_SQDMLSL | LOP_SQDMULL);
    if sqd && (u != 0 || (sz != 1 && sz != 2)) {
        return unsup("sqdmull");
    }
    let uu = if sqd || op == LOP_PMULL { 0 } else { u };
    ln::gen(
        c,
        lopu(op, uu),
        sz,
        ne,
        d,
        sz + 1,
        0,
        false,
        n,
        sz,
        off,
        mi,
        sz,
        off,
        -1,
        0,
    );
    Ok(())
}

// ---------------------------------------------------------------- two-register misc

fn misc2(c: &mut Cpu, w: u32, scalar: bool) -> R<()> {
    let (q, u, sz, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 22, 2),
        bits(w, 12, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if opc >= 0b11000 || opc == 0b10110 || opc == 0b10111 || (0b01100..=0b01111).contains(&opc) {
        return misc2_fp(c, w, scalar);
    }
    if scalar {
        return misc2_scalar(c, u, sz, opc, d, n);
    }
    let ne = lanes(q, sz);
    if opc == 0 || (opc == 1 && u == 0) {
        let gsz = match (u, opc) {
            (0, 0) => Some(3),
            (1, 0) => Some(2),
            (0, 1) => Some(1),
            _ => None,
        };
        let gsz = match gsz {
            Some(g) if sz < g => g,
            _ => return unsup("rev"),
        };
        let es = 1u32 << sz;
        let gb = 1u32 << gsz;
        let idx: Vec<u32> = (0..16)
            .map(|i| {
                let (g, o) = (i / gb * gb, i % gb);
                let (e, b) = (o / es, o % es);
                g + (gb / es - 1 - e) * es + b
            })
            .collect();
        let a = v(c, n);
        let r = shuf(a, a, 0, &idx);
        vw(c, q, d, r);
        return Ok(());
    }
    if sz == 3 && q == 0 && ![0b10010, 0b10100, 0b10011].contains(&opc) {
        return unsup("misc 1d");
    }
    if opc == 2 || opc == 6 {
        if sz == 3 {
            return unsup("addlp size");
        }
        ln::addlp(c, u != 0, opc == 6, q, sz, d, n);
        return Ok(());
    }
    if opc == 3 {
        ln::same(
            c,
            if u != 0 { LOP_USQADD } else { LOP_SUQADD },
            sz,
            ne,
            d,
            n,
            -1,
            0,
            -1,
        );
        return Ok(());
    }
    if opc == 4 {
        if sz == 3 {
            return unsup("clz size");
        }
        ln::same(
            c,
            if u != 0 { LOP_CLZ } else { LOP_CLS },
            sz,
            ne,
            d,
            n,
            -1,
            0,
            -1,
        );
        return Ok(());
    }
    if opc == 5 {
        if u == 0 {
            if sz != 0 {
                return unsup("cnt size");
            }
            ln::same(c, LOP_CNT, 0, ne, d, n, -1, 0, -1);
            return Ok(());
        }
        if sz == 0 {
            let r = !v(c, n);
            vw(c, q, d, r);
            return Ok(());
        }
        if sz == 1 {
            ln::same(c, LOP_RBIT, 0, lanes(q, 0), d, n, -1, 0, -1);
            return Ok(());
        }
        return unsup("not/rbit size");
    }
    if opc == 7 {
        ln::same(
            c,
            if u != 0 { LOP_SQNEG } else { LOP_SQABS },
            sz,
            ne,
            d,
            n,
            -1,
            0,
            -1,
        );
        return Ok(());
    }
    let a = v(c, n);
    let sx = |x: u64| sign_ext(x, sz);
    let r = match (opc, u) {
        (8, 0) => map1(a, sz, ne, |x| mask(sx(x) > 0, sz)),
        (8, _) => map1(a, sz, ne, |x| mask(sx(x) >= 0, sz)),
        (9, 0) => map1(a, sz, ne, |x| mask(sx(x) == 0, sz)),
        (9, _) => map1(a, sz, ne, |x| mask(sx(x) <= 0, sz)),
        (10, 0) => map1(a, sz, ne, |x| mask(sx(x) < 0, sz)),
        (11, 0) => map1(a, sz, ne, |x| sx(x).wrapping_abs() as u64),
        (11, _) => map1(a, sz, ne, |x| x.wrapping_neg()),
        _ => {
            if opc == 0b10010 || opc == 0b10100 {
                if sz == 3 {
                    return unsup("xtn size");
                }
                let op = match (opc, u) {
                    (0b10010, 0) => LOP_XTN,
                    (0b10010, _) => LOP_SQXTUN,
                    (_, 0) => LOP_SQXTN,
                    _ => LOP_UQXTN,
                };
                let ne = 8 >> sz;
                ln::gen(
                    c,
                    op,
                    sz,
                    ne,
                    d,
                    sz,
                    q * ne,
                    q != 0,
                    n,
                    sz + 1,
                    0,
                    -1,
                    0,
                    0,
                    -1,
                    0,
                );
                return Ok(());
            }
            if opc == 0b10011 && u != 0 {
                if sz == 3 {
                    return unsup("shll size");
                }
                let ne = 8 >> sz;
                ln::gen(
                    c,
                    LOP_SHLL,
                    sz,
                    ne,
                    d,
                    sz + 1,
                    0,
                    false,
                    n,
                    sz,
                    q * ne,
                    -1,
                    0,
                    0,
                    -1,
                    0,
                );
                return Ok(());
            }
            return unsup("misc2 opcode");
        }
    };
    vw(c, q, d, r);
    Ok(())
}

fn misc2_scalar(c: &mut Cpu, u: u32, sz: u32, opc: u32, d: u32, n: u32) -> R<()> {
    if opc == 3 {
        ln::same(
            c,
            if u != 0 { LOP_USQADD } else { LOP_SUQADD },
            sz,
            1,
            d,
            n,
            -1,
            0,
            -1,
        );
        return Ok(());
    }
    if opc == 7 {
        ln::same(
            c,
            if u != 0 { LOP_SQNEG } else { LOP_SQABS },
            sz,
            1,
            d,
            n,
            -1,
            0,
            -1,
        );
        return Ok(());
    }
    if opc == 0b10010 || opc == 0b10100 {
        if sz == 3 || (opc == 0b10010 && u == 0) {
            return unsup("scalar xtn");
        }
        let op = match (opc, u) {
            (0b10010, _) => LOP_SQXTUN,
            (_, 0) => LOP_SQXTN,
            _ => LOP_UQXTN,
        };
        ln::gen(c, op, sz, 1, d, sz, 0, false, n, sz + 1, 0, -1, 0, 0, -1, 0);
        return Ok(());
    }
    if sz != 3 {
        return unsup("scalar misc size");
    }
    let a = vs64(c, n);
    let sa = a as i64;
    let r = match (opc, u) {
        (8, 0) => mask(sa > 0, 3),
        (8, _) => mask(sa >= 0, 3),
        (9, 0) => mask(sa == 0, 3),
        (9, _) => mask(sa <= 0, 3),
        (10, 0) => mask(sa < 0, 3),
        (11, 0) => {
            if sa < 0 {
                a.wrapping_neg()
            } else {
                a
            }
        }
        (11, _) => a.wrapping_neg(),
        _ => return unsup("scalar misc opcode"),
    };
    set_v(c, d, r as u128);
    Ok(())
}

fn misc2_fp(c: &mut Cpu, w: u32, scalar: bool) -> R<()> {
    let (q, u, a_, sz, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 23, 1),
        bits(w, 22, 1),
        bits(w, 12, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    let t = sz;
    let fsz = 2 + sz;
    if opc == 0b10110 || opc == 0b10111 {
        if a_ != 0 {
            return unsup("fcvtn a");
        }
        if opc == 0b10110 && u != 0 {
            if sz == 0 {
                return unsup("fcvtxn size");
            }
            if scalar {
                let r = fp::d2s_odd(c, sfd(c, n));
                set_v(c, d, r.to_bits() as u128);
                return Ok(());
            }
            fcvtn(c, 1, q, true, d, n);
            return Ok(());
        }
        if scalar || u != 0 {
            return unsup("fcvtn/l");
        }
        if opc == 0b10110 {
            fcvtn(c, sz, q, false, d, n);
        } else {
            fcvtl(c, sz, q, d, n);
        }
        return Ok(());
    }
    if !scalar && sz != 0 && q == 0 {
        return unsup("fp misc 2d Q");
    }
    let x = vq(c, q, n);
    let ne = lanes(q, fsz);
    if (0b01100..=0b01111).contains(&opc) {
        if a_ == 0 {
            return unsup("fp16 misc");
        }
        if opc == 0b01111 {
            if scalar {
                return unsup("scalar fabs");
            }
            let sign = if sz == 0 {
                0x8000_0000u64
            } else {
                0x8000_0000_0000_0000
            };
            let r = map1(v(c, n), fsz, lanes(1, fsz), |e| {
                if u != 0 {
                    e ^ sign
                } else {
                    e & !sign
                }
            });
            vw(c, q, d, r);
            return Ok(());
        }
        let (op, swap) = match (opc, u) {
            (0b01100, 0) => (2, false),
            (0b01100, 1) => (1, false),
            (0b01101, 0) => (0, false),
            (0b01101, 1) => (1, true),
            (0b01110, 0) => (2, true),
            _ => return unsup("fcm zero"),
        };
        let lane = |c: &Cpu, e: u64| {
            if swap {
                fcmp_lane(c, t, op, 0, e)
            } else {
                fcmp_lane(c, t, op, e, 0)
            }
        };
        if scalar {
            let r = lane(c, sget(c, t, n));
            set_v(c, d, r as u128);
            return Ok(());
        }
        let r = map1(x, fsz, ne, |e| lane(c, e));
        vw(c, q, d, r);
        return Ok(());
    }
    if opc == 0b11111 {
        if a_ == 0 {
            return unsup("fp misc 11111");
        }
        if u != 0 {
            if scalar {
                return unsup("scalar fsqrt");
            }
            let r = map1(x, fsz, ne, |e| fop(c, t, FOP_SQRT, e, e, 0));
            vw(c, q, d, r);
            return Ok(());
        }
        if !scalar {
            return unsup("vector frecpx");
        }
        let r = if t == 0 {
            fs::recpx(c, sfs(c, n)).to_bits() as u64
        } else {
            fd::recpx(c, sfd(c, n)).to_bits()
        };
        fset(c, t, d, r);
        return Ok(());
    }
    if opc == 0b11000 || opc == 0b11001 {
        if scalar {
            return unsup("scalar frint");
        }
        let rm = match (u, a_, opc & 1) {
            (0, 0, 0) => 0,
            (0, 0, 1) => 2,
            (0, 1, 0) => 1,
            (0, 1, 1) => 3,
            (1, 0, 0) => 4,
            (1, 0, 1) => 5,
            (1, 1, 1) => 6,
            _ => return unsup("frint"),
        };
        let r = map1(x, fsz, ne, |e| {
            if t == 0 {
                fs::frint(c, f32::from_bits(e as u32), rm).to_bits() as u64
            } else {
                fd::frint(c, f64::from_bits(e), rm).to_bits()
            }
        });
        vw(c, q, d, r);
        return Ok(());
    }
    if opc == 0b11010 || opc == 0b11011 || (opc == 0b11100 && a_ == 0) {
        let rm = match (opc, a_) {
            (0b11010, 0) => 0,
            (0b11010, _) => 1,
            (0b11011, 0) => 2,
            (0b11011, _) => 3,
            _ => 4,
        };
        let ib = 32 << sz;
        if scalar {
            let r = toint_lane(c, t, sget(c, t, n), rm, 0, ib, u != 0) & lmask(fsz);
            set_v(c, d, r as u128);
            return Ok(());
        }
        let r = map1(x, fsz, ne, |e| toint_lane(c, t, e, rm, 0, ib, u != 0));
        vw(c, q, d, r);
        return Ok(());
    }
    if opc == 0b11100 {
        if scalar || sz != 0 {
            return unsup("urecpe");
        }
        ln::same(
            c,
            if u != 0 { LOP_URSQRTE } else { LOP_URECPE },
            2,
            lanes(q, 2),
            d,
            n,
            -1,
            0,
            -1,
        );
        return Ok(());
    }
    if opc == 0b11101 {
        if a_ == 0 {
            let ib = 32 << sz;
            if scalar {
                let r = fromint_lane(c, t, sget(c, t, n), ib, u != 0, 0);
                fset(c, t, d, r);
                return Ok(());
            }
            let r = map1(v(c, n), fsz, ne, |e| fromint_lane(c, t, e, ib, u != 0, 0));
            vw(c, q, d, r);
            return Ok(());
        }
        let est = |c: &Cpu, e: u64| -> u64 {
            match (u != 0, t) {
                (false, 0) => fs::recpe(c, f32::from_bits(e as u32)).to_bits() as u64,
                (false, _) => fd::recpe(c, f64::from_bits(e)).to_bits(),
                (true, 0) => fs::rsqrte(c, f32::from_bits(e as u32)).to_bits() as u64,
                (true, _) => fd::rsqrte(c, f64::from_bits(e)).to_bits(),
            }
        };
        if scalar {
            let r = est(c, sget(c, t, n));
            fset(c, t, d, r);
            return Ok(());
        }
        let r = map1(x, fsz, ne, |e| est(c, e));
        vw(c, q, d, r);
        return Ok(());
    }
    unsup("fp misc opcode")
}

/// FCVTL / FCVTL2 (h -> s, s -> d).
fn fcvtl(c: &mut Cpu, sz: u32, part: u32, d: u32, n: u32) {
    let a = v(c, n);
    let mut r = 0u128;
    if sz == 0 {
        for i in 0..4 {
            lset(
                &mut r,
                2,
                i,
                fp::h2s(c, lget(a, 1, 4 * part + i) as u16).to_bits() as u64,
            );
        }
    } else {
        for i in 0..2 {
            lset(
                &mut r,
                3,
                i,
                fp::s2d(c, f32::from_bits(lget(a, 2, 2 * part + i) as u32)).to_bits(),
            );
        }
    }
    set_v(c, d, r);
}
/// FCVTN / FCVTN2 (s -> h, d -> s), FCVTXN / FCVTXN2.
fn fcvtn(c: &mut Cpu, sz: u32, part: u32, odd: bool, d: u32, n: u32) {
    let a = v(c, n);
    let mut r = if part != 0 { v(c, d) as u64 as u128 } else { 0 };
    if sz == 0 {
        for i in 0..4 {
            lset(
                &mut r,
                1,
                4 * part + i,
                fp::s2h(c, f32::from_bits(lget(a, 2, i) as u32)) as u64,
            );
        }
    } else {
        for i in 0..2 {
            let x = f64::from_bits(lget(a, 3, i));
            let s = if odd {
                fp::d2s_odd(c, x)
            } else {
                fp::d2s(c, x)
            };
            lset(&mut r, 2, 2 * part + i, s.to_bits() as u64);
        }
    }
    set_v(c, d, r);
}

// ---------------------------------------------------------------- across lanes / pairwise

fn across(c: &mut Cpu, w: u32) -> R<()> {
    let (q, u, sz, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 22, 2),
        bits(w, 12, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if opc == 0b01100 || opc == 0b01111 {
        if u == 0 {
            return unsup("fp16 across");
        }
        if q == 0 || sz & 1 != 0 {
            return unsup("fp across size");
        }
        let op = match (opc, sz >> 1) {
            (0b01100, 0) => FOP_MAXNM,
            (0b01100, _) => FOP_MINNM,
            (_, 0) => FOP_MAX,
            _ => FOP_MIN,
        };
        let a = v(c, n);
        let l = |i| lget(a, 2, i);
        let lo = fop(c, 0, op, l(0), l(1), 0);
        let hi = fop(c, 0, op, l(2), l(3), 0);
        let r = fop(c, 0, op, lo, hi, 0);
        set_v(c, d, r as u32 as u128);
        return Ok(());
    }
    let op = match opc {
        0b00011 => Some(1),
        0b01010 => Some(2),
        0b11010 => Some(3),
        0b11011 => Some(0),
        _ => None,
    };
    match op {
        Some(op) if !((op == 0 && u != 0) || sz == 3 || (sz == 2 && q == 0)) => {
            ln::across(c, op, u != 0, q, sz, d, n);
            Ok(())
        }
        _ => unsup("across"),
    }
}

fn scalar_pairwise(c: &mut Cpu, w: u32) -> R<()> {
    let (u, sz, opc, n, d) = (
        bits(w, 29, 1),
        bits(w, 22, 2),
        bits(w, 12, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if u == 0 {
        if opc == 0b11011 && sz == 3 {
            let r = vs64(c, n).wrapping_add(vhi64(c, n));
            set_v(c, d, r as u128);
            return Ok(());
        }
        return unsup("scalar pairwise");
    }
    let op = match (opc, sz >> 1) {
        (0b01100, 0) => FOP_MAXNM,
        (0b01100, 1) => FOP_MINNM,
        (0b01101, 0) => FOP_ADD,
        (0b01111, 0) => FOP_MAX,
        (0b01111, 1) => FOP_MIN,
        _ => return unsup("scalar fp pairwise"),
    };
    let t = sz & 1;
    let fsz = 2 + t;
    let a = v(c, n);
    let r = fop(c, t, op, lget(a, fsz, 0), lget(a, fsz, 1), 0);
    fset(c, t, d, r);
    Ok(())
}

// ---------------------------------------------------------------- modified immediate

fn modimm(c: &mut Cpu, w: u32) -> R<()> {
    let (q, op, cmode, o2, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 12, 4),
        bits(w, 11, 1),
        bits(w, 0, 5),
    );
    let imm8 = (bits(w, 16, 3) << 5) | bits(w, 5, 5);
    if o2 != 0 {
        return unsup("fmov half imm");
    }
    if cmode == 15 && op != 0 && q == 0 {
        return unsup("fmov 1d");
    }
    let mut imm = simd_expand_imm(op, cmode, imm8);
    let imm2 = (imm as u128) << 64 | imm as u128;
    if (cmode < 8 && cmode & 1 != 0) || ((8..12).contains(&cmode) && cmode & 1 != 0) {
        let r = v(c, d);
        let r = if op != 0 { r & !imm2 } else { r | imm2 };
        vw(c, q, d, r);
        return Ok(());
    }
    if op != 0 && !(cmode == 14 || cmode == 15) {
        imm = !imm;
    }
    let val: u128 = if q != 0 {
        imm as u128 | ((imm as u128) << 64)
    } else {
        imm as u128
    };
    set_v(c, d, val);
    Ok(())
}

// ---------------------------------------------------------------- shift by immediate

fn shift_imm(c: &mut Cpu, w: u32, scalar: bool) -> R<()> {
    let (mut q, u, immh, immb, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 19, 4),
        bits(w, 16, 3),
        bits(w, 11, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if immh == 0 {
        return unsup("shift immh");
    }
    let sz = 31 - immh.leading_zeros();
    let es = 8u32 << sz;
    let ih = (immh << 3) | immb;
    let (rsh, lsh) = (2 * es - ih, ih - es);
    if scalar {
        q = 1;
    }
    let ne = if scalar { 1 } else { lanes(q, sz) };
    let narrow = (0b10000..=0b10011).contains(&opc);
    if opc == 0b11100 || opc == 0b11111 {
        if sz < 2 {
            return unsup("fp16 fixed");
        }
        if sz == 3 && q == 0 {
            return unsup("fixed 1d");
        }
        let t = sz - 2;
        let fb = rsh;
        if opc == 0b11100 {
            if scalar {
                let r = fromint_lane(c, t, sget(c, t, n), es, u != 0, fb);
                fset(c, t, d, r);
                return Ok(());
            }
            let r = map1(v(c, n), sz, lanes(1, sz), |e| {
                fromint_lane(c, t, e, es, u != 0, fb)
            });
            vw(c, q, d, r);
            return Ok(());
        }
        if scalar {
            let r = toint_lane(c, t, sget(c, t, n), 3, fb, es, u != 0) & lmask(sz);
            set_v(c, d, r as u128);
            return Ok(());
        }
        let r = map1(vq(c, q, n), sz, lanes(1, sz), |e| {
            toint_lane(c, t, e, 3, fb, es, u != 0)
        });
        vw(c, q, d, r);
        return Ok(());
    }
    if narrow {
        if sz == 3 {
            return unsup("narrow shift size");
        }
        let ne = if scalar { 1 } else { 8 >> sz };
        let (op, ou) = match (opc, u) {
            (0b10000, 0) => (LOP_SHRN, 0),
            (0b10001, 0) => (LOP_RSHRN, 0),
            (0b10000, _) => (LOP_QSHRUN, 0),
            (0b10001, _) => (LOP_QRSHRUN, 0),
            (0b10010, 0) => (LOP_QSHRN, 0),
            (0b10010, _) => (LOP_QSHRN, 1),
            (_, 0) => (LOP_QRSHRN, 0),
            _ => (LOP_QRSHRN, 1),
        };
        if scalar && (op == LOP_SHRN || op == LOP_RSHRN) {
            return unsup("scalar shrn");
        }
        let part = if scalar { 0 } else { q };
        ln::gen(
            c,
            lopu(op, ou),
            sz,
            ne,
            d,
            sz,
            part * ne,
            part != 0,
            n,
            sz + 1,
            0,
            -1,
            0,
            0,
            -1,
            rsh as u64,
        );
        return Ok(());
    }
    if opc == 0b10100 {
        if scalar || sz == 3 {
            return unsup("sshll");
        }
        let ne = 8 >> sz;
        ln::gen(
            c,
            lopu(LOP_SHLLI, u),
            sz,
            ne,
            d,
            sz + 1,
            0,
            false,
            n,
            sz,
            q * ne,
            -1,
            0,
            0,
            -1,
            lsh as u64,
        );
        return Ok(());
    }
    if sz == 3 && q == 0 {
        return unsup("shift 1d");
    }
    if scalar && sz != 3 && opc != 0b01100 && opc != 0b01110 {
        return unsup("scalar shift size");
    }
    // `fin`: the vector form (Q) or lane 0 alone (scalar)
    let fin = |c: &mut Cpu, r: u128| {
        if scalar {
            set_v(c, d, r as u64 as u128)
        } else {
            vw(c, q, d, r)
        }
    };
    let nl = lanes(1, sz);
    match opc {
        0 | 2 => {
            let a = v(c, n);
            let mut r = if u != 0 {
                if rsh >= es {
                    0
                } else {
                    map1(a, sz, nl, |e| e >> rsh)
                }
            } else {
                map1(a, sz, nl, |e| (sign_ext(e, sz) >> rsh.min(es - 1)) as u64)
            };
            if opc == 2 {
                r = map2(v(c, d), r, sz, nl, |x, y| x.wrapping_add(y));
            }
            fin(c, r);
        }
        4 | 6 => ln::same(
            c,
            lopu(if opc == 4 { LOP_RSHRI } else { LOP_RSRAI }, u),
            sz,
            ne,
            d,
            n,
            -1,
            rsh as u64,
            -1,
        ),
        8 => {
            if u == 0 {
                return unsup("sri U");
            }
            ln::same(c, LOP_SRI, sz, ne, d, n, -1, rsh as u64, -1);
        }
        10 => {
            if u != 0 {
                ln::same(c, LOP_SLI, sz, ne, d, n, -1, lsh as u64, -1);
            } else {
                let r = map1(v(c, n), sz, nl, |e| e << lsh);
                fin(c, r);
            }
        }
        12 => {
            if u == 0 {
                return unsup("sqshlu U");
            }
            ln::same(c, LOP_QSHLUI, sz, ne, d, n, -1, lsh as u64, -1);
        }
        14 => ln::same(c, lopu(LOP_QSHLI, u), sz, ne, d, n, -1, lsh as u64, -1),
        _ => return unsup("shift imm opcode"),
    }
    Ok(())
}

// ---------------------------------------------------------------- indexed element

fn indexed(c: &mut Cpu, w: u32, scalar: bool) -> R<()> {
    let (q, u, sz, l, mb, rm, opc, h, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 22, 2),
        bits(w, 21, 1),
        bits(w, 20, 1),
        bits(w, 16, 4),
        bits(w, 12, 4),
        bits(w, 11, 1),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    let fpi = [1, 5, 9].contains(&opc) && !(u != 0 && opc != 9);
    if fpi {
        if sz < 2 {
            return unsup("fp16 indexed");
        }
        let t = sz & 1;
        let idx = if t != 0 {
            if l != 0 {
                return unsup("indexed L");
            }
            h
        } else {
            (h << 1) | l
        };
        let m = (mb << 4) | rm;
        if !scalar && t != 0 && q == 0 {
            return unsup("indexed 2d Q");
        }
        let fsz = 2 + t;
        let mul = if u != 0 { FOP_MULX } else { FOP_MUL };
        let el = lget(v(c, m), fsz, idx);
        if scalar {
            let r = if opc == 9 {
                fop(c, t, mul, sget(c, t, n), el, 0)
            } else {
                let x = if opc == 1 {
                    sget(c, t, n)
                } else {
                    fneg(t, sget(c, t, n))
                };
                fop(c, t, FOP_MADD, sget(c, t, d), x, el)
            };
            fset(c, t, d, r);
            return Ok(());
        }
        let ne = lanes(q, fsz);
        let (nv, acc) = (vq(c, q, n), vq(c, q, d));
        let mut r = 0u128;
        for i in 0..ne {
            let a = lget(nv, fsz, i);
            let e = if opc == 9 {
                fop(c, t, mul, a, el, 0)
            } else {
                let x = if opc == 1 { a } else { fneg(t, a) };
                fop(c, t, FOP_MADD, lget(acc, fsz, i), x, el)
            };
            lset(&mut r, fsz, i, e);
        }
        vw(c, q, d, r);
        return Ok(());
    }
    let (idx, m) = match sz {
        1 => ((h << 2) | (l << 1) | mb, rm),
        2 => ((h << 1) | l, (mb << 4) | rm),
        _ => return unsup("indexed size"),
    };
    let (mi, ii) = (m as i32, idx as i32);
    if scalar {
        let op = match (u, opc) {
            (0, 3) => LOP_SQDMLAL,
            (0, 7) => LOP_SQDMLSL,
            (0, 11) => LOP_SQDMULL,
            (0, 12) => LOP_SQDMULH,
            (0, 13) => LOP_SQRDMULH,
            _ => return unsup("scalar indexed"),
        };
        let szd = if op == LOP_SQDMULH || op == LOP_SQRDMULH {
            sz
        } else {
            sz + 1
        };
        ln::gen(c, op, sz, 1, d, szd, 0, false, n, sz, 0, mi, sz, 0, ii, 0);
        return Ok(());
    }
    let ne = lanes(q, sz);
    let el = lget(v(c, m), sz, idx);
    if (u, opc) == (0, 8) {
        let r = map1(v(c, n), sz, ne, |x| x.wrapping_mul(el));
        vw(c, q, d, r);
        return Ok(());
    }
    if (u, opc) == (1, 0) || (u, opc) == (1, 4) {
        let p = map1(v(c, n), sz, ne, |x| x.wrapping_mul(el));
        let r = map2(v(c, d), p, sz, ne, |o, p| {
            if opc == 0 {
                o.wrapping_add(p)
            } else {
                o.wrapping_sub(p)
            }
        });
        vw(c, q, d, r);
        return Ok(());
    }
    if (u, opc) == (0, 12) || (u, opc) == (0, 13) {
        ln::same(
            c,
            if opc == 12 { LOP_SQDMULH } else { LOP_SQRDMULH },
            sz,
            ne,
            d,
            n,
            mi,
            0,
            ii,
        );
        return Ok(());
    }
    let lng = match opc {
        2 => LOP_MLAL,
        6 => LOP_MLSL,
        10 => LOP_MULL,
        3 => LOP_SQDMLAL,
        7 => LOP_SQDMLSL,
        11 => LOP_SQDMULL,
        _ => return unsup("indexed opcode"),
    };
    let sq = matches!(lng, LOP_SQDMLAL | LOP_SQDMLSL | LOP_SQDMULL);
    if u != 0 && sq {
        return unsup("indexed opcode");
    }
    let ne = 8 >> sz;
    ln::gen(
        c,
        lopu(lng, if sq { 0 } else { u }),
        sz,
        ne,
        d,
        sz + 1,
        0,
        false,
        n,
        sz,
        q * ne,
        mi,
        sz,
        0,
        ii,
        0,
    );
    Ok(())
}

// ---------------------------------------------------------------- scalar FP

fn ftype(w: u32) -> R<u32> {
    let t = bits(w, 22, 2);
    if t > 1 {
        return unsup("fp type");
    }
    Ok(t)
}

fn fp_dp3(c: &mut Cpu, w: u32) -> R<()> {
    let t = ftype(w)?;
    let (o1, o0) = (bits(w, 21, 1), bits(w, 15, 1));
    let (d, n, m, a) = (bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5), bits(w, 10, 5));
    let (na, nn) = (o1 != 0, (o1 ^ o0) != 0);
    let mut x = sget(c, t, a);
    let mut y = sget(c, t, n);
    if na {
        x = fneg(t, x);
    }
    if nn {
        y = fneg(t, y);
    }
    let r = fop(c, t, FOP_MADD, x, y, sget(c, t, m));
    fset(c, t, d, r);
    Ok(())
}

fn fp_dp2(c: &mut Cpu, w: u32) -> R<()> {
    let t = ftype(w)?;
    let (opc, d, n, m) = (bits(w, 12, 4), bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5));
    const OPS: [u32; 9] = [
        FOP_MUL, FOP_DIV, FOP_ADD, FOP_SUB, FOP_MAX, FOP_MIN, FOP_MAXNM, FOP_MINNM, FOP_NMUL,
    ];
    if opc > 8 {
        return unsup("fp dp2");
    }
    let r = fop(c, t, OPS[opc as usize], sget(c, t, n), sget(c, t, m), 0);
    fset(c, t, d, r);
    Ok(())
}

fn fp_dp1(c: &mut Cpu, w: u32) -> R<()> {
    let t = bits(w, 22, 2);
    let (opc, d, n) = (bits(w, 15, 6), bits(w, 0, 5), bits(w, 5, 5));
    if t == 3 {
        let h = c.v[n as usize] as u16;
        match opc {
            4 => {
                let r = fp::h2s(c, h).to_bits() as u64;
                fset(c, 0, d, r)
            }
            5 => {
                let r = fp::h2d(c, h).to_bits();
                fset(c, 1, d, r)
            }
            _ => return unsup("fp16 dp1"),
        }
        return Ok(());
    }
    if t == 2 {
        return unsup("fp type");
    }
    let sign = if t == 0 {
        0x8000_0000u64
    } else {
        0x8000_0000_0000_0000
    };
    match opc {
        0 => {
            let r = sget(c, t, n);
            set_v(c, d, r as u128)
        }
        1 => {
            let r = sget(c, t, n) & !sign;
            set_v(c, d, r as u128)
        }
        2 => {
            let r = sget(c, t, n) ^ sign;
            set_v(c, d, r as u128)
        }
        3 => {
            let x = sget(c, t, n);
            let r = fop(c, t, FOP_SQRT, x, x, 0);
            fset(c, t, d, r);
        }
        4 | 5 | 7 => {
            let to = match opc {
                4 => 0,
                5 => 1,
                _ => 3,
            };
            if to == t {
                return unsup("fcvt same");
            }
            let r: u128 = if to == 3 {
                (if t == 0 {
                    fp::s2h(c, sfs(c, n))
                } else {
                    fp::dd2h(c, sfd(c, n))
                }) as u128
            } else if to != 0 {
                fp::s2d(c, sfs(c, n)).to_bits() as u128
            } else {
                fp::d2s(c, sfd(c, n)).to_bits() as u128
            };
            set_v(c, d, r);
        }
        _ => {
            let rm = match opc {
                8 => 0,
                9 => 1,
                10 => 2,
                11 => 3,
                12 => 4,
                14 => 5,
                15 => 6,
                _ => return unsup("fp dp1 opcode"),
            };
            let r = if t == 0 {
                fs::frint(c, sfs(c, n), rm).to_bits() as u64
            } else {
                fd::frint(c, sfd(c, n), rm).to_bits()
            };
            fset(c, t, d, r);
        }
    }
    Ok(())
}

fn fcmp(c: &Cpu, t: u32, a: u64, b: u64) -> u64 {
    if t == 0 {
        fs::cmp(c, f32::from_bits(a as u32), f32::from_bits(b as u32))
    } else {
        fd::cmp(c, f64::from_bits(a), f64::from_bits(b))
    }
}

fn fp_cmp(c: &mut Cpu, w: u32) -> R<()> {
    let t = ftype(w)?;
    if bits(w, 14, 2) != 0 || bits(w, 0, 3) != 0 {
        return unsup("fcmp op");
    }
    let (n, m, zero) = (bits(w, 5, 5), bits(w, 16, 5), bits(w, 3, 1));
    let b = if zero != 0 { 0 } else { sget(c, t, m) };
    c.nzcv = fcmp(c, t, sget(c, t, n), b);
    Ok(())
}

fn fp_ccmp(c: &mut Cpu, w: u32) -> R<()> {
    let t = ftype(w)?;
    let (n, m, cnd, nzcv) = (bits(w, 5, 5), bits(w, 16, 5), bits(w, 12, 4), bits(w, 0, 4));
    c.nzcv = if cond(c, cnd) {
        fcmp(c, t, sget(c, t, n), sget(c, t, m))
    } else {
        (nzcv as u64) << 28
    };
    Ok(())
}

fn fp_csel(c: &mut Cpu, w: u32) -> R<()> {
    let t = ftype(w)?;
    let (d, n, m, cnd) = (bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5), bits(w, 12, 4));
    let r = if cond(c, cnd) {
        sget(c, t, n)
    } else {
        sget(c, t, m)
    };
    set_v(c, d, r as u128);
    Ok(())
}

fn fp_imm(c: &mut Cpu, w: u32) -> R<()> {
    let t = ftype(w)?;
    if bits(w, 5, 5) != 0 {
        return unsup("fmov imm5");
    }
    let (imm8, d) = (bits(w, 13, 8), bits(w, 0, 5));
    set_v(c, d, vfp_expand_imm(imm8, 32 << t) as u128);
    Ok(())
}

fn fp_fixed(c: &mut Cpu, w: u32) -> R<()> {
    let (sf, t, rmode, opc, scale, n, d) = (
        bits(w, 31, 1),
        bits(w, 22, 2),
        bits(w, 19, 2),
        bits(w, 16, 3),
        bits(w, 10, 6),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if t > 1 {
        return unsup("fixed type");
    }
    if sf == 0 && scale < 32 {
        return unsup("fixed scale");
    }
    let (fb, ib) = (64 - scale, if sf != 0 { 64 } else { 32 });
    if rmode == 0 && (opc == 2 || opc == 3) {
        let src = if sf != 0 { xr(c, n) } else { wr(c, n) };
        let r = fromint_lane(c, t, src, ib, opc & 1 != 0, fb);
        fset(c, t, d, r);
        return Ok(());
    }
    if rmode == 3 && (opc == 0 || opc == 1) {
        let r = toint_lane(c, t, sget(c, t, n), 3, fb, ib, opc != 0);
        set_reg(c, sf, d, r, false);
        return Ok(());
    }
    unsup("fixed opcode")
}

fn fp_int(c: &mut Cpu, w: u32) -> R<()> {
    let (sf, t, rmode, opc, n, d) = (
        bits(w, 31, 1),
        bits(w, 22, 2),
        bits(w, 19, 2),
        bits(w, 16, 3),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if opc == 6 || opc == 7 {
        if t == 2 && sf != 0 && rmode == 1 {
            if opc == 6 {
                let r = vhi64(c, n);
                set_reg(c, 1, d, r, false);
            } else {
                let lo = c.v[d as usize] as u64 as u128;
                c.v[d as usize] = lo | (xr(c, n) as u128) << 64;
            }
            return Ok(());
        }
        if rmode != 0 || t != sf || t > 1 {
            return unsup("fmov general");
        }
        if opc == 6 {
            let r = if sf != 0 { vs64(c, n) } else { vs32(c, n) };
            set_reg(c, sf, d, r, false);
        } else {
            let r = if sf != 0 { xr(c, n) } else { wr(c, n) };
            set_v(c, d, r as u128);
        }
        return Ok(());
    }
    if t > 1 {
        return unsup("fp int type");
    }
    let ib = if sf != 0 { 64 } else { 32 };
    if opc == 2 || opc == 3 {
        if rmode != 0 {
            return unsup("cvtf rmode");
        }
        let src = if sf != 0 { xr(c, n) } else { wr(c, n) };
        let r = fromint_lane(c, t, src, ib, opc & 1 != 0, 0);
        fset(c, t, d, r);
        return Ok(());
    }
    let rm = if opc == 4 || opc == 5 {
        if rmode != 0 {
            return unsup("fcvta rmode");
        }
        4
    } else {
        rmode
    };
    let r = toint_lane(c, t, sget(c, t, n), rm, 0, ib, opc & 1 != 0);
    set_reg(c, sf, d, r, false);
    Ok(())
}

// ---------------------------------------------------------------- loads / stores

/// FP / SIMD loads and stores (simd.rs `ldst`).
pub unsafe fn ldst(c: &mut Cpu, w: u32, pc: u64) -> R<()> {
    let rt = bits(w, 0, 5);
    if bits(w, 27, 3) == 0b011 && bits(w, 24, 2) == 0 {
        let opc = bits(w, 30, 2);
        if opc == 3 {
            return unsup("ldr literal opc");
        }
        let a = pc.wrapping_add((sext(bits(w, 5, 19) as u64, 19) * 4) as u64);
        let r = ld_v(a, [2, 3, 4][opc as usize]);
        set_v(c, rt, r);
        return Ok(());
    }
    if bits(w, 31, 1) == 0 && (bits(w, 24, 6) == 0b001100 || bits(w, 24, 6) == 0b001101) {
        return ldst_struct(c, w);
    }
    if bits(w, 27, 3) == 0b101 {
        return ldst_pair(c, w);
    }
    if bits(w, 27, 3) == 0b111 {
        return ldst_reg(c, w);
    }
    unsup(format!("ldst simd {w:08x}"))
}

fn set_base(c: &mut Cpu, rn: u32, v: u64) {
    if rn == 31 {
        c.sp = v
    } else {
        c.x[rn as usize] = v
    }
}

unsafe fn ldst_pair(c: &mut Cpu, w: u32) -> R<()> {
    let (opc, mode, l) = (bits(w, 30, 2), bits(w, 23, 2), bits(w, 22, 1));
    let (imm7, rt2, rn, rt) = (
        sext(bits(w, 15, 7) as u64, 7),
        bits(w, 10, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if opc == 3 {
        return unsup("pair opc 3");
    }
    let scale = 2 + opc;
    let off = imm7 * (1i64 << scale);
    let base = xs(c, rn);
    let (post, wb) = (mode == 1, mode == 1 || mode == 3);
    let ea = if post {
        base
    } else {
        base.wrapping_add(off as u64)
    };
    let sz = 1u64 << scale;
    if l != 0 {
        let (a, b) = (ld_v(ea, scale), ld_v(ea.wrapping_add(sz), scale));
        set_v(c, rt, a);
        set_v(c, rt2, b);
    } else {
        let m = if scale == 4 {
            !0u128
        } else {
            (1u128 << (8 << scale)) - 1
        };
        st_v(ea, scale, v(c, rt) & m);
        st_v(ea.wrapping_add(sz), scale, v(c, rt2) & m);
    }
    if wb {
        set_base(
            c,
            rn,
            if post {
                ea.wrapping_add(off as u64)
            } else {
                ea
            },
        );
    }
    Ok(())
}

unsafe fn ldst_reg(c: &mut Cpu, w: u32) -> R<()> {
    let (size, opc, rn, rt) = (bits(w, 30, 2), bits(w, 22, 2), bits(w, 5, 5), bits(w, 0, 5));
    let scale = if opc & 2 != 0 {
        if size != 0 {
            return unsup("ldst simd size");
        }
        4
    } else {
        size
    };
    let l = opc & 1;
    let base = xs(c, rn);
    let mut wb: Option<u64> = None;
    let ea;
    match bits(w, 24, 2) {
        1 => ea = base.wrapping_add((bits(w, 10, 12) as u64) << scale),
        0 => {
            let kind = bits(w, 10, 2);
            if bits(w, 21, 1) != 0 {
                if kind != 2 {
                    return unsup("ldst simd reg kind");
                }
                let (rm, option, s) = (bits(w, 16, 5), bits(w, 13, 3), bits(w, 12, 1));
                if ![2, 3, 6, 7].contains(&option) {
                    return unsup("ldst simd reg option");
                }
                ea = base.wrapping_add(crate::interp::exec::extend(
                    c,
                    rm,
                    option,
                    if s != 0 { scale } else { 0 },
                ));
            } else {
                let imm9 = sext(bits(w, 12, 9) as u64, 9) as u64;
                match kind {
                    0 => ea = base.wrapping_add(imm9),
                    1 => {
                        ea = base;
                        wb = Some(base.wrapping_add(imm9));
                    }
                    3 => {
                        ea = base.wrapping_add(imm9);
                        wb = Some(ea);
                    }
                    _ => return unsup("ldst simd unprivileged"),
                }
            }
        }
        _ => return unsup("ldst simd"),
    }
    if l != 0 {
        let r = ld_v(ea, scale);
        set_v(c, rt, r);
    } else {
        let m = if scale == 4 {
            !0u128
        } else {
            (1u128 << (8 << scale)) - 1
        };
        st_v(ea, scale, v(c, rt) & m);
    }
    if let Some(wbv) = wb {
        set_base(c, rn, wbv);
    }
    Ok(())
}

unsafe fn ldst_struct(c: &mut Cpu, w: u32) -> R<()> {
    let (q, l, post, rm, rn, rt, sz) = (
        bits(w, 30, 1),
        bits(w, 22, 1),
        bits(w, 23, 1),
        bits(w, 16, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
        bits(w, 10, 2),
    );
    if post == 0 && rm != 0 {
        return unsup("ldst struct rm");
    }
    let ea = xs(c, rn);
    let total;
    if bits(w, 24, 1) == 0 {
        if bits(w, 21, 1) != 0 {
            return unsup("ldst multiple bit21");
        }
        let opc = bits(w, 12, 4);
        let (selem, rpt) = match opc {
            0 => (4, 1),
            2 => (1, 4),
            4 => (3, 1),
            6 => (1, 3),
            7 => (1, 1),
            8 => (2, 1),
            10 => (1, 2),
            _ => return unsup("ldst multiple opcode"),
        };
        if sz == 3 && q == 0 && selem > 1 {
            return unsup("ldst multiple 1d");
        }
        total = (8u32 << q) * selem * rpt;
        if selem == 1 {
            let step = if q != 0 { 16 } else { 8 };
            for i in 0..rpt {
                let r = (rt + i) % 32;
                let a = ea.wrapping_add((i * step) as u64);
                if l != 0 {
                    let val = ld_v(a, if q != 0 { 4 } else { 3 });
                    set_v(c, r, val);
                } else if q != 0 {
                    st_v(a, 4, v(c, r));
                } else {
                    st_v(a, 3, v(c, r) as u64 as u128);
                }
            }
        } else if l != 0 {
            ln::ldm(c, q, sz, selem, rpt, rt, ea);
        } else {
            ln::stm(c, q, sz, selem, rpt, rt, ea);
        }
    } else {
        let (opc, s, r) = (bits(w, 13, 3), bits(w, 12, 1), bits(w, 21, 1));
        let mut scale = opc >> 1;
        let selem = (((opc & 1) << 1) | r) + 1;
        if scale == 3 {
            if l == 0 || s != 0 {
                return unsup("ldnr");
            }
            total = selem << sz;
            ln::ldr(c, q, sz, selem, rt, ea);
        } else {
            let idx;
            if scale == 0 {
                idx = (q << 3) | (s << 2) | sz;
            } else if scale == 1 {
                if sz & 1 != 0 {
                    return unsup("ld1 h size");
                }
                idx = (q << 2) | (s << 1) | (sz >> 1);
            } else {
                if sz & 2 != 0 {
                    return unsup("ld1 s size");
                }
                if sz & 1 != 0 {
                    if s != 0 {
                        return unsup("ld1 d S");
                    }
                    idx = q;
                    scale = 3;
                } else {
                    idx = (q << 1) | s;
                }
            }
            total = selem << scale;
            if l != 0 {
                ln::lds(c, scale, selem, idx, rt, ea);
            } else {
                ln::sts(c, scale, selem, idx, rt, ea);
            }
        }
    }
    if post != 0 {
        let nv = if rm == 31 {
            ea.wrapping_add(total as u64)
        } else {
            ea.wrapping_add(xr(c, rm))
        };
        set_base(c, rn, nv);
    }
    Ok(())
}

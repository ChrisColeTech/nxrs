//!
//! Portable C lowering of AArch64 FP / AdvSIMD / crypto / CRC32 instructions (lifter --simd=c).
//! Every encoding is decoded at lift time and turned into C over the Cpu struct, calling the static inline
//! helpers in runtime/simd.h with constant fields. Covers ARMv8.0-A + Crypto + CRC32; unallocated or
//! later-architecture encodings return `Unsupported`. Output must stay byte-identical to the Python.

use crate::xlate::util::{
    bits, extend_expr, hx, set_reg, sext, unsup, w as wr, x as xr, xs, LResult, COND_NAMES,
};

// ---------------------------------------------------------------- module helpers

fn lop(name: &str, u: u32) -> String {
    if u != 0 {
        format!("LOP_{name} | LOP_U")
    } else {
        format!("LOP_{name}")
    }
}

fn vu(sz: u32) -> &'static str {
    ["vu8", "vu16", "vu32", "vu64"][sz as usize]
}

fn vs(sz: u32) -> &'static str {
    ["vs8", "vs16", "vs32", "vs64"][sz as usize]
}

fn ut(sz: u32) -> &'static str {
    ["uint8_t", "uint16_t", "uint32_t", "uint64_t"][sz as usize]
}

fn vr(t: &str, n: u32) -> String {
    format!("VREG({t}, {n})")
}

fn vq(t: &str, q: u32, n: u32) -> String {
    if q != 0 {
        format!("VREG({t}, {n})")
    } else {
        format!("VQ({t}, 0, {n})")
    }
}

fn vwr(t: &str, q: u32, d: u32, e: &str) -> String {
    format!("VW({t}, {q}, {d}, {e});")
}

fn cond(c: u32) -> String {
    if c < 14 {
        format!("COND_{}(c)", COND_NAMES[c as usize].to_uppercase())
    } else {
        "1".into()
    }
}

#[allow(clippy::too_many_arguments)]
fn gen(
    op: &str,
    sz: u32,
    ne: u32,
    d: u32,
    szd: u32,
    offd: u32,
    keep: u32,
    n: u32,
    szn: u32,
    offn: u32,
    m: i64,
    szm: u32,
    offm: u32,
    bidx: i64,
    bimm: u64,
) -> String {
    format!(
        "sl_gen(c, {op}, {sz}, {ne}, {d}, {szd}, {offd}, {keep}, {n}, {szn}, {offn}, {m}, {szm}, {offm}, {bidx}, {});",
        hx(bimm)
    )
}

/// Lane-wise op, all operands the same size.
#[allow(clippy::too_many_arguments)]
fn same(op: &str, sz: u32, ne: u32, d: u32, n: u32, m: i64, bimm: u64, bidx: i64) -> String {
    gen(op, sz, ne, d, sz, 0, 0, n, sz, 0, m, sz, 0, bidx, bimm)
}

fn same2(op: &str, sz: u32, ne: u32, d: u32, n: u32) -> String {
    same(op, sz, ne, d, n, -1, 0, -1)
}

fn same3(op: &str, sz: u32, ne: u32, d: u32, n: u32, m: u32) -> String {
    same(op, sz, ne, d, n, m as i64, 0, -1)
}

fn lanes(q: u32, sz: u32) -> u32 {
    (8 << q) >> sz
}

fn shuf(a: &str, b: &str, idx: &[u32]) -> String {
    let s: Vec<String> = idx.iter().map(|i| i.to_string()).collect();
    format!("SHUF({a}, {b}, {})", s.join(", "))
}

fn u128s(v: u128) -> String {
    let lo = v as u64;
    let hi = (v >> 64) as u64;
    if hi != 0 {
        format!("((u128){} << 64 | {})", hx(hi), hx(lo))
    } else {
        hx(lo)
    }
}

#[allow(non_snake_case)]
#[derive(Clone, Copy)]
struct Ft {
    F: &'static str,
    VT: &'static str,
    VU: &'static str,
    SF: &'static str,
    SU: &'static str,
    SET: &'static str,
}

fn ft(t: u32) -> Ft {
    if t == 0 {
        Ft {
            F: "s",
            VT: "vf32",
            VU: "vu32",
            SF: "VSF32",
            SU: "VS32",
            SET: "FP_SETS",
        }
    } else {
        Ft {
            F: "d",
            VT: "vf64",
            VU: "vu64",
            SF: "VSF64",
            SU: "VS64",
            SET: "FP_SETD",
        }
    }
}

fn fop3(key: (u32, u32, u32)) -> Option<&'static str> {
    Some(match key {
        (0, 0, 0b11000) => "FOP_MAXNM",
        (0, 0, 0b11010) => "FOP_ADD",
        (0, 0, 0b11011) => "FOP_MULX",
        (0, 0, 0b11110) => "FOP_MAX",
        (0, 0, 0b11111) => "FOP_RECPS",
        (0, 1, 0b11000) => "FOP_MINNM",
        (0, 1, 0b11010) => "FOP_SUB",
        (0, 1, 0b11110) => "FOP_MIN",
        (0, 1, 0b11111) => "FOP_RSQRTS",
        (1, 0, 0b11011) => "FOP_MUL",
        (1, 0, 0b11111) => "FOP_DIV",
        (1, 1, 0b11010) => "FOP_ABD",
        _ => return None,
    })
}

fn fpairv(key: (u32, u32, u32)) -> Option<&'static str> {
    Some(match key {
        (1, 0, 0b11000) => "FOP_MAXNM",
        (1, 1, 0b11000) => "FOP_MINNM",
        (1, 0, 0b11010) => "FOP_ADD",
        (1, 0, 0b11110) => "FOP_MAX",
        (1, 1, 0b11110) => "FOP_MIN",
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

fn pm(u: u32) -> &'static str {
    if u != 0 {
        "-"
    } else {
        "+"
    }
}

// ---------------------------------------------------------------- top level

fn fp16_misc2(w: u32) -> LResult<String> {
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
    let x = vq("vu16", q, n);
    Ok(vwr(
        "vu16",
        q,
        d,
        &if u != 0 {
            format!("{x} ^ (uint16_t)0x8000")
        } else {
            format!("{x} & (uint16_t)0x7fff")
        },
    ))
}

/// SimdC.dp: AdvSIMD / FP / crypto data processing.
pub fn dp(w: u32, _pc: u64) -> LResult<String> {
    let (op0, op1, op2, op3) = (
        bits(w, 28, 4),
        bits(w, 23, 2),
        bits(w, 19, 4),
        bits(w, 10, 9),
    );
    let o1b = op1 & 2 != 0;
    if op0 == 0b0100 && !o1b && (op2 & 7) == 5 && (op3 & 0b110000011) == 0b10 {
        return crypto_aes(w);
    }
    if op0 == 0b0101 && !o1b && op2 & 4 == 0 && (op3 & 0b000100011) == 0 {
        return crypto_sha3(w);
    }
    if op0 == 0b0101 && !o1b && (op2 & 7) == 5 && (op3 & 0b110000011) == 0b10 {
        return crypto_sha2(w);
    }
    if (op0 & 0b1101) == 0b0101 {
        if op1 == 0 && op2 & 0b1100 == 0 && (op3 & 0b000100001) == 1 {
            return scalar_copy(w);
        }
        if !o1b && (op2 & 7) == 4 && (op3 & 0b110000011) == 0b10 {
            return misc2(w, true);
        }
        if !o1b && (op2 & 7) == 6 && (op3 & 0b110000011) == 0b10 {
            return scalar_pairwise(w);
        }
        if !o1b && op2 & 4 != 0 && (op3 & 3) == 0 {
            return three_diff(w, true);
        }
        if !o1b && op2 & 4 != 0 && op3 & 1 != 0 {
            return three_same(w, true);
        }
        if op1 == 2 && op3 & 1 != 0 {
            return shift_imm(w, true);
        }
        if o1b && op3 & 1 == 0 {
            return indexed(w, true);
        }
        return unsup(format!("simd scalar {w:08x}"));
    }
    if (op0 & 0b1011) == 0 && !o1b && op2 & 4 == 0 && (op3 & 0b000100011) == 0 {
        return tbl(w);
    }
    if (op0 & 0b1011) == 0 && !o1b && op2 & 4 == 0 && (op3 & 0b000100011) == 2 {
        return permute(w);
    }
    if (op0 & 0b1011) == 0b0010 && !o1b && op2 & 4 == 0 && (op3 & 0b000100001) == 0 {
        return ext(w);
    }
    if (op0 & 0b1001) == 0 {
        if op1 == 0 && op2 & 0b1100 == 0 && (op3 & 0b000100001) == 1 {
            return copy(w);
        }
        if !o1b && op2 & 4 != 0 && op3 & 1 != 0 {
            return three_same(w, false);
        }
        if !o1b && op2 & 4 != 0 && (op3 & 3) == 0 {
            return three_diff(w, false);
        }
        if !o1b && (op2 & 7) == 4 && (op3 & 0b110000011) == 0b10 {
            return misc2(w, false);
        }
        if op1 == 1 && op2 == 0b1111 && (op3 & 0b110000011) == 0b10 {
            return fp16_misc2(w);
        }
        if !o1b && (op2 & 7) == 6 && (op3 & 0b110000011) == 0b10 {
            return across(w);
        }
        if op1 == 2 && op2 == 0 && op3 & 1 != 0 {
            return modimm(w);
        }
        if op1 == 2 && op2 != 0 && op3 & 1 != 0 {
            return shift_imm(w, false);
        }
        if o1b && op3 & 1 == 0 {
            return indexed(w, false);
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
            return fp_dp3(w);
        }
        if op2 & 4 == 0 {
            return fp_fixed(w);
        }
        if (op3 & 0b111111) == 0 {
            return fp_int(w);
        }
        if (op3 & 0b11111) == 0b10000 {
            return fp_dp1(w);
        }
        if (op3 & 0b1111) == 0b1000 {
            return fp_cmp(w);
        }
        if (op3 & 0b111) == 0b100 {
            return fp_imm(w);
        }
        return match op3 & 3 {
            1 => fp_ccmp(w),
            2 => fp_dp2(w),
            3 => fp_csel(w),
            _ => unsup("fp op3"),
        };
    }
    unsup(format!("fp/simd {w:08x}"))
}

// ---------------------------------------------------------------- crypto

fn crypto_aes(w: u32) -> LResult<String> {
    if bits(w, 22, 2) != 0 {
        return unsup("aes size");
    }
    let opc = bits(w, 12, 5);
    if !(4..=7).contains(&opc) {
        return unsup("aes opcode");
    }
    Ok(format!(
        "sv_aes(c, {}, {}, {});",
        opc - 4,
        bits(w, 0, 5),
        bits(w, 5, 5)
    ))
}

fn crypto_sha3(w: u32) -> LResult<String> {
    if bits(w, 22, 2) != 0 {
        return unsup("sha size");
    }
    let (opc, d, n, m) = (bits(w, 12, 3), bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5));
    match opc {
        0..=2 => Ok(format!("sv_sha1hash(c, {opc}, {d}, {n}, {m});")),
        3 => Ok(format!("sv_sha_misc(c, 0, {d}, {n}, {m});")),
        4 | 5 => Ok(format!(
            "sv_sha256hash(c, {}, {d}, {n}, {m});",
            (opc == 4) as u32
        )),
        6 => Ok(format!("sv_sha_misc(c, 3, {d}, {n}, {m});")),
        _ => unsup("sha3 opcode"),
    }
}

fn crypto_sha2(w: u32) -> LResult<String> {
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
    Ok(format!("sv_sha_misc(c, {op}, {d}, {n}, 0);"))
}

// ---------------------------------------------------------------- copy / permute / table

fn imm5_dec(imm5: u32) -> LResult<(u32, u32)> {
    if imm5 & 15 == 0 {
        return unsup("copy imm5");
    }
    let sz = imm5.trailing_zeros();
    Ok((sz, imm5 >> (sz + 1)))
}

const PTR: [&str; 4] = ["uint8_t", "sa16", "sa32", "sa64"];

fn scalar_copy(w: u32) -> LResult<String> {
    if bits(w, 29, 1) != 0 || bits(w, 11, 4) != 0 {
        return unsup("scalar copy");
    }
    let (sz, idx) = imm5_dec(bits(w, 16, 5))?;
    if sz > 3 {
        return unsup("scalar dup size");
    }
    let (d, n) = (bits(w, 0, 5), bits(w, 5, 5));
    Ok(format!(
        "c->v[{d}] = (({} *)&c->v[{n}])[{idx}];",
        PTR[sz as usize]
    ))
}

fn copy(w: u32) -> LResult<String> {
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
    let p = PTR[sz as usize];
    if op != 0 {
        if q == 0 {
            return unsup("ins elem Q");
        }
        let idx2 = imm4 >> sz;
        return Ok(format!(
            "(({p} *)&c->v[{d}])[{idx}] = (({p} *)&c->v[{n}])[{idx2}];"
        ));
    }
    let t = vu(sz);
    match imm4 {
        0 => {
            if sz == 3 && q == 0 {
                return unsup("dup 1d");
            }
            let nl = 16 >> sz;
            Ok(vwr(
                t,
                q,
                d,
                &shuf(&vr(t, n), &vr(t, n), &vec![idx; nl as usize]),
            ))
        }
        1 => {
            if sz == 3 && q == 0 {
                return unsup("dup 1d");
            }
            Ok(vwr(t, q, d, &format!("({t}){{}} + ({}){}", ut(sz), xr(n))))
        }
        3 => {
            if q == 0 {
                return unsup("ins Q");
            }
            Ok(format!(
                "(({p} *)&c->v[{d}])[{idx}] = ({}){};",
                ut(sz),
                xr(n)
            ))
        }
        5 => {
            if sz >= 2 + q {
                return unsup("smov size");
            }
            let st = ["int8_t", "int16_t", "int32_t"][sz as usize];
            Ok(set_reg(
                q,
                d,
                &format!("(uint64_t)(int64_t)({st})(({p} *)&c->v[{n}])[{idx}]"),
                false,
            ))
        }
        7 => {
            if (q != 0 && sz != 3) || (q == 0 && sz == 3) {
                return unsup("umov size");
            }
            Ok(set_reg(q, d, &format!("(({p} *)&c->v[{n}])[{idx}]"), false))
        }
        _ => unsup("copy imm4"),
    }
}

fn permute(w: u32) -> LResult<String> {
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
    idx.extend(std::iter::repeat_n(0, (nl - ne) as usize));
    let t = vu(sz);
    Ok(vwr(t, q, d, &shuf(&vr(t, n), &vr(t, m), &idx)))
}

fn ext(w: u32) -> LResult<String> {
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
        let mut v: Vec<u32> = (0..8)
            .map(|i| {
                if imm4 + i < 8 {
                    imm4 + i
                } else {
                    16 + imm4 + i - 8
                }
            })
            .collect();
        v.extend([0; 8]);
        v
    };
    Ok(vwr("vu8", q, d, &shuf(&vr("vu8", n), &vr("vu8", m), &idx)))
}

fn tbl(w: u32) -> LResult<String> {
    let (q, m, ln, op, n, d) = (
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
    Ok(format!("sv_tbl(c, {q}, {ln}, {op}, {d}, {n}, {m});"))
}

// ---------------------------------------------------------------- three same

fn three_same(w: u32, scalar: bool) -> LResult<String> {
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
        return three_same_fp(w, scalar);
    }
    if scalar {
        return three_same_scalar(u, sz, opc, d, n, m);
    }
    if sz == 3 && q == 0 && opc != 3 {
        return unsup("3same 1d");
    }
    let ne = lanes(q, sz);
    let (t, s) = (vu(sz), vs(sz));
    let (a, b, o) = (vr(t, n), vr(t, m), vr(t, d));
    let (sa, sb) = (vr(s, n), vr(s, m));
    let (x, y) = if u != 0 { (&a, &b) } else { (&sa, &sb) };
    if [0, 2, 4, 14, 15, 12, 13].contains(&opc) && sz == 3 {
        return unsup("3same size");
    }
    match opc {
        0 => return Ok(same3(&lop("HADD", u), sz, ne, d, n, m)),
        1 => {
            return Ok(vwr(
                t,
                q,
                d,
                &format!("({t})__builtin_elementwise_add_sat({x}, {y})"),
            ))
        }
        2 => return Ok(same3(&lop("RHADD", u), sz, ne, d, n, m)),
        3 => {
            let (a64, b64, o64) = (vr("vu64", n), vr("vu64", m), vr("vu64", d));
            let e = match (u, sz) {
                (0, 0) => format!("{a64} & {b64}"),
                (0, 1) => format!("{a64} & ~{b64}"),
                (0, 2) => format!("{a64} | {b64}"),
                (0, _) => format!("{a64} | ~{b64}"),
                (_, 0) => format!("{a64} ^ {b64}"),
                (_, 1) => format!("({o64} & {a64}) | (~{o64} & {b64})"),
                (_, 2) => format!("({o64} & ~{b64}) | ({a64} & {b64})"),
                (_, _) => format!("({o64} & {b64}) | ({a64} & ~{b64})"),
            };
            return Ok(vwr("vu64", q, d, &e));
        }
        4 => return Ok(same3(&lop("HSUB", u), sz, ne, d, n, m)),
        5 => {
            return Ok(vwr(
                t,
                q,
                d,
                &format!("({t})__builtin_elementwise_sub_sat({x}, {y})"),
            ))
        }
        6 => return Ok(vwr(t, q, d, &format!("({t})({x} > {y})"))),
        7 => return Ok(vwr(t, q, d, &format!("({t})({x} >= {y})"))),
        8..=11 => {
            return Ok(same3(
                &lop(["SHL", "QSHL", "RSHL", "QRSHL"][(opc - 8) as usize], u),
                sz,
                ne,
                d,
                n,
                m,
            ));
        }
        12 => {
            return Ok(vwr(
                t,
                q,
                d,
                &format!("({t})__builtin_elementwise_max({x}, {y})"),
            ))
        }
        13 => {
            return Ok(vwr(
                t,
                q,
                d,
                &format!("({t})__builtin_elementwise_min({x}, {y})"),
            ))
        }
        14 | 15 => {
            let e = format!("({t})__builtin_elementwise_max({x}, {y}) - ({t})__builtin_elementwise_min({x}, {y})");
            return Ok(vwr(
                t,
                q,
                d,
                &if opc == 15 { format!("{o} + ({e})") } else { e },
            ));
        }
        16 => return Ok(vwr(t, q, d, &format!("{a} {} {b}", pm(u)))),
        17 => {
            return Ok(vwr(
                t,
                q,
                d,
                &if u != 0 {
                    format!("({t})({a} == {b})")
                } else {
                    format!("({t})(({a} & {b}) != 0)")
                },
            ));
        }
        _ => {}
    }
    if sz == 3 && opc != 23 {
        return unsup("3same size");
    }
    match opc {
        18 => Ok(vwr(t, q, d, &format!("{o} {} {a} * {b}", pm(u)))),
        19 => {
            if u != 0 {
                if sz != 0 {
                    return unsup("pmul size");
                }
                return Ok(same3(&lop("PMUL", 0), 0, ne, d, n, m));
            }
            Ok(vwr(t, q, d, &format!("{a} * {b}")))
        }
        20 | 21 | 23 => {
            if opc == 23 && u != 0 {
                return unsup("addp U");
            }
            let (ev, od) = pair_idx(q, sz);
            let (aa, bb) = if u != 0 || opc == 23 {
                (&a, &b)
            } else {
                (&sa, &sb)
            };
            let (l, r) = (shuf(aa, bb, &ev), shuf(aa, bb, &od));
            if opc == 23 {
                return Ok(vwr(t, q, d, &format!("({t})({l} + {r})")));
            }
            Ok(vwr(
                t,
                q,
                d,
                &format!(
                    "({t})__builtin_elementwise_{}({l}, {r})",
                    if opc == 20 { "max" } else { "min" }
                ),
            ))
        }
        22 => {
            if sz != 1 && sz != 2 {
                return unsup("sqdmulh size");
            }
            Ok(same3(
                &lop(if u != 0 { "SQRDMULH" } else { "SQDMULH" }, 0),
                sz,
                ne,
                d,
                n,
                m,
            ))
        }
        _ => unsup("3same opcode"),
    }
}

/// Lane indices (into SHUF(a, b)) of the left / right operands of a pairwise op; for Q=0 the padding lanes
/// point at a's upper half.
fn pair_idx(q: u32, sz: u32) -> (Vec<u32>, Vec<u32>) {
    let (ne, nl) = (lanes(q, sz), 16 >> sz);
    let src: Vec<u32> = (0..ne).chain((0..ne).map(|i| nl + i)).collect();
    let pad = ne % nl;
    let mut ev: Vec<u32> = (0..ne).map(|k| src[(2 * k) as usize]).collect();
    let mut od: Vec<u32> = (0..ne).map(|k| src[(2 * k + 1) as usize]).collect();
    ev.extend(std::iter::repeat_n(pad, (nl - ne) as usize));
    od.extend(std::iter::repeat_n(pad, (nl - ne) as usize));
    (ev, od)
}

fn three_same_scalar(u: u32, sz: u32, opc: u32, d: u32, n: u32, m: u32) -> LResult<String> {
    if opc == 1 || opc == 5 {
        let op = if opc == 1 {
            lop(if u != 0 { "UQADD" } else { "SQADD" }, 0)
        } else {
            lop(if u != 0 { "UQSUB" } else { "SQSUB" }, 0)
        };
        return Ok(same3(&op, sz, 1, d, n, m));
    }
    if opc == 9 || opc == 11 {
        return Ok(same3(
            &lop(if opc == 9 { "QSHL" } else { "QRSHL" }, u),
            sz,
            1,
            d,
            n,
            m,
        ));
    }
    if opc == 22 {
        if sz != 1 && sz != 2 {
            return unsup("sqdmulh size");
        }
        return Ok(same3(
            &lop(if u != 0 { "SQRDMULH" } else { "SQDMULH" }, 0),
            sz,
            1,
            d,
            n,
            m,
        ));
    }
    if sz != 3 {
        return unsup("scalar 3same size");
    }
    let (a, b) = (format!("VS64({n})"), format!("VS64({m})"));
    let (sa, sb) = (format!("(int64_t)VS64({n})"), format!("(int64_t)VS64({m})"));
    let (x, y) = if u != 0 { (&a, &b) } else { (&sa, &sb) };
    match opc {
        6 => Ok(format!("c->v[{d}] = ({x} > {y}) ? ~0ull : 0;")),
        7 => Ok(format!("c->v[{d}] = ({x} >= {y}) ? ~0ull : 0;")),
        8 | 10 => Ok(same3(
            &lop(if opc == 8 { "SHL" } else { "RSHL" }, u),
            3,
            1,
            d,
            n,
            m,
        )),
        16 => Ok(format!("c->v[{d}] = (uint64_t)({a} {} {b});", pm(u))),
        17 => Ok(if u != 0 {
            format!("c->v[{d}] = ({a} == {b}) ? ~0ull : 0;")
        } else {
            format!("c->v[{d}] = ({a} & {b}) ? ~0ull : 0;")
        }),
        _ => unsup("scalar 3same opcode"),
    }
}

fn three_same_fp(w: u32, scalar: bool) -> LResult<String> {
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
    let f = ft(sz);
    let (fl, vt, vuu, sf, set) = (f.F, f.VT, f.VU, f.SF, f.SET);
    let key = (u, a_, opc);
    if let Some(cop) = fcmp3(key) {
        if scalar {
            return Ok(format!(
                "c->v[{d}] = ({})vcmp_{fl}(c, {cop}, ({vt}){{{sf}({n})}}, ({vt}){{{sf}({m})}})[0];",
                ut(2 + sz)
            ));
        }
        return Ok(vwr(
            vuu,
            q,
            d,
            &format!("vcmp_{fl}(c, {cop}, {}, {})", vq(vt, q, n), vq(vt, q, m)),
        ));
    }
    if scalar {
        if !matches!(
            key,
            (0, 0, 0b11011) | (0, 0, 0b11111) | (0, 1, 0b11111) | (1, 1, 0b11010)
        ) {
            return unsup("scalar fp 3same");
        }
        let fop = fop3(key).unwrap();
        let x = if fop == "FOP_RECPS" || fop == "FOP_RSQRTS" {
            format!("neg_{fl}({sf}({n}))")
        } else {
            format!("{sf}({n})")
        };
        return Ok(format!("{set}({d}, op_{fl}(c, {fop}, {x}, {sf}({m}), 0));"));
    }
    let (mut a, b) = (vq(vt, q, n), vq(vt, q, m));
    let fu = fl.to_uppercase();
    if let Some(p) = fpairv(key) {
        let (ev, od) = pair_idx(q, 2 + sz);
        return Ok(format!(
            "SV_FOP_{fu}({p}, {q}, {d}, {}, {}, ({vt}){{}});",
            shuf(&a, &b, &ev),
            shuf(&a, &b, &od)
        ));
    }
    if u == 0 && opc == 0b11001 {
        let mm = if a_ != 0 {
            format!("vneg_{fl}({a})")
        } else {
            a.clone()
        };
        return Ok(format!(
            "SV_FOP_{fu}(FOP_MADD, {q}, {d}, {}, {mm}, {b});",
            vq(vt, q, d)
        ));
    }
    let Some(fop) = fop3(key) else {
        return unsup("fp 3same opcode");
    };
    if fop == "FOP_RECPS" || fop == "FOP_RSQRTS" {
        a = format!("vneg_{fl}({a})");
    }
    Ok(format!(
        "SV_FOP_{fu}({fop}, {q}, {d}, {a}, {b}, ({vt}){{}});"
    ))
}

// ---------------------------------------------------------------- three different

fn three_diff(w: u32, scalar: bool) -> LResult<String> {
    let (q, u, sz, m, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 22, 2),
        bits(w, 16, 5),
        bits(w, 12, 4),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    let mi = m as i64;
    if scalar {
        if u != 0 || ![9, 11, 13].contains(&opc) || (sz != 1 && sz != 2) {
            return unsup("scalar 3diff");
        }
        let op = match opc {
            9 => "SQDMLAL",
            11 => "SQDMLSL",
            _ => "SQDMULL",
        };
        return Ok(gen(
            &lop(op, 0),
            sz,
            1,
            d,
            sz + 1,
            0,
            0,
            n,
            sz,
            0,
            mi,
            sz,
            0,
            -1,
            0,
        ));
    }
    if opc == 14 {
        if u != 0 {
            return unsup("pmull U");
        }
        if sz == 3 {
            return Ok(format!("sl_pmull64(c, {q}, {d}, {n}, {m});"));
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
        let op = ["ADDHN", "RADDHN", "SUBHN", "RSUBHN"][((opc == 6) as u32 * 2 + u) as usize];
        return Ok(gen(
            &lop(op, 0),
            sz,
            ne,
            d,
            sz,
            off,
            q,
            n,
            sz + 1,
            0,
            mi,
            sz + 1,
            0,
            -1,
            0,
        ));
    }
    if opc == 1 || opc == 3 {
        let op = if opc == 1 { "ADDW" } else { "SUBW" };
        return Ok(gen(
            &lop(op, u),
            sz,
            ne,
            d,
            sz + 1,
            0,
            0,
            n,
            sz + 1,
            0,
            mi,
            sz,
            off,
            -1,
            0,
        ));
    }
    let op = match opc {
        0 => "ADDL",
        2 => "SUBL",
        5 => "ABAL",
        7 => "ABDL",
        8 => "MLAL",
        10 => "MLSL",
        12 => "MULL",
        9 => "SQDMLAL",
        11 => "SQDMLSL",
        13 => "SQDMULL",
        _ => "PMULL",
    };
    if op.starts_with("SQD") && (u != 0 || (sz != 1 && sz != 2)) {
        return unsup("sqdmull");
    }
    let uu = if op.starts_with("SQD") || op.starts_with("PM") {
        0
    } else {
        u
    };
    Ok(gen(
        &lop(op, uu),
        sz,
        ne,
        d,
        sz + 1,
        0,
        0,
        n,
        sz,
        off,
        mi,
        sz,
        off,
        -1,
        0,
    ))
}

// ---------------------------------------------------------------- two-register misc

fn misc2(w: u32, scalar: bool) -> LResult<String> {
    let (q, u, sz, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 22, 2),
        bits(w, 12, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if opc >= 0b11000 || opc == 0b10110 || opc == 0b10111 || (0b01100..=0b01111).contains(&opc) {
        return misc2_fp(w, scalar);
    }
    if scalar {
        return misc2_scalar(u, sz, opc, d, n);
    }
    let ne = lanes(q, sz);
    let (t, s) = (vu(sz), vs(sz));
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
        return Ok(vwr("vu8", q, d, &shuf(&vr("vu8", n), &vr("vu8", n), &idx)));
    }
    if sz == 3 && q == 0 && ![0b10010, 0b10100, 0b10011].contains(&opc) {
        return unsup("misc 1d");
    }
    if opc == 2 || opc == 6 {
        if sz == 3 {
            return unsup("addlp size");
        }
        return Ok(format!(
            "sl_addlp(c, {u}, {}, {q}, {sz}, {d}, {n});",
            (opc == 6) as u32
        ));
    }
    if opc == 3 {
        return Ok(same2(
            &lop(if u != 0 { "USQADD" } else { "SUQADD" }, 0),
            sz,
            ne,
            d,
            n,
        ));
    }
    if opc == 4 {
        if sz == 3 {
            return unsup("clz size");
        }
        return Ok(same2(
            &lop(if u != 0 { "CLZ" } else { "CLS" }, 0),
            sz,
            ne,
            d,
            n,
        ));
    }
    if opc == 5 {
        if u == 0 {
            if sz != 0 {
                return unsup("cnt size");
            }
            return Ok(same2(&lop("CNT", 0), 0, ne, d, n));
        }
        if sz == 0 {
            return Ok(vwr("vu64", q, d, &format!("~{}", vr("vu64", n))));
        }
        if sz == 1 {
            return Ok(same2(&lop("RBIT", 0), 0, lanes(q, 0), d, n));
        }
        return unsup("not/rbit size");
    }
    if opc == 7 {
        return Ok(same2(
            &lop(if u != 0 { "SQNEG" } else { "SQABS" }, 0),
            sz,
            ne,
            d,
            n,
        ));
    }
    let (a, sa) = (vr(t, n), vr(s, n));
    if opc == 8 {
        return Ok(vwr(
            t,
            q,
            d,
            &format!("({t})({sa} {} 0)", if u != 0 { ">=" } else { ">" }),
        ));
    }
    if opc == 9 {
        return Ok(vwr(
            t,
            q,
            d,
            &format!("({t})({sa} {} 0)", if u != 0 { "<=" } else { "==" }),
        ));
    }
    if opc == 10 && u == 0 {
        return Ok(vwr(t, q, d, &format!("({t})({sa} < 0)")));
    }
    if opc == 11 {
        if u != 0 {
            return Ok(vwr(t, q, d, &format!("-{a}")));
        }
        let sh = (8 << sz) - 1;
        return Ok(vwr(
            t,
            q,
            d,
            &format!("(({t}){sa} ^ ({t})({sa} >> {sh})) - ({t})({sa} >> {sh})"),
        ));
    }
    if opc == 0b10010 || opc == 0b10100 {
        if sz == 3 {
            return unsup("xtn size");
        }
        let op = match (opc, u) {
            (0b10010, 0) => "XTN",
            (0b10010, _) => "SQXTUN",
            (_, 0) => "SQXTN",
            _ => "UQXTN",
        };
        let ne = 8 >> sz;
        return Ok(gen(
            &lop(op, 0),
            sz,
            ne,
            d,
            sz,
            q * ne,
            q,
            n,
            sz + 1,
            0,
            -1,
            0,
            0,
            -1,
            0,
        ));
    }
    if opc == 0b10011 && u != 0 {
        if sz == 3 {
            return unsup("shll size");
        }
        let ne = 8 >> sz;
        return Ok(gen(
            &lop("SHLL", 0),
            sz,
            ne,
            d,
            sz + 1,
            0,
            0,
            n,
            sz,
            q * ne,
            -1,
            0,
            0,
            -1,
            0,
        ));
    }
    unsup("misc2 opcode")
}

fn misc2_scalar(u: u32, sz: u32, opc: u32, d: u32, n: u32) -> LResult<String> {
    if opc == 3 {
        return Ok(same2(
            &lop(if u != 0 { "USQADD" } else { "SUQADD" }, 0),
            sz,
            1,
            d,
            n,
        ));
    }
    if opc == 7 {
        return Ok(same2(
            &lop(if u != 0 { "SQNEG" } else { "SQABS" }, 0),
            sz,
            1,
            d,
            n,
        ));
    }
    if opc == 0b10010 || opc == 0b10100 {
        if sz == 3 || (opc == 0b10010 && u == 0) {
            return unsup("scalar xtn");
        }
        let op = match (opc, u) {
            (0b10010, _) => "SQXTUN",
            (_, 0) => "SQXTN",
            _ => "UQXTN",
        };
        return Ok(gen(
            &lop(op, 0),
            sz,
            1,
            d,
            sz,
            0,
            0,
            n,
            sz + 1,
            0,
            -1,
            0,
            0,
            -1,
            0,
        ));
    }
    if sz != 3 {
        return unsup("scalar misc size");
    }
    let (a, sa) = (format!("VS64({n})"), format!("(int64_t)VS64({n})"));
    match opc {
        8 => Ok(format!(
            "c->v[{d}] = ({sa} {} 0) ? ~0ull : 0;",
            if u != 0 { ">=" } else { ">" }
        )),
        9 => Ok(format!(
            "c->v[{d}] = ({sa} {} 0) ? ~0ull : 0;",
            if u != 0 { "<=" } else { "==" }
        )),
        10 if u == 0 => Ok(format!("c->v[{d}] = ({sa} < 0) ? ~0ull : 0;")),
        11 => Ok(if u != 0 {
            format!("c->v[{d}] = (uint64_t)-{a};")
        } else {
            format!("c->v[{d}] = ({sa} < 0) ? (uint64_t)-{a} : {a};")
        }),
        _ => unsup("scalar misc opcode"),
    }
}

/// FP two-register misc, vector and scalar.
fn misc2_fp(w: u32, scalar: bool) -> LResult<String> {
    let (q, u, a_, sz, opc, n, d) = (
        bits(w, 30, 1),
        bits(w, 29, 1),
        bits(w, 23, 1),
        bits(w, 22, 1),
        bits(w, 12, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    let f = ft(sz);
    let (fl, vt, vuu, sf, su, set) = (f.F, f.VT, f.VU, f.SF, f.SU, f.SET);
    if opc == 0b10110 || opc == 0b10111 {
        if a_ != 0 {
            return unsup("fcvtn a");
        }
        if opc == 0b10110 && u != 0 {
            if sz == 0 {
                return unsup("fcvtxn size");
            }
            if scalar {
                return Ok(format!("FP_SETS({d}, simd_d2s_odd(c, VSF64({n})));"));
            }
            return Ok(format!("sv_fcvtn(c, 1, {q}, 1, {d}, {n});"));
        }
        if scalar || u != 0 {
            return unsup("fcvtn/l");
        }
        if opc == 0b10110 {
            return Ok(format!("sv_fcvtn(c, {sz}, {q}, 0, {d}, {n});"));
        }
        return Ok(format!("sv_fcvtl(c, {sz}, {q}, {d}, {n});"));
    }
    if !scalar && sz != 0 && q == 0 {
        return unsup("fp misc 2d Q");
    }
    let x = vq(vt, q, n);
    if (0b01100..=0b01111).contains(&opc) {
        if a_ == 0 {
            return unsup("fp16 misc");
        }
        if opc == 0b01111 {
            if scalar {
                return unsup("scalar fabs");
            }
            let mask = if sz == 0 {
                "0x80000000u"
            } else {
                "0x8000000000000000ull"
            };
            let r = vr(vuu, n);
            return Ok(vwr(
                vuu,
                q,
                d,
                &if u != 0 {
                    format!("{r} ^ {mask}")
                } else {
                    format!("{r} & ~{mask}")
                },
            ));
        }
        let (op, swap) = match (opc, u) {
            (0b01100, 0) => (2, 0),
            (0b01100, 1) => (1, 0),
            (0b01101, 0) => (0, 0),
            (0b01101, 1) => (1, 1),
            (0b01110, 0) => (2, 1),
            _ => return unsup("fcm zero"),
        };
        if scalar {
            let (mut a, mut b) = (format!("({vt}){{{sf}({n})}}"), format!("({vt}){{}}"));
            if swap != 0 {
                std::mem::swap(&mut a, &mut b);
            }
            return Ok(format!(
                "c->v[{d}] = ({})vcmp_{fl}(c, {op}, {a}, {b})[0];",
                ut(2 + sz)
            ));
        }
        let (mut a, mut b) = (x.clone(), format!("({vt}){{}}"));
        if swap != 0 {
            std::mem::swap(&mut a, &mut b);
        }
        return Ok(vwr(vuu, q, d, &format!("vcmp_{fl}(c, {op}, {a}, {b})")));
    }
    if opc == 0b11111 {
        if a_ == 0 {
            return unsup("fp misc 11111");
        }
        if u != 0 {
            if scalar {
                return unsup("scalar fsqrt");
            }
            return Ok(format!(
                "SV_FOP_{}(FOP_SQRT, {q}, {d}, {x}, {x}, ({vt}){{}});",
                fl.to_uppercase()
            ));
        }
        if !scalar {
            return unsup("vector frecpx");
        }
        return Ok(format!("{set}({d}, recpx_{fl}(c, {sf}({n})));"));
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
        return Ok(vwr(vt, q, d, &format!("vfrint_{fl}(c, {x}, {rm})")));
    }
    if opc == 0b11010 || opc == 0b11011 || (opc == 0b11100 && a_ == 0) {
        let rm = match (opc, a_) {
            (0b11010, 0) => 0,
            (0b11010, _) => 1,
            (0b11011, 0) => 2,
            (0b11011, _) => 3,
            _ => 4,
        };
        if scalar {
            return Ok(format!(
                "c->v[{d}] = ({})toint_{fl}(c, {sf}({n}), {rm}, 0, {}, {u});",
                ut(2 + sz),
                32 << sz
            ));
        }
        return Ok(vwr(
            vuu,
            q,
            d,
            &format!("vtoint_{fl}(c, {x}, {rm}, 0, {u})"),
        ));
    }
    if opc == 0b11100 {
        if scalar || sz != 0 {
            return unsup("urecpe");
        }
        return Ok(same2(
            &lop(if u != 0 { "URSQRTE" } else { "URECPE" }, 0),
            2,
            lanes(q, 2),
            d,
            n,
        ));
    }
    if opc == 0b11101 {
        if a_ == 0 {
            if scalar {
                return Ok(format!(
                    "{set}({d}, fromint_{fl}(c, {su}({n}), {}, {u}, 0));",
                    32 << sz
                ));
            }
            return Ok(vwr(
                vt,
                q,
                d,
                &format!("vfromint_{fl}(c, {}, 0, {u})", vr(vuu, n)),
            ));
        }
        let nm = if u != 0 { "rsqrte" } else { "recpe" };
        if scalar {
            return Ok(format!("{set}({d}, {nm}_{fl}(c, {sf}({n})));"));
        }
        return Ok(vwr(vt, q, d, &format!("v{nm}_{fl}(c, {x})")));
    }
    unsup("fp misc opcode")
}

// ---------------------------------------------------------------- across lanes / pairwise

fn across(w: u32) -> LResult<String> {
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
        let fop = match (opc, sz >> 1) {
            (0b01100, 0) => "FOP_MAXNM",
            (0b01100, _) => "FOP_MINNM",
            (_, 0) => "FOP_MAX",
            _ => "FOP_MIN",
        };
        return Ok(format!("sv_fred_s(c, {fop}, {d}, {n});"));
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
            Ok(format!("sl_across(c, {op}, {u}, {q}, {sz}, {d}, {n});"))
        }
        _ => unsup("across"),
    }
}

fn scalar_pairwise(w: u32) -> LResult<String> {
    let (u, sz, opc, n, d) = (
        bits(w, 29, 1),
        bits(w, 22, 2),
        bits(w, 12, 5),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if u == 0 {
        if opc == 0b11011 && sz == 3 {
            return Ok(format!("c->v[{d}] = (uint64_t)(VS64({n}) + VHI64({n}));"));
        }
        return unsup("scalar pairwise");
    }
    let fop = match (opc, sz >> 1) {
        (0b01100, 0) => "FOP_MAXNM",
        (0b01100, 1) => "FOP_MINNM",
        (0b01101, 0) => "FOP_ADD",
        (0b01111, 0) => "FOP_MAX",
        (0b01111, 1) => "FOP_MIN",
        _ => return unsup("scalar fp pairwise"),
    };
    let f = ft(sz & 1);
    let r = vr(f.VT, n);
    Ok(format!(
        "{}({d}, pair_{}(c, {fop}, {r}[0], {r}[1]));",
        f.SET, f.F
    ))
}

// ---------------------------------------------------------------- modified immediate

fn modimm(w: u32) -> LResult<String> {
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
    let c4 = cmode;
    if (c4 < 8 && c4 & 1 != 0) || ((8..12).contains(&c4) && c4 & 1 != 0) {
        let r = vr("vu64", d);
        if op != 0 {
            return Ok(vwr("vu64", q, d, &format!("{r} & {}", hx(!imm))));
        }
        return Ok(vwr("vu64", q, d, &format!("{r} | {}", hx(imm))));
    }
    if op != 0 && !(c4 == 14 || c4 == 15) {
        imm = !imm;
    }
    let v: u128 = if q != 0 {
        imm as u128 | ((imm as u128) << 64)
    } else {
        imm as u128
    };
    Ok(format!("c->v[{d}] = {};", u128s(v)))
}

// ---------------------------------------------------------------- shift by immediate

fn shift_imm(w: u32, scalar: bool) -> LResult<String> {
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
        let f = ft(sz - 2);
        let fb = rsh;
        if opc == 0b11100 {
            if scalar {
                return Ok(format!(
                    "{}({d}, fromint_{}(c, {}({n}), {es}, {u}, {fb}));",
                    f.SET, f.F, f.SU
                ));
            }
            return Ok(vwr(
                f.VT,
                q,
                d,
                &format!("vfromint_{}(c, {}, {fb}, {u})", f.F, vr(f.VU, n)),
            ));
        }
        if scalar {
            return Ok(format!(
                "c->v[{d}] = ({})toint_{}(c, {}({n}), 3, {fb}, {es}, {u});",
                ut(sz),
                f.F,
                f.SF
            ));
        }
        return Ok(vwr(
            f.VU,
            q,
            d,
            &format!("vtoint_{}(c, {}, 3, {fb}, {u})", f.F, vq(f.VT, q, n)),
        ));
    }
    if narrow {
        if sz == 3 {
            return unsup("narrow shift size");
        }
        let ne = if scalar { 1 } else { 8 >> sz };
        let (name, ou) = match (opc, u) {
            (0b10000, 0) => ("SHRN", 0),
            (0b10001, 0) => ("RSHRN", 0),
            (0b10000, _) => ("QSHRUN", 0),
            (0b10001, _) => ("QRSHRUN", 0),
            (0b10010, 0) => ("QSHRN", 0),
            (0b10010, _) => ("QSHRN", 1),
            (_, 0) => ("QRSHRN", 0),
            _ => ("QRSHRN", 1),
        };
        if scalar && (name == "SHRN" || name == "RSHRN") {
            return unsup("scalar shrn");
        }
        let part = if scalar { 0 } else { q };
        return Ok(gen(
            &lop(name, ou),
            sz,
            ne,
            d,
            sz,
            part * ne,
            part,
            n,
            sz + 1,
            0,
            -1,
            0,
            0,
            -1,
            rsh as u64,
        ));
    }
    if opc == 0b10100 {
        if scalar || sz == 3 {
            return unsup("sshll");
        }
        let ne = 8 >> sz;
        return Ok(gen(
            &lop("SHLLI", u),
            sz,
            ne,
            d,
            sz + 1,
            0,
            0,
            n,
            sz,
            q * ne,
            -1,
            0,
            0,
            -1,
            lsh as u64,
        ));
    }
    if sz == 3 && q == 0 {
        return unsup("shift 1d");
    }
    if scalar && sz != 3 && opc != 0b01100 && opc != 0b01110 {
        return unsup("scalar shift size");
    }
    let (t, s) = (vu(sz), vs(sz));
    let fin = |e: String| -> String {
        if !scalar {
            vwr(t, q, d, &e)
        } else {
            format!("c->v[{d}] = (uint64_t)(({e})[0]);")
        }
    };
    match opc {
        0 | 2 => {
            let mut e = if u != 0 {
                if rsh >= es {
                    format!("({t}){{}}")
                } else {
                    format!("({} >> {rsh})", vr(t, n))
                }
            } else {
                format!("({t})({} >> {})", vr(s, n), rsh.min(es - 1))
            };
            if opc == 2 {
                e = format!("{} + {e}", vr(t, d));
            }
            Ok(fin(e))
        }
        4 | 6 => Ok(same(
            &lop(if opc == 4 { "RSHRI" } else { "RSRAI" }, u),
            sz,
            ne,
            d,
            n,
            -1,
            rsh as u64,
            -1,
        )),
        8 => {
            if u == 0 {
                return unsup("sri U");
            }
            Ok(same(&lop("SRI", 0), sz, ne, d, n, -1, rsh as u64, -1))
        }
        10 => {
            if u != 0 {
                return Ok(same(&lop("SLI", 0), sz, ne, d, n, -1, lsh as u64, -1));
            }
            Ok(fin(format!("{} << {lsh}", vr(t, n))))
        }
        12 => {
            if u == 0 {
                return unsup("sqshlu U");
            }
            Ok(same(&lop("QSHLUI", 0), sz, ne, d, n, -1, lsh as u64, -1))
        }
        14 => Ok(same(&lop("QSHLI", u), sz, ne, d, n, -1, lsh as u64, -1)),
        _ => unsup("shift imm opcode"),
    }
}

// ---------------------------------------------------------------- indexed element

fn indexed(w: u32, scalar: bool) -> LResult<String> {
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
    let fp = [1, 5, 9].contains(&opc) && !(u != 0 && opc != 9);
    if fp {
        if sz < 2 {
            return unsup("fp16 indexed");
        }
        let fsz = sz & 1;
        let idx = if fsz != 0 {
            if l != 0 {
                return unsup("indexed L");
            }
            h
        } else {
            (h << 1) | l
        };
        let m = (mb << 4) | rm;
        if !scalar && fsz != 0 && q == 0 {
            return unsup("indexed 2d Q");
        }
        let f = ft(fsz);
        let (fl, vt, sf, set) = (f.F, f.VT, f.SF, f.SET);
        let mul = if u != 0 { "FOP_MULX" } else { "FOP_MUL" };
        if scalar {
            let el = format!("{}[{idx}]", vr(vt, m));
            if opc == 9 {
                return Ok(format!(
                    "{set}({d}, op_{fl}(c, {mul}, {sf}({n}), {el}, 0));"
                ));
            }
            let x = if opc == 1 {
                format!("{sf}({n})")
            } else {
                format!("neg_{fl}({sf}({n}))")
            };
            return Ok(format!(
                "{set}({d}, op_{fl}(c, FOP_MADD, {sf}({d}), {x}, {el}));"
            ));
        }
        let el = format!("vsplat_{fl}({}, {idx}, {q})", vr(vt, m));
        let fu = fl.to_uppercase();
        if opc == 9 {
            return Ok(format!(
                "SV_FOP_{fu}({mul}, {q}, {d}, {}, {el}, ({vt}){{}});",
                vq(vt, q, n)
            ));
        }
        let x = if opc == 1 {
            vq(vt, q, n)
        } else {
            format!("vneg_{fl}({})", vq(vt, q, n))
        };
        return Ok(format!(
            "SV_FOP_{fu}(FOP_MADD, {q}, {d}, {}, {x}, {el});",
            vq(vt, q, d)
        ));
    }
    let (idx, m) = match sz {
        1 => ((h << 2) | (l << 1) | mb, rm),
        2 => ((h << 1) | l, (mb << 4) | rm),
        _ => return unsup("indexed size"),
    };
    let t = vu(sz);
    let (mi, ii) = (m as i64, idx as i64);
    if scalar {
        let op = match (u, opc) {
            (0, 3) => "SQDMLAL",
            (0, 7) => "SQDMLSL",
            (0, 11) => "SQDMULL",
            (0, 12) => "SQDMULH",
            (0, 13) => "SQRDMULH",
            _ => return unsup("scalar indexed"),
        };
        if op == "SQDMULH" || op == "SQRDMULH" {
            return Ok(gen(
                &lop(op, 0),
                sz,
                1,
                d,
                sz,
                0,
                0,
                n,
                sz,
                0,
                mi,
                sz,
                0,
                ii,
                0,
            ));
        }
        return Ok(gen(
            &lop(op, 0),
            sz,
            1,
            d,
            sz + 1,
            0,
            0,
            n,
            sz,
            0,
            mi,
            sz,
            0,
            ii,
            0,
        ));
    }
    let ne = lanes(q, sz);
    let el = shuf(&vr(t, m), &vr(t, m), &vec![idx; (16 >> sz) as usize]);
    if (u, opc) == (0, 8) {
        return Ok(vwr(t, q, d, &format!("{} * {el}", vr(t, n))));
    }
    if (u, opc) == (1, 0) || (u, opc) == (1, 4) {
        return Ok(vwr(
            t,
            q,
            d,
            &format!(
                "{} {} {} * {el}",
                vr(t, d),
                if opc == 0 { "+" } else { "-" },
                vr(t, n)
            ),
        ));
    }
    if (u, opc) == (0, 12) || (u, opc) == (0, 13) {
        return Ok(same(
            &lop(if opc == 12 { "SQDMULH" } else { "SQRDMULH" }, 0),
            sz,
            ne,
            d,
            n,
            mi,
            0,
            ii,
        ));
    }
    let lng = match opc {
        2 => "MLAL",
        6 => "MLSL",
        10 => "MULL",
        3 => "SQDMLAL",
        7 => "SQDMLSL",
        11 => "SQDMULL",
        _ => return unsup("indexed opcode"),
    };
    let sq = lng.starts_with("SQ");
    if u != 0 && sq {
        return unsup("indexed opcode");
    }
    let ne = 8 >> sz;
    Ok(gen(
        &lop(lng, if sq { 0 } else { u }),
        sz,
        ne,
        d,
        sz + 1,
        0,
        0,
        n,
        sz,
        q * ne,
        mi,
        sz,
        0,
        ii,
        0,
    ))
}

// ---------------------------------------------------------------- scalar FP

fn ftype(w: u32) -> LResult<u32> {
    let t = bits(w, 22, 2);
    if t > 1 {
        return unsup("fp type");
    }
    Ok(t)
}

fn fp_dp3(w: u32) -> LResult<String> {
    let t = ftype(w)?;
    let (o1, o0) = (bits(w, 21, 1), bits(w, 15, 1));
    let (d, n, m, a) = (bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5), bits(w, 10, 5));
    let (na, nn) = (o1, o1 ^ o0);
    Ok(format!(
        "fp_fma_{}(c, {na}, {nn}, {d}, {n}, {m}, {a});",
        ft(t).F
    ))
}

fn fp_dp2(w: u32) -> LResult<String> {
    let t = ftype(w)?;
    let (opc, d, n, m) = (bits(w, 12, 4), bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5));
    const OPS: [&str; 9] = [
        "FOP_MUL",
        "FOP_DIV",
        "FOP_ADD",
        "FOP_SUB",
        "FOP_MAX",
        "FOP_MIN",
        "FOP_MAXNM",
        "FOP_MINNM",
        "FOP_NMUL",
    ];
    if opc > 8 {
        return unsup("fp dp2");
    }
    Ok(format!(
        "fp_op_{}(c, {}, {d}, {n}, {m});",
        ft(t).F,
        OPS[opc as usize]
    ))
}

fn fp_dp1(w: u32) -> LResult<String> {
    let t = bits(w, 22, 2);
    let (opc, d, n) = (bits(w, 15, 6), bits(w, 0, 5), bits(w, 5, 5));
    if t == 3 {
        return match opc {
            4 => Ok(format!("FP_SETS({d}, simd_h2s(c, VS16({n})));")),
            5 => Ok(format!("FP_SETD({d}, simd_h2d(c, VS16({n})));")),
            _ => unsup("fp16 dp1"),
        };
    }
    if t == 2 {
        return unsup("fp type");
    }
    let f = ft(t);
    let sign = if t == 0 {
        "0x80000000u"
    } else {
        "0x8000000000000000ull"
    };
    let (fl, sf, su, set) = (f.F, f.SF, f.SU, f.SET);
    match opc {
        0 => return Ok(format!("c->v[{d}] = {su}({n});")),
        1 => return Ok(format!("c->v[{d}] = {su}({n}) & ~{sign};")),
        2 => return Ok(format!("c->v[{d}] = {su}({n}) ^ {sign};")),
        3 => return Ok(format!("fp_op_{fl}(c, FOP_SQRT, {d}, {n}, {n});")),
        4 | 5 | 7 => {
            let to = match opc {
                4 => 0,
                5 => 1,
                _ => 3,
            };
            if to == t {
                return unsup("fcvt same");
            }
            if to == 3 {
                return Ok(format!(
                    "c->v[{d}] = {}(c, {sf}({n}));",
                    if t == 0 { "simd_s2h" } else { "simd_dd2h" }
                ));
            }
            return Ok(if to != 0 {
                format!("FP_SETD({d}, simd_s2d(c, VSF32({n})));")
            } else {
                format!("FP_SETS({d}, simd_d2s(c, VSF64({n})));")
            });
        }
        _ => {}
    }
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
    Ok(format!("{set}({d}, frint_{fl}(c, {sf}({n}), {rm}));"))
}

fn fp_cmp(w: u32) -> LResult<String> {
    let t = ftype(w)?;
    if bits(w, 14, 2) != 0 || bits(w, 0, 3) != 0 {
        return unsup("fcmp op");
    }
    let f = ft(t);
    let (n, m, zero) = (bits(w, 5, 5), bits(w, 16, 5), bits(w, 3, 1));
    let b = if zero != 0 {
        "0".to_string()
    } else {
        format!("{}({m})", f.SF)
    };
    Ok(format!("c->nzcv = cmp_{}(c, {}({n}), {b});", f.F, f.SF))
}

fn fp_ccmp(w: u32) -> LResult<String> {
    let t = ftype(w)?;
    let f = ft(t);
    let (n, m, cnd, nzcv) = (bits(w, 5, 5), bits(w, 16, 5), bits(w, 12, 4), bits(w, 0, 4));
    Ok(format!(
        "c->nzcv = {} ? cmp_{}(c, {sf}({n}), {sf}({m})) : {};",
        cond(cnd),
        f.F,
        hx((nzcv as u64) << 28),
        sf = f.SF
    ))
}

fn fp_csel(w: u32) -> LResult<String> {
    let t = ftype(w)?;
    let su = ft(t).SU;
    let (d, n, m, cnd) = (bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5), bits(w, 12, 4));
    Ok(format!(
        "c->v[{d}] = {} ? {su}({n}) : {su}({m});",
        cond(cnd)
    ))
}

fn fp_imm(w: u32) -> LResult<String> {
    let t = ftype(w)?;
    if bits(w, 5, 5) != 0 {
        return unsup("fmov imm5");
    }
    let (imm8, d) = (bits(w, 13, 8), bits(w, 0, 5));
    Ok(format!(
        "c->v[{d}] = {};",
        hx(vfp_expand_imm(imm8, 32 << t))
    ))
}

fn fp_fixed(w: u32) -> LResult<String> {
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
    let f = ft(t);
    let (fb, ib) = (64 - scale, if sf != 0 { 64 } else { 32 });
    if rmode == 0 && (opc == 2 || opc == 3) {
        let src = if sf != 0 { xr(n) } else { wr(n) };
        return Ok(format!(
            "{}({d}, fromint_{}(c, {src}, {ib}, {}, {fb}));",
            f.SET,
            f.F,
            opc & 1
        ));
    }
    if rmode == 3 && (opc == 0 || opc == 1) {
        return Ok(set_reg(
            sf,
            d,
            &format!("toint_{}(c, {}({n}), 3, {fb}, {ib}, {opc})", f.F, f.SF),
            false,
        ));
    }
    unsup("fixed opcode")
}

fn fp_int(w: u32) -> LResult<String> {
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
            return Ok(if opc == 6 {
                set_reg(1, d, &format!("VHI64({n})"), false)
            } else {
                format!("VHI64({d}) = {};", xr(n))
            });
        }
        if rmode != 0 || t != sf || t > 1 {
            return unsup("fmov general");
        }
        if opc == 6 {
            return Ok(set_reg(
                sf,
                d,
                &if sf != 0 {
                    format!("VS64({n})")
                } else {
                    format!("VS32({n})")
                },
                false,
            ));
        }
        return Ok(format!(
            "c->v[{d}] = {};",
            if sf != 0 { xr(n) } else { wr(n) }
        ));
    }
    if t > 1 {
        return unsup("fp int type");
    }
    let f = ft(t);
    let ib = if sf != 0 { 64 } else { 32 };
    if opc == 2 || opc == 3 {
        if rmode != 0 {
            return unsup("cvtf rmode");
        }
        let src = if sf != 0 { xr(n) } else { wr(n) };
        return Ok(format!(
            "{}({d}, fromint_{}(c, {src}, {ib}, {}, 0));",
            f.SET,
            f.F,
            opc & 1
        ));
    }
    let rm = if opc == 4 || opc == 5 {
        if rmode != 0 {
            return unsup("fcvta rmode");
        }
        4
    } else {
        rmode
    };
    Ok(set_reg(
        sf,
        d,
        &format!(
            "toint_{}(c, {}({n}), {rm}, 0, {ib}, {})",
            f.F,
            f.SF,
            opc & 1
        ),
        false,
    ))
}

// ---------------------------------------------------------------- loads / stores

/// SimdC.ldst: FP/SIMD loads and stores.
pub fn ldst(w: u32, pc: u64) -> LResult<String> {
    let rt = bits(w, 0, 5);
    if bits(w, 27, 3) == 0b011 && bits(w, 24, 2) == 0 {
        let opc = bits(w, 30, 2);
        if opc == 3 {
            return unsup("ldr literal opc");
        }
        let addr = pc.wrapping_add((sext(bits(w, 5, 19) as u64, 19) * 4) as u64);
        return Ok(format!(
            "c->v[{rt}] = {}({});",
            ["LD32", "LD64", "LD128"][opc as usize],
            hx(addr)
        ));
    }
    if bits(w, 31, 1) == 0 && (bits(w, 24, 6) == 0b001100 || bits(w, 24, 6) == 0b001101) {
        return ldst_struct(w);
    }
    if bits(w, 27, 3) == 0b101 {
        return ldst_pair(w);
    }
    if bits(w, 27, 3) == 0b111 {
        return ldst_reg(w);
    }
    unsup(format!("ldst simd {w:08x}"))
}

fn sreg(rn: u32) -> String {
    if rn == 31 {
        "c->sp".into()
    } else {
        format!("c->x[{rn}]")
    }
}

fn ldst_pair(w: u32) -> LResult<String> {
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
    let base = xs(rn);
    let (post, wb) = (mode == 1, mode == 1 || mode == 3);
    let ea = if post {
        base.clone()
    } else if off >= 0 {
        format!("{base} + {off}")
    } else {
        format!("{base} - {}", -off)
    };
    let (ld, st) = match scale {
        2 => ("LD32", "ST32"),
        3 => ("LD64", "ST64"),
        _ => ("LD128", "ST128"),
    };
    let sz = 1u32 << scale;
    let mut lines = vec![format!("uint64_t ea = {ea};")];
    if l != 0 {
        lines.push(format!("u128 a = {ld}(ea), b = {ld}(ea + {sz});"));
        lines.push(format!("c->v[{rt}] = a; c->v[{rt2}] = b;"));
    } else {
        let (v1, v2) = if scale == 4 {
            (format!("c->v[{rt}]"), format!("c->v[{rt2}]"))
        } else {
            let cast = if scale == 2 { "VS32" } else { "VS64" };
            (format!("{cast}({rt})"), format!("{cast}({rt2})"))
        };
        lines.push(format!("{st}(ea, {v1}); {st}(ea + {sz}, {v2});"));
    }
    if wb {
        let dst = sreg(rn);
        if !post {
            lines.push(format!("{dst} = ea;"));
        } else if off >= 0 {
            lines.push(format!("{dst} = ea + {off};"));
        } else {
            lines.push(format!("{dst} = ea - {};", -off));
        }
    }
    Ok(format!("{{ {} }}", lines.join(" ")))
}

fn ldst_reg(w: u32) -> LResult<String> {
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
    let base = xs(rn);
    let mut wb: Option<String> = None;
    let ea;
    match bits(w, 24, 2) {
        1 => ea = format!("{base} + {}", hx((bits(w, 10, 12) as u64) << scale)),
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
                ea = format!(
                    "{base} + {}",
                    extend_expr(rm, option, if s != 0 { scale } else { 0 })
                );
            } else {
                let imm9 = sext(bits(w, 12, 9) as u64, 9);
                let si = if imm9 >= 0 {
                    format!("+ {imm9}")
                } else {
                    format!("- {}", -imm9)
                };
                match kind {
                    0 => ea = format!("{base} {si}"),
                    1 => {
                        ea = base.clone();
                        wb = Some(format!("{base} {si}"));
                    }
                    3 => {
                        ea = format!("{base} {si}");
                        wb = Some("ea".into());
                    }
                    _ => return unsup("ldst simd unprivileged"),
                }
            }
        }
        _ => return unsup("ldst simd"),
    }
    let mut parts = vec![format!("uint64_t ea = {ea};")];
    if l != 0 {
        parts.push(format!(
            "c->v[{rt}] = {}(ea);",
            ["LD8", "LD16", "LD32", "LD64", "LD128"][scale as usize]
        ));
    } else {
        let src = match scale {
            0 => format!("VS8({rt})"),
            1 => format!("VS16({rt})"),
            2 => format!("VS32({rt})"),
            3 => format!("VS64({rt})"),
            _ => format!("c->v[{rt}]"),
        };
        parts.push(format!(
            "{}(ea, {src});",
            ["ST8", "ST16", "ST32", "ST64", "ST128"][scale as usize]
        ));
    }
    if let Some(wb) = wb {
        parts.push(format!("{} = {wb};", sreg(rn)));
    }
    Ok(format!("{{ {} }}", parts.join(" ")))
}

fn ldst_struct(w: u32) -> LResult<String> {
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
    let base = xs(rn);
    let total;
    let acc;
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
            let items: Vec<String> = (0..rpt)
                .map(|i| {
                    let r = (rt + i) % 32;
                    if l != 0 {
                        format!(
                            "c->v[{r}] = {}(ea + {});",
                            if q != 0 { "LD128" } else { "LD64" },
                            i * step
                        )
                    } else if q != 0 {
                        format!("ST128(ea + {}, c->v[{r}]);", i * step)
                    } else {
                        format!("ST64(ea + {}, VS64({r}));", i * step)
                    }
                })
                .collect();
            acc = items.join(" ");
        } else {
            acc = format!(
                "sl_{}(c, {q}, {sz}, {selem}, {rpt}, {rt}, ea);",
                if l != 0 { "ldm" } else { "stm" }
            );
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
            acc = format!("sl_ldr(c, {q}, {sz}, {selem}, {rt}, ea);");
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
            acc = format!(
                "sl_{}(c, {scale}, {selem}, {idx}, {rt}, ea);",
                if l != 0 { "lds" } else { "sts" }
            );
        }
    }
    let mut parts = vec![format!("uint64_t ea = {base};"), acc];
    if post != 0 {
        let dst = sreg(rn);
        parts.push(if rm == 31 {
            format!("{dst} = ea + {total};")
        } else {
            format!("{dst} = ea + {};", xr(rm))
        });
    }
    Ok(format!("{{ {} }}", parts.join(" ")))
}

// ---------------------------------------------------------------- CRC32

/// SimdC.crc: CRC32{B,H,W,X} / CRC32C*.
pub fn crc(w: u32) -> LResult<String> {
    let (sf, rm, c, sz, rn, rd) = (
        bits(w, 31, 1),
        bits(w, 16, 5),
        bits(w, 12, 1),
        bits(w, 10, 2),
        bits(w, 5, 5),
        bits(w, 0, 5),
    );
    if (sz == 3) != (sf != 0) {
        return unsup("crc size");
    }
    let val = if sz == 3 { xr(rm) } else { wr(rm) };
    Ok(set_reg(
        0,
        rd,
        &format!(
            "{}({}, {val}, {})",
            if c != 0 { "CRC32C" } else { "CRC32" },
            wr(rn),
            1 << sz
        ),
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected strings taken verbatim from scripts/simd/dump_py.py output over MKW 1.8.0 sdk.
    #[test]
    fn simd_fadd_scalar() {
        assert_eq!(dp(0x1e262890, 0).unwrap(), "fp_op_s(c, FOP_ADD, 16, 4, 6);");
    }

    #[test]
    fn simd_vector_add() {
        assert_eq!(
            dp(0x4eb186d6, 0).unwrap(),
            "VW(vu32, 1, 22, VREG(vu32, 22) + VREG(vu32, 17));"
        );
    }

    #[test]
    fn simd_ldr_q_imm() {
        assert_eq!(
            ldst(0x3dc02660, 0).unwrap(),
            "{ uint64_t ea = c->x[19] + 0x90ull; c->v[0] = LD128(ea); }"
        );
    }

    #[test]
    fn simd_ld1_multiple() {
        assert_eq!(
            ldst(0x4c40a5e0, 0).unwrap(),
            "{ uint64_t ea = c->x[15]; c->v[0] = LD128(ea + 0); c->v[1] = LD128(ea + 16); }"
        );
    }

    #[test]
    fn simd_fcvtzs() {
        assert_eq!(
            dp(0x1e38024d, 0).unwrap(),
            "c->x[13] = (uint32_t)(toint_s(c, VSF32(18), 3, 0, 32, 0));"
        );
    }

    #[test]
    fn simd_modimm_movi() {
        assert_eq!(
            dp(0x4f04e400, 0).unwrap(),
            "c->v[0] = ((u128)0x8080808080808080ull << 64 | 0x8080808080808080ull);"
        );
    }

    #[test]
    fn simd_aes() {
        assert_eq!(dp(0x4e287800, 0).unwrap(), "sv_aes(c, 3, 0, 0);");
    }

    #[test]
    fn simd_crc32() {
        assert_eq!(
            crc(0x1acb4108).unwrap(),
            "c->x[8] = (uint32_t)(CRC32((uint32_t)c->x[8], (uint32_t)c->x[11], 1));"
        );
    }

    #[test]
    fn simd_unsupported_message() {
        assert_eq!(crc(0x9acb4108).unwrap_err().0, "crc size");
    }
}

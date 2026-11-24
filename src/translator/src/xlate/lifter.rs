//! AArch64 -> C, one instruction at a time (portable C path: FP/SIMD lowered by `simd`).
//! Only the portable C path is implemented: an inline-asm path would need an AArch64 host.

use crate::xlate::simd;
use crate::xlate::util::*;

fn wr(r: u32) -> String {
    crate::xlate::util::w(r)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Op,
    B,
    Bcond,
    Bl,
    Blr,
    Br,
    Ret,
    Trap,
}

/// Result of lifting one instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insn {
    pub kind: Kind,
    /// C statements (Op, Trap).
    pub code: String,
    /// Direct branch target (B, Bcond, Bl).
    pub target: u64,
    /// Register expression of an indirect branch (Br, Blr).
    pub target_reg: String,
    /// Condition expression (Bcond).
    pub cond: String,
}

impl Insn {
    pub fn op(code: impl Into<String>) -> Insn {
        Insn {
            kind: Kind::Op,
            code: code.into(),
            target: 0,
            target_reg: String::new(),
            cond: String::new(),
        }
    }
    pub fn trap(code: impl Into<String>) -> Insn {
        Insn {
            kind: Kind::Trap,
            ..Insn::op(code)
        }
    }
    fn branch(kind: Kind, target: u64) -> Insn {
        Insn {
            kind,
            target,
            ..Insn::op("")
        }
    }
    fn bcond(target: u64, cond: String) -> Insn {
        Insn {
            kind: Kind::Bcond,
            target,
            cond,
            ..Insn::op("")
        }
    }
    fn reg_branch(kind: Kind, reg: String) -> Insn {
        Insn {
            kind,
            target_reg: reg,
            ..Insn::op("")
        }
    }
    fn ret() -> Insn {
        Insn {
            kind: Kind::Ret,
            ..Insn::op("")
        }
    }
}

/// Lifter for one module (guest addresses are absolute; the base only matters to callers).
#[derive(Debug, Clone, Copy, Default)]
pub struct Lifter;

fn shift_expr(sf: u32, val: &str, typ: u32, amt: u32) -> String {
    if amt == 0 && typ != 3 {
        return val.to_string();
    }
    match typ {
        0 => format!("({val} << {amt})"),
        1 => format!("({val} >> {amt})"),
        2 => {
            if sf != 0 {
                format!("(uint64_t)((int64_t){val} >> {amt})")
            } else {
                format!("(uint32_t)((int32_t){val} >> {amt})")
            }
        }
        _ => format!("ROR{}({val}, {amt})", if sf != 0 { 64 } else { 32 }),
    }
}

fn pc_rel(pc: u64, off: i64) -> u64 {
    pc.wrapping_add(off as u64)
}

impl Lifter {
    pub fn new() -> Lifter {
        Lifter
    }

    /// Module.lift: Unsupported becomes an rt_udf trap with the message.
    pub fn lift_or_trap(&self, w: u32, pc: u64) -> Insn {
        match self.lift(w, pc) {
            Ok(i) => i,
            Err(e) => Insn::trap(format!("rt_udf(c, {}); /* {} */", hx(pc), e.0)),
        }
    }

    /// pc is absolute.
    pub fn lift(&self, w: u32, pc: u64) -> LResult<Insn> {
        let op0 = bits(w, 25, 4);
        if (op0 & 0b1110) == 0b1000 {
            return self.dp_imm(w, pc);
        }
        if (op0 & 0b1110) == 0b1010 {
            return self.branch_sys(w, pc);
        }
        if (op0 & 0b0101) == 0b0100 {
            return self.ldst(w, pc);
        }
        if (op0 & 0b0111) == 0b0101 {
            return self.dp_reg(w, pc);
        }
        if (op0 & 0b0111) == 0b0111 {
            return Ok(Insn::op(simd::dp(w, pc)?));
        }
        Ok(Insn::trap(format!("rt_udf(c, {});", hx(pc))))
    }

    // ------------------------------------------------------------ data processing (imm)

    fn dp_imm(&self, w: u32, pc: u64) -> LResult<Insn> {
        let op = bits(w, 23, 3);
        let sf = bits(w, 31, 1);
        let (rd, rn) = (bits(w, 0, 5), bits(w, 5, 5));
        let op_ = |s: String| Ok(Insn::op(s));
        match op {
            0 | 1 => {
                let imm = sext(((bits(w, 5, 19) as u64) << 2) | bits(w, 29, 2) as u64, 21);
                let val = if bits(w, 31, 1) != 0 {
                    (pc & !0xFFF).wrapping_add((imm << 12) as u64)
                } else {
                    pc_rel(pc, imm)
                };
                op_(set_reg(1, rd, &hx(val), false))
            }
            2 => {
                let (opc, s, sh) = (bits(w, 30, 1), bits(w, 29, 1), bits(w, 22, 1));
                let imm = (bits(w, 10, 12) as u64) << if sh != 0 { 12 } else { 0 };
                let a = reg(sf, rn, true);
                if s != 0 {
                    let f = format!(
                        "{}{}",
                        if opc != 0 { "subs" } else { "adds" },
                        if sf != 0 { "64" } else { "32" }
                    );
                    return op_(set_reg(sf, rd, &format!("{f}(c, {a}, {})", hx(imm)), false));
                }
                op_(set_reg(
                    sf,
                    rd,
                    &format!("{a} {} {}", if opc != 0 { "-" } else { "+" }, hx(imm)),
                    true,
                ))
            }
            4 => {
                let (opc, n) = (bits(w, 29, 2), bits(w, 22, 1));
                if sf == 0 && n != 0 {
                    return unsup("logical imm N=1 32-bit");
                }
                let size = if sf != 0 { 64 } else { 32 };
                let (imm, _) = decode_bit_masks(n, bits(w, 10, 6), bits(w, 16, 6), size, true)?;
                let a = reg(sf, rn, false);
                match opc {
                    0 => op_(set_reg(sf, rd, &format!("{a} & {}", hx(imm)), true)),
                    1 => op_(set_reg(sf, rd, &format!("{a} | {}", hx(imm)), true)),
                    2 => op_(set_reg(sf, rd, &format!("{a} ^ {}", hx(imm)), true)),
                    _ => op_(set_reg(
                        sf,
                        rd,
                        &format!("logic{size}(c, {a} & {})", hx(imm)),
                        false,
                    )),
                }
            }
            5 => {
                let (opc, hw) = (bits(w, 29, 2), bits(w, 21, 2));
                if sf == 0 && hw > 1 {
                    return unsup("movw hw");
                }
                let imm = (bits(w, 5, 16) as u64) << (hw * 16);
                match opc {
                    0 => op_(set_reg(
                        sf,
                        rd,
                        &hx(!imm & ones(if sf != 0 { 64 } else { 32 })),
                        false,
                    )),
                    2 => op_(set_reg(sf, rd, &hx(imm), false)),
                    3 => {
                        let mask = !(0xFFFFu64 << (hw * 16));
                        op_(set_reg(
                            sf,
                            rd,
                            &format!("({} & {}) | {}", x(rd), hx(mask), hx(imm)),
                            false,
                        ))
                    }
                    _ => unsup("movw opc"),
                }
            }
            6 => {
                let (opc, n) = (bits(w, 29, 2), bits(w, 22, 1));
                let (immr, imms) = (bits(w, 16, 6), bits(w, 10, 6));
                let size = if sf != 0 { 64 } else { 32 };
                if n != sf {
                    return unsup("bitfield N");
                }
                let (wmask, tmask) = decode_bit_masks(n, imms, immr, size, false)?;
                let src = reg(sf, rn, false);
                let rot = if immr != 0 {
                    format!("ROR{size}({src}, {immr})")
                } else {
                    src.clone()
                };
                match opc {
                    2 => op_(set_reg(
                        sf,
                        rd,
                        &format!("{rot} & {}", hx(wmask & tmask)),
                        false,
                    )),
                    0 => {
                        let top = format!("(uint{size}_t)(-(int{size}_t)(({src} >> {imms}) & 1))");
                        op_(set_reg(
                            sf,
                            rd,
                            &format!(
                                "({top} & {}) | ({rot} & {})",
                                hx(!tmask & ones(size)),
                                hx(wmask & tmask)
                            ),
                            false,
                        ))
                    }
                    1 => {
                        let dst = reg(sf, rd, false);
                        let bot = format!(
                            "(({dst} & {}) | ({rot} & {}))",
                            hx(!wmask & ones(size)),
                            hx(wmask)
                        );
                        op_(set_reg(
                            sf,
                            rd,
                            &format!(
                                "({dst} & {}) | ({bot} & {})",
                                hx(!tmask & ones(size)),
                                hx(tmask)
                            ),
                            false,
                        ))
                    }
                    _ => unsup("bitfield opc"),
                }
            }
            7 => {
                let (rm, lsb) = (bits(w, 16, 5), bits(w, 10, 6));
                let size = if sf != 0 { 64 } else { 32 };
                if lsb == 0 {
                    return op_(set_reg(sf, rd, &reg(sf, rm, false), false));
                }
                op_(set_reg(
                    sf,
                    rd,
                    &format!(
                        "({} >> {lsb}) | ((uint{size}_t){} << {})",
                        reg(sf, rm, false),
                        reg(sf, rn, false),
                        size - lsb
                    ),
                    false,
                ))
            }
            _ => unsup(format!("dp_imm {w:08x}")),
        }
    }

    // ------------------------------------------------------------ branches / system

    fn branch_sys(&self, w: u32, pc: u64) -> LResult<Insn> {
        if bits(w, 26, 5) == 0b00101 {
            let t = pc_rel(pc, sext(bits(w, 0, 26) as u64, 26) * 4);
            return Ok(Insn::branch(
                if bits(w, 31, 1) != 0 {
                    Kind::Bl
                } else {
                    Kind::B
                },
                t,
            ));
        }
        if bits(w, 25, 7) == 0b0101010 && bits(w, 4, 1) == 0 {
            let t = pc_rel(pc, sext(bits(w, 5, 19) as u64, 19) * 4);
            let cond = bits(w, 0, 4);
            if cond >= 14 {
                return Ok(Insn::branch(Kind::B, t));
            }
            return Ok(Insn::bcond(
                t,
                format!("COND_{}(c)", COND_NAMES[cond as usize].to_uppercase()),
            ));
        }
        if bits(w, 25, 6) == 0b011010 {
            let (sf, nz, rt) = (bits(w, 31, 1), bits(w, 24, 1), bits(w, 0, 5));
            let t = pc_rel(pc, sext(bits(w, 5, 19) as u64, 19) * 4);
            return Ok(Insn::bcond(
                t,
                format!(
                    "{} {} 0",
                    reg(sf, rt, false),
                    if nz != 0 { "!=" } else { "==" }
                ),
            ));
        }
        if bits(w, 25, 6) == 0b011011 {
            let b = (bits(w, 31, 1) << 5) | bits(w, 19, 5);
            let (nz, rt) = (bits(w, 24, 1), bits(w, 0, 5));
            let t = pc_rel(pc, sext(bits(w, 5, 14) as u64, 14) * 4);
            return Ok(Insn::bcond(
                t,
                format!(
                    "(({} >> {b}) & 1) {} 0",
                    x(rt),
                    if nz != 0 { "!=" } else { "==" }
                ),
            ));
        }
        if bits(w, 25, 7) == 0b1101011 {
            let (opc, rn) = (bits(w, 21, 4), bits(w, 5, 5));
            if opc == 2
                && matches!(bits(w, 10, 6), 2 | 3)
                && rn == 31
                && bits(w, 0, 5) == 31
                && bits(w, 16, 5) == 31
            {
                return Ok(Insn::ret());
            }
            if bits(w, 16, 5) != 31 || bits(w, 10, 6) != 0 || bits(w, 0, 5) != 0 {
                return unsup("branch reg variant");
            }
            return match opc {
                0 => Ok(Insn::reg_branch(Kind::Br, x(rn))),
                1 => Ok(Insn::reg_branch(Kind::Blr, x(rn))),
                2 => Ok(Insn::ret()),
                _ => unsup("branch reg opc"),
            };
        }
        if bits(w, 24, 8) == 0b11010100 {
            let (opc, ll, imm) = (bits(w, 21, 3), bits(w, 0, 5), bits(w, 5, 16));
            if opc == 0 && ll == 1 {
                return Ok(Insn::op(format!("rt_svc(c, {imm}, {});", hx(pc))));
            }
            if opc == 1 && ll == 0 {
                return Ok(Insn::trap(format!("rt_brk(c, {imm}, {});", hx(pc))));
            }
            return unsup("exception");
        }
        if bits(w, 22, 10) == 0b1101010100 {
            return self.system(w);
        }
        unsup(format!("branch_sys {w:08x}"))
    }

    fn system(&self, w: u32) -> LResult<Insn> {
        let l = bits(w, 21, 1);
        let (op0, op1, crn, crm, op2, rt) = (
            bits(w, 19, 2),
            bits(w, 16, 3),
            bits(w, 12, 4),
            bits(w, 8, 4),
            bits(w, 5, 3),
            bits(w, 0, 5),
        );
        if op0 == 0 && l == 0 {
            if crn == 2 && op1 == 3 && rt == 31 {
                return Ok(Insn::op("/* hint */"));
            }
            if crn == 3 && op1 == 3 && rt == 31 {
                if op2 == 2 {
                    return Ok(Insn::op("c->excl_addr = ~0ull;"));
                }
                return Ok(Insn::op("__atomic_thread_fence(__ATOMIC_SEQ_CST);"));
            }
            return unsup(format!("pstate/system {w:08x}"));
        }
        if op0 == 1 {
            if l == 0 && op1 == 3 && crn == 7 && crm == 4 && op2 == 1 {
                return Ok(Insn::op(format!(
                    "memset((void*)(uintptr_t)({} & ~63ull), 0, 64);",
                    x(rt)
                )));
            }
            return Ok(Insn::op("/* cache maintenance */"));
        }
        let key = (op0, op1, crn, crm, op2);
        let key_s = format!("({op0}, {op1}, {crn}, {crm}, {op2})");
        if l != 0 {
            let src = match key {
                (3, 3, 13, 0, 3) => "c->tpidrro",
                (3, 3, 13, 0, 2) => "c->tpidr",
                (3, 3, 14, 0, 0) => "RT_TICK_HZ",
                (3, 3, 14, 0, 1) | (3, 3, 14, 0, 2) => "rt_ticks()",
                (3, 3, 4, 2, 0) => "c->nzcv",
                (3, 3, 4, 4, 0) => "c->fpcr",
                (3, 3, 4, 4, 1) => "c->fpsr",
                (3, 3, 0, 0, 7) => "4ull",
                (3, 3, 0, 0, 1) => "0x8444c004ull",
                _ => return unsup(format!("mrs {key_s}")),
            };
            return Ok(Insn::op(set_reg(1, rt, src, false)));
        }
        let v = x(rt);
        let dst = match key {
            (3, 3, 13, 0, 2) => format!("c->tpidr = {v};"),
            (3, 3, 4, 2, 0) => format!("c->nzcv = {v} & 0xf0000000ull;"),
            (3, 3, 4, 4, 0) => format!("c->fpcr = {v} & 0x7c00000ull; rt_fpcr_sync(c);"),
            (3, 3, 4, 4, 1) => format!("c->fpsr = {v} & 0xf800009full;"),
            _ => return unsup(format!("msr {key_s}")),
        };
        Ok(Insn::op(dst))
    }

    // ------------------------------------------------------------ loads / stores

    fn ldst(&self, w: u32, pc: u64) -> LResult<Insn> {
        if bits(w, 26, 1) != 0 {
            return Ok(Insn::op(simd::ldst(w, pc)?));
        }
        if bits(w, 24, 6) == 0b001000 {
            return self.ldst_excl(w);
        }
        if bits(w, 27, 3) == 0b011 && bits(w, 24, 2) == 0 {
            let (opc, rt) = (bits(w, 30, 2), bits(w, 0, 5));
            let addr = pc_rel(pc, sext(bits(w, 5, 19) as u64, 19) * 4);
            return Ok(Insn::op(match opc {
                0 => set_reg(0, rt, &format!("LD32({})", hx(addr)), false),
                1 => set_reg(1, rt, &format!("LD64({})", hx(addr)), false),
                2 => set_reg(
                    1,
                    rt,
                    &format!("(int64_t)(int32_t)LD32({})", hx(addr)),
                    false,
                ),
                _ => "/* prfm */".into(),
            }));
        }
        if bits(w, 27, 3) == 0b101 {
            return self.ldst_pair(w);
        }
        if bits(w, 27, 3) == 0b111 {
            return self.ldst_reg(w);
        }
        if (w & 0xFFE0_0000) == 0xD920_0000 && bits(w, 10, 2) != 0 {
            return Ok(self.stg(w));
        }
        unsup(format!("ldst {w:08x}"))
    }

    fn stg(&self, w: u32) -> Insn {
        let (mode, rn) = (bits(w, 10, 2), bits(w, 5, 5));
        if mode == 2 {
            return Insn::op("/* stg */");
        }
        let off = sext(bits(w, 12, 9) as u64, 9) << 4;
        let dst = if rn == 31 {
            "c->sp".to_string()
        } else {
            format!("c->x[{rn}]")
        };
        Insn::op(format!(
            "{dst} = {} + {}; /* stg */",
            xs(rn),
            hx(off as u64)
        ))
    }

    fn ldst_excl(&self, w: u32) -> LResult<Insn> {
        let (size, o2, l, o1, rs, _o0, rn, rt) = (
            bits(w, 30, 2),
            bits(w, 23, 1),
            bits(w, 22, 1),
            bits(w, 21, 1),
            bits(w, 16, 5),
            bits(w, 15, 1),
            bits(w, 5, 5),
            bits(w, 0, 5),
        );
        let nbits = 8 << size;
        let addr = xs(rn);
        if o1 != 0 {
            if o2 != 0 {
                return Ok(self.cas(size, rs, rt, &addr));
            }
            if bits(w, 31, 1) != 0 {
                return Ok(self.excl_pair(bits(w, 30, 1), l, rs, bits(w, 10, 5), rt, &addr));
            }
            return self.casp(bits(w, 30, 1), rs, rt, &addr);
        }
        if o2 == 0 {
            if l != 0 {
                return Ok(Insn::op(set_reg(
                    1,
                    rt,
                    &format!("ldx{nbits}(c, {addr})"),
                    false,
                )));
            }
            return Ok(Insn::op(format!(
                "{{ uint32_t st = stx{nbits}(c, {addr}, {}); {} }}",
                x(rt),
                set_reg(0, rs, "st", false)
            )));
        }
        if l != 0 {
            return Ok(Insn::op(set_reg(
                1,
                rt,
                &format!("lda{nbits}({addr})"),
                false,
            )));
        }
        Ok(Insn::op(format!("stl{nbits}({addr}, {});", x(rt))))
    }

    fn excl_pair(&self, sz: u32, l: u32, rs: u32, rt2: u32, rt: u32, addr: &str) -> Insn {
        if l != 0 {
            if sz != 0 {
                return Insn::op(format!(
                    "{{ __uint128_t p = ldxp128(c, {addr}); {} {} }}",
                    set_reg(1, rt, "(uint64_t)p", false),
                    set_reg(1, rt2, "(uint64_t)(p >> 64)", false)
                ));
            }
            return Insn::op(format!(
                "{{ uint64_t p = ldx64(c, {addr}); {} {} }}",
                set_reg(0, rt, "p", false),
                set_reg(0, rt2, "p >> 32", false)
            ));
        }
        let st = if sz != 0 {
            format!("stxp128(c, {addr}, {}, {})", x(rt), x(rt2))
        } else {
            format!(
                "stx64(c, {addr}, (uint64_t){} | ((uint64_t){} << 32))",
                wr(rt),
                wr(rt2)
            )
        };
        Insn::op(format!(
            "{{ uint32_t st = {st}; {} }}",
            set_reg(0, rs, "st", false)
        ))
    }

    fn cas(&self, size: u32, rs: u32, rt: u32, addr: &str) -> Insn {
        let t = format!("uint{}_t", 8 << size);
        Insn::op(format!(
            "{{ {t} e = ({t}){}; __atomic_compare_exchange_n(({t} *)(uintptr_t)({addr}), &e, ({t}){}, 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST); {} }}",
            x(rs),
            x(rt),
            set_reg((size == 3) as u32, rs, "e", false)
        ))
    }

    fn casp(&self, sz: u32, rs: u32, rt: u32, addr: &str) -> LResult<Insn> {
        if rs & 1 != 0 || rt & 1 != 0 {
            return unsup("casp odd register");
        }
        let (t, e, v, lo, hi);
        if sz != 0 {
            t = "__uint128_t";
            e = format!("(({t}){} << 64) | {}", x(rs + 1), x(rs));
            v = format!("(({t}){} << 64) | {}", x(rt + 1), x(rt));
            lo = set_reg(1, rs, "(uint64_t)e", false);
            hi = set_reg(1, rs + 1, "(uint64_t)(e >> 64)", false);
        } else {
            t = "uint64_t";
            e = format!("(uint64_t){} | ((uint64_t){} << 32)", wr(rs), wr(rs + 1));
            v = format!("(uint64_t){} | ((uint64_t){} << 32)", wr(rt), wr(rt + 1));
            lo = set_reg(0, rs, "e", false);
            hi = set_reg(0, rs + 1, "e >> 32", false);
        }
        Ok(Insn::op(format!(
            "{{ {t} e = {e}; __atomic_compare_exchange_n(({t} *)(uintptr_t)({addr}), &e, {v}, 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST); {lo} {hi} }}"
        )))
    }

    fn atomic(&self, w: u32) -> LResult<Insn> {
        let (size, a, r, rs, o3, opc, rn, rt) = (
            bits(w, 30, 2),
            bits(w, 23, 1),
            bits(w, 22, 1),
            bits(w, 16, 5),
            bits(w, 15, 1),
            bits(w, 12, 3),
            bits(w, 5, 5),
            bits(w, 0, 5),
        );
        let nbits = 8 << size;
        let t = format!("uint{nbits}_t");
        let p = format!("({t} *)(uintptr_t)({})", xs(rn));
        let v = format!("({t}){}", x(rs));
        let order = [
            "__ATOMIC_RELAXED",
            "__ATOMIC_RELEASE",
            "__ATOMIC_ACQUIRE",
            "__ATOMIC_ACQ_REL",
        ][(a << 1 | r) as usize];
        let sf = (size == 3) as u32;
        let expr = if o3 != 0 {
            if opc == 4 && rs == 31 && a != 0 && r == 0 {
                return Ok(Insn::op(set_reg(
                    sf,
                    rt,
                    &format!("__atomic_load_n({p}, __ATOMIC_ACQUIRE)"),
                    false,
                )));
            }
            if opc != 0 {
                return unsup("ldapr");
            }
            format!("__atomic_exchange_n({p}, {v}, {order})")
        } else if opc < 4 {
            let (f, operand) = match opc {
                0 => ("add", v.clone()),
                1 => ("and", format!("({t})~{}", x(rs))),
                2 => ("xor", v.clone()),
                _ => ("or", v.clone()),
            };
            format!("__atomic_fetch_{f}({p}, {operand}, {order})")
        } else {
            let f = ["smax", "smin", "umax", "umin"][(opc - 4) as usize];
            format!("atomic_{f}{nbits}({}, {v})", xs(rn))
        };
        Ok(Insn::op(set_reg(sf, rt, &expr, false)))
    }

    fn ldst_pair(&self, w: u32) -> LResult<Insn> {
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
        // opc 01 without L is STGP (MTE, ARMv8.5), not a 32-bit STP; opc 01 L with no-allocate (mode 0) is unallocated
        if opc == 1 && (l == 0 || mode == 0) {
            return unsup("pair opc 1 (STGP / unallocated)");
        }
        let scale = if opc == 2 { 3 } else { 2 };
        let off = imm7 << scale;
        let base = xs(rn);
        let post = mode == 1;
        let wb = mode == 1 || mode == 3;
        let ea = if post {
            base.clone()
        } else if off >= 0 {
            format!("{base} + {off}")
        } else {
            format!("{base} - {}", -off)
        };
        let mut lines = vec![format!("uint64_t ea = {ea};")];
        if l != 0 {
            lines.push(
                match opc {
                    2 => "uint64_t a = LD64(ea), b = LD64(ea + 8);",
                    1 => "uint64_t a = (int64_t)(int32_t)LD32(ea), b = (int64_t)(int32_t)LD32(ea + 4);",
                    _ => "uint64_t a = LD32(ea), b = LD32(ea + 4);",
                }
                .to_string(),
            );
        } else if opc == 2 {
            lines.push(format!("ST64(ea, {}); ST64(ea + 8, {});", x(rt), x(rt2)));
        } else {
            lines.push(format!("ST32(ea, {}); ST32(ea + 4, {});", wr(rt), wr(rt2)));
        }
        if wb {
            let dst = if rn == 31 {
                "c->sp".to_string()
            } else {
                format!("c->x[{rn}]")
            };
            if !post {
                lines.push(format!("{dst} = ea;"));
            } else if off >= 0 {
                lines.push(format!("{dst} = ea + {off};"));
            } else {
                lines.push(format!("{dst} = ea - {};", -off));
            }
        }
        if l != 0 {
            lines.push(set_reg(1, rt, "a", false));
            lines.push(set_reg(1, rt2, "b", false));
        }
        Ok(Insn::op(format!("{{ {} }}", lines.join(" "))))
    }

    fn ldst_reg(&self, w: u32) -> LResult<Insn> {
        let (size, opc, rn, rt) = (bits(w, 30, 2), bits(w, 22, 2), bits(w, 5, 5), bits(w, 0, 5));
        let base = xs(rn);
        let mut wb: Option<String> = None;
        let ea;
        if bits(w, 24, 2) == 1 {
            ea = format!("{base} + {}", hx((bits(w, 10, 12) as u64) << size));
        } else {
            let kind = bits(w, 10, 2);
            if bits(w, 21, 1) != 0 {
                if kind == 0 {
                    return self.atomic(w);
                }
                if kind != 2 {
                    return unsup("ldst reg pac");
                }
                let (rm, option, s) = (bits(w, 16, 5), bits(w, 13, 3), bits(w, 12, 1));
                if !matches!(option, 2 | 3 | 6 | 7) {
                    return unsup("ldst reg option");
                }
                ea = format!(
                    "{base} + {}",
                    extend_expr(rm, option, if s != 0 { size } else { 0 })
                );
            } else {
                let imm9 = sext(bits(w, 12, 9) as u64, 9);
                let sign_imm = if imm9 >= 0 {
                    format!("+ {imm9}")
                } else {
                    format!("- {}", -imm9)
                };
                match kind {
                    0 | 2 => ea = format!("{base} {sign_imm}"),
                    1 => {
                        ea = base.clone();
                        wb = Some(format!("{base} {sign_imm}"));
                    }
                    _ => {
                        ea = format!("{base} {sign_imm}");
                        wb = Some("ea".into());
                    }
                }
            }
        }
        let nbits = 8 << size;
        let mut parts;
        let load;
        if opc == 0 {
            parts = vec![format!(
                "ST{nbits}(ea, {});",
                if size == 3 { x(rt) } else { wr(rt) }
            )];
            load = None;
        } else if opc == 1 {
            parts = vec![format!("uint64_t v = LD{nbits}(ea);")];
            load = Some(set_reg(1, rt, "v", false));
        } else if size == 3 {
            return Ok(Insn::op("/* prfm */"));
        } else if size == 2 && opc == 3 {
            return unsup("ldst size2 opc3");
        } else {
            parts = vec![format!(
                "uint64_t v = (uint64_t)(int64_t)(int{nbits}_t)LD{nbits}(ea);"
            )];
            load = Some(set_reg(if opc == 2 { 1 } else { 0 }, rt, "v", false));
        }
        parts.insert(0, format!("uint64_t ea = {ea};"));
        if let Some(wb) = wb {
            let dst = if rn == 31 {
                "c->sp".to_string()
            } else {
                format!("c->x[{rn}]")
            };
            parts.push(format!("{dst} = {wb};"));
        }
        if let Some(l) = load {
            parts.push(l);
        }
        Ok(Insn::op(format!("{{ {} }}", parts.join(" "))))
    }

    // ------------------------------------------------------------ data processing (reg)

    fn dp_reg(&self, w: u32, _pc: u64) -> LResult<Insn> {
        let sf = bits(w, 31, 1);
        let (rd, rn, rm) = (bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5));
        let size = if sf != 0 { 64 } else { 32 };
        let r = |n: u32| reg(sf, n, false);
        let op_ = |s: String| Ok(Insn::op(s));
        if bits(w, 28, 1) == 0 {
            if bits(w, 24, 4) == 0b1010 {
                let (opc, sh, n, amt) = (
                    bits(w, 29, 2),
                    bits(w, 22, 2),
                    bits(w, 21, 1),
                    bits(w, 10, 6),
                );
                if sf == 0 && amt >= 32 {
                    return unsup("logical shift amount");
                }
                let mut b = shift_expr(sf, &r(rm), sh, amt);
                if n != 0 {
                    b = format!("(uint{size}_t)~{b}");
                }
                let a = r(rn);
                let expr = match opc {
                    1 => format!("{a} | {b}"),
                    2 => format!("{a} ^ {b}"),
                    _ => format!("{a} & {b}"),
                };
                if opc == 3 {
                    return op_(set_reg(
                        sf,
                        rd,
                        &format!("logic{size}(c, (uint{size}_t)({expr}))"),
                        false,
                    ));
                }
                return op_(set_reg(sf, rd, &expr, false));
            }
            if bits(w, 24, 4) == 0b1011 {
                let (op, s) = (bits(w, 30, 1), bits(w, 29, 1));
                let (a, b, sp_ok);
                if bits(w, 21, 1) == 0 {
                    let (sh, amt) = (bits(w, 22, 2), bits(w, 10, 6));
                    if sh == 3 {
                        return unsup("add shift ror");
                    }
                    a = r(rn);
                    b = shift_expr(sf, &r(rm), sh, amt);
                    sp_ok = false;
                } else {
                    if bits(w, 22, 2) != 0 {
                        return unsup("add ext opt");
                    }
                    let (option, amt) = (bits(w, 13, 3), bits(w, 10, 3));
                    a = reg(sf, rn, true);
                    let e = extend_expr(rm, option, amt);
                    b = if sf == 0 {
                        format!("(uint32_t)({e})")
                    } else {
                        e
                    };
                    sp_ok = s == 0;
                }
                if s != 0 {
                    let f = format!("{}{size}", if op != 0 { "subs" } else { "adds" });
                    return op_(set_reg(
                        sf,
                        rd,
                        &format!("{f}(c, {a}, (uint{size}_t)({b}))"),
                        false,
                    ));
                }
                return op_(set_reg(
                    sf,
                    rd,
                    &format!("{a} {} {b}", if op != 0 { "-" } else { "+" }),
                    sp_ok,
                ));
            }
            return unsup(format!("dp_reg {w:08x}"));
        }
        let op2 = bits(w, 21, 4);
        if bits(w, 24, 4) == 0b1011 {
            let (op54, op31, o0, ra) = (
                bits(w, 29, 2),
                bits(w, 21, 3),
                bits(w, 15, 1),
                bits(w, 10, 5),
            );
            if op54 != 0 {
                return unsup("dp3 op54");
            }
            let sgn = if o0 != 0 { "-" } else { "+" };
            return match op31 {
                0 => op_(set_reg(
                    sf,
                    rd,
                    &format!("{} {sgn} {} * {}", r(ra), r(rn), r(rm)),
                    false,
                )),
                1 => op_(set_reg(
                    1,
                    rd,
                    &format!(
                        "{} {sgn} (uint64_t)((int64_t)(int32_t){} * (int64_t)(int32_t){})",
                        x(ra),
                        wr(rn),
                        wr(rm)
                    ),
                    false,
                )),
                5 => op_(set_reg(
                    1,
                    rd,
                    &format!(
                        "{} {sgn} (uint64_t){} * (uint64_t){}",
                        x(ra),
                        wr(rn),
                        wr(rm)
                    ),
                    false,
                )),
                2 => op_(set_reg(
                    1,
                    rd,
                    &format!(
                        "(uint64_t)(((__int128)(int64_t){} * (__int128)(int64_t){}) >> 64)",
                        x(rn),
                        x(rm)
                    ),
                    false,
                )),
                6 => op_(set_reg(
                    1,
                    rd,
                    &format!(
                        "(uint64_t)(((unsigned __int128){} * {}) >> 64)",
                        x(rn),
                        x(rm)
                    ),
                    false,
                )),
                _ => unsup("dp3"),
            };
        }
        if op2 == 0 {
            let (op, s) = (bits(w, 30, 1), bits(w, 29, 1));
            let f = format!(
                "{}{}{size}",
                if op != 0 { "sbc" } else { "adc" },
                if s != 0 { "s" } else { "" }
            );
            return op_(set_reg(
                sf,
                rd,
                &format!("{f}(c, {}, {})", r(rn), r(rm)),
                false,
            ));
        }
        if op2 == 2 {
            let (op, imm_form, cond, nzcv) = (
                bits(w, 30, 1),
                bits(w, 11, 1),
                bits(w, 12, 4),
                bits(w, 0, 4),
            );
            let b = if imm_form != 0 { hx(rm as u64) } else { r(rm) };
            let f = format!("{}{size}", if op != 0 { "subs" } else { "adds" });
            return op_(format!(
                "if ({}) (void){f}(c, {}, {b}); else c->nzcv = {};",
                cond_expr(cond),
                r(rn),
                hx((nzcv as u64) << 28)
            ));
        }
        if op2 == 4 {
            let (op, o2, cond) = (bits(w, 30, 1), bits(w, 10, 2), bits(w, 12, 4));
            let b = r(rm);
            let alt = match (op, o2) {
                (0, 0) => b.clone(),
                (0, 1) => format!("{b} + 1"),
                (1, 0) => format!("~{b}"),
                (1, 1) => format!("-{b}"),
                _ => return unsup("csel variant"),
            };
            return op_(set_reg(
                sf,
                rd,
                &format!("({}) ? {} : (uint{size}_t)({alt})", cond_expr(cond), r(rn)),
                false,
            ));
        }
        if op2 == 6 {
            let opcode = bits(w, 10, 6);
            if bits(w, 30, 1) != 0 {
                if bits(w, 16, 5) == 1 && sf != 0 && opcode <= 0x11 {
                    return op_("/* pac */".into());
                }
                if bits(w, 16, 5) != 0 {
                    return unsup("dp1 opcode2");
                }
                let a = r(rn);
                let expr = match opcode {
                    0 => format!("rbit{size}({a})"),
                    1 => format!("rev16_{size}({a})"),
                    2 => {
                        if sf == 0 {
                            format!("__builtin_bswap32({a})")
                        } else {
                            format!("rev32_64({a})")
                        }
                    }
                    3 if sf != 0 => format!("__builtin_bswap64({a})"),
                    4 => format!("clz{size}({a})"),
                    5 => format!("cls{size}({a})"),
                    _ => return unsup("dp1"),
                };
                return op_(set_reg(sf, rd, &expr, false));
            }
            if (16..=23).contains(&opcode) {
                return op_(simd::crc(w)?);
            }
            if opcode == 12 && sf != 0 {
                return op_(set_reg(
                    1,
                    rd,
                    &format!(
                        "((uint64_t)rt_pacga({}, {}) << 32)",
                        reg(1, rn, false),
                        reg(1, rm, true)
                    ),
                    false,
                ));
            }
            let (a, b) = (r(rn), r(rm));
            let expr = match opcode {
                2 => format!("udiv{size}({a}, {b})"),
                3 => format!("sdiv{size}({a}, {b})"),
                8 => format!("{a} << ({b} & {})", size - 1),
                9 => format!("{a} >> ({b} & {})", size - 1),
                10 => format!("(uint{size}_t)((int{size}_t){a} >> ({b} & {}))", size - 1),
                11 => format!("ROR{size}({a}, {b} & {})", size - 1),
                _ => return unsup("dp2"),
            };
            return op_(set_reg(sf, rd, &expr, false));
        }
        unsup(format!("dp_reg {w:08x}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(w: u32, pc: u64) -> String {
        Lifter.lift_or_trap(w, pc).code
    }

    #[test]
    fn integer() {
        // add x0, x1, #0x10
        assert_eq!(
            op(0x91004020, 0x1000),
            "c->x[0] = (uint64_t)(c->x[1] + 0x10ull);"
        );
        // cmp w0, #3 (subs wzr, w0, #3)
        assert_eq!(
            op(0x7100_0c1f, 0x1000),
            "(void)(subs32(c, (uint32_t)c->x[0], 0x3ull));"
        );
        // mov x29, sp (add x29, sp, #0)
        assert_eq!(
            op(0x910003fd, 0x1000),
            "c->x[29] = (uint64_t)(c->sp + 0x0ull);"
        );
        // movz w0, #1
        assert_eq!(op(0x52800020, 0x1000), "c->x[0] = (uint32_t)(0x1ull);");
        // adrp x0, #0x1000 at pc 0x7000001234
        assert_eq!(
            op(0xb0000000, 0x7000001234),
            "c->x[0] = (uint64_t)(0x7000002000ull);"
        );
    }

    #[test]
    fn load_store() {
        // ldr x0, [x1, #8]
        assert_eq!(
            op(0xf9400420, 0),
            "{ uint64_t ea = c->x[1] + 0x8ull; uint64_t v = LD64(ea); c->x[0] = (uint64_t)(v); }"
        );
        // stp x29, x30, [sp, #-16]!
        assert_eq!(
            op(0xa9bf7bfd, 0),
            "{ uint64_t ea = c->sp - 16; ST64(ea, c->x[29]); ST64(ea + 8, c->x[30]); c->sp = ea; }"
        );
        // ldrb w0, [x1], #1
        assert_eq!(
            op(0x38401420, 0),
            "{ uint64_t ea = c->x[1]; uint64_t v = LD8(ea); c->x[1] = c->x[1] + 1; c->x[0] = (uint64_t)(v); }"
        );
    }

    #[test]
    fn branches() {
        let l = Lifter;
        // bl +8
        let i = l.lift(0x94000002, 0x1000).unwrap();
        assert_eq!((i.kind, i.target), (Kind::Bl, 0x1008));
        // b.ne -4
        let i = l.lift(0x54ffffe1, 0x1000).unwrap();
        assert_eq!(
            (i.kind, i.target, i.cond.as_str()),
            (Kind::Bcond, 0xffc, "COND_NE(c)")
        );
        // ret
        assert_eq!(l.lift(0xd65f03c0, 0).unwrap().kind, Kind::Ret);
        // br x16
        let i = l.lift(0xd61f0200, 0).unwrap();
        assert_eq!((i.kind, i.target_reg.as_str()), (Kind::Br, "c->x[16]"));
        // cbz x0, +8
        let i = l.lift(0xb4000040, 0x100).unwrap();
        assert_eq!((i.target, i.cond.as_str()), (0x108, "c->x[0] == 0"));
    }

    #[test]
    fn atomics() {
        // ldaxr w8, [x0]
        assert_eq!(
            op(0x885ffc08, 0),
            "c->x[8] = (uint64_t)(ldx32(c, c->x[0]));"
        );
        // casal x1, x2, [x0]
        assert_eq!(
            op(0xc8e1fc02, 0),
            "{ uint64_t e = (uint64_t)c->x[1]; __atomic_compare_exchange_n((uint64_t *)(uintptr_t)(c->x[0]), &e, (uint64_t)c->x[2], 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST); c->x[1] = (uint64_t)(e); }"
        );
        // ldadd w1, w0, [x2]  (LSE, relaxed)
        assert_eq!(op(0xb8210040, 0), "c->x[0] = (uint32_t)(__atomic_fetch_add((uint32_t *)(uintptr_t)(c->x[2]), (uint32_t)c->x[1], __ATOMIC_RELAXED));");
        // dmb ish
        assert_eq!(
            op(0xd5033bbf, 0),
            "__atomic_thread_fence(__ATOMIC_SEQ_CST);"
        );
        // svc #0x1
        assert_eq!(op(0xd4000021, 0x10), "rt_svc(c, 1, 0x10ull);");
    }

    #[test]
    fn stgp_is_not_a_32_bit_stp() {
        // stgp x11, x14, [x23] (MTE): found by the dynarmic oracle lowered as `stp w11, w14, [x23]`
        assert_eq!(Lifter.lift_or_trap(0x69003aeb, 0x20).kind, Kind::Trap);
        // ldpsw x1, x2, [x3, #8] stays a pair load
        assert_eq!(Lifter.lift_or_trap(0x69410861, 0x20).kind, Kind::Op);
    }

    #[test]
    fn unsupported_becomes_trap() {
        let i = Lifter.lift_or_trap(0x00000000, 0x20);
        assert_eq!(i.kind, Kind::Trap);
        assert_eq!(i.code, "rt_udf(c, 0x20ull);");
    }
}

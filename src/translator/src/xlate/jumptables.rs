//!
//! Static jump-table resolution for computed `br`. See the Python module docstring for the method: the resolver walks
//! back from the `br` over its block and the predecessor blocks (`explore`), evaluates each path forward symbolically
//! (`run`), and enumerates the values the path conditions or the construction allow for the unknowns the target depends
//! on (`solve`). A function is resolved as a whole; tables and register facts are iterated to a common fixpoint, with
//! the work per function capped (MAX_STEPS).
//!
//! The port is result-identical with the Python (checked on every br of Mario Kart World's modules by
//! `tests/jumptables_dump.rs`). Things that matter for that:
//! - expressions are `Rc<Ex>`; Python tuple equality is the derived structural equality, and Python identity (`is`,
//!   used by the fact analysis to see which registers an instruction wrote) is `Rc::ptr_eq`, so every place the
//!   Python returns an existing tuple returns the same `Rc` here, and every place it builds one builds a new `Rc`;
//! - fresh unknowns are numbered per path state in the same order;
//! - the step budget counts exactly the same work; dict orders that reach a result (the table map, the call facts)
//!   keep insertion order;
//! - Python exceptions other than the internal `_Unknown`/`_Exhausted` (reserved bitmasks, negative shift counts,
//!   malformed .eh_frame) propagate out of `resolve`/`is_computed` in Python; here they make `resolve` return None
//!   and `is_computed` false.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use crate::loader::Module;

use crate::xlate::util::{bits, decode_bit_masks};

const MAX_BLOCK: u64 = 64;
const MAX_DEPTH: usize = 4;
const MAX_PATHS: i64 = 32;
const MAX_COUNT: usize = 4096;
const MAX_CANDS: usize = 8;
const MAX_SET: usize = 8;
const MAX_STEPS: i64 = 1 << 19;
const MAX_ROUNDS: usize = 8;

const M32: u64 = 0xFFFF_FFFF;
const M64: u64 = u64::MAX;
const HS: u32 = 2;
const LO: u32 = 3;
const HI: u32 = 8;
const LS: u32 = 9;

/// Python exceptions: `_Unknown`, `_Exhausted`, and anything else (which escapes the Python resolver).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum X {
    Unknown,
    Exhausted,
    Raise,
}
type R<T> = Result<T, X>;

fn mask(sz: u32) -> u64 {
    if sz == 32 { M32 } else { M64 }
}

/// Python `sext` on an unbounded int (does not mask above bit n).
fn psext(v: i128, n: u32) -> i128 {
    if (v >> (n - 1)) & 1 != 0 { v - (1i128 << n) } else { v }
}

// ---------------------------------------------------------------- expressions

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Op {
    Add,
    Sub,
    And,
    Orr,
    Eor,
    Bic,
    Orn,
    Eon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Sh {
    Lsl,
    Lsr,
    Asr,
    Ror,
}
const SHIFT_NAMES: [Sh; 4] = [Sh::Lsl, Sh::Lsr, Sh::Asr, Sh::Ror];

type E = Rc<Ex>;

/// The Python expression tuples (and flags records, which are tuples of the same kind).
#[derive(Debug, PartialEq, Eq, Hash)]
enum Ex {
    C(u64),
    R(u32),
    U(u32),
    V(u64, u32),
    Vw(u64, u32),
    Lo(E),
    Zx(E),
    Sx(E),
    Bin(Op, u32, E, E),
    Shift(Sh, u32, E, u32),
    Bfm(u32, bool, u32, u32, E),
    Bfi(u32, u32, u32, E, E),
    Xt(u32, bool, u32, E),
    Ld(u32, bool, u32, E, u32),
    Oneof(u32, Rc<[u64]>),
    Csel(u32, E, E, u32, E),
    Logic(u32, E),
    Ccmp(u32, E, E, bool, u32, u32, Option<E>),
}

impl Ex {
    /// Tuple-valued elements, in tuple order.
    fn children(&self) -> Vec<&E> {
        match self {
            Ex::C(_) | Ex::R(_) | Ex::U(_) | Ex::V(..) | Ex::Vw(..) | Ex::Oneof(..) => vec![],
            Ex::Lo(e) | Ex::Zx(e) | Ex::Sx(e) | Ex::Logic(_, e) => vec![e],
            Ex::Bin(_, _, a, b) => vec![a, b],
            Ex::Shift(_, _, a, _) | Ex::Bfm(_, _, _, _, a) | Ex::Xt(_, _, _, a) | Ex::Ld(_, _, _, a, _) => vec![a],
            Ex::Bfi(_, _, _, a, b) => vec![a, b],
            Ex::Csel(_, x, y, _, s) => vec![x, y, s],
            Ex::Ccmp(_, a, b, _, _, _, p) => match p {
                Some(p) => vec![a, b, p],
                None => vec![a, b],
            },
        }
    }
    fn cval(&self) -> Option<u64> {
        if let Ex::C(v) = self { Some(*v) } else { None }
    }
    fn is_c(&self) -> bool {
        matches!(self, Ex::C(_))
    }
}

fn mk(e: Ex) -> E {
    Rc::new(e)
}

fn c(v: i128, sz: u32) -> E {
    mk(Ex::C((v & mask(sz) as i128) as u64))
}

fn c64(v: i128) -> E {
    c(v, 64)
}

fn lo(e: &E) -> E {
    match &**e {
        Ex::C(v) => mk(Ex::C(v & M32)),
        Ex::Zx(x) | Ex::Sx(x) => x.clone(),
        Ex::Oneof(_, vs) if vs.iter().max().copied().unwrap_or(0) <= M32 => e.clone(),
        _ => mk(Ex::Lo(e.clone())),
    }
}

fn zx(e: &E) -> E {
    if matches!(**e, Ex::C(_) | Ex::Oneof(..)) { e.clone() } else { mk(Ex::Zx(e.clone())) }
}

fn sx(e: &E) -> E {
    match **e {
        Ex::C(v) => c64(psext(v as i128, 32)),
        _ => mk(Ex::Sx(e.clone())),
    }
}

fn bin_eval(op: Op, a: u64, b: u64, sz: u32) -> u64 {
    let m = mask(sz);
    match op {
        Op::Add => ((a as u128 + b as u128) & m as u128) as u64,
        Op::Sub => a.wrapping_sub(b) & m,
        Op::And => a & b,
        Op::Orr => a | b,
        Op::Eor => a ^ b,
        Op::Bic => a & !b & m,
        Op::Orn => (a | !b) & m,
        Op::Eon => (a ^ !b) & m,
    }
}

/// _SH with 0 < n < sz.
fn sh_eval(op: Sh, a: u64, n: u32, sz: u32) -> u64 {
    let m = mask(sz) as u128;
    match op {
        Sh::Lsl => (((a as u128) << n) & m) as u64,
        Sh::Lsr => a >> n,
        Sh::Asr => ((psext(a as i128, sz) >> n) & m as i128) as u64,
        Sh::Ror => ((((a as u128) >> n) | ((a as u128) << (sz - n))) & m) as u64,
    }
}

fn binop(op: Op, sz: u32, a: &E, b: &E) -> E {
    if let (Ex::C(x), Ex::C(y)) = (&**a, &**b) {
        return c(bin_eval(op, *x, *y, sz) as i128, sz);
    }
    if matches!(op, Op::Add | Op::Orr | Op::Eor) && a.cval() == Some(0) {
        return b.clone();
    }
    if matches!(op, Op::Add | Op::Sub | Op::Orr | Op::Eor) && b.cval() == Some(0) {
        return a.clone();
    }
    if op == Op::And {
        if let Some(bv) = b.cval() {
            if sz == 64 && bv <= M32 {
                return if bv != M32 { zx(&binop(Op::And, 32, &lo(a), b)) } else { zx(&lo(a)) };
            }
            if bv == mask(sz) {
                return a.clone();
            }
        }
    }
    mk(Ex::Bin(op, sz, a.clone(), b.clone()))
}

fn shift(op: Sh, sz: u32, a: &E, n: i64) -> E {
    let n = n.rem_euclid(sz as i64) as u32;
    if n == 0 {
        return a.clone();
    }
    if let Ex::C(v) = **a {
        return c(sh_eval(op, v, n, sz) as i128, sz);
    }
    mk(Ex::Shift(op, sz, a.clone(), n))
}

fn bfm(sz: u32, signed: bool, immr: u32, imms: u32, x: &E) -> R<E> {
    if !signed && immr == 0 && imms == 31 && sz == 64 {
        return Ok(zx(&lo(x)));
    }
    if signed && immr == 0 && imms == 31 && sz == 64 {
        return Ok(sx(&lo(x)));
    }
    if imms == sz - 1 {
        return Ok(shift(if signed { Sh::Asr } else { Sh::Lsr }, sz, x, immr as i64));
    }
    if !signed && imms + 1 == immr {
        return Ok(shift(Sh::Lsl, sz, x, sz as i64 - immr as i64));
    }
    if !signed && immr == 0 {
        return Ok(binop(Op::And, sz, x, &c64((1i128 << (imms + 1)) - 1)));
    }
    if !signed && sz == 64 && immr <= imms && imms < 32 {
        return Ok(zx(&bfm(32, false, immr, imms, &lo(x))?));
    }
    match **x {
        Ex::C(v) => Ok(c(bfm_eval(sz, signed, immr, imms, v)? as i128, sz)),
        _ => Ok(mk(Ex::Bfm(sz, signed, immr, imms, x.clone()))),
    }
}

fn bfi(sz: u32, immr: u32, imms: u32, dst: &E, src: &E) -> R<E> {
    if let (Ex::C(d), Ex::C(s)) = (&**dst, &**src) {
        return Ok(c(bfi_eval(sz, immr, imms, *d as u128, *s as u128)? as i128, sz));
    }
    Ok(mk(Ex::Bfi(sz, immr, imms, dst.clone(), src.clone())))
}

fn bfi_eval(sz: u32, immr: u32, imms: u32, d: u128, v: u128) -> R<u64> {
    let (wmask, tmask) = decode_bit_masks(if sz == 64 { 1 } else { 0 }, imms, immr, sz, false).map_err(|_| X::Raise)?;
    let (wmask, tmask, m) = (wmask as u128, tmask as u128, mask(sz) as u128);
    let r = if immr != 0 {
        if immr > sz {
            return Err(X::Raise); // negative shift count in Python
        }
        ((v >> immr) | if sz - immr >= 128 { 0 } else { v << (sz - immr) }) & m
    } else {
        v
    };
    let bot = (d & !wmask & m) | (r & wmask);
    Ok(((d & !tmask & m) | (bot & tmask)) as u64)
}

fn extend(e: &E, option: u32, sz: u32) -> E {
    if option == 3 || option == 7 {
        return if sz == 64 { e.clone() } else { lo(e) };
    }
    if option == 2 {
        return if sz == 64 { zx(&lo(e)) } else { lo(e) };
    }
    if option == 6 {
        return if sz == 64 { sx(&lo(e)) } else { lo(e) };
    }
    let (b, signed) = (if option & 3 == 0 { 8 } else { 16 }, option >= 4);
    let v = lo(e);
    if let Ex::C(vv) = *v {
        let x = vv & ((1u64 << b) - 1);
        return c(if signed { psext(x as i128, b) } else { x as i128 }, sz);
    }
    mk(Ex::Xt(b, signed, sz, v))
}

fn bfm_eval(sz: u32, signed: bool, immr: u32, imms: u32, v: u64) -> R<u64> {
    let m = mask(sz) as i128;
    let v = v as u128;
    if imms >= immr {
        let w = imms - immr + 1;
        let f = (v >> immr) & ((1u128 << w) - 1);
        let f = if signed { psext(f as i128, w) } else { f as i128 };
        return Ok((f & m) as u64);
    }
    let f = (v & ((1u128 << (imms + 1)) - 1)) as i128;
    let f = if signed { psext(f, imms + 1) } else { f };
    if immr > sz {
        return Err(X::Raise);
    }
    Ok((f.wrapping_shl(sz - immr) & m) as u64)
}

// ---------------------------------------------------------------- symbolic execution of one path

#[derive(Clone)]
enum Cond {
    Flags(E, u32, bool),
    Zero(u32, E, bool),
    Bit(E, u32, bool),
}

struct State {
    regs: Vec<E>,
    flags: Option<E>,
    epoch: u32,
    mem: Vec<(E, u64, u64, E)>,
    conds: Vec<Cond>,
    fresh: u32,
}

impl State {
    fn new() -> State {
        State { regs: (0..32).map(|r| mk(Ex::R(r))).collect(), flags: None, epoch: 0, mem: vec![], conds: vec![], fresh: 0 }
    }
    fn unknown(&mut self) -> E {
        self.fresh += 1;
        mk(Ex::U(self.fresh))
    }
    fn get(&self, r: u32, sz: u32, sp: bool) -> E {
        if r == 31 && !sp {
            return c64(0);
        }
        let e = &self.regs[r as usize];
        if sz == 64 { e.clone() } else { lo(e) }
    }
    fn set(&mut self, r: u32, sz: u32, e: E, sp: bool) {
        if r == 31 && !sp {
            return;
        }
        self.regs[r as usize] = if sz == 64 { e } else { zx(&e) };
    }
    fn kill(&mut self, r: u32, sz: u32, sp: bool) {
        if r != 31 || sp {
            let u = self.unknown();
            self.regs[r as usize] = if sz == 64 { u } else { mk(Ex::Zx(u)) };
        }
    }
    fn clobber(&mut self) {
        self.epoch += 1;
        self.mem.clear();
    }
    fn kill_all(&mut self) {
        for r in 0..32 {
            self.regs[r] = self.unknown();
        }
        self.flags = None;
        self.clobber();
    }
    fn call(&mut self) {
        for r in (0..19).chain([30]) {
            self.regs[r] = self.unknown();
        }
        self.flags = None;
        self.clobber();
    }
    fn store(&mut self, addr: &E, size: u64, value: E) {
        let (base, off) = split(addr);
        self.epoch += 1;
        self.mem.retain(|m| {
            m.0 == base && ((m.1 as u128 + m.2 as u128) <= off as u128 || (off as u128 + size as u128) <= m.1 as u128)
        });
        self.mem.push((base, off, size, value));
    }
    fn load(&self, size: u32, signed: bool, sz: u32, addr: &E) -> E {
        let (base, off) = split(addr);
        for (b, o, n, v) in &self.mem {
            if *b == base && *o == off && *n == size as u64 {
                if size == 8 {
                    return v.clone();
                }
                if size == 4 {
                    return if signed && sz == 64 { sx(v) } else { v.clone() };
                }
                if signed {
                    if let Ex::C(vv) = **v {
                        return c(psext((vv & ((1u64 << (8 * size)) - 1)) as i128, 8 * size), sz);
                    }
                    return mk(Ex::Xt(8 * size, true, sz, v.clone()));
                }
                return binop(Op::And, 32, v, &c64((1i128 << (8 * size)) - 1));
            }
        }
        mk(Ex::Ld(size, signed, sz, addr.clone(), self.epoch))
    }
}

fn split(addr: &E) -> (E, u64) {
    if let Ex::Bin(Op::Add, _, a, b) = &**addr {
        if let Ex::C(v) = **b {
            return (a.clone(), v);
        }
    }
    if let Ex::C(v) = **addr {
        return (c64(0), v);
    }
    (addr.clone(), 0)
}

fn sx_bits(w: u32, lo_: u32, n: u32) -> i128 {
    psext(bits(w, lo_, n) as i128, n)
}

/// Base, address and written-back base of the load/store register forms.
fn addr_imm(st: &State, w: u32, scale: u32) -> Option<(E, E, Option<E>)> {
    let rn = bits(w, 5, 5);
    let base = st.get(rn, 64, true);
    if bits(w, 24, 1) != 0 {
        let a = binop(Op::Add, 64, &base, &c64((bits(w, 10, 12) as i128) << scale));
        return Some((base, a, None));
    }
    if bits(w, 21, 1) != 0 {
        if bits(w, 10, 2) != 2 {
            return None;
        }
        let mut off = extend(&st.get(bits(w, 16, 5), 64, false), bits(w, 13, 3), 64);
        if bits(w, 12, 1) != 0 {
            off = shift(Sh::Lsl, 64, &off, scale as i64);
        }
        let a = binop(Op::Add, 64, &base, &off);
        return Some((base, a, None));
    }
    let imm = sx_bits(w, 12, 9);
    let mode = bits(w, 10, 2);
    let new = binop(Op::Add, 64, &base, &c64(imm));
    match mode {
        1 => Some((base.clone(), base, Some(new))),
        3 => Some((base, new.clone(), Some(new))),
        _ => Some((base, new, None)),
    }
}

fn ldst(st: &mut State, w: u32, pc: u64) {
    let (rt, rn) = (bits(w, 0, 5), bits(w, 5, 5));
    if (w & 0x3B00_0000) == 0x1800_0000 {
        let (opc, v) = (bits(w, 30, 2), bits(w, 26, 1));
        if v == 0 && opc != 3 {
            let a = c64(pc as i128 + sx_bits(w, 5, 19) * 4);
            let e = if opc != 0 {
                st.load(if opc == 1 { 8 } else { 4 }, opc == 2, 64, &a)
            } else {
                zx(&st.load(4, false, 32, &a))
            };
            st.set(rt, 64, e, false);
        }
        return;
    }
    if (w & 0x3A00_0000) == 0x2800_0000 {
        let (mode, load, v, opc) = (bits(w, 23, 2), bits(w, 22, 1), bits(w, 26, 1), bits(w, 30, 2));
        let scale = 2 + if v == 0 { bits(w, 31, 1) } else { opc };
        let base = st.get(rn, 64, true);
        let new = binop(Op::Add, 64, &base, &c64(sx_bits(w, 15, 7) << scale));
        let addr = if mode == 1 { base } else { new.clone() };
        let rt2 = bits(w, 10, 5);
        if load != 0 && v == 0 {
            let sz = if opc != 0 { 64 } else { 32 };
            st.kill(rt, sz, false);
            st.kill(rt2, sz, false);
        } else if load == 0 && v == 0 && opc != 1 {
            let n = 1u64 << scale;
            let (a, b) = (st.get(rt, 8 * n as u32, false), st.get(rt2, 8 * n as u32, false));
            st.store(&addr, n, a);
            st.store(&binop(Op::Add, 64, &addr, &c64(n as i128)), n, b);
        } else if load == 0 {
            st.clobber();
        }
        if mode == 1 || mode == 3 {
            st.set(rn, 64, new, true);
        }
        return;
    }
    let m = w & 0x3B20_0C00;
    if [0x3800_0000, 0x3800_0400, 0x3800_0C00, 0x3820_0800].contains(&m) || (w & 0x3B00_0000) == 0x3900_0000 {
        let (size, v, opc) = (bits(w, 30, 2), bits(w, 26, 1), bits(w, 22, 2));
        if v != 0 {
            let scale = size | (bits(w, 23, 1) << 2);
            let Some(r) = addr_imm(st, w, scale) else {
                st.kill_all();
                return;
            };
            if opc & 1 == 0 {
                st.clobber();
            }
            if let Some(n) = r.2 {
                st.set(rn, 64, n, true);
            }
            return;
        }
        let Some((_, addr, new)) = addr_imm(st, w, size) else {
            st.kill_all();
            return;
        };
        if opc == 0 {
            let val = st.get(rt, if size == 3 { 64 } else { 32 }, false);
            st.store(&addr, 1 << size, val);
        } else if opc == 1 {
            let e = if size < 3 { zx(&st.load(1 << size, false, 32, &addr)) } else { st.load(8, false, 64, &addr) };
            st.set(rt, 64, e, false);
        } else if size == 3 {
            // prfm
        } else if opc == 2 {
            let e = st.load(1 << size, true, 64, &addr);
            st.set(rt, 64, e, false);
        } else if size < 2 {
            let e = st.load(1 << size, true, 32, &addr);
            st.set(rt, 32, e, false);
        } else {
            st.kill_all();
            return;
        }
        if let Some(n) = new {
            st.set(rn, 64, n, true);
        }
        return;
    }
    if (w & 0xBE00_0000) == 0x0C00_0000 {
        if bits(w, 22, 1) == 0 {
            st.clobber();
        }
        if bits(w, 23, 1) != 0 {
            st.kill(rn, 64, true);
        }
        return;
    }
    let sz = if bits(w, 30, 2) == 3 { 64 } else { 32 };
    if (w & 0x3F00_0000) == 0x0800_0000 {
        st.kill(rt, sz, false);
        st.kill(bits(w, 16, 5), 32, false);
        st.kill(bits(w, 10, 5), sz, false);
        st.clobber();
        return;
    }
    if (w & 0x3B20_0C00) == 0x3820_0000 {
        st.kill(rt, sz, false);
        st.clobber();
        return;
    }
    st.kill_all();
}

fn dp_imm(st: &mut State, w: u32, pc: u64) -> R<()> {
    let (sf, rd, rn) = (bits(w, 31, 1), bits(w, 0, 5), bits(w, 5, 5));
    let sz = if sf != 0 { 64 } else { 32 };
    let op = bits(w, 23, 3);
    if op == 0 || op == 1 {
        let imm = psext((((bits(w, 5, 19) as i128) << 2) | bits(w, 29, 2) as i128) as i128, 21);
        let v = if sf != 0 { ((pc & !0xFFF) as i128) + (imm << 12) } else { pc as i128 + imm };
        st.set(rd, 64, c64(v), false);
    } else if op == 2 {
        let (opc, s) = (bits(w, 30, 1), bits(w, 29, 1));
        let imm = c((bits(w, 10, 12) as i128) << if bits(w, 22, 1) != 0 { 12 } else { 0 }, sz);
        let a = st.get(rn, sz, true);
        let o = if opc != 0 { Op::Sub } else { Op::Add };
        let r = binop(o, sz, &a, &imm);
        if s != 0 {
            st.flags = Some(mk(Ex::Bin(o, sz, a, imm)));
        }
        st.set(rd, sz, r, s == 0);
    } else if op == 4 {
        let (opc, n) = (bits(w, 29, 2), bits(w, 22, 1));
        if sf == 0 && n != 0 {
            st.kill_all();
            return Ok(());
        }
        let (imm, _) = decode_bit_masks(n, bits(w, 10, 6), bits(w, 16, 6), sz, true).map_err(|_| X::Raise)?;
        let o = [Op::And, Op::Orr, Op::Eor, Op::And][opc as usize];
        let r = binop(o, sz, &st.get(rn, sz, false), &c(imm as i128, sz));
        if opc == 3 {
            st.flags = Some(mk(Ex::Logic(sz, r.clone())));
        }
        st.set(rd, sz, r, opc != 3);
    } else if op == 5 {
        let (opc, hw) = (bits(w, 29, 2), bits(w, 21, 2));
        let imm = (bits(w, 5, 16) as i128) << (hw * 16);
        if opc == 0 {
            st.set(rd, sz, c(!imm, sz), false);
        } else if opc == 2 {
            st.set(rd, sz, c(imm, sz), false);
        } else if opc == 3 {
            let old = st.get(rd, sz, false);
            let e = match *old {
                Ex::C(v) => c((v as i128 & !(0xFFFFi128 << (hw * 16))) | imm, sz),
                _ => st.unknown(),
            };
            st.set(rd, sz, e, false);
        } else {
            st.kill_all();
        }
    } else if op == 6 {
        let opc = bits(w, 29, 2);
        if opc == 1 {
            let e = bfi(sz, bits(w, 16, 6), bits(w, 10, 6), &st.get(rd, sz, false), &st.get(rn, sz, false))?;
            st.set(rd, sz, e, false);
        } else if opc == 3 {
            st.kill(rd, sz, false);
        } else {
            let e = bfm(sz, opc == 0, bits(w, 16, 6), bits(w, 10, 6), &st.get(rn, sz, false))?;
            st.set(rd, sz, e, false);
        }
    } else if bits(w, 16, 5) == rn {
        let e = shift(Sh::Ror, sz, &st.get(rn, sz, false), bits(w, 10, 6) as i64);
        st.set(rd, sz, e, false);
    } else {
        st.kill(rd, sz, false);
    }
    Ok(())
}

fn dp_reg(st: &mut State, w: u32) {
    let (sf, rd, rn, rm) = (bits(w, 31, 1), bits(w, 0, 5), bits(w, 5, 5), bits(w, 16, 5));
    let sz = if sf != 0 { 64 } else { 32 };
    if (w & 0x1F00_0000) == 0x0A00_0000 {
        let (opc, n) = (bits(w, 29, 2), bits(w, 21, 1));
        let b = shift(SHIFT_NAMES[bits(w, 22, 2) as usize], sz, &st.get(rm, sz, false), bits(w, 10, 6) as i64);
        let name = [[Op::And, Op::Orr, Op::Eor, Op::And], [Op::Bic, Op::Orn, Op::Eon, Op::Bic]][n as usize][opc as usize];
        let r = binop(name, sz, &st.get(rn, sz, false), &b);
        if opc == 3 {
            st.flags = Some(mk(Ex::Logic(sz, r.clone())));
        }
        st.set(rd, sz, r, false);
    } else if (w & 0x1F20_0000) == 0x0B00_0000 {
        let (opc, s, typ) = (bits(w, 30, 1), bits(w, 29, 1), bits(w, 22, 2));
        if typ == 3 {
            st.kill_all();
            return;
        }
        let a = st.get(rn, sz, false);
        let b = shift(SHIFT_NAMES[typ as usize], sz, &st.get(rm, sz, false), bits(w, 10, 6) as i64);
        let o = if opc != 0 { Op::Sub } else { Op::Add };
        if s != 0 {
            st.flags = Some(mk(Ex::Bin(o, sz, a.clone(), b.clone())));
        }
        st.set(rd, sz, binop(o, sz, &a, &b), false);
    } else if (w & 0x1F20_0000) == 0x0B20_0000 {
        let (opc, s) = (bits(w, 30, 1), bits(w, 29, 1));
        let a = st.get(rn, sz, true);
        let b = shift(Sh::Lsl, sz, &extend(&st.get(rm, 64, false), bits(w, 13, 3), sz), bits(w, 10, 3) as i64);
        let o = if opc != 0 { Op::Sub } else { Op::Add };
        if s != 0 {
            st.flags = Some(mk(Ex::Bin(o, sz, a.clone(), b.clone())));
        }
        st.set(rd, sz, binop(o, sz, &a, &b), s == 0);
    } else if (w & 0x1FE0_0000) == 0x1A40_0000 && bits(w, 29, 1) != 0 {
        let b = if bits(w, 11, 1) != 0 { c(rm as i128, sz) } else { st.get(rm, sz, false) };
        let prev = st.flags.take();
        st.flags = Some(mk(Ex::Ccmp(sz, st.get(rn, sz, false), b, bits(w, 30, 1) == 0, bits(w, 12, 4), bits(w, 0, 4), prev)));
    } else if (w & 0x1FE0_0000) == 0x1A00_0000 && (w & 0xFC00) != 0 {
        st.flags = None;
    } else if (w & 0x1FE0_0800) == 0x1A80_0000 && bits(w, 29, 1) == 0 {
        let (x, mut y) = (st.get(rn, sz, false), st.get(rm, sz, false));
        if bits(w, 10, 1) != 0 {
            y = if bits(w, 30, 1) == 0 { binop(Op::Add, sz, &y, &c64(1)) } else { binop(Op::Sub, sz, &c64(0), &y) };
        } else if bits(w, 30, 1) != 0 {
            y = binop(Op::Eor, sz, &y, &c64(mask(sz) as i128));
        }
        let sel = match &st.flags {
            Some(f) => f.clone(),
            None => st.unknown(),
        };
        let e = if x == y { x } else { mk(Ex::Csel(sz, x, y, bits(w, 12, 4), sel)) };
        st.set(rd, sz, e, false);
    } else {
        st.kill(rd, sz, false);
        if bits(w, 29, 1) != 0 {
            st.flags = None;
        }
    }
}

fn fp_simd(st: &mut State, w: u32) {
    if (w & 0x5F20_3C00) == 0x1E20_2000 || (w & 0x5F20_0C00) == 0x1E20_0400 {
        st.flags = None;
    } else if (w & 0x5F20_0000) == 0x1E00_0000 || (w & 0x5F20_FC00) == 0x1E20_0000 {
        if [0, 1, 4, 5, 6].contains(&bits(w, 16, 3)) {
            st.kill(bits(w, 0, 5), if bits(w, 31, 1) != 0 { 64 } else { 32 }, false);
        }
    } else if (w & 0x9FE0_8400) == 0x0E00_0400 && [5, 7].contains(&bits(w, 11, 4)) {
        st.kill(bits(w, 0, 5), if bits(w, 30, 1) != 0 { 64 } else { 32 }, false);
    }
}

fn step(st: &mut State, w: u32, pc: u64) -> R<()> {
    let op0 = bits(w, 25, 4);
    if (op0 & 0b1110) == 0b1000 {
        dp_imm(st, w, pc)?;
    } else if (op0 & 0b0101) == 0b0100 {
        ldst(st, w, pc);
    } else if (op0 & 0b0111) == 0b0101 {
        dp_reg(st, w);
    } else if (op0 & 0b0111) == 0b0111 {
        fp_simd(st, w);
    } else if (w & 0xFC00_0000) == 0x9400_0000 || (w & 0xFFFF_FC1F) == 0xD63F_0000 {
        st.call();
    } else if (w & 0xFFFF_F01F) == 0xD503_201F || (w & 0xFFFF_F01F) == 0xD503_301F {
    } else if (w & 0xFFF0_0000) == 0xD530_0000 {
        st.kill(bits(w, 0, 5), 64, false);
    } else if (w & 0xFFF0_0000) == 0xD510_0000 {
        st.flags = None;
    } else if (w & 0xFFF8_0000) == 0xD508_0000 {
        st.clobber();
    } else {
        st.kill_all();
    }
    Ok(())
}

fn edge(st: &mut State, w: u32, taken: bool) {
    if (w & 0xFF00_0010) == 0x5400_0000 {
        let cond = bits(w, 0, 4);
        if cond < 14 {
            if let Some(f) = &st.flags {
                st.conds.push(Cond::Flags(f.clone(), cond, taken));
            }
        }
    } else if (w & 0x7E00_0000) == 0x3400_0000 {
        let sz = if bits(w, 31, 1) != 0 { 64 } else { 32 };
        let x = st.get(bits(w, 0, 5), sz, false);
        st.conds.push(Cond::Zero(sz, x, taken != (bits(w, 24, 1) != 0)));
    } else if (w & 0x7E00_0000) == 0x3600_0000 {
        let bit = (bits(w, 31, 1) << 5) | bits(w, 19, 5);
        let x = st.get(bits(w, 0, 5), 64, false);
        st.conds.push(Cond::Bit(x, bit, taken == (bits(w, 24, 1) != 0)));
    }
}

// ---------------------------------------------------------------- facts

#[derive(Clone, PartialEq, Eq, Debug)]
enum Fact {
    Z,
    Set(Rc<[u64]>),
    E(E),
}
type Facts = BTreeMap<u32, Fact>;

fn init(st: &mut State, facts: &Facts) {
    for (&r, v) in facts {
        st.regs[r as usize] = match v {
            Fact::Z => mk(Ex::Zx(mk(Ex::Lo(mk(Ex::R(r)))))),
            Fact::Set(s) => mk(Ex::Oneof(r, s.clone())),
            Fact::E(e) => e.clone(),
        };
    }
}

fn clean(e: &E, depth: u32) -> bool {
    match &**e {
        Ex::C(_) | Ex::V(..) | Ex::Vw(..) => true,
        Ex::R(_) | Ex::U(_) | Ex::Ld(..) | Ex::Oneof(..) => false,
        _ if depth == 0 => false,
        x => x.children().into_iter().all(|y| clean(y, depth - 1)),
    }
}

fn names(e: &E, a: u64) -> bool {
    match &**e {
        Ex::V(b, _) | Ex::Vw(b, _) => *b == a,
        Ex::C(_) | Ex::R(_) | Ex::U(_) => false,
        x => x.children().into_iter().any(|y| names(y, a)),
    }
}

fn facts_of(st: &State) -> Facts {
    let mut out = Facts::new();
    for (r, e) in st.regs[..31].iter().enumerate() {
        if clean(e, 8) {
            out.insert(r as u32, Fact::E(e.clone()));
        } else if let Ex::Oneof(_, s) = &**e {
            out.insert(r as u32, Fact::Set(s.clone()));
        } else if let Ex::Zx(_) = &**e {
            out.insert(r as u32, Fact::Z);
        }
    }
    out
}

fn values(v: &Fact) -> Option<Vec<u64>> {
    match v {
        Fact::E(e) => e.cval().map(|x| vec![x]),
        Fact::Set(s) => Some(s.to_vec()),
        Fact::Z => None,
    }
}

fn fzext(v: &Fact) -> bool {
    match v {
        Fact::Z => true,
        Fact::E(e) if matches!(**e, Ex::Zx(_)) => true,
        _ => values(v).is_some_and(|s| s.iter().max().copied().unwrap_or(0) <= M32),
    }
}

fn meet(a: Option<&Facts>, b: Option<&Facts>) -> Option<Facts> {
    let (a, b) = match (a, b) {
        (None, None) => return None,
        (None, Some(b)) => return Some(b.clone()),
        (Some(a), None) => return Some(a.clone()),
        (Some(a), Some(b)) => (a, b),
    };
    let mut out = Facts::new();
    for (r, v) in a {
        let Some(u) = b.get(r) else { continue };
        if u == v {
            out.insert(*r, v.clone());
            continue;
        }
        if *u != Fact::Z && *v != Fact::Z {
            if let (Some(x), Some(y)) = (values(u), values(v)) {
                let s: BTreeSet<u64> = x.into_iter().chain(y).collect();
                if s.len() <= MAX_SET {
                    out.insert(*r, Fact::Set(s.into_iter().collect::<Vec<_>>().into()));
                    continue;
                }
            }
        }
        if fzext(u) && fzext(v) {
            out.insert(*r, Fact::Z);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------- enumeration

type Nzcv = (u64, bool, bool, u64);

fn holds(cond: u32, f: Nzcv) -> bool {
    let (n, z, c, v) = f;
    let r = match cond >> 1 {
        0 => z,
        1 => c,
        2 => n != 0,
        3 => v != 0,
        4 => c && !z,
        5 => n == v,
        6 => n == v && !z,
        _ => true,
    };
    r != (cond & 1 != 0 && cond != 15)
}

fn imm_nzcv(imm: u32) -> Nzcv {
    ((imm >> 3 & 1) as u64, imm >> 2 & 1 != 0, imm >> 1 & 1 != 0, (imm & 1) as u64)
}

fn nzcv(kind: Option<Op>, sz: u32, x: u64, y: u64) -> Nzcv {
    let (m, top) = (mask(sz) as i128, sz - 1);
    let (x, y) = (x as i128, y as i128);
    match kind {
        None => (((x >> top) & 1) as u64, x == 0, false, 0),
        Some(Op::Add) => {
            let r = (x + y) & m;
            (((r >> top) & 1) as u64, r == 0, x + y > m, (((!(x ^ y)) & (x ^ r)) >> top & 1) as u64)
        }
        _ => {
            let r = (x - y) & m;
            (((r >> top) & 1) as u64, r == 0, x >= y, (((x ^ y) & (x ^ r)) >> top & 1) as u64)
        }
    }
}

fn swap(c: u32) -> Option<u32> {
    match c {
        HS => Some(LS),
        LS => Some(HS),
        HI => Some(LO),
        LO => Some(HI),
        10 => Some(13),
        13 => Some(10),
        11 => Some(12),
        12 => Some(11),
        0 => Some(0),
        1 => Some(1),
        _ => None,
    }
}

fn bounds(st: &State) -> Vec<(E, Vec<u64>)> {
    let mut out = vec![];
    for rec in &st.conds {
        let (f, cond, hold) = match rec {
            Cond::Zero(_, x, h) => {
                if *h {
                    out.push((x.clone(), vec![0]));
                }
                continue;
            }
            Cond::Bit(..) => continue,
            Cond::Flags(f, cond, h) => (f, *cond, *h),
        };
        let (sz, a, b) = match &**f {
            Ex::Ccmp(sz, a, b, add, _, imm, _) => {
                if *add || holds(cond, imm_nzcv(*imm)) == hold {
                    continue;
                }
                (*sz, a.clone(), b.clone())
            }
            Ex::Bin(Op::Sub, sz, a, b) => (*sz, a.clone(), b.clone()),
            _ => continue,
        };
        let _ = sz;
        let mut cc = Some(if hold { cond } else { cond ^ 1 });
        let (mut a, mut b) = (a, b);
        if a.is_c() && !b.is_c() {
            std::mem::swap(&mut a, &mut b);
            cc = swap(cc.unwrap());
        }
        if !b.is_c() || a.is_c() {
            continue;
        }
        let n = b.cval().unwrap() as u128;
        let len = match cc {
            Some(LS) => Some(n + 1),
            Some(LO) => Some(n),
            Some(0) => Some(1),
            _ => None,
        };
        if let Some(len) = len {
            if len > 0 && len <= MAX_COUNT as u128 {
                let vals: Vec<u64> = if cc == Some(0) { vec![n as u64] } else { (0..len as u64).collect() };
                let key = if matches!(*a, Ex::Zx(_) | Ex::Sx(_)) { lo(&a) } else { a };
                out.push((key, vals));
            }
        }
    }
    out
}

fn subexprs(e: &E, out: &mut Vec<E>) {
    if matches!(**e, Ex::C(_) | Ex::R(_) | Ex::U(_) | Ex::V(..) | Ex::Vw(..)) {
        return;
    }
    out.push(e.clone());
    if matches!(**e, Ex::Oneof(..)) {
        return;
    }
    for x in e.children() {
        subexprs(x, out);
    }
}

fn indices(e: &E, out: &mut Vec<E>) {
    if let Ex::Ld(_, _, _, addr, _) = &**e {
        subexprs(addr, out);
    }
    if !matches!(**e, Ex::C(_) | Ex::R(_) | Ex::U(_) | Ex::V(..) | Ex::Vw(..)) {
        for x in e.children() {
            indices(x, out);
        }
    }
}

fn sorted_set(it: impl IntoIterator<Item = u64>) -> Vec<u64> {
    let s: BTreeSet<u64> = it.into_iter().collect();
    s.into_iter().collect()
}

fn dom_or_c(x: &E) -> R<Option<Vec<u64>>> {
    match **x {
        Ex::C(v) => Ok(Some(vec![v])),
        _ => domain(x),
    }
}

/// Values e can take by construction, or None.
fn domain(e: &E) -> R<Option<Vec<u64>>> {
    match &**e {
        Ex::Bin(Op::And, _, _, b) if b.cval().is_some_and(|m| m.count_ones() <= 10) => {
            let m = b.cval().unwrap();
            let (mut out, mut x) = (vec![], 0u64);
            loop {
                out.push(x);
                x = x.wrapping_sub(m) & m;
                if x == 0 {
                    return Ok(Some(out));
                }
            }
        }
        Ex::Bfm(_, false, immr, imms, _) if immr <= imms && *imms < immr + 10 => {
            Ok(Some((0..1u64 << (imms - immr + 1)).collect()))
        }
        Ex::Bfm(sz, false, immr, imms, _) if imms < immr && *imms < 8 => {
            if immr > sz {
                return Err(X::Raise);
            }
            let sh = sz - immr;
            Ok(Some((0..1u128 << (imms + 1)).map(|v| ((v << sh) & mask(*sz) as u128) as u64).collect()))
        }
        Ex::Bin(op @ (Op::Orr | Op::Add | Op::Eor), sz, a, b) => {
            let (da, db) = (dom_or_c(a)?, dom_or_c(b)?);
            let (Some(da), Some(db)) = (da, db) else { return Ok(None) };
            if da.len() * db.len() > 256 {
                return Ok(None);
            }
            Ok(Some(sorted_set(da.iter().flat_map(|&x| db.iter().map(move |&y| bin_eval(*op, x, y, *sz))))))
        }
        Ex::Shift(Sh::Lsr, sz, _, n) if (*n as i64) > *sz as i64 - 10 => Ok(Some((0..1u64 << (sz - n)).collect())),
        Ex::Xt(_, false, _, _) | Ex::Ld(1, false, _, _, _) => Ok(Some((0..256).collect())),
        Ex::Csel(_, x, y, _, _) => {
            let (da, db) = (dom_or_c(x)?, dom_or_c(y)?);
            match (da, db) {
                (Some(a), Some(b)) if a.len() + b.len() <= 256 => Ok(Some(sorted_set(a.into_iter().chain(b)))),
                _ => Ok(None),
            }
        }
        Ex::Zx(x) => domain(x),
        Ex::Lo(x) => Ok(domain(x)?.map(|d| sorted_set(d.into_iter().map(|v| v & M32)))),
        Ex::Shift(Sh::Lsl, sz, a, n) => {
            Ok(domain(a)?.map(|d| sorted_set(d.into_iter().map(|v| (((v as u128) << n) & mask(*sz) as u128) as u64))))
        }
        Ex::Oneof(_, s) => Ok(Some(s.to_vec())),
        Ex::Bfi(sz, immr, imms, dst, _) => {
            let dom = dom_or_c(dst)?;
            let (width, at) = if imms >= immr { (imms - immr + 1, *immr) } else { (imms + 1, 0) };
            let Some(dom) = dom else { return Ok(None) };
            if width > 4 || (dom.len() << width) > 256 {
                return Ok(None);
            }
            let mut out = BTreeSet::new();
            for &d in &dom {
                for i in 0..1u128 << width {
                    out.insert(bfi_eval(*sz, *immr, *imms, d as u128, i << at)?);
                }
            }
            Ok(Some(out.into_iter().collect()))
        }
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------- control flow

fn branch_target(w: u32, pc: u64) -> Option<i128> {
    let pc = pc as i128;
    if (w & 0xFC00_0000) == 0x1400_0000 {
        return Some(pc + psext((w & 0x3FF_FFFF) as i128, 26) * 4);
    }
    if (w & 0xFF00_0010) == 0x5400_0000 || (w & 0x7E00_0000) == 0x3400_0000 {
        return Some(pc + sx_bits(w, 5, 19) * 4);
    }
    if (w & 0x7E00_0000) == 0x3600_0000 {
        return Some(pc + sx_bits(w, 5, 14) * 4);
    }
    None
}

fn no_fallthrough(w: u32) -> bool {
    (w & 0xFC00_0000) == 0x1400_0000
        || (w & 0xFF00_001E) == 0x5400_000E
        || ((w & 0xFE00_0000) == 0xD600_0000 && (w & 0x01E0_0000) != 0x0020_0000)
        || (w >> 16) == 0
        || (w & 0xFFE0_001F) == 0xD420_0000
        || (w & 0xFFE0_001F) == 0xD440_0000
}

fn ends_block(w: u32) -> bool {
    no_fallthrough(w) || (w & 0xFF00_0010) == 0x5400_0000 || (w & 0x7C00_0000) == 0x3400_0000
}

fn is_br(w: u32) -> bool {
    (w & 0xFFFF_FC1F) == 0xD61F_0000
}

fn is_call(w: u32) -> bool {
    (w & 0xFC00_0000) == 0x9400_0000 || (w & 0xFFFF_FC1F) == 0xD63F_0000
}

type Pads = BTreeMap<u64, Vec<(i128, i128)>>;
/// Table map in Python dict (insertion) order; None = unresolved (may reach any block).
type Tables = Vec<(u64, Option<BTreeSet<u64>>)>;
type Seg = (u64, u64, Option<bool>);

struct Cfg<'a> {
    entry: u64,
    end: u64,
    pads: &'a Pads,
    leaders: Vec<u64>,
    stops: Vec<u64>,
    unknown: HashSet<u64>,
    everywhere: Vec<u64>,
    preds: HashMap<u64, Vec<(u64, Option<bool>)>>,
}

impl<'a> Cfg<'a> {
    fn new(m: &Module, index: &[(i64, u64)], entry: u64, end: u64, tables: &Tables, pads: &'a Pads) -> Cfg<'a> {
        let i = index.partition_point(|x| x.0 < entry as i64);
        let j = index.partition_point(|x| x.0 < end as i64);
        let mut labels: Vec<(u64, Vec<u64>)> = vec![];
        for &(d, s) in &index[i..j] {
            match labels.last_mut() {
                Some((ld, v)) if *ld == d as u64 => v.push(s),
                _ => labels.push((d as u64, vec![s])),
            }
        }
        let mut leaders: BTreeSet<u64> = BTreeSet::new();
        leaders.insert(entry);
        leaders.extend(labels.iter().map(|x| x.0));
        leaders.extend(pads.keys().copied());
        for (_, ts) in tables {
            if let Some(ts) = ts {
                leaders.extend(ts.iter().copied().filter(|&t| entry <= t && t < end));
            }
        }
        let mut a = entry;
        while a < end {
            if ends_block(m.word(a)) && a + 4 < end {
                leaders.insert(a + 4);
            }
            a += 4;
        }
        let leaders: Vec<u64> = leaders.into_iter().collect();
        let mut stops: Vec<u64> = leaders[1..].to_vec();
        stops.push(end);
        let mut unknown: HashSet<u64> = HashSet::new();
        unknown.insert(entry);
        for (t, srcs) in &labels {
            if srcs.iter().any(|&a| !(entry <= a && a < end)) {
                unknown.insert(*t);
            }
        }
        let everywhere = tables.iter().filter(|(_, ts)| ts.is_none()).map(|(br, _)| *br).collect();
        let mut preds: HashMap<u64, Vec<(u64, Option<bool>)>> = leaders.iter().map(|&b| (b, vec![])).collect();
        for (&_b, &stop) in leaders.iter().zip(stops.iter()) {
            let last = stop - 4;
            let w = m.word(last);
            let t = branch_target(w, last);
            let cond = ((w & 0xFF00_0010) == 0x5400_0000 && (w & 0xE) != 0xE) || (w & 0x7C00_0000) == 0x3400_0000;
            if let Some(t) = t {
                if entry as i128 <= t && t < end as i128 {
                    let tk = if cond && t != stop as i128 { Some(true) } else { None };
                    preds.get_mut(&(t as u64)).unwrap().push((last, tk));
                }
            }
            if !no_fallthrough(w) && stop < end && t != Some(stop as i128) {
                preds.get_mut(&stop).unwrap().push((last, if cond { Some(false) } else { None }));
            }
        }
        for (br, ts) in tables {
            if let Some(ts) = ts {
                for &t in ts {
                    if entry <= t && t < end {
                        preds.get_mut(&t).unwrap().push((*br, None));
                    }
                }
            }
        }
        Cfg { entry, end, pads, leaders, stops, unknown, everywhere, preds }
    }

    fn block(&self, a: u64) -> u64 {
        self.leaders[self.leaders.partition_point(|&l| l <= a) - 1]
    }

    fn path_preds(&self, b: u64) -> Option<Vec<Seg>> {
        if self.unknown.contains(&b) || self.pads.contains_key(&b) || self.preds[&b].is_empty() {
            return None;
        }
        Some(self.preds[&b].iter().map(|&(src, taken)| (self.block(src), src, taken)).collect())
    }
}

// ---------------------------------------------------------------- landing pads

fn byte(img: &[u8], o: i128) -> R<u8> {
    if o < 0 || o >= img.len() as i128 {
        return Err(X::Raise);
    }
    Ok(img[o as usize])
}

fn uleb(img: &[u8], mut o: i128) -> R<(i128, i128)> {
    let (mut v, mut sh) = (0i128, 0u32);
    loop {
        let b = byte(img, o)?;
        o += 1;
        if sh < 120 {
            v |= ((b & 0x7F) as i128) << sh;
        }
        sh += 7;
        if b & 0x80 == 0 {
            return Ok((v, o));
        }
    }
}

fn sleb(img: &[u8], o: i128) -> R<(i128, i128)> {
    let (v, o2) = uleb(img, o)?;
    let n = (7 * (o2 - o)) as u32;
    if n > 126 {
        return Err(X::Raise);
    }
    Ok((if (v >> (n - 1)) & 1 != 0 { v - (1i128 << n) } else { v }, o2))
}

fn le(img: &[u8], o: i128, n: usize, signed: bool) -> i128 {
    let mut v: i128 = 0;
    let mut k = 0;
    if o >= 0 {
        for i in 0..n {
            let p = o as usize + i;
            if p >= img.len() {
                break;
            }
            v |= (img[p] as i128) << (8 * i);
            k += 1;
        }
    }
    if signed && k > 0 && (v >> (8 * k - 1)) & 1 != 0 {
        v -= 1i128 << (8 * k);
    }
    v
}

fn enc(img: &[u8], o: i128, e: u8) -> R<(i128, i128)> {
    if e == 0xFF {
        return Ok((0, o));
    }
    let fmt = e & 0x0F;
    let (mut v, o2) = match fmt {
        0x01 => uleb(img, o)?,
        0x09 => sleb(img, o)?,
        _ => {
            let (n, signed) = match fmt {
                0x00 => (8, false),
                0x02 => (2, false),
                0x03 => (4, false),
                0x04 => (8, false),
                0x0A => (2, true),
                0x0B => (4, true),
                0x0C => (8, true),
                _ => return Err(X::Raise),
            };
            (le(img, o, n, signed), o + n as i128)
        }
    };
    if e & 0x70 == 0x10 {
        v += o;
    }
    Ok((v, o2))
}

fn landing_pads(m: &Module) -> R<BTreeMap<i128, Vec<(i128, i128)>>> {
    let img = &m.image[..];
    let hdr = m.nso.eh_start as i128;
    let mut pads = BTreeMap::new();
    let n = img.len() as i128;
    if hdr != 0 && hdr < n {
        let (mut o, _) = enc(img, hdr + 4, byte(img, hdr + 1)?)?;
        let mut cies: HashMap<i128, (u8, Option<u8>)> = HashMap::new();
        while o + 8 <= n {
            let length = le(img, o, 4, false);
            if length == 0 || length == 0xFFFF_FFFF {
                break;
            }
            let (nxt, cid) = (o + 4 + length, le(img, o + 4, 4, false));
            if cid == 0 {
                let mut p = o + 9;
                let z = img.get(p as usize..).ok_or(X::Raise)?.iter().position(|&b| b == 0).ok_or(X::Raise)?;
                let aug: Vec<u8> = img[p as usize..p as usize + z].to_vec();
                p += aug.len() as i128 + 1;
                p = uleb(img, p)?.1;
                p = sleb(img, p)?.1;
                p = if byte(img, o + 8)? == 1 { p + 1 } else { uleb(img, p)?.1 };
                let (mut rr, mut ll) = (0u8, None);
                if aug.first() == Some(&b'z') {
                    p = uleb(img, p)?.1;
                    for &ch in &aug[1..] {
                        if ch == b'P' {
                            p = enc(img, p + 1, byte(img, p)?)?.1;
                        } else if ch == b'L' || ch == b'R' {
                            let v = byte(img, p)?;
                            if ch == b'L' {
                                ll = Some(v);
                            } else {
                                rr = v;
                            }
                            p += 1;
                        }
                    }
                }
                cies.insert(o, (rr, ll));
            } else if let Some(&(rr, Some(ll))) = cies.get(&(o + 4 - cid)) {
                let (start, p) = enc(img, o + 8, rr)?;
                let (_, p) = enc(img, p, rr & 0x0F)?;
                let (_, p) = uleb(img, p)?;
                let (lsda, _) = enc(img, p, ll)?;
                if lsda != 0 {
                    lsda_parse(img, lsda, start, m.base as i128, &mut pads)?;
                }
            }
            o = nxt;
        }
    }
    Ok(pads)
}

fn lsda_parse(img: &[u8], o: i128, func: i128, base: i128, pads: &mut BTreeMap<i128, Vec<(i128, i128)>>) -> R<()> {
    let lp_enc = byte(img, o)?;
    let (mut lpstart, mut o) = enc(img, o + 1, lp_enc)?;
    if lp_enc == 0xFF {
        lpstart = func;
    }
    let tt_enc = byte(img, o)?;
    o += 1;
    if tt_enc != 0xFF {
        o = uleb(img, o)?.1;
    }
    let cs_enc = byte(img, o)?;
    let (n, o2) = uleb(img, o + 1)?;
    o = o2;
    let stop = o + n;
    while o < stop {
        let (s, o1) = enc(img, o, cs_enc & 0x0F)?;
        let (ln, o1) = enc(img, o1, cs_enc & 0x0F)?;
        let (lp, o1) = enc(img, o1, cs_enc & 0x0F)?;
        o = uleb(img, o1)?.1;
        if lp != 0 {
            pads.entry(base + lpstart + lp).or_default().push((base + func + s, base + func + s + ln));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- resolver

/// Per-module resolver with the caches the Python keeps in weakref dicts (_INDEX, _FUNCS, _PADS).
pub struct JumpTables<'m> {
    m: &'m Module,
    index: Option<Vec<(i64, u64)>>,
    pads: Option<R<BTreeMap<i128, Vec<(i128, i128)>>>>,
    func: Option<((u64, u64), HashMap<u64, Option<BTreeSet<u64>>>)>,
    budget: i64,
}

type Env<'e> = &'e [(E, u64)];

impl<'m> JumpTables<'m> {
    pub fn new(m: &'m Module) -> Self {
        JumpTables { m, index: None, pads: None, func: None, budget: 0 }
    }

    /// Python resolve(mod, br_pc, entry, end): sorted distinct targets in [entry, end), or None.
    pub fn resolve(&mut self, br_pc: u64, entry: u64, end: u64) -> Option<Vec<u64>> {
        let t = self.table_targets(br_pc, entry, end).ok()??;
        if t[0] < entry || *t.last().unwrap() >= end {
            return None;
        }
        Some(t)
    }

    /// Python is_computed(mod, br_pc, entry).
    pub fn is_computed(&mut self, br_pc: u64, entry: u64) -> bool {
        self.computed(br_pc, entry).unwrap_or(false)
    }

    fn table_targets(&mut self, br_pc: u64, entry: u64, end: u64) -> R<Option<Vec<u64>>> {
        if !self.computed(br_pc, entry)? {
            return Ok(None);
        }
        let f = self.function(entry, end)?;
        Ok(f.get(&br_pc).cloned().flatten().map(|s| s.into_iter().collect()))
    }

    fn computed(&self, br_pc: u64, entry: u64) -> R<bool> {
        let m = self.m;
        let w = m.word(br_pc);
        if !is_br(w) {
            return Ok(false);
        }
        let mut s = br_pc;
        while s > entry && br_pc - s < 4 * MAX_BLOCK && !ends_block(m.word(s - 4)) {
            s -= 4;
        }
        let mut dummy = 0i64;
        let (_, t) = run(m, &[(s, br_pc, None)], bits(w, 5, 5), None, None, &mut dummy)?;
        Ok(match &*t {
            Ex::C(v) => m.in_text(*v),
            Ex::Bin(Op::Add | Op::Sub, ..) => refers(m, &t),
            _ => false,
        })
    }

    fn index(&mut self) -> &[(i64, u64)] {
        if self.index.is_none() {
            let m = self.m;
            let mut v: Vec<(i64, u64)> = vec![];
            for (i, &w) in m.words.iter().enumerate() {
                let pc = m.lo + 4 * i as u64;
                if let Some(t) = branch_target(w, pc) {
                    v.push((t as i64, pc));
                }
            }
            v.sort_by_key(|x| x.0); // stable, like np.argsort(kind='stable')
            self.index = Some(v);
        }
        self.index.as_ref().unwrap()
    }

    fn function(&mut self, entry: u64, end: u64) -> R<HashMap<u64, Option<BTreeSet<u64>>>> {
        if let Some((k, r)) = &self.func {
            if *k == (entry, end) {
                return Ok(r.clone());
            }
        }
        let m = self.m;
        let mut brs = vec![];
        let mut a = entry;
        while a < end {
            if is_br(m.word(a)) && self.computed(a, entry)? {
                brs.push(a);
            }
            a += 4;
        }
        if self.pads.is_none() {
            self.pads = Some(landing_pads(m));
        }
        let all = self.pads.as_ref().unwrap().as_ref().map_err(|e| *e)?;
        let pads: Pads = all.range(entry as i128..end as i128).map(|(k, v)| (*k as u64, v.clone())).collect();
        self.index();
        self.budget = MAX_STEPS;
        let result = match self.resolve_all(entry, end, &brs, &pads) {
            Ok(r) => r,
            Err(X::Exhausted) => brs.iter().map(|&a| (a, None)).collect(),
            Err(e) => return Err(e),
        };
        self.func = Some(((entry, end), result.clone()));
        Ok(result)
    }

    fn resolve_all(&mut self, entry: u64, end: u64, brs: &[u64], pads: &Pads) -> R<HashMap<u64, Option<BTreeSet<u64>>>> {
        let mut tables: Vec<(u64, BTreeSet<u64>)> = vec![];
        let mut result: HashMap<u64, Option<BTreeSet<u64>>> = brs.iter().map(|&a| (a, None)).collect();
        let rounds = if brs.is_empty() { 0 } else { MAX_ROUNDS };
        for _ in 0..rounds {
            let mut grown = false;
            let t: Tables = tables.iter().map(|(a, s)| (*a, Some(s.clone()))).collect();
            for (a, r) in self.round(entry, end, brs, &t, pads)? {
                let Some(r) = r else { continue };
                match tables.iter_mut().find(|x| x.0 == a) {
                    Some(x) => {
                        if !r.is_subset(&x.1) {
                            x.1.extend(r);
                            grown = true;
                        }
                    }
                    None => {
                        tables.push((a, r));
                        grown = true;
                    }
                }
            }
            if !grown {
                break;
            }
        }
        let mut dead: HashSet<u64> = HashSet::new();
        for _ in 0..rounds {
            let full: Tables =
                brs.iter().map(|&a| (a, tables.iter().find(|x| x.0 == a).map(|x| x.1.clone()))).collect();
            let new: HashMap<u64, Option<BTreeSet<u64>>> = self.round(entry, end, brs, &full, pads)?.into_iter().collect();
            let fullm: HashMap<u64, Option<BTreeSet<u64>>> = full.iter().cloned().collect();
            let empty = BTreeSet::new();
            let bad: Vec<u64> = brs
                .iter()
                .copied()
                .filter(|a| {
                    let (n, f) = (&new[a], &fullm[a]);
                    (n.is_none() && f.is_some())
                        || (n.is_some()
                            && !dead.contains(a)
                            && !n.as_ref().unwrap().is_subset(f.as_ref().unwrap_or(&empty)))
                })
                .collect();
            if bad.is_empty() {
                result = brs.iter().map(|&a| (a, if fullm[&a].is_some() { new[&a].clone() } else { None })).collect();
                break;
            }
            for a in bad {
                match &new[&a] {
                    None => {
                        tables.retain(|x| x.0 != a);
                        dead.insert(a);
                    }
                    Some(n) => match tables.iter_mut().find(|x| x.0 == a) {
                        Some(x) => x.1.extend(n.iter().copied()),
                        None => tables.push((a, n.clone())),
                    },
                }
            }
        }
        Ok(result)
    }

    fn round(&mut self, entry: u64, end: u64, brs: &[u64], tables: &Tables, pads: &Pads) -> R<Vec<(u64, Option<BTreeSet<u64>>)>> {
        let m = self.m;
        let index = self.index.take().unwrap();
        let cfg = Cfg::new(m, &index, entry, end, tables, pads);
        self.index = Some(index);
        let mut facts: Option<HashMap<u64, Facts>> = None;
        let mut new = vec![];
        for &a in brs {
            let mut r = explore(m, &cfg, None, a, &mut self.budget)?;
            if r.is_none() {
                if facts.is_none() {
                    facts = Some(facts_fix(m, &cfg, &mut self.budget)?.unwrap_or_default());
                }
                let f = facts.as_ref().unwrap();
                if !f.is_empty() {
                    r = explore(m, &cfg, Some(f), a, &mut self.budget)?;
                }
            }
            new.push((a, r));
        }
        Ok(new)
    }
}

fn spend(budget: &mut i64, n: i64) -> R<()> {
    *budget -= n;
    if *budget < 0 { Err(X::Exhausted) } else { Ok(()) }
}

/// Symbolic state at the br that ends path and the br register value. `budget` None = do not spend.
fn run(m: &Module, path: &[Seg], rn: u32, facts: Option<&Facts>, spend_on: Option<()>, budget: &mut i64) -> R<(State, E)> {
    if spend_on.is_some() {
        let n: u64 = path.iter().map(|&(s, l, _)| (l - s) >> 2).sum();
        spend(budget, n as i64 + 1)?;
    }
    let mut st = State::new();
    if let Some(f) = facts {
        init(&mut st, f);
    }
    for &(start, last, taken) in &path[..path.len() - 1] {
        let mut a = start;
        while a < last {
            step(&mut st, m.word(a), a)?;
            a += 4;
        }
        let w = m.word(last);
        if let Some(t) = taken {
            edge(&mut st, w, t);
        } else if branch_target(w, last).is_none() && !is_br(w) {
            step(&mut st, w, last)?;
        }
    }
    let (start, last, _) = *path.last().unwrap();
    let mut a = start;
    while a < last {
        step(&mut st, m.word(a), a)?;
        a += 4;
    }
    let t = st.get(rn, 64, false);
    Ok((st, t))
}

fn refers(m: &Module, e: &E) -> bool {
    let (mut has_addr, mut has_var) = (false, false);
    let mut stack = vec![e.clone()];
    while let Some(x) = stack.pop() {
        match &*x {
            Ex::C(v) => has_addr |= m.in_image(*v, 1),
            Ex::R(_) | Ex::U(_) | Ex::V(..) | Ex::Vw(..) => has_var = true,
            Ex::Ld(_, _, _, addr, _) => {
                has_var = true;
                has_addr = true;
                stack.push(addr.clone());
            }
            other => stack.extend(other.children().into_iter().cloned()),
        }
    }
    has_addr && has_var
}

fn const_mem(m: &Module, a: u64, n: u64) -> bool {
    for (off, size) in [m.nso.text, m.nso.rodata] {
        let lo_ = m.base as u128 + off as u128;
        if lo_ <= a as u128 && a as u128 + n as u128 <= lo_ + size as u128 {
            return true;
        }
    }
    false
}

fn eval(m: &Module, e: &E, env: Env) -> R<u64> {
    if let Ex::C(v) = **e {
        return Ok(v);
    }
    for (x, v) in env {
        if e == x {
            return Ok(*v);
        }
    }
    match &**e {
        Ex::Lo(x) => Ok(eval(m, x, env)? & M32),
        Ex::Zx(x) => eval(m, x, env),
        Ex::Sx(x) => Ok((psext(eval(m, x, env)? as i128, 32) & M64 as i128) as u64),
        Ex::Bin(op, sz, a, b) => {
            let x = eval(m, a, env)?;
            Ok(bin_eval(*op, x, eval(m, b, env)?, *sz))
        }
        Ex::Shift(op, sz, a, n) => Ok(sh_eval(*op, eval(m, a, env)?, *n, *sz)),
        Ex::Xt(b, signed, sz, x) => {
            let v = eval(m, x, env)? & ((1u64 << b) - 1);
            Ok(((if *signed { psext(v as i128, *b) } else { v as i128 }) & mask(*sz) as i128) as u64)
        }
        Ex::Bfm(sz, signed, immr, imms, x) => bfm_eval(*sz, *signed, *immr, *imms, eval(m, x, env)?),
        Ex::Bfi(sz, immr, imms, d, s) => {
            let dv = eval(m, d, env)?;
            bfi_eval(*sz, *immr, *imms, dv as u128, eval(m, s, env)? as u128)
        }
        Ex::Ld(size, signed, sz, addr, _) => {
            let a = eval(m, addr, env)?;
            if !const_mem(m, a, *size as u64) {
                return Err(X::Unknown);
            }
            Ok((m.read(a, *size as usize, *signed) & mask(*sz) as i128) as u64)
        }
        _ => Err(X::Unknown),
    }
}

fn catch_unknown<T>(r: R<T>, dflt: T) -> R<T> {
    match r {
        Err(X::Unknown) => Ok(dflt),
        x => x,
    }
}

fn flags_of(m: &Module, f: &E, env: Env) -> R<Vec<Option<Nzcv>>> {
    match &**f {
        Ex::Ccmp(sz, a, b, add, cond, imm, prev) => {
            let ps = match prev {
                Some(p) => flags_of(m, p, env)?,
                None => vec![None],
            };
            let (mut compare, mut fixed) = (false, false);
            for p in ps {
                if p.is_none() || holds(*cond, p.unwrap()) {
                    compare = true;
                }
                if p.is_none() || !holds(*cond, p.unwrap()) {
                    fixed = true;
                }
            }
            let mut out = if fixed { vec![Some(imm_nzcv(*imm))] } else { vec![] };
            if compare {
                let g = mk(Ex::Bin(if *add { Op::Add } else { Op::Sub }, *sz, a.clone(), b.clone()));
                out.extend(flags_of(m, &g, env)?);
            }
            Ok(out)
        }
        Ex::Logic(sz, r) => catch_unknown(eval(m, r, env).map(|x| vec![Some(nzcv(None, *sz, x, 0))]), vec![None]),
        Ex::Bin(op, sz, a, b) => {
            let r = (|| {
                let x = eval(m, a, env)?;
                let y = eval(m, b, env)?;
                Ok(vec![Some(nzcv(Some(*op), *sz, x, y))])
            })();
            catch_unknown(r, vec![None])
        }
        _ => unreachable!("flags record"),
    }
}

fn violated(m: &Module, rec: &Cond, env: Env) -> R<bool> {
    let r = (|| match rec {
        Cond::Flags(f, cond, h) => {
            let opts = flags_of(m, f, env)?;
            Ok(opts.iter().all(|o| o.is_some()) && opts.iter().all(|o| holds(*cond, o.unwrap()) != *h))
        }
        Cond::Zero(sz, x, h) => Ok(((eval(m, x, env)? & mask(*sz)) == 0) != *h),
        Cond::Bit(x, bit, h) => Ok(((eval(m, x, env)? >> bit) & 1 != 0) != *h),
    })();
    catch_unknown(r, false)
}

fn combinations(n: usize, k: usize, mut f: impl FnMut(&[usize]) -> R<bool>) -> R<bool> {
    if k > n {
        return Ok(false);
    }
    let mut idx: Vec<usize> = (0..k).collect();
    loop {
        if f(&idx)? {
            return Ok(true);
        }
        let mut i = k;
        loop {
            if i == 0 {
                return Ok(false);
            }
            i -= 1;
            if idx[i] != i + n - k {
                break;
            }
            if i == 0 {
                return Ok(false);
            }
        }
        idx[i] += 1;
        for j in i + 1..k {
            idx[j] = idx[j - 1] + 1;
        }
    }
}

/// Targets of t over the values the path proves for the unknowns in it, or None.
fn solve(m: &Module, t: &E, st: &State, masks: bool, budget: &mut i64) -> R<Option<BTreeSet<u64>>> {
    match eval(m, t, &[]) {
        Ok(v) => return Ok(Some([v].into_iter().collect())),
        Err(X::Unknown) => {}
        Err(e) => return Err(e),
    }
    let mut cands = bounds(st);
    if masks {
        let limit = cands.iter().filter(|c| c.1.len() > 1).map(|c| c.1.len()).min().unwrap_or(MAX_COUNT);
        let mut subs = vec![];
        indices(t, &mut subs);
        for e in subs {
            if let Some(d) = domain(&e)? {
                if d.len() <= limit {
                    cands.push((e, d));
                }
            }
        }
    }
    cands.sort_by_key(|c| c.1.len());
    let mut seen: HashSet<E> = HashSet::new();
    let mut uniq: Vec<(E, Vec<u64>)> = vec![];
    for (e, d) in cands {
        if seen.insert(e.clone()) {
            uniq.push((e, d));
        }
    }
    let mut found: Option<Option<BTreeSet<u64>>> = None;
    for k in 1..=3 {
        let list = if k == 1 { &uniq[..] } else { &uniq[..uniq.len().min(MAX_CANDS)] };
        let done = combinations(list.len(), k, |combo| {
            let mut size: u128 = 1;
            for &i in combo {
                size *= list[i].1.len() as u128;
            }
            if size > MAX_COUNT as u128 {
                return Ok(false);
            }
            if combo.iter().any(|&i| list[i].1.is_empty()) {
                return Ok(false); // empty product: no values, out stays empty
            }
            let mut out = BTreeSet::new();
            let mut pos = vec![0usize; k];
            let mut env: Vec<(E, u64)> = combo.iter().map(|&i| (list[i].0.clone(), 0)).collect();
            loop {
                spend(budget, 1)?;
                for j in 0..k {
                    env[j].1 = list[combo[j]].1[pos[j]];
                }
                let mut bad = false;
                for rec in &st.conds {
                    if violated(m, rec, &env)? {
                        bad = true;
                        break;
                    }
                }
                if !bad {
                    match eval(m, t, &env) {
                        Ok(v) => {
                            out.insert(v);
                        }
                        Err(X::Unknown) => {
                            if !out.is_empty() {
                                found = Some(None);
                                return Ok(true);
                            }
                            return Ok(false);
                        }
                        Err(e) => return Err(e),
                    }
                }
                // next in itertools.product order (last fastest)
                let mut j = k;
                loop {
                    if j == 0 {
                        if !out.is_empty() {
                            found = Some(Some(std::mem::take(&mut out)));
                            return Ok(true);
                        }
                        return Ok(false);
                    }
                    j -= 1;
                    pos[j] += 1;
                    if pos[j] < list[combo[j]].1.len() {
                        break;
                    }
                    pos[j] = 0;
                }
            }
        })?;
        if done {
            return Ok(found.unwrap());
        }
    }
    Ok(None)
}

/// Targets of the table jump at br_pc over every path into its dispatch block, or None.
fn explore(m: &Module, cfg: &Cfg, facts: Option<&HashMap<u64, Facts>>, br_pc: u64, budget: &mut i64) -> R<Option<BTreeSet<u64>>> {
    let rn = bits(m.word(br_pc), 5, 5);
    let mut paths = MAX_PATHS;

    fn go(
        m: &Module,
        cfg: &Cfg,
        facts: Option<&HashMap<u64, Facts>>,
        rn: u32,
        path: Vec<Seg>,
        depth: usize,
        paths: &mut i64,
        budget: &mut i64,
    ) -> R<Option<BTreeSet<u64>>> {
        *paths -= 1;
        if *paths < 0 {
            return Ok(None);
        }
        let f = facts.and_then(|f| f.get(&path[0].0));
        let (st, t) = run(m, &path, rn, f, Some(()), budget)?;
        if let Some(r) = solve(m, &t, &st, false, budget)? {
            return Ok(Some(r));
        }
        let preds = if depth < MAX_DEPTH { cfg.path_preds(path[0].0) } else { None };
        if let Some(preds) = preds {
            let mut out = BTreeSet::new();
            let mut broke = false;
            for seg in preds {
                let mut p = vec![seg];
                p.extend(path.iter().copied());
                match go(m, cfg, facts, rn, p, depth + 1, paths, budget)? {
                    None => {
                        broke = true;
                        break;
                    }
                    Some(r) => out.extend(r),
                }
            }
            if !broke {
                return Ok(Some(out));
            }
        }
        solve(m, &t, &st, true, budget)
    }

    let out = go(m, cfg, facts, rn, vec![(cfg.block(br_pc), br_pc, None)], 0, &mut paths, budget)?;
    match out {
        Some(o) if !o.is_empty() && !o.iter().any(|&a| a % 4 != 0 || !m.in_text(a)) => Ok(Some(o)),
        _ => Ok(None),
    }
}

/// {block start: facts} from the function-wide must-analysis, or None if it does not settle.
fn facts_fix(m: &Module, cfg: &Cfg, budget: &mut i64) -> R<Option<HashMap<u64, Facts>>> {
    let mut out: HashMap<u64, Facts> = HashMap::new();
    let mut ins: HashMap<u64, Facts> = HashMap::new();
    let mut calls: Vec<(u64, Facts)> = vec![]; // dict in insertion order
    let mut call_ix: HashMap<u64, usize> = HashMap::new();
    let mut name_set: HashSet<u64> = HashSet::new();
    let _ = (cfg.entry, cfg.end);
    for _ in 0..64 {
        spend(budget, cfg.leaders.len() as i64)?;
        let mut changed = false;
        let mut anywhere: Option<Facts> = None;
        for br in &cfg.everywhere {
            anywhere = meet(anywhere.as_ref(), out.get(br));
        }
        for (&b, &stop) in cfg.leaders.iter().zip(cfg.stops.iter()) {
            let d: Facts = if cfg.unknown.contains(&b) {
                Facts::new()
            } else {
                let mut d: Option<Facts> = None;
                for (src, _) in &cfg.preds[&b] {
                    d = meet(d.as_ref(), out.get(src));
                }
                if let Some(ranges) = cfg.pads.get(&b) {
                    for &(lo_, hi_) in ranges {
                        for (a, cf) in &calls {
                            if lo_ <= *a as i128 && (*a as i128) < hi_ {
                                d = meet(d.as_ref(), Some(cf));
                            }
                        }
                    }
                }
                d = meet(d.as_ref(), anywhere.as_ref());
                match d {
                    None => continue,
                    Some(d) => d,
                }
            };
            if ins.get(&b) == Some(&d) && out.contains_key(&(stop - 4)) {
                continue;
            }
            ins.insert(b, d.clone());
            spend(budget, ((stop - b) >> 2) as i64)?;
            let mut st = State::new();
            init(&mut st, &d);
            let mut a = b;
            while a < stop {
                let w = m.word(a);
                if is_call(w) {
                    let cf: Facts = facts_of(&st).into_iter().filter(|(r, _)| (19..30).contains(r)).collect();
                    match call_ix.get(&a) {
                        Some(&i) => {
                            if calls[i].1 != cf {
                                calls[i].1 = cf;
                                changed = true;
                            }
                        }
                        None => {
                            call_ix.insert(a, calls.len());
                            calls.push((a, cf));
                            changed = true;
                        }
                    }
                }
                if a == stop - 4 && (is_br(w) || branch_target(w, a).is_some()) {
                    break;
                }
                let before: Vec<E> = st.regs[..31].to_vec();
                step(&mut st, w, a)?;
                for r in 0..31 {
                    if Rc::ptr_eq(&st.regs[r], &before[r]) {
                        continue;
                    }
                    let e = st.regs[r].clone();
                    if !matches!(*e, Ex::Oneof(..)) && !clean(&e, 8) {
                        st.regs[r] = if matches!(*e, Ex::Zx(_)) {
                            mk(Ex::Zx(mk(Ex::Vw(a, r as u32))))
                        } else {
                            mk(Ex::V(a, r as u32))
                        };
                        name_set.insert(a);
                    }
                }
                if name_set.contains(&a) {
                    for r in 0..31 {
                        if Rc::ptr_eq(&st.regs[r], &before[r]) && names(&st.regs[r], a) {
                            st.regs[r] = st.unknown();
                        }
                    }
                }
                a += 4;
            }
            let d = facts_of(&st);
            if out.get(&(stop - 4)) != Some(&d) {
                out.insert(stop - 4, d);
                changed = true;
            }
        }
        if !changed {
            return Ok(Some(ins));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation() {
        let r = mk(Ex::R(3));
        // lo(zx(e)) = e, and #0xffffffff = zx(lo(e))
        let z = zx(&r);
        assert!(Rc::ptr_eq(&lo(&z), &r));
        let a = binop(Op::And, 64, &r, &c64(M32 as i128));
        assert_eq!(a, mk(Ex::Zx(mk(Ex::Lo(r.clone())))));
        // x + 0 = x (same object)
        assert!(Rc::ptr_eq(&binop(Op::Add, 64, &r, &c64(0)), &r));
        // constants fold and mask
        assert_eq!(binop(Op::Sub, 32, &c64(1), &c64(2)), c64(M32 as i128));
        assert_eq!(c(-1, 32).cval(), Some(M32));
        // uxtw via ubfm #0, #31
        assert_eq!(bfm(64, false, 0, 31, &r).unwrap(), mk(Ex::Zx(mk(Ex::Lo(r.clone())))));
        // lsl #2 as ubfm #62, #61
        assert_eq!(bfm(64, false, 62, 61, &r).unwrap(), mk(Ex::Shift(Sh::Lsl, 64, r.clone(), 2)));
    }

    #[test]
    fn domains_and_clean() {
        let r = mk(Ex::R(0));
        let and = binop(Op::And, 32, &lo(&r), &c64(0b101));
        assert_eq!(domain(&and).unwrap(), Some(vec![0, 1, 4, 5]));
        assert!(clean(&mk(Ex::Bin(Op::Add, 64, mk(Ex::V(4, 1)), c64(8))), 8));
        assert!(!clean(&mk(Ex::Bin(Op::Add, 64, r.clone(), c64(8))), 8));
        assert_eq!(mask(32), M32);
        assert_eq!(mask(64), M64);
    }

    #[test]
    fn bounds_from_cmp_bhi() {
        // cmp w0, #3 ; b.hi (not taken) -> w0 in 0..=3
        let mut st = State::new();
        step(&mut st, 0x7100_0C1F, 0x1000).unwrap(); // subs wzr, w0, #3
        edge(&mut st, 0x5400_0008, false); // b.hi
        let b = bounds(&st);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].0, lo(&mk(Ex::R(0))));
        assert_eq!(b[0].1, vec![0, 1, 2, 3]);
    }

    #[test]
    fn combos() {
        let mut v = vec![];
        combinations(4, 2, |c| {
            v.push(c.to_vec());
            Ok(false)
        })
        .unwrap();
        assert_eq!(v, vec![vec![0, 1], vec![0, 2], vec![0, 3], vec![1, 2], vec![1, 3], vec![2, 3]]);
    }
}

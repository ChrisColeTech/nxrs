//! nxnative's jump-table cases (`dm/tools/tests/test_jump_solver.py`): each offset jump table form a game brought, with
//! that game's instruction sequence, through this translator's resolver (`xlate::jumptables`, Switch2RECOMP's
//! method) instead of nxnative's `jump_solver.py`. A case nxnative proves and this resolver does not is a test of its
//! own, `#[ignore]`d with what is missing, so the gap is listed and runs with `--ignored` (2026-11-20: 8 forms proved,
//! 4 not: the shifted 64-bit index, the complemented index, a mask's excluded top value, a bit test above a mask).

use translator::loader::Module;
use translator::xlate::jumptables::JumpTables;

const TABLE: u64 = 0x4000;
const RET: u32 = 0xd65f03c0;
const NOP: u32 = 0xD503201F;
const EQ: u32 = 0;
const HS: u32 = 2;
const LS: u32 = 9;
const HI: u32 = 8;
const GE: u32 = 10;

fn adrp(d: u32, pc: u64, target: u64) -> u32 {
    let imm = (((target & !0xfff) as i64 - (pc & !0xfff) as i64) >> 12) as u32;
    0x9000_0000 | ((imm & 3) << 29) | (((imm >> 2) & 0x7ffff) << 5) | d
}
fn addi(d: u32, n: u32, imm: u32) -> u32 {
    0x9100_0000 | (imm << 10) | (n << 5) | d
}
fn addr(d: u32, n: u32, m: u32) -> u32 {
    0x8B00_0000 | (m << 16) | (n << 5) | d
}
fn br(n: u32) -> u32 {
    0xD61F_0000 | (n << 5)
}
fn b(at: u32, to: u32) -> u32 {
    0x1400_0000 | ((to.wrapping_sub(at) >> 2) & 0x3ff_ffff)
}
fn cmp_w(r: u32, n: u32) -> u32 {
    0x7100_001f | n << 10 | r << 5
}
fn cmp_x(r: u32, n: u32) -> u32 {
    0xf100_001f | n << 10 | r << 5
}
fn bcond(cond: u32, at: u32, to: u32) -> u32 {
    0x5400_0000 | ((to.wrapping_sub(at) >> 2) & 0x7ffff) << 5 | cond
}
fn tbnz(r: u32, bit: u32, at: u32, to: u32) -> u32 {
    0x3700_0000 | (bit & 31) << 19 | ((to.wrapping_sub(at) >> 2) & 0x3fff) << 5 | r
}
fn and_w(d: u32, s: u32, k: u32) -> u32 {
    0x1200_0000 | (k - 1) << 10 | s << 5 | d
}
fn and_x(d: u32, s: u32, k: u32) -> u32 {
    0x9240_0000 | (k - 1) << 10 | s << 5 | d
}
fn zext(d: u32, s: u32) -> u32 {
    0x9240_7c00 | s << 5 | d
}
fn tst_w(s: u32, k: u32) -> u32 {
    0x7200_0000 | (k - 1) << 10 | s << 5 | 31
}
fn mov_w(d: u32, s: u32) -> u32 {
    0x2a00_03e0 | s << 16 | d
}
fn mvn_w(d: u32, s: u32) -> u32 {
    0x2a20_03e0 | s << 16 | d
}
fn movz(d: u32, n: u32) -> u32 {
    0x5280_0000 | n << 5 | d
}
fn sub_w(d: u32, s: u32, n: u32) -> u32 {
    0x5100_0000 | n << 10 | s << 5 | d
}
fn ldr_w(d: u32) -> u32 {
    0xb940_0000 | 2 << 10 | 31 << 5 | d
}
fn ldr_x(d: u32) -> u32 {
    0xf940_0000 | 1 << 10 | 31 << 5 | d
}
fn ldrsw(e: u32, b: u32, i: u32) -> u32 {
    0xb8a0_7800 | i << 16 | b << 5 | e
}
fn lsr_x(d: u32, s: u32, n: u32) -> u32 {
    0xd340_fc00 | n << 16 | s << 5 | d
}
fn ubfx_x(d: u32, s: u32, lsb: u32, k: u32) -> u32 {
    0xd340_0000 | lsb << 16 | (lsb + k - 1) << 10 | s << 5 | d
}

/// The image: code at 0, the table at TABLE (entries target - TABLE as i32, then a word that is no entry).
struct Case {
    words: Vec<u32>,
}
impl Case {
    fn new() -> Case {
        Case {
            words: vec![0; 0x6000 / 4],
        }
    }
    fn table(&mut self, targets: &[u32]) {
        let at = (TABLE / 4) as usize;
        for (i, &t) in targets.iter().enumerate() {
            self.words[at + i] = (t as i64 - TABLE as i64) as i32 as u32;
        }
        self.words[at + targets.len()] = 0x1000;
    }
    fn base(r: u32) -> Vec<u32> {
        vec![adrp(r, 0, TABLE), addi(r, r, (TABLE & 0xfff) as u32)]
    }
    /// The resolver's targets for the `br x18` of `code`, function [0, TABLE).
    fn site(&mut self, code: &[u32]) -> Option<Vec<u64>> {
        let mut w = self.words.clone();
        w[..code.len()].copy_from_slice(code);
        let at = 4 * code.iter().position(|&x| x == br(18))? as u64;
        let m = Module::code("case", 0, 0, w, vec![0]);
        let mut jt = JumpTables::new(&m);
        jt.resolve(at, 0, TABLE)
    }
}

fn t(v: &[u32]) -> Option<Vec<u64>> {
    Some(v.iter().map(|&x| x as u64).collect())
}

fn pad(mut w: Vec<u32>, n: usize) -> Vec<u32> {
    w.extend(std::iter::repeat_n(RET, n));
    w
}

/// Fire Emblem Engage main+0x12eec94: cmp w18,#N; b.hi; ldrsw x18,[xB,x18,lsl #2]; add x18,x18,xB
#[test]
fn compare_and_b_hi() {
    let mut c = Case::new();
    let tg = [36, 40, 44, 48, 52];
    c.table(&tg);
    let mut w = Case::base(4);
    w.extend([
        ldr_w(18),
        cmp_w(18, 4),
        bcond(HI, 16, 56),
        ldrsw(18, 4, 18),
        addr(18, 18, 4),
        br(18),
    ]);
    assert_eq!(c.site(&pad(w, 6)), t(&tg));
}

/// nxnative issue #977: b.hs leaves one entry fewer than b.hi; a compare no branch acts on bounds nothing; a signed
/// branch lets a negative index pass.
#[test]
fn the_branch_that_acts_on_the_compare_decides() {
    let mut c = Case::new();
    let tg = [40, 44, 48, 52, 56];
    c.table(&tg);
    let mut run = |guard: &[u32]| {
        let mut w = Case::base(4);
        w.push(ldr_w(18));
        w.extend_from_slice(guard);
        w.extend([ldrsw(18, 4, 18), addr(18, 18, 4), br(18)]);
        let n = 16 - w.len();
        c.site(&pad(w, n))
    };
    assert_eq!(run(&[cmp_w(18, 4), bcond(HS, 16, 60), NOP]), t(&tg[..4]));
    assert_eq!(run(&[cmp_w(18, 4), NOP, NOP]), None);
    assert_eq!(run(&[cmp_w(18, 4), bcond(GE, 16, 60), NOP]), None);
}

/// A nearer compare with its own guard after a b.hi bounds the index further (nxnative #977).
#[test]
fn a_nearer_compare_with_its_own_guard_bounds() {
    let mut c = Case::new();
    let tg = [40, 44, 48, 52, 56];
    c.table(&tg);
    let mut w = Case::base(4);
    w.extend([
        ldr_w(18),
        cmp_w(18, 4),
        bcond(HI, 16, 60),
        cmp_w(18, 1),
        bcond(HI, 24, 60),
        ldrsw(18, 4, 18),
        addr(18, 18, 4),
        br(18),
    ]);
    let n = 16 - w.len();
    assert_eq!(c.site(&pad(w, n)), t(&tg[..2]));
}

#[test]
fn b_ls_into_the_dispatch() {
    let mut c = Case::new();
    let tg = [36, 40, 44, 48, 52];
    c.table(&tg);
    let mut w = Case::base(4);
    w.extend([
        ldr_w(18),
        cmp_w(18, 4),
        bcond(LS, 16, 24),
        b(20, 64),
        ldrsw(18, 4, 18),
        addr(18, 18, 4),
        br(18),
    ]);
    assert_eq!(c.site(&pad(w, 6)), t(&tg));
}

/// Link's Awakening subsdk1+0xae9a4: cmp w21,#7; b.hi; mov w18,w21
#[test]
fn copy_after_the_check() {
    let mut c = Case::new();
    let tg: Vec<u32> = (0..5).map(|i| 64 + 4 * i).collect();
    c.table(&tg);
    let mut w = Case::base(4);
    w.extend([
        ldr_w(21),
        cmp_w(21, 4),
        bcond(HI, 16, 60),
        mov_w(18, 21),
        ldrsw(18, 4, 18),
        addr(18, 18, 4),
        br(18),
    ]);
    assert_eq!(c.site(&pad(w, 12)), t(&tg));
}

/// Link's Awakening GLSLC subsdk0+0x2f403c: and w16,w18,#0x3f; cmp w16,#0xe; b.hi; and x18,x18,#0x3f. subsdk0+0x1be6e8:
/// sub w0,w18,#0xd1; cmp w0,#0x31
#[test]
fn masked_copy_and_offset_checks() {
    let mut c = Case::new();
    let tg: Vec<u32> = (0..15).map(|i| 48 + 4 * i).collect();
    c.table(&tg);
    let mut w = Case::base(4);
    w.extend([
        ldr_w(18),
        and_w(16, 18, 6),
        cmp_w(16, 14),
        bcond(HI, 20, 44),
        and_x(18, 18, 6),
        ldrsw(18, 4, 18),
        addr(18, 18, 4),
        br(18),
    ]);
    assert_eq!(c.site(&pad(w, 20)), t(&tg));
    let mut c = Case::new();
    let tg: Vec<u32> = (0..50).map(|i| 44 + 4 * i).collect();
    c.table(&tg);
    let mut w = Case::base(4);
    w.extend([
        ldr_w(18),
        sub_w(0, 18, 0xd1),
        cmp_w(0, 0x31),
        bcond(HI, 20, 40),
        ldrsw(18, 4, 0),
        addr(18, 18, 4),
        br(18),
    ]);
    assert_eq!(c.site(&pad(w, 55)), t(&tg));
}

/// OCTOPATH TRAVELER main+0x1676f30: a 64-bit compare bounds a 64-bit index; a 32-bit compare only where the upper half
/// is known zero.
#[test]
fn compare_widths() {
    let mut c = Case::new();
    let tg: Vec<u32> = (0..5).map(|i| 36 + 4 * i).collect();
    c.table(&tg);
    let mut run = |load: u32, check: u32| {
        let mut w = Case::base(4);
        w.extend([
            load,
            check,
            bcond(HI, 16, 56),
            ldrsw(18, 4, 22),
            addr(18, 18, 4),
            br(18),
        ]);
        c.site(&pad(w, 10))
    };
    assert_eq!(run(ldr_x(22), cmp_x(22, 4)), t(&tg));
    assert_eq!(run(ldr_w(22), cmp_w(22, 4)), t(&tg));
    assert_eq!(
        run(ldr_x(22), cmp_w(22, 4)),
        None,
        "ldr x22: the upper half is unknown"
    );
}

/// ASTRAL CHAIN main+0x6db104: lsr x17,x11,#32; cmp w17,#5; b.hi: the shift leaves the upper half zero
#[test]
#[ignore = "this resolver does not prove it (nxnative jump_solver.py does): an index made by lsr / ubfx from a 64-bit load, bounded by a 32-bit compare"]
fn shifted_down_then_a_32_bit_compare() {
    let mut c = Case::new();
    let tg: Vec<u32> = (0..5).map(|i| 40 + 4 * i).collect();
    c.table(&tg);
    let mut run = |shift: u32| {
        let mut w = Case::base(4);
        w.extend([
            ldr_x(11),
            shift,
            cmp_w(22, 4),
            bcond(HI, 20, 60),
            ldrsw(18, 4, 22),
            addr(18, 18, 4),
            br(18),
        ]);
        c.site(&pad(w, 10))
    };
    assert_eq!(run(lsr_x(22, 11, 32)), t(&tg));
    assert_eq!(run(ubfx_x(22, 11, 8, 32)), t(&tg));
    assert_eq!(
        run(lsr_x(22, 11, 31)),
        None,
        "33 bits: the compare leaves the top one unknown"
    );
}

/// Mario Tennis Aces main+0x35837c: cmp w8,#4; b.hi; and x18,x8,#0xffffffff
#[test]
fn zero_extended_after_the_check() {
    let mut c = Case::new();
    let tg: Vec<u32> = (0..5).map(|i| 40 + 4 * i).collect();
    c.table(&tg);
    let mut w = Case::base(16);
    w.extend([
        ldr_x(8),
        cmp_w(8, 4),
        bcond(HI, 16, 60),
        zext(18, 8),
        ldrsw(18, 16, 18),
        addr(18, 18, 16),
        br(18),
    ]);
    assert_eq!(c.site(&pad(w, 8)), t(&tg));
}

/// Return of the Obra Dinn main+0x6eea88: tst w17,#7; b.eq; mvn w17,w17; and x18,x17,#7: 7 entries
#[test]
#[ignore = "this resolver does not prove it (nxnative jump_solver.py does): a tst / b.eq guard on a mask, then mvn: the excluded value of the complemented index"]
fn complemented_index() {
    let mut c = Case::new();
    let tg: Vec<u32> = (0..7).map(|i| 44 + 4 * i).collect();
    c.table(&tg);
    let mut w = Case::base(16);
    w.extend([
        ldr_w(17),
        tst_w(17, 3),
        bcond(EQ, 16, 72),
        mvn_w(17, 17),
        and_x(18, 17, 3),
        ldrsw(18, 16, 18),
        addr(18, 18, 16),
        br(18),
    ]);
    assert_eq!(c.site(&pad(w.clone(), 10)), t(&tg));
}

/// Tied Together: and w18,w17,#7; cmp w18,#7; b.eq: 7 entries
#[test]
#[ignore = "this resolver does not prove it (nxnative jump_solver.py does): a mask whose highest value b.eq sends away (cmp w, #7; b.eq after and #7)"]
fn mask_whose_highest_value_leaves() {
    let mut c = Case::new();
    let tg: Vec<u32> = (0..7).map(|i| 40 + 4 * i).collect();
    c.table(&tg);
    let mut w = Case::base(16);
    w.extend([
        ldr_w(17),
        and_w(18, 17, 3),
        cmp_w(18, 7),
        bcond(EQ, 20, 68),
        ldrsw(18, 16, 18),
        addr(18, 18, 16),
        br(18),
    ]);
    assert_eq!(c.site(&pad(w, 10)), t(&tg));
}

/// ULTRA STREET FIGHTER II main+0x590ee8: tbnz w17,#3; and x18,x17,#0xf: 8 entries
#[test]
#[ignore = "this resolver does not prove it (nxnative jump_solver.py does): a tbnz on the bit above a mask (tbnz w17, #3 before and x18, x17, #0xf)"]
fn bit_test_before_the_mask() {
    let mut c = Case::new();
    let tg: Vec<u32> = (0..8).map(|i| 44 + 4 * i).collect();
    c.table(&tg);
    let mut w = Case::base(13);
    w.extend([
        ldr_w(17),
        tbnz(17, 3, 12, 40),
        and_x(18, 17, 4),
        ldrsw(18, 13, 18),
        addr(18, 18, 13),
        br(18),
        RET,
    ]);
    assert_eq!(c.site(&pad(w, 10)), t(&tg));
    let _ = movz(0, 0);
}

//! Instruction classes: the A64 encoding groups the lifter lowers (ARM ARM C4.1 top-level decode), as a fixed-bit
//! mask and value; the other bits are drawn at random and a word is kept when the lifter lowers it and dynarmic runs it.

/// How a class touches memory: which registers must hold an address into the data window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mem {
    /// No memory access.
    None,
    /// Rn (bits 9:5) is the base; `align` is the base alignment (exclusives and atomics need natural alignment).
    Base { align: u64, writeback: Wb },
    /// Rn is the base, Rm (bits 20:16) a small signed offset register.
    BaseIndex { writeback: Wb },
    /// PC-relative literal (LDR literal): reads the code window.
    Literal,
}

/// Writeback of the base, and which data registers must differ from it (CONSTRAINED UNPREDICTABLE otherwise).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wb {
    No,
    /// Rt (bits 4:0) and, for pairs, Rt2 (bits 14:10) must not be Rn.
    Yes { pair: bool },
}

pub struct Class {
    pub name: &'static str,
    pub mask: u32,
    pub value: u32,
    pub mem: Mem,
    /// Extra fixed words to draw from instead of mask/value (system registers).
    pub words: &'static [u32],
}

const fn c(name: &'static str, mask: u32, value: u32, mem: Mem) -> Class {
    Class { name, mask, value, mem, words: &[] }
}

const NOWB: Wb = Wb::No;
const WB: Wb = Wb::Yes { pair: false };
const WB2: Wb = Wb::Yes { pair: true };
const B8: Mem = Mem::Base { align: 8, writeback: NOWB };

pub static CLASSES: &[Class] = &[
    // ---- data processing, immediate
    c("adr", 0x1F00_0000, 0x1000_0000, Mem::None),
    c("addsub_imm", 0x1F80_0000, 0x1100_0000, Mem::None),
    c("logic_imm", 0x1F80_0000, 0x1200_0000, Mem::None),
    c("movewide", 0x1F80_0000, 0x1280_0000, Mem::None),
    c("bitfield", 0x1F80_0000, 0x1300_0000, Mem::None),
    c("extract", 0x1F80_0000, 0x1380_0000, Mem::None),
    // ---- data processing, register
    c("logic_shifted", 0x1F00_0000, 0x0A00_0000, Mem::None),
    c("addsub_shifted", 0x1F20_0000, 0x0B00_0000, Mem::None),
    c("addsub_ext", 0x1F20_0000, 0x0B20_0000, Mem::None),
    c("adc", 0x1FE0_FC00, 0x1A00_0000, Mem::None),
    c("ccmp", 0x1FE0_0000, 0x1A40_0000, Mem::None),
    c("csel", 0x1FE0_0000, 0x1A80_0000, Mem::None),
    c("dp2_crc", 0x5FE0_0000, 0x1AC0_0000, Mem::None),
    c("dp1", 0x5FE0_0000, 0x5AC0_0000, Mem::None),
    c("dp3", 0x1F00_0000, 0x1B00_0000, Mem::None),
    // ---- branches (the next pc is compared)
    c("b_bl", 0x7C00_0000, 0x1400_0000, Mem::None),
    c("bcond", 0xFF00_0010, 0x5400_0000, Mem::None),
    c("cbz", 0x7E00_0000, 0x3400_0000, Mem::None),
    c("tbz", 0x7E00_0000, 0x3600_0000, Mem::None),
    c("br_blr_ret", 0xFF9F_FC1F, 0xD61F_0000, Mem::None),
    // ---- system: NZCV, FPCR, FPSR, TPIDR(RO)_EL0 moves, flag manipulation
    Class {
        name: "sys_regs",
        mask: 0xFFFF_FFE0,
        value: 0,
        mem: Mem::None,
        words: &[
            0xD53B_4200, 0xD51B_4200, // mrs/msr nzcv
            0xD53B_4400, 0xD53B_4420, // mrs fpcr, fpsr
            0xD51B_4420,              // msr fpsr
            0xD53B_D040, 0xD51B_D040, // mrs/msr tpidr_el0
            0xD53B_D060,              // mrs tpidrro_el0
        ],
    },
    // ---- loads and stores, general registers
    c("ldst_uimm", 0x3F00_0000, 0x3900_0000, B8),
    c("ldst_unscaled", 0x3F20_0C00, 0x3800_0000, B8),
    c("ldst_post", 0x3F20_0C00, 0x3800_0400, Mem::Base { align: 8, writeback: WB }),
    c("ldst_pre", 0x3F20_0C00, 0x3800_0C00, Mem::Base { align: 8, writeback: WB }),
    c("ldst_reg", 0x3F20_0C00, 0x3820_0800, Mem::BaseIndex { writeback: NOWB }),
    c("ldp_stp", 0x3F80_0000, 0x2900_0000, B8),
    c("ldp_stp_post", 0x3F80_0000, 0x2880_0000, Mem::Base { align: 8, writeback: WB2 }),
    c("ldp_stp_pre", 0x3F80_0000, 0x2980_0000, Mem::Base { align: 8, writeback: WB2 }),
    c("ldr_literal", 0x3B00_0000, 0x1800_0000, Mem::Literal),
    c("ldst_exclusive", 0x3F00_0000, 0x0800_0000, Mem::Base { align: 16, writeback: NOWB }),
    c("lse_atomic", 0x3F20_0C00, 0x3820_0000, Mem::Base { align: 16, writeback: NOWB }),
    c("lse_cas", 0x3FA0_7C00, 0x08A0_7C00, Mem::Base { align: 16, writeback: NOWB }),
    // ---- loads and stores, SIMD & FP registers
    c("ldst_uimm_fp", 0x3F00_0000, 0x3D00_0000, Mem::Base { align: 16, writeback: NOWB }),
    c("ldst_unscaled_fp", 0x3F20_0C00, 0x3C00_0000, B8),
    c("ldst_post_fp", 0x3F20_0C00, 0x3C00_0400, Mem::Base { align: 16, writeback: NOWB }),
    c("ldst_pre_fp", 0x3F20_0C00, 0x3C00_0C00, Mem::Base { align: 16, writeback: NOWB }),
    c("ldst_reg_fp", 0x3F20_0C00, 0x3C20_0800, Mem::BaseIndex { writeback: NOWB }),
    c("ldp_stp_fp", 0x3F80_0000, 0x2D00_0000, Mem::Base { align: 16, writeback: NOWB }),
    c("simd_ldst_multi", 0xBFBF_0000, 0x0C00_0000, Mem::Base { align: 16, writeback: NOWB }),
    c("simd_ldst_multi_post", 0xBFA0_0000, 0x0C80_0000, Mem::BaseIndex { writeback: NOWB }),
    c("simd_ldst_single", 0xBF9F_0000, 0x0D00_0000, Mem::Base { align: 16, writeback: NOWB }),
    // ---- scalar floating point
    c("fp_dp1", 0x5F20_7C00, 0x1E20_4000, Mem::None),
    c("fp_dp2", 0x5F20_0C00, 0x1E20_0800, Mem::None),
    c("fp_dp3", 0x5F00_0000, 0x1F00_0000, Mem::None),
    c("fp_compare", 0x5F20_3C00, 0x1E20_2000, Mem::None),
    c("fp_ccmp", 0x5F20_0C00, 0x1E20_0400, Mem::None),
    c("fp_csel", 0x5F20_0C00, 0x1E20_0C00, Mem::None),
    c("fp_imm", 0x5F20_1C00, 0x1E20_1000, Mem::None),
    c("fp_int_conv", 0x5F20_FC00, 0x1E20_0000, Mem::None),
    c("fp_fixed_conv", 0x5F20_0000, 0x1E00_0000, Mem::None),
    // ---- Advanced SIMD
    c("simd_three_same", 0x9F20_0400, 0x0E20_0400, Mem::None),
    c("simd_three_diff", 0x9F20_0C00, 0x0E20_0000, Mem::None),
    c("simd_two_misc", 0x9F3E_0C00, 0x0E20_0800, Mem::None),
    c("simd_across", 0x9F3E_0C00, 0x0E30_0800, Mem::None),
    c("simd_copy", 0x9FE0_8400, 0x0E00_0400, Mem::None),
    c("simd_mod_imm", 0x9FF8_0400, 0x0F00_0400, Mem::None),
    c("simd_shift_imm", 0x9F80_0400, 0x0F00_0400, Mem::None),
    c("simd_x_elem", 0x9F00_0400, 0x0F00_0000, Mem::None),
    c("simd_permute", 0xBF20_8C00, 0x0E00_0800, Mem::None),
    c("simd_ext", 0xBF20_8400, 0x2E00_0000, Mem::None),
    c("simd_tbl", 0xBF20_8C00, 0x0E00_0000, Mem::None),
    c("simd_scalar_three_same", 0xDF20_0400, 0x5E20_0400, Mem::None),
    c("simd_scalar_two_misc", 0xDF3E_0C00, 0x5E20_0800, Mem::None),
    c("simd_scalar_pairwise", 0xDF3E_0C00, 0x5E30_0800, Mem::None),
    c("simd_scalar_copy", 0xDFE0_8400, 0x5E00_0400, Mem::None),
    c("simd_scalar_shift_imm", 0xDF80_0400, 0x5F00_0400, Mem::None),
    c("simd_scalar_x_elem", 0xDF00_0400, 0x5F00_0000, Mem::None),
    // ---- crypto
    c("crypto_aes", 0xFF3E_0C00, 0x4E28_0800, Mem::None),
    c("crypto_sha3reg", 0xFF20_8C00, 0x5E00_0000, Mem::None),
    c("crypto_sha2reg", 0xFF3E_0C00, 0x5E28_0800, Mem::None),
];

pub fn rn(w: u32) -> u32 {
    (w >> 5) & 31
}
pub fn rm(w: u32) -> u32 {
    (w >> 16) & 31
}
pub fn rt(w: u32) -> u32 {
    w & 31
}
pub fn rt2(w: u32) -> u32 {
    (w >> 10) & 31
}

/// A drawn word the two sides may legitimately disagree on (CONSTRAINED UNPREDICTABLE register overlaps).
pub fn unpredictable(class: &Class, w: u32) -> bool {
    let n = rn(w);
    match class.mem {
        Mem::Base { writeback: Wb::Yes { pair }, .. } | Mem::BaseIndex { writeback: Wb::Yes { pair } } => {
            if n != 31 && (rt(w) == n || (pair && rt2(w) == n)) {
                return true;
            }
        }
        _ => {}
    }
    match class.name {
        // a register offset equal to the base makes the address twice the window's
        "ldst_reg" | "ldst_reg_fp" | "simd_ldst_multi_post" => rm(w) == n,
        // load pair into one register
        "ldp_stp" | "ldp_stp_post" | "ldp_stp_pre" | "ldp_stp_fp" => (w >> 22) & 1 == 1 && rt(w) == rt2(w),
        // store-exclusive status register overlapping the data or the base; load-pair exclusive into one register
        "ldst_exclusive" => {
            let s = rm(w);
            let store = (w >> 22) & 1 == 0 && (w >> 23) & 1 == 0;
            (store && (s == rt(w) || s == n || ((w >> 21) & 1 == 1 && s == rt2(w))))
                || ((w >> 22) & 1 == 1 && (w >> 21) & 1 == 1 && rt(w) == rt2(w))
                || ((w >> 23) & 1 == 1 && (w >> 21) & 1 == 1) // CAS / CASP family: its own rules, drawn separately below
        }
        _ => false,
    }
}

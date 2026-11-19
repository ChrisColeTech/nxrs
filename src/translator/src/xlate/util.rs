//! Bit helpers and C register-expression helpers.

/// An encoding the lifter does not handle; becomes an `rt_udf` trap with the message as a comment.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unsupported(pub String);

pub type LResult<T> = Result<T, Unsupported>;

/// `Err(Unsupported(msg))`.
pub fn unsup<T>(msg: impl Into<String>) -> LResult<T> {
    Err(Unsupported(msg.into()))
}

pub const COND_NAMES: [&str; 16] =
    ["eq", "ne", "cs", "cc", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "al", "nv"];

#[inline]
pub fn bits(w: u32, lo: u32, n: u32) -> u32 {
    (w >> lo) & ((1u32 << n) - 1)
}

/// Sign-extend the low n bits of v.
#[inline]
pub fn sext(v: u64, n: u32) -> i64 {
    if v & (1u64 << (n - 1)) != 0 {
        v as i64 - (1i64 << n)
    } else {
        v as i64
    }
}

/// (1 << n) - 1 for n in 0..=64 (as u128 so 64 works).
#[inline]
pub fn ones(n: u32) -> u64 {
    if n >= 64 {
        u64::MAX
    } else {
        (1u64 << n) - 1
    }
}

pub fn ror(v: u64, r: u32, size: u32) -> u64 {
    let r = r % size;
    if r == 0 {
        v
    } else {
        ((v >> r) | (v << (size - r))) & ones(size)
    }
}

pub fn replicate(v: u64, esize: u32, size: u32) -> u64 {
    let mut out = 0u64;
    let mut i = 0;
    while i < size {
        out |= v << i;
        i += esize;
    }
    out
}

/// AArch64 DecodeBitMasks: (wmask, tmask).
pub fn decode_bit_masks(n: u32, imms: u32, immr: u32, size: u32, immediate: bool) -> LResult<(u64, u64)> {
    let x = (n << 6) | (!imms & 0x3F);
    if x == 0 {
        return unsup("reserved bitmask");
    }
    let length = 31 - x.leading_zeros(); // bit_length - 1
    if length < 1 {
        return unsup("reserved bitmask");
    }
    let levels = ones(length) as u32;
    if immediate && (imms & levels) == levels {
        return unsup("reserved bitmask");
    }
    let s = imms & levels;
    let r = immr & levels;
    let diff = s.wrapping_sub(r) & 0x3F;
    let esize = 1u32 << length;
    let d = diff & levels;
    let welem = ones(s + 1);
    let telem = ones(d + 1);
    let wmask = replicate(ror(welem, r, esize), esize, size);
    let tmask = replicate(telem, esize, size);
    Ok((wmask, tmask))
}

/// `0x{v:x}ull`.
pub fn hx(v: u64) -> String {
    format!("0x{v:x}ull")
}

/// hx of a signed value the way Python formats a negative int: `0x-5ull`. (Callers normally mask first.)
pub fn hx_i(v: i128) -> String {
    if v < 0 {
        format!("-0x{:x}ull", -v)
    } else {
        format!("0x{v:x}ull")
    }
}

pub fn x(r: u32) -> String {
    if r == 31 { "0ull".into() } else { format!("c->x[{r}]") }
}

pub fn xs(r: u32) -> String {
    if r == 31 { "c->sp".into() } else { format!("c->x[{r}]") }
}

pub fn w(r: u32) -> String {
    if r == 31 { "0u".into() } else { format!("(uint32_t)c->x[{r}]") }
}

pub fn ws(r: u32) -> String {
    if r == 31 { "(uint32_t)c->sp".into() } else { format!("(uint32_t)c->x[{r}]") }
}

/// Python `R(sf, r, sp=False)`.
pub fn reg(sf: u32, r: u32, sp: bool) -> String {
    match (sf != 0, sp) {
        (true, true) => xs(r),
        (true, false) => x(r),
        (false, true) => ws(r),
        (false, false) => w(r),
    }
}

/// Python `set_reg(sf, r, expr, sp=False)`: statement writing expr to Rd (zero-extending 32-bit results).
pub fn set_reg(sf: u32, r: u32, expr: &str, sp: bool) -> String {
    if r == 31 && !sp {
        return format!("(void)({expr});");
    }
    let dst = if r == 31 { "c->sp".to_string() } else { format!("c->x[{r}]") };
    if sf != 0 {
        format!("{dst} = (uint64_t)({expr});")
    } else {
        format!("{dst} = (uint32_t)({expr});")
    }
}

/// Python `extend_expr(r, option, shift)`.
pub fn extend_expr(r: u32, option: u32, shift: u32) -> String {
    let base = x(r);
    let e = match option {
        0 => format!("(uint64_t)(uint8_t){base}"),
        1 => format!("(uint64_t)(uint16_t){base}"),
        2 => format!("(uint64_t)(uint32_t){base}"),
        4 => format!("(uint64_t)(int64_t)(int8_t){base}"),
        5 => format!("(uint64_t)(int64_t)(int16_t){base}"),
        6 => format!("(uint64_t)(int64_t)(int32_t){base}"),
        _ => base,
    };
    if shift != 0 { format!("({e} << {shift})") } else { e }
}

/// Python `cond_expr(cond)` / simd_c `cond(c)`.
pub fn cond_expr(cond: u32) -> String {
    if cond < 14 {
        format!("COND_{}(c)", COND_NAMES[cond as usize].to_uppercase())
    } else {
        "1".into()
    }
}

//! Function starts of a loaded module from its `.eh_frame_hdr` (found through MOD0 in guest memory), for the
//! interpreter's frames: an address belongs to the function whose start is the greatest one at or below it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

static CACHE: Mutex<Option<HashMap<u64, Arc<Vec<u64>>>>> = Mutex::new(None);

unsafe fn r32(a: u64) -> u32 {
    core::ptr::read_unaligned(a as *const u32)
}

/// One DWARF-encoded pointer at `a` (`base` for datarel): (value, size). Only the fixed-size formats.
unsafe fn encoded(a: u64, enc: u8, base: u64) -> Option<(i64, u64)> {
    let (v, n) = match enc & 0x0F {
        0x0B => (r32(a) as i32 as i64, 4),
        0x03 => (r32(a) as i64, 4),
        0x0C | 0x04 => (core::ptr::read_unaligned(a as *const i64), 8),
        _ => return None,
    };
    Some((
        match enc & 0x70 {
            0x10 => v.wrapping_add(a as i64),
            0x30 => v.wrapping_add(base as i64),
            _ => v,
        },
        n,
    ))
}

/// The sorted function starts of the module at `base` (`size` bytes mapped), empty without unwind tables.
unsafe fn read(base: u64, size: u64) -> Vec<u64> {
    let inside = |a: u64, n: u64| a >= base && a + n <= base + size;
    if !inside(base + 4, 4) {
        return vec![];
    }
    let mod0 = base + r32(base + 4) as u64;
    if !inside(mod0, 24) || r32(mod0) != u32::from_le_bytes(*b"MOD0") {
        return vec![];
    }
    let hdr = (mod0 as i64 + r32(mod0 + 16) as i32 as i64) as u64;
    if !inside(hdr, 4) {
        return vec![];
    }
    let b = |o: u64| core::ptr::read_unaligned((hdr + o) as *const u8);
    let (ptr_enc, cnt_enc, tbl_enc) = (b(1), b(2), b(3));
    let mut a = hdr + 4;
    let Some((_, n)) = encoded(a, ptr_enc, hdr) else { return vec![] };
    a += n;
    let Some((count, n)) = encoded(a, cnt_enc, hdr) else { return vec![] };
    a += n;
    let mut v = vec![];
    for _ in 0..count.clamp(0, 1 << 22) {
        if !inside(a, 16) {
            break;
        }
        let Some((loc, n)) = encoded(a, tbl_enc, hdr) else { break };
        a += n;
        let Some((_, n)) = encoded(a, tbl_enc, hdr) else { break };
        a += n;
        v.push(loc as u64);
    }
    v.sort_unstable();
    v
}

/// The start of the function holding `pc`, when its module has unwind tables.
pub fn function_of(pc: u64) -> Option<u64> {
    let m = crate::dispatch::table()?.module_at(pc)?;
    let starts = {
        let mut g = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let map = g.get_or_insert_with(HashMap::new);
        // SAFETY: the module is mapped at its base for its size
        map.entry(m.base).or_insert_with(|| Arc::new(unsafe { read(m.base, m.size) })).clone()
    };
    let k = starts.partition_point(|&s| s <= pc);
    (k > 0).then(|| starts[k - 1])
}

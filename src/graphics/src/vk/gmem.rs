//! Raw global memory: a shader's reads and writes at addresses it loaded from memory (Turbo 78ece7b, nxnative's
//! design; the translator's GlobalToStorage.TryGenerateRawStorageOp).
//!
//! Such an access cannot be given a buffer of its own. The translator reserves two storage buffers under constant
//! slot 255 (which no game has): at offset 0 the **memory pool table**, at offset 1 the **write log**. A shader finds
//! the pool holding an address (a tagged hint for its 2 MiB chunk, else a binary search over the table's starts), adds
//! that pool's delta (its buffer's device address minus its guest address) and goes through a physical storage
//! buffer pointer. Donkey Kong Bananza's terrain depends on it (gsys's vertex shader c9a56f796ea4a4cd writes each
//! chunk's indirect draw record through an address it reads from a list), as do Mario Kart World's
//! c9a56f796ea4a4cd / ce770ef0597876ca.
//!
//! - The table: 8,192 entries of 32 bytes {start, end (exclusive), delta, hint, 0}, sorted by start; entry 0 is empty and
//!   unused entries start at ~0. Hint k (in entry k's word 6) names the entry holding all of the 2 MiB chunks whose
//!   number is k modulo 8,192: chunk << 13 | entry, 0 = none. Pools are listed once their buffer exists. The table is
//!   built again when a pool comes, goes or gets its buffer, and staged once per submission that uses it.
//! - The log: a bitmap of the table's entries the shaders stored to or did atomics on, one per submission. At its
//!   completion the pools it names are copied back to guest memory by the next submission (the CPU may read what the
//!   GPU wrote there).

use super::queue::Sub;
use super::Gpu;
use ash::vk;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

pub const CB_SLOT: i32 = 0xFF;
const ENTRIES: usize = 8192;
const ENTRY: usize = 32;
const HINT_SHIFT: u32 = 21;
const LOG_BYTES: usize = ENTRIES / 8;
/// the largest pool copied back whole after a shader wrote into it through the table
const WB_MAX: u64 = 64 << 20;

struct Table {
    gen: u64,
    bytes: std::sync::Arc<Vec<u8>>,
    /// the listed ranges by entry index (entry 0 empty)
    ranges: std::sync::Arc<Vec<(u64, u64)>>,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);
/// pools a shader wrote through the table, to be copied back by the next submission
static WRITTEN: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

fn build(g: &Gpu) -> Table {
    let gen = super::mem::POOL_GEN.load(Ordering::Relaxed);
    let mut bytes = vec![0u8; ENTRIES * ENTRY];
    let mut ranges = vec![(0u64, 0u64)];
    let mut left = 0;
    {
        let m = g.mem.lock().unwrap();
        let mut last_end = 0u64;
        for p in m.by_base.values() {
            // one entry per buffer: a pool over the device's largest buffer lists each of its windows
            for (wlo, whi, da) in p.da_ranges() {
                // pools the game laid over each other: the first one has the overlap
                let (lo, hi) = (wlo.max(last_end), whi);
                if lo >= hi {
                    continue;
                }
                if ranges.len() == ENTRIES {
                    left += 1;
                    continue;
                }
                let i = ranges.len();
                let e = &mut bytes[i * ENTRY..(i + 1) * ENTRY];
                e[0..8].copy_from_slice(&lo.to_le_bytes());
                e[8..16].copy_from_slice(&hi.to_le_bytes());
                e[16..24].copy_from_slice(&da.wrapping_sub(wlo).to_le_bytes());
                ranges.push((lo, hi));
                last_end = hi;
            }
        }
    }
    for i in ranges.len()..ENTRIES {
        bytes[i * ENTRY..i * ENTRY + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    }
    // hints: the entry holding a whole 2 MiB chunk, tagged with the chunk (only addresses below 2^40 are tagged)
    for (i, &(lo, hi)) in ranges.iter().enumerate().skip(1) {
        let (c0, c1) = (lo.div_ceil(1 << HINT_SHIFT), hi >> HINT_SHIFT);
        for c in c0..c1 {
            if c >= 1 << (32 - 13) {
                break;
            }
            let slot = (c as usize) & (ENTRIES - 1);
            let h = &mut bytes[slot * ENTRY + 24..slot * ENTRY + 28];
            if u32::from_le_bytes(h.try_into().unwrap()) == 0 {
                h.copy_from_slice(&(((c as u32) << 13) | i as u32).to_le_bytes());
            }
        }
    }
    if left > 0 {
        crate::log::red_once(&format!("raw global memory: {left} more memory pools than the table's {ENTRIES} entries; reads through loaded pointers into them give 0"));
    }
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, Ordering::Relaxed) {
        crate::vklog!("raw global memory: a shader reads or writes through pointers it loaded from memory; the memory pool table has {} ranges (written again when pools change)", ranges.len() - 1);
    }
    Table { gen, bytes: std::sync::Arc::new(bytes), ranges: std::sync::Arc::new(ranges) }
}

/// The binding of constant slot 255 at word `word` (0: the table, 1: the log) for a draw of `sub`; None when the
/// device has no buffer device addresses (red line once).
pub fn binding(g: &Gpu, sub: &mut Sub, word: u32) -> Option<vk::DescriptorBufferInfo> {
    if !g.d.caps.bda {
        crate::log::red_once("raw global memory: the device has no buffer device addresses; shaders that read or write through loaded pointers get an empty table");
        return None;
    }
    if sub.gmem.is_none() {
        // a shader may reach any byte of a listed pool: pools backed in granules are bound whole first
        {
            let mut m = g.mem.lock().unwrap();
            for p in m.by_base.values_mut().filter(|p| p.gran.is_some() && p.da != 0) {
                let (base, size) = (p.base, p.size);
                if let Err(e) = p.bind(g, sub, base, size) {
                    crate::log::red_once(&e);
                }
            }
            // and every CPU write the upload budget left for later goes now
            for p in m.by_base.values_mut().filter(|p| !p.deferred.is_empty()) {
                p.upload_deferred(g, sub, 0, u64::MAX);
            }
        }
        let (bytes, ranges) = {
            let mut t = TABLE.lock().unwrap();
            if t.as_ref().is_none_or(|t| t.gen != super::mem::POOL_GEN.load(Ordering::Relaxed)) {
                *t = Some(build(g));
            }
            let t = t.as_ref().unwrap();
            (t.bytes.clone(), t.ranges.clone())
        };
        let table = sub.stage_bytes(g, &bytes);
        let log = sub.stage_bytes_fresh(g, &[0u8; LOG_BYTES]);
        sub.gmem = Some((table, log, ranges));
    }
    let (table, log, _) = sub.gmem.as_ref().unwrap();
    Some(match word {
        0 => vk::DescriptorBufferInfo { buffer: table.0, offset: table.1, range: (ENTRIES * ENTRY) as u64 },
        _ => vk::DescriptorBufferInfo { buffer: log.0, offset: log.1, range: LOG_BYTES as u64 },
    })
}

/// At the end of a recording: the log is read at its completion (the pools it names are written back next).
pub fn end(g: &Gpu, sub: &mut Sub) {
    let Some((_, (buf, off), ranges)) = sub.gmem.take() else { return };
    let Some(ptr) = sub.stage.iter().find(|b| b.buf == buf).map(|b| b.ptr as usize) else { return };
    let _ = g;
    sub.post.push(super::queue::Post::Callback(Box::new(move |_g: &Gpu| {
        // SAFETY: the submission's staging memory (host-visible, mapped) stays until it is recycled, after this
        let log = unsafe { core::slice::from_raw_parts((ptr + off as usize) as *const u32, LOG_BYTES / 4) };
        let mut w = WRITTEN.lock().unwrap();
        for (k, &word) in log.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let b = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if let Some(&(lo, hi)) = ranges.get(k * 32 + b) {
                    if hi - lo > WB_MAX {
                        crate::log::red_once(&format!("raw global memory: a shader wrote into a {} MB pool through a loaded pointer; pools over {} MB are not copied back to guest memory", (hi - lo) >> 20, WB_MAX >> 20));
                        continue;
                    }
                    if hi > lo && !w.contains(&(lo, hi - lo)) {
                        w.push((lo, hi - lo));
                        super::perf::inc(super::perf::C::GmemLearned);
                    }
                }
            }
        }
    })));
}

/// Start of a submission: the pools shaders wrote through the table are copied back to guest memory by it.
pub fn begin(g: &Gpu, sub: &mut Sub) {
    let w = std::mem::take(&mut *WRITTEN.lock().unwrap());
    for (a, n) in w {
        super::mem::note_gpu_write(g, sub, a, n);
    }
}

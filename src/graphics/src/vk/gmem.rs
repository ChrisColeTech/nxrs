//! Raw global memory: a shader's reads and writes at addresses it loaded from memory (nxnative's
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
//!   built again when a pool comes, goes or gets its buffer, and staged immutably per generation used by a submission.
//! - The log: a bitmap of the table's entries the shaders stored to or did atomics on, paired with each version. At its
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
    identities: std::sync::Arc<Vec<u64>>,
    /// more pool windows than entries: this map is incomplete and is never bound
    truncated: bool,
}

pub struct Version {
    pub table: (vk::Buffer, u64),
    pub log: (vk::Buffer, u64),
    pub ranges: std::sync::Arc<Vec<(u64, u64)>>,
    identities: std::sync::Arc<Vec<u64>>,
    pub generation: u64,
}

impl Version {
    pub(crate) fn token(&self) -> super::lost::bindings::RawVersion {
        super::lost::bindings::RawVersion {
            generation: self.generation,
            table: (vk::Handle::as_raw(self.table.0), self.table.1),
            log: (vk::Handle::as_raw(self.log.0), self.log.1),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Bindings {
    pub descriptors: [vk::DescriptorBufferInfo; 2],
    pub token: super::lost::bindings::RawVersion,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);
/// pools a shader wrote through the table, to be copied back by the next submission
static WRITTEN: Mutex<Vec<(u64, u64, u64)>> = Mutex::new(Vec::new());

fn build(g: &Gpu) -> Table {
    let m = g.mem.lock().unwrap();
    let gen = super::mem::POOL_GEN.load(Ordering::Acquire);
    let windows = visible_ranges(m.by_base.values().map(|p| (p.base, p.da_ranges())));
    let (mut table, _left) = encode(gen, &windows);
    table.identities = std::sync::Arc::new(table.ranges.iter().map(|&(lo, _)| {
        m.by_base.range(..=lo).next_back().map_or(0, |(_, p)| p.identity)
    }).collect());
    drop(m);
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, Ordering::Relaxed) {
        crate::vklog!("raw global memory: a shader reads or writes through pointers it loaded from memory; the memory pool table has {} ranges (written again when pools change)", table.ranges.len() - 1);
    }
    table
}

/// Match mem::find: the highest pool base at or below an address owns it. A later pool hides the earlier pool's
/// tail, including when it has no buffer yet or ends before the earlier pool. Keep buffer-window deltas intact.
/// Input includes ALL pools in ascending base order, not just the ones with device addresses.
fn visible_ranges(
    pools: impl Iterator<Item = (u64, Vec<(u64, u64, u64)>)>,
) -> Vec<(u64, u64, u64)> {
    let mut pools = pools.peekable();
    let mut out = Vec::new();
    while let Some((_, windows)) = pools.next() {
        let next = pools.peek().map(|p| p.0).unwrap_or(u64::MAX);
        for (lo, hi, da) in windows {
            let hi = hi.min(next);
            if lo < hi && da != 0 {
                out.push((lo, hi, da));
            }
        }
    }
    out
}

fn encode(gen: u64, windows: &[(u64, u64, u64)]) -> (Table, usize) {
    let mut bytes = vec![0u8; ENTRIES * ENTRY];
    let mut ranges = vec![(0u64, 0u64)];
    let mut left = 0;
    for &(lo, hi, da) in windows {
        if ranges.len() == ENTRIES {
            left += 1;
            continue;
        }
        let i = ranges.len();
        let e = &mut bytes[i * ENTRY..(i + 1) * ENTRY];
        e[0..8].copy_from_slice(&lo.to_le_bytes());
        e[8..16].copy_from_slice(&hi.to_le_bytes());
        e[16..24].copy_from_slice(&da.wrapping_sub(lo).to_le_bytes());
        ranges.push((lo, hi));
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
    (
        Table {
            gen,
            bytes: std::sync::Arc::new(bytes),
            ranges: std::sync::Arc::new(ranges),
            identities: std::sync::Arc::new(Vec::new()),
            truncated: left > 0,
        },
        left,
    )
}

/// The binding of constant slot 255 at word `word` (0: the table, 1: the log) for a draw of `sub`; None when the
/// device has no buffer device addresses (red line once).
pub fn binding(g: &Gpu, sub: &mut Sub, word: u32) -> Option<vk::DescriptorBufferInfo> {
    bindings(g, sub).map(|pair| pair.descriptors[usize::from(word != 0)])
}

/// Resolve both descriptors together after ordinary resources have been backed.
/// Old versions remain staged and their logs keep the original map through completion.
pub(crate) fn bindings(g: &Gpu, sub: &mut Sub) -> Option<Bindings> {
    if !g.d.caps.bda {
        crate::log::red_once("raw global memory: the device has no buffer device addresses; shaders that read or write through loaded pointers get an empty table");
        return None;
    }
    if sub.gmem.as_ref().is_some_and(|v| v.generation != super::mem::POOL_GEN.load(Ordering::Acquire)) {
        end(g, sub);
    }
    if sub.gmem.is_none() {
        // a shader may reach any byte of a listed pool: pools backed in granules are bound whole first
        {
            let mut m = g.mem.lock().unwrap();
            for p in m
                .by_base
                .values_mut()
                .filter(|p| p.gran.is_some() && p.da != 0)
            {
                let (base, size) = (p.base, p.size);
                if p.bind(g, sub, base, size).is_err() {
                    // No device memory to back the whole pool: a table naming its unbound granules is not
                    // bound. The draw gets the empty table (as without buffer device addresses).
                    crate::log::red_once("raw global memory: a granule pool could not be backed whole (device memory); shaders that read or write through loaded pointers get an empty table");
                    return None;
                }
            }
            // and every CPU write the upload budget left for later goes now
            for p in m.by_base.values_mut().filter(|p| !p.deferred.is_empty()) {
                p.upload_deferred(g, sub, 0, u64::MAX);
            }
        }
        let (bytes, ranges, identities, generation) = {
            let mut t = TABLE.lock().unwrap();
            if t.as_ref()
                .is_none_or(|t| t.gen != super::mem::POOL_GEN.load(Ordering::Relaxed))
            {
                *t = Some(build(g));
            }
            let t = t.as_ref().unwrap();
            if t.truncated {
                crate::log::red_once("raw global memory: more pool windows than the table's 8,191 entries; shaders that read or write through loaded pointers get an empty table");
                return None;
            }
            (t.bytes.clone(), t.ranges.clone(), t.identities.clone(), t.gen)
        };
        // Never content-deduplicate a generation: address reuse may encode identical
        // bytes but still requires distinct descriptor identity and a separate log.
        let table = sub.stage_bytes_fresh(g, &bytes);
        let log = sub.stage_bytes_fresh(g, &[0u8; LOG_BYTES]);
        sub.gmem = Some(Version { table, log, ranges, identities, generation });
    }
    let version = sub.gmem.as_ref().unwrap();
    let (table, log) = (version.table, version.log);
    Some(Bindings { token: version.token(), descriptors: [
        vk::DescriptorBufferInfo {
            buffer: table.0,
            offset: table.1,
            range: (ENTRIES * ENTRY) as u64,
        },
        vk::DescriptorBufferInfo {
            buffer: log.0,
            offset: log.1,
            range: LOG_BYTES as u64,
        },
    ] })
}

fn logged_ranges(log: &[u32], ranges: &[(u64, u64)], identities: &[u64]) -> Vec<(u64, u64, u64)> {
    let mut writes = Vec::new();
    for (k, &word) in log.iter().enumerate() {
        let mut bits = word;
        while bits != 0 {
            let b = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let i = k * 32 + b;
            if let (Some(&(lo, hi)), Some(&identity)) = (ranges.get(i), identities.get(i)) {
                if identity != 0 && hi > lo && hi - lo <= WB_MAX {
                    writes.push((lo, hi - lo, identity));
                }
            }
        }
    }
    writes
}

/// At the end of a recording: the log is read at its completion (the pools it names are written back next).
pub fn end(g: &Gpu, sub: &mut Sub) {
    let Some(version) = sub.gmem.take() else {
        return;
    };
    let (buf, off) = version.log;
    let ranges = version.ranges;
    let identities = version.identities;
    let Some(ptr) = sub
        .stage
        .iter()
        .find(|b| b.buf == buf)
        .map(|b| b.ptr as usize)
    else {
        return;
    };
    let _ = g;
    sub.post.push(super::queue::Post::Callback(Box::new(move |_g: &Gpu| {
        // SAFETY: the submission's staging memory (host-visible, mapped) stays until it is recycled, after this
        let log = unsafe { core::slice::from_raw_parts((ptr + off as usize) as *const u32, LOG_BYTES / 4) };
        let mut w = WRITTEN.lock().unwrap();
        for write in logged_ranges(log, &ranges, &identities) {
            if !w.contains(&write) {
                w.push(write);
                super::perf::inc(super::perf::C::GmemLearned);
            }
        }
    })));
}

/// Start of a submission: the pools shaders wrote through the table are copied back to guest memory by it.
pub fn begin(g: &Gpu, sub: &mut Sub) {
    let w = std::mem::take(&mut *WRITTEN.lock().unwrap());
    for (a, n, identity) in w {
        // Check identity under the same lock used to select the actual backing.
        super::mem::note_gpu_write_version(g, sub, a, n, Some(identity));
    }
}

/// One selected direct vertex draw: storage slot 0 is an array of u64 guest pointers indexed by VertexIndex,
/// each naming a five-u32 indirect record. This is c9a56f796ea4a4cd's ABI, not a title-specific rendering rule.
/// NXGFX_RAW_PROBE_SHADER=<16 hex digits>, NXGFX_RAW_PROBE_AT_S=106, NXGFX_RAW_PROBE_COUNT=8 (1..32).
/// CPU pointers only select transfer-copy candidates. GPU pointer readback validates each candidate before its
/// record is reported as evidence; a mismatch is explicit, never presented as the GPU pointer's destination.
/// One 256 KiB table snapshot and up to 4096 pointers (32 KiB); record readbacks stay capped at 32 initializer
/// samples and 32 consumer-selected records. Joining the full list avoids mistaking an unused prefix for no draws.
/// Consumers are matched on the same queue and frame. No new submits, waits, pipelines or pass boundaries.
pub mod probe {
    use super::*;
    use std::sync::{atomic::AtomicBool, OnceLock};

    const MAX_RECORDS: usize = 32;
    const MAX_POINTERS: usize = 4096;
    const RECORD_BYTES: u64 = 20;

    struct Config {
        key: [u8; 8],
        at: f64,
        count: usize,
    }

    fn config() -> Option<&'static Config> {
        static C: OnceLock<Option<Config>> = OnceLock::new();
        C.get_or_init(|| {
            let text = std::env::var("NXGFX_RAW_PROBE_SHADER").ok()?;
            if text.len() != 16 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                crate::vklog!("raw probe: expected a 16-digit shader key; disabled");
                return None;
            }
            let mut key = [0; 8];
            for (i, b) in key.iter_mut().enumerate() {
                *b = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
            }
            let at = std::env::var("NXGFX_RAW_PROBE_AT_S")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(106.0);
            if !at.is_finite() || at < 0.0 {
                return None;
            }
            let count = std::env::var("NXGFX_RAW_PROBE_COUNT")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(8)
                .clamp(1, MAX_RECORDS);
            Some(Config { key, at, count })
        })
        .as_ref()
    }

    #[derive(Clone)]
    struct Candidate {
        guest: u64,
        pool: Option<(u64, u64)>,
        device: Option<u64>,
        copy: Option<(vk::Buffer, u64)>,
        status: &'static str,
        consumed: bool,
    }

    pub struct Probe {
        key: [u8; 8],
        frame: u64,
        first: i32,
        count: i32,
        limit: usize,
        source_guest: u64,
        source: Option<vk::DescriptorBufferInfo>,
        issued: bool,
        captured: bool,
        summary_done: bool,
        candidates: Vec<Candidate>,
        pointers: std::collections::BTreeMap<u64, usize>,
        gpu_pointers: std::sync::Arc<OnceLock<Vec<u64>>>,
        snapshot: Option<std::sync::Arc<Vec<u8>>>,
        consumers: Vec<u64>,
        geometry: Vec<u64>,
        single_attempts: u64,
        multi_attempts: u64,
        geometry_attempts: u64,
        unmatched_logs: usize,
    }

    pub fn matches(key: &[u8; 8]) -> bool {
        config().is_some_and(|c| c.key == *key)
    }

    pub fn arm(key: [u8; 8], first: i32, count: i32, source_guest: u64) -> Option<Probe> {
        let c = config()?;
        if key != c.key || crate::log::elapsed() < c.at {
            return None;
        }
        static USED: AtomicBool = AtomicBool::new(false);
        if USED.swap(true, Ordering::Relaxed) {
            return None;
        }
        let frame = super::super::present::frames() + 1;
        crate::vklog!("raw probe: armed frame {frame} shader {} first {first} count {count} list {source_guest:#x}; max {} records, one draw, no forced pool creation", crate::nvn::prog::key_hex(&key), c.count);
        Some(Probe {
            key,
            frame,
            first,
            count,
            limit: c.count,
            source_guest,
            source: None,
            issued: false,
            captured: false,
            summary_done: false,
            candidates: Vec::new(),
            pointers: Default::default(),
            gpu_pointers: Default::default(),
            snapshot: None,
            consumers: Vec::new(),
            geometry: Vec::new(),
            single_attempts: 0,
            multi_attempts: 0,
            geometry_attempts: 0,
            unmatched_logs: 0,
        })
    }

    /// Window in the actual descriptor, not just the game's declared size (read-only storage may extend it).
    fn sample_window(first: i32, count: i32, limit: usize, range: u64) -> Option<(u64, usize)> {
        let (offset, n) = list_window(first, count, range)?;
        let n = n.min(limit.min(MAX_RECORDS));
        (n != 0).then_some((offset, n))
    }

    fn list_window(first: i32, count: i32, range: u64) -> Option<(u64, usize)> {
        if first < 0 || count <= 0 {
            return None;
        }
        let offset = (first as u64).checked_mul(8)?;
        let available = range.checked_sub(offset)? / 8;
        let n = (count as u64).min(MAX_POINTERS as u64).min(available) as usize;
        (n != 0).then_some((offset, n))
    }

    fn record_index(base: u64, guest: u64, stride: u32, max: u32) -> Option<u32> {
        if stride == 0 {
            return None;
        }
        let delta = guest.checked_sub(base)?;
        let index = delta / stride as u64;
        (delta % stride as u64 == 0 && index < max as u64).then_some(index as u32)
    }

    /// The exact snapshot already staged for this submission; do not use TABLE, which may have moved on.
    fn staged_table(sub: &Sub) -> Option<Vec<u8>> {
        let (buffer, offset) = &sub.gmem.as_ref()?.table;
        let b = sub.stage.iter().find(|b| b.buf == *buffer)?;
        let end = offset.checked_add((ENTRIES * ENTRY) as u64)?;
        if end > b.size {
            return None;
        }
        // Staging is mapped, initialized host memory owned by the not-yet-submitted Sub.
        Some(
            unsafe { core::slice::from_raw_parts(b.ptr.add(*offset as usize), ENTRIES * ENTRY) }
                .to_vec(),
        )
    }

    /// Reproduce the translator's tagged hint / predecessor search and exclusive-end check for a scalar store.
    fn table_entry(bytes: &[u8], guest: u64) -> Option<(usize, u64)> {
        if bytes.len() != ENTRIES * ENTRY {
            return None;
        }
        let u64_at = |i: usize, off: usize| {
            u64::from_le_bytes(
                bytes[i * ENTRY + off..i * ENTRY + off + 8]
                    .try_into()
                    .unwrap(),
            )
        };
        let chunk = guest >> HINT_SHIFT;
        let hpos = (chunk as usize & (ENTRIES - 1)) * ENTRY + 24;
        let hint = u32::from_le_bytes(bytes[hpos..hpos + 4].try_into().unwrap());
        let hinted = hint as usize & (ENTRIES - 1);
        let mut i = 0;
        if chunk < (1 << 19) && hinted != 0 && (hint >> 13) as u64 == chunk {
            i = hinted;
        } else {
            let mut step = ENTRIES / 2;
            while step != 0 {
                let candidate = i + step;
                if u64_at(candidate, 0) <= guest {
                    i = candidate;
                }
                step /= 2;
            }
        }
        (guest < u64_at(i, 8)).then(|| (i, guest.wrapping_add(u64_at(i, 16))))
    }

    fn record_for_pointer(
        data: &[u8],
        n: usize,
        i: usize,
        cpu: u64,
        copied: bool,
    ) -> Option<[u32; 5]> {
        if !copied || n > MAX_POINTERS || i >= n || i >= MAX_RECORDS {
            return None;
        }
        let gpu = u64::from_le_bytes(data.get(i * 8..i * 8 + 8)?.try_into().ok()?);
        if gpu != cpu {
            return None;
        }
        let offset = n * 8 + i * RECORD_BYTES as usize;
        let bytes = data.get(offset..offset + RECORD_BYTES as usize)?;
        Some(std::array::from_fn(|w| {
            u32::from_le_bytes(bytes[w * 4..w * 4 + 4].try_into().unwrap())
        }))
    }

    /// Observe existing buffer backing only. Calling mem::resolve here would hide a missing-table-entry defect by
    /// creating its target buffer. No guest access, binding, uploads, or nested locks while this lock is held.
    fn candidate(g: &Gpu, guest: u64) -> Candidate {
        let mut c = Candidate {
            guest,
            pool: None,
            device: None,
            copy: None,
            status: "no_pool",
            consumed: false,
        };
        if guest == 0 {
            c.status = "null";
            return c;
        }
        if guest % 4 != 0 {
            c.status = "unaligned";
            return c;
        }
        let Some(end) = guest.checked_add(RECORD_BYTES) else {
            c.status = "overflow";
            return c;
        };
        let m = g.mem.lock().unwrap();
        let Some((_, p)) = m.by_base.range(..=guest).next_back() else {
            return c;
        };
        c.pool = Some((p.base, p.size));
        if end > p.base.saturating_add(p.size) {
            c.status = "outside_pool";
            return c;
        }
        if p.da == 0 {
            c.status = "pool_without_buffer";
            return c;
        }
        c.device = p
            .da_ranges()
            .iter()
            .find(|&&(lo, hi, _)| guest >= lo && end <= hi)
            .map(|&(lo, _, da)| da + (guest - lo));
        if let Some(gr) = &p.gran {
            let (lo, hi) = super::super::mem::granules_of(gr.size, guest - p.base, RECORD_BYTES);
            if hi > gr.mem.len() || gr.mem[lo..hi].iter().any(Option::is_none) {
                c.status = "unbacked_granules";
                return c;
            }
        }
        c.copy = p.one(guest, RECORD_BYTES).map(|(b, o, _, _)| (b, o));
        if c.copy.is_some_and(|(_, o)| o % 4 != 0) {
            c.copy = None;
            c.status = "unaligned_buffer_offset";
            return c;
        }
        c.status = if c.copy.is_some() {
            "backed"
        } else {
            "crosses_buffer_window"
        };
        c
    }

    impl Probe {
        pub fn next_frame(&mut self) {
            if self.summary_done || super::super::present::frames() + 1 == self.frame {
                return;
            }
            self.summary_done = true;
            let pending: Vec<_> = self
                .candidates
                .iter()
                .filter(|c| !c.consumed)
                .map(|c| c.guest)
                .collect();
            crate::vklog!("raw probe: frame {} complete, issued {} captured {}, initializer-list {} unique pointers; single attempts {} multi commands {} geometry setups {}; {} consumer records captured, {} prefix samples matched of {}; unconsumed-prefix {pending:x?}",
                self.frame, self.issued, self.captured, self.pointers.len(), self.single_attempts, self.multi_attempts,
                self.geometry_attempts, self.consumers.len(), self.candidates.len() - pending.len(), self.candidates.len());
        }

        fn needs_binding(&self, key: &[u8; 8]) -> bool {
            !self.issued && !self.captured && self.key == *key
        }

        pub fn source_binding(
            &mut self,
            key: &[u8; 8],
            info: vk::DescriptorBufferInfo,
            zero: bool,
        ) {
            if self.needs_binding(key) {
                if zero {
                    // ZERO has no TRANSFER_SRC usage, and its contents are not a guest pointer list.
                    crate::vklog!(
                        "raw probe: source binding is the ZERO fallback; no pointer-list copy"
                    );
                } else {
                    self.source = Some(info);
                }
            }
        }

        pub fn issued(&mut self, key: &[u8; 8]) {
            if self.key == *key {
                self.issued = true;
            }
        }

        pub fn skipped(&mut self) {
            if !self.issued && !self.captured {
                self.captured = true;
                crate::vklog!("raw probe: selected producer draw was skipped before issue");
            }
        }

        pub fn after_init_pass(&mut self, g: &Gpu, sub: &mut Sub) {
            if !self.issued || self.captured {
                return;
            }
            self.captured = true;
            let Some(source) = self.source else {
                crate::vklog!(
                    "raw probe: selected draw had no captured VS c0+0x110 source binding"
                );
                return;
            };
            let Some(table) = staged_table(sub) else {
                crate::vklog!("raw probe: selected draw had no staged global-memory table");
                return;
            };
            let table = std::sync::Arc::new(table);
            self.snapshot = Some(table.clone());
            let Some((list_off, n)) =
                sample_window(self.first, self.count, self.limit, source.range)
            else {
                crate::vklog!("raw probe: pointer sample outside source descriptor, first {} count {} range {:#x}", self.first, self.count, source.range);
                return;
            };
            let (_, total) = list_window(self.first, self.count, source.range).unwrap();
            if source.offset % 4 != 0 {
                crate::vklog!("raw probe: unaligned source buffer offset; no transfer copy");
                return;
            }
            let Some(at) = self
                .source_guest
                .checked_add(list_off)
                .filter(|a| a.checked_add(total as u64 * 8).is_some())
            else {
                return;
            };
            // No-fault reads: lazy/uncommitted CPU pages yield zeros; the GPU copy below will expose any disagreement.
            let cpu = super::super::mem::guest_bytes(at, total as u64 * 8);
            for (i, b) in cpu.chunks_exact(8).enumerate() {
                let a = u64::from_le_bytes(b.try_into().unwrap());
                self.pointers.entry(a).or_insert(i);
            }
            let candidates: Vec<_> = cpu
                .chunks_exact(8)
                .take(n)
                .map(|b| candidate(g, u64::from_le_bytes(b.try_into().unwrap())))
                .collect();
            let bytes = total as u64 * 8 + n as u64 * RECORD_BYTES;
            let Ok(rb) = g.buffer(
                bytes,
                vk::BufferUsageFlags::TRANSFER_DST,
                gpu_allocator::MemoryLocation::GpuToCpu,
                "raw pointer probe",
            ) else {
                crate::vklog!("raw probe: readback allocation failed");
                return;
            };
            super::super::queue::full_barrier(g, sub.main);
            unsafe {
                g.dev().cmd_copy_buffer(
                    sub.main,
                    source.buffer,
                    rb.buf,
                    &[vk::BufferCopy {
                        src_offset: source.offset + list_off,
                        dst_offset: 0,
                        size: total as u64 * 8,
                    }],
                );
                for (i, c) in candidates.iter().enumerate() {
                    if let Some((b, o)) = c.copy {
                        g.dev().cmd_copy_buffer(
                            sub.main,
                            b,
                            rb.buf,
                            &[vk::BufferCopy {
                                src_offset: o,
                                dst_offset: total as u64 * 8 + i as u64 * RECORD_BYTES,
                                size: RECORD_BYTES,
                            }],
                        );
                    }
                }
            }
            super::super::queue::full_barrier(g, sub.main);
            let ptr = rb.ptr as usize;
            sub.transient.push(rb);
            let (frame, serial, first) = (self.frame, sub.serial, self.first);
            crate::vklog!("raw probe: after-init-pass frame {frame} sub {serial}, {total} of {} pointers joined, {n} prefix records at source buffer {:#x}+{:#x}, range {:#x}, snapshot {} entries, current pool generation {}",
                self.count,
                vk::Handle::as_raw(source.buffer), source.offset + list_off, source.range,
                sub.gmem.as_ref().unwrap().ranges.len() - 1, sub.gmem.as_ref().unwrap().generation);
            self.candidates = candidates.clone();
            let gpu_pointers = self.gpu_pointers.clone();
            sub.post.push(super::super::queue::Post::Callback(Box::new(move |_| {
                let data = unsafe { core::slice::from_raw_parts(ptr as *const u8, bytes as usize) };
                let _ = gpu_pointers.set(data[..total * 8].chunks_exact(8)
                    .map(|b| u64::from_le_bytes(b.try_into().unwrap())).collect());
                for (i, c) in candidates.iter().enumerate() {
                    let gpu = u64::from_le_bytes(data[i * 8..i * 8 + 8].try_into().unwrap());
                    let entries: Vec<_> = (0..5).map(|w| gpu.checked_add(w * 4).and_then(|a| table_entry(&table, a))).collect();
                    let same_pointer = gpu == c.guest;
                    let record = record_for_pointer(data, total, i, c.guest, c.copy.is_some());
                    let mapping = if !same_pointer { "CPU_GPU_POINTER_MISMATCH" }
                        else if entries.iter().any(Option::is_none) { "TABLE_MISS" }
                        else if c.copy.is_some() && c.device.is_some_and(|a| entries.iter().enumerate().all(|(w, e)| e.is_some_and(|(_, d)| d == a + w as u64 * 4))) { "SAME_BUFFER" }
                        else { "DIFFERENT_OR_UNBACKED_BUFFER" };
                    crate::vklog!("raw probe init: frame {frame} sub {serial} vertex {} CPU {:#x} GPU {gpu:#x} {mapping} table {entries:x?} pool(base,size) {:x?} {} device {:x?} after_init {record:?} expected [0, 1, 0, 0, 0]",
                        first as usize + i, c.guest, c.pool, c.status, c.device);
                }
            })));
        }

        pub fn before_indirect(
            &mut self,
            g: &Gpu,
            sub: &mut Sub,
            guest: u64,
            raw: (vk::Buffer, u64),
            used: (vk::Buffer, u64),
            size: u32,
        ) {
            if !self.captured
                || super::super::present::frames() + 1 != self.frame
                || !matches!(size, 16 | 20)
            {
                return;
            }
            self.single_attempts += 1;
            if !self.pointers.contains_key(&guest) && self.unmatched_logs < 4 {
                self.unmatched_logs += 1;
                let lower = self
                    .pointers
                    .range(..=guest)
                    .next_back()
                    .map(|(&a, &i)| (a, i, guest - a));
                crate::vklog!("raw probe unmatched: frame {} sub {} single {size}-byte record {guest:#x}, preceding initializer (address,index,delta) {lower:x?}", self.frame, sub.serial);
            }
            self.capture_consumer(g, sub, guest, raw, used, size, "single", 0, None);
        }

        pub fn geometry_record(&mut self, sub: &Sub, guest: u64, key: &[u8; 8]) {
            if !self.captured || super::super::present::frames() + 1 != self.frame {
                return;
            }
            self.geometry_attempts += 1;
            if self.geometry.len() >= self.limit || self.geometry.contains(&guest) {
                return;
            }
            self.geometry.push(guest);
            crate::vklog!("raw probe geometry: frame {} sub {} shader {} counter {guest:#x} initializer-index {:?} (binding observed at draw setup, not proof of a shader write)",
                self.frame, sub.serial, crate::nvn::prog::key_hex(key), self.pointers.get(&guest));
        }

        pub fn before_multi(
            &mut self,
            g: &Gpu,
            sub: &mut Sub,
            base: u64,
            raw: (vk::Buffer, u64),
            used: (vk::Buffer, u64),
            size: u32,
            stride: u32,
            max: u32,
            count: (vk::Buffer, u64),
        ) {
            if !self.captured || super::super::present::frames() + 1 != self.frame {
                return;
            }
            self.multi_attempts += 1;
            if !matches!(size, 16 | 20)
                || stride < size
                || stride % 4 != 0
                || raw.1 % 4 != 0
                || used.1 % 4 != 0
                || count.1 % 4 != 0
            {
                return;
            }
            let matches: Vec<_> = self
                .pointers
                .keys()
                .filter_map(|&a| record_index(base, a, stride, max).map(|i| (a, i)))
                .filter(|(a, _)| !self.consumers.contains(a))
                .take(self.limit - self.consumers.len())
                .collect();
            for (a, i) in matches {
                let off = i as u64 * stride as u64;
                self.capture_consumer(
                    g,
                    sub,
                    a,
                    (raw.0, raw.1 + off),
                    (used.0, used.1 + off),
                    size,
                    "multi",
                    i,
                    Some(count),
                );
            }
        }

        fn capture_consumer(
            &mut self,
            g: &Gpu,
            sub: &mut Sub,
            guest: u64,
            raw: (vk::Buffer, u64),
            used: (vk::Buffer, u64),
            size: u32,
            kind: &'static str,
            index: u32,
            count: Option<(vk::Buffer, u64)>,
        ) {
            let Some(&list_index) = self.pointers.get(&guest) else {
                return;
            };
            if self.consumers.len() >= self.limit || self.consumers.contains(&guest) {
                return;
            }
            self.consumers.push(guest);
            for c in &mut self.candidates {
                if c.guest == guest {
                    c.consumed = true;
                }
            }
            let bytes = 2 * size as u64 + if count.is_some() { 4 } else { 0 };
            let Ok(rb) = g.buffer(
                bytes,
                vk::BufferUsageFlags::TRANSFER_DST,
                gpu_allocator::MemoryLocation::GpuToCpu,
                "raw indirect probe",
            ) else {
                return;
            };
            super::super::queue::full_barrier(g, sub.main);
            for (i, (b, o)) in [raw, used].into_iter().enumerate() {
                unsafe {
                    g.dev().cmd_copy_buffer(
                        sub.main,
                        b,
                        rb.buf,
                        &[vk::BufferCopy {
                            src_offset: o,
                            dst_offset: i as u64 * size as u64,
                            size: size as u64,
                        }],
                    );
                }
            }
            if let Some((b, o)) = count {
                unsafe {
                    g.dev().cmd_copy_buffer(
                        sub.main,
                        b,
                        rb.buf,
                        &[vk::BufferCopy {
                            src_offset: o,
                            dst_offset: 2 * size as u64,
                            size: 4,
                        }],
                    );
                }
            }
            super::super::queue::full_barrier(g, sub.main);
            let ptr = rb.ptr as usize;
            sub.transient.push(rb);
            let (frame, serial) = (self.frame, sub.serial);
            let device = super::super::mem::buffer_address(raw.0).map(|a| a + raw.1);
            let gpu_pointers = self.gpu_pointers.clone();
            let entries: Vec<_> = (0..size / 4)
                .map(|w| {
                    guest
                        .checked_add(w as u64 * 4)
                        .and_then(|a| self.snapshot.as_ref().and_then(|t| table_entry(t, a)))
                })
                .collect();
            sub.post.push(super::super::queue::Post::Callback(Box::new(move |_| {
                let words = unsafe { core::slice::from_raw_parts(ptr as *const u32, bytes as usize / 4) };
                let raw = &words[..size as usize / 4];
                let used = &words[size as usize / 4..size as usize / 2];
                let count = words.get(size as usize / 2);
                let gpu = gpu_pointers.get().and_then(|p| p.get(list_index)).copied();
                crate::vklog!("raw probe consume: frame {frame} sub {serial} {kind} candidate {guest:#x} initializer-index {list_index} GPU-pointer {gpu:x?} verified {} init-table {entries:x?} device {device:x?} raw {raw:?} clamped {used:?} record-index {index} GPU-count {count:?} (before draw setup; single path after append handling)", gpu == Some(guest));
            })));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sample_window_honors_vertex_first_descriptor_end_and_hard_cap() {
            assert_eq!(sample_window(3, 96, 8, 1024), Some((24, 8)));
            assert_eq!(sample_window(3, 96, 8, 39), Some((24, 1)));
            assert_eq!(sample_window(3, 96, 8, 24), None);
            assert_eq!(sample_window(-1, 96, 8, 1024), None);
            assert_eq!(sample_window(0, 0, 8, 1024), None);
            assert_eq!(
                sample_window(0, 96, usize::MAX, u64::MAX),
                Some((0, MAX_RECORDS))
            );
        }

        #[test]
        fn snapshot_lookup_distinguishes_missing_target_and_word_crossing_end() {
            let (t, _) = super::super::encode(1, &[(0x1000, 0x1010, 0x8000)]);
            assert_eq!(table_entry(&t.bytes, 0x100c), Some((1, 0x800c)));
            assert_eq!(table_entry(&t.bytes, 0x1010), None);
            let (new, _) =
                super::super::encode(2, &[(0x1000, 0x1010, 0x8000), (0x2000, 0x3000, 0x9000)]);
            assert_eq!(
                table_entry(&t.bytes, 0x2000),
                None,
                "the staged snapshot must stay old"
            );
            assert_eq!(table_entry(&new.bytes, 0x2000), Some((2, 0x9000)));
            let chunk = 1 << HINT_SHIFT;
            let high = (1 << 40) + chunk; // same hint slot, outside the tag's supported address range
            let (tagged, _) = super::super::encode(
                3,
                &[(chunk, 2 * chunk, 0x1000), (high, high + chunk, 0x9000)],
            );
            assert_eq!(table_entry(&tagged.bytes, chunk + 4), Some((1, 0x1004)));
            assert_eq!(table_entry(&tagged.bytes, high + 4), Some((2, 0x9004)));
        }

        #[test]
        fn gpu_pointer_must_validate_candidate_before_reporting_record() {
            let mut data = [0u8; 56]; // two pointers followed by two five-word records
            data[8..16].copy_from_slice(&0x1234u64.to_le_bytes());
            data[40..44].copy_from_slice(&1u32.to_le_bytes());
            assert_eq!(
                record_for_pointer(&data, 2, 1, 0x1234, true),
                Some([0, 1, 0, 0, 0])
            );
            assert_eq!(record_for_pointer(&data, 2, 1, 0x5678, true), None);
            assert_eq!(record_for_pointer(&data, 2, 1, 0x1234, false), None);
            assert_eq!(record_for_pointer(&data[..55], 2, 1, 0x1234, true), None);
        }

        #[test]
        fn consumer_join_is_not_limited_to_the_initializer_record_sample() {
            assert_eq!(list_window(0, 1482, 0x22b40), Some((0, 1482)));
            assert_eq!(sample_window(0, 1482, 16, 0x22b40), Some((0, 16)));
            let mut readback = vec![0u8; 1482 * 8 + 20];
            readback[..8].copy_from_slice(&0x1000u64.to_le_bytes());
            readback[1482 * 8 + 4..1482 * 8 + 8].copy_from_slice(&1u32.to_le_bytes());
            assert_eq!(
                record_for_pointer(&readback, 1482, 0, 0x1000, true),
                Some([0, 1, 0, 0, 0])
            );
            assert_eq!(list_window(0, 10000, u64::MAX), Some((0, MAX_POINTERS)));
            assert_eq!(record_index(0x1000, 0x10c0, 64, 4), Some(3));
            assert_eq!(record_index(0x1000, 0x1000 + 900 * 64, 64, 1482), Some(900));
            assert_eq!(record_index(0x1000, 0x1100, 64, 4), None);
            assert_eq!(record_index(0x1000, 0x1004, 64, 4), None);
            assert_eq!(record_index(0x1000, 0xffc, 64, 4), None);
            assert_eq!(record_index(0x1000, 0x1000, 0, 4), None);
            assert_eq!(record_index(0x1000, 0x1000, 64, 0), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_learned_pair(guest: &mut [u8], before_b: &[u8], after_b: &[u8],
        schedule_identity: u64, completion_identity: u64) {
        // Exact production learn/decode, identity admission, queue copy plan and
        // merge functions, with CPU byte fixtures at their Vulkan-copy boundary.
        let writes = logged_ranges(&[2], &[(0, 0), (0x1000, 0x1000 + guest.len() as u64)], &[0, 7]);
        for (_, _, identity) in writes {
            if !super::super::mem::writeback_identity_matches(Some(identity), schedule_identity) { continue; }
            let (mut baseline, mut result) = (Vec::new(), Vec::new());
            super::super::queue::record_writeback_pair(true, |copy| match copy {
                super::super::queue::WritebackCopy::Baseline => baseline = before_b.to_vec(),
                super::super::queue::WritebackCopy::Result => result = after_b.to_vec(),
            });
            if super::super::mem::writeback_identity_matches(Some(identity), completion_identity) {
                super::super::mem::merge_changed_bytes(guest, &result, &baseline);
            }
        }
    }

    #[test]
    #[ignore = "CPU-only known GAR-5 failure: learned raw writes lack the producer baseline"]
    fn raw_only_producer_delta_must_survive_learning_in_a_later_submission() {
        let mut guest = [0, 0];
        // A raw-only store changed device[0] from 0 to 1. Only after A completes
        // does its bitmap schedule B. Both actual B copy phases see device[0]==1.
        complete_learned_pair(&mut guest, &[1, 0], &[1, 0], 7, 7);
        assert_eq!(guest, [1, 0], "GAR-5: the current B baseline loses A's 0->1 delta");
    }

    #[test]
    #[ignore = "CPU-only known GAR-5 failure: need producer provenance while preserving concurrent CPU bytes"]
    fn raw_only_delta_must_preserve_an_unrelated_concurrent_cpu_write() {
        let mut guest = [0, 9];
        complete_learned_pair(&mut guest, &[1, 0], &[1, 0], 7, 7);
        assert_eq!(guest, [1, 9]);
    }

    #[test]
    fn learned_pair_refuses_replacement_at_schedule_and_completion() {
        for (schedule, completion) in [(8, 8), (7, 8)] {
            let mut replacement = [5, 9];
            complete_learned_pair(&mut replacement, &[0, 0], &[1, 0], schedule, completion);
            assert_eq!(replacement, [5, 9]);
        }
    }

    #[test]
    fn ordinary_explicit_producer_pair_lands_delta_and_keeps_cpu_neighbors() {
        // Explicit writeback was registered in the producer itself, so its actual
        // pre/main copy plan observes the correct before=0 and after=1.
        let mut guest = [0, 9];
        let (mut baseline, mut result) = (Vec::new(), Vec::new());
        super::super::queue::record_writeback_pair(true, |copy| match copy {
            super::super::queue::WritebackCopy::Baseline => baseline = vec![0, 0],
            super::super::queue::WritebackCopy::Result => result = vec![1, 0],
        });
        assert!(super::super::mem::writeback_identity_matches(Some(7), 7));
        assert_eq!(super::super::mem::merge_changed_bytes(&mut guest, &result, &baseline), 1);
        assert_eq!(guest, [1, 9]);
    }

    #[test]
    fn overfull_table_is_marked_truncated_not_bound() {
        let windows: Vec<(u64, u64, u64)> = (1..=ENTRIES as u64)
            .map(|k| (k << 16, (k << 16) + 0x100, k << 32))
            .collect();
        let (full, left) = encode(1, &windows[..ENTRIES - 1]);
        assert_eq!(left, 0);
        assert!(!full.truncated);
        let (over, left) = encode(2, &windows);
        assert_eq!(left, 1);
        assert!(over.truncated);
    }

    #[test]
    fn late_backing_and_address_reuse_keep_old_table_and_log_meaning() {
        let (old, _) = encode(10, &[(0x2000, 0x3000, 0x8000)]);
        // A new lower-base pool shifts old entry 1 to entry 2. The old log must
        // still decode entry 1 against its original map, never against `new`.
        let (new, _) = encode(11, &[(0x1000, 0x1800, 0x9000), (0x2000, 0x3000, 0xa000)]);
        assert_eq!(lookup(&old, 0x1004), None);
        assert_eq!(lookup(&new, 0x1004), Some(0x9004));
        assert_eq!(lookup(&old, 0x2004), Some(0x8004));
        assert_eq!(lookup(&new, 0x2004), Some(0xa004));
        assert_eq!(logged_ranges(&[2], &old.ranges, &[0, 7]), vec![(0x2000, 0x1000, 7)]);
        assert_eq!(logged_ranges(&[2], &new.ranges, &[0, 8, 9]), vec![(0x1000, 0x800, 8)]);
        assert_eq!(logged_ranges(&[4], &new.ranges, &[0, 8, 9]), vec![(0x2000, 0x1000, 9)]);
        assert!(logged_ranges(&[u32::MAX], &old.ranges, &[0]).is_empty());
    }

    fn lookup(t: &Table, a: u64) -> Option<u64> {
        let word = |i: usize, off: usize| {
            u64::from_le_bytes(
                t.bytes[i * ENTRY + off..i * ENTRY + off + 8]
                    .try_into()
                    .unwrap(),
            )
        };
        // The shader first tries a tagged 2 MiB hint, then searches starts and checks the exclusive end.
        let chunk = a >> HINT_SHIFT;
        let h = u32::from_le_bytes(
            t.bytes[(chunk as usize & (ENTRIES - 1)) * ENTRY + 24..][..4]
                .try_into()
                .unwrap(),
        );
        let hinted = (h as usize) & (ENTRIES - 1);
        let i = if hinted != 0 && (h >> 13) as u64 == chunk {
            hinted
        } else {
            (0..ENTRIES).rfind(|&i| word(i, 0) <= a)?
        };
        (a < word(i, 8)).then(|| a.wrapping_add(word(i, 16)))
    }

    #[test]
    fn overlap_uses_highest_pool_base_and_does_not_resume_outer_tail() {
        let pools = [
            (0x1000, vec![(0x1000, 0x9000, 0x10000)]),
            (0x3000, vec![(0x3000, 0x5000, 0x20000)]),
            (0xa000, vec![(0xa000, 0xb000, 0x30000)]),
        ];
        let (t, left) = encode(1, &visible_ranges(pools.into_iter()));
        assert_eq!(left, 0);
        assert_eq!(
            &*t.ranges,
            &[(0, 0), (0x1000, 0x3000), (0x3000, 0x5000), (0xa000, 0xb000)]
        );
        assert_eq!(lookup(&t, 0x2ffc), Some(0x11ffc));
        assert_eq!(lookup(&t, 0x3004), Some(0x20004));
        assert_eq!(lookup(&t, 0x5000), None);
        assert_eq!(lookup(&t, 0x8000), None);
        assert_eq!(lookup(&t, 0xa000), Some(0x30000));
    }

    #[test]
    fn unbacked_pool_still_shadows_an_older_buffer_and_its_hint() {
        let chunk = 1 << HINT_SHIFT;
        let pools = [
            (chunk, vec![(chunk, 4 * chunk, 0x1000)]),
            (2 * chunk + 4096, vec![]),
        ];
        let (t, _) = encode(1, &visible_ranges(pools.into_iter()));
        assert_eq!(lookup(&t, chunk + 4), Some(0x1004));
        assert_eq!(lookup(&t, 2 * chunk), Some(0x1000 + chunk));
        assert_eq!(lookup(&t, 2 * chunk + 4096), None);
        assert_eq!(lookup(&t, 3 * chunk), None);
    }

    #[test]
    fn overlap_clips_split_windows_without_changing_device_deltas() {
        let pools = [
            (
                0x1000,
                vec![
                    (0x1000, 0x3000, 0x9000),
                    (0x3000, 0x5000, 0x100),
                    (0x5000, 0x7000, 0xd000),
                ],
            ),
            (0x4000, vec![(0x4000, 0x6000, 0xf000)]),
        ];
        let (t, _) = encode(1, &visible_ranges(pools.into_iter()));
        assert_eq!(lookup(&t, 0x3004), Some(0x104));
        assert_eq!(lookup(&t, 0x3ffc), Some(0x10fc));
        assert_eq!(lookup(&t, 0x4000), Some(0xf000));
        assert_eq!(lookup(&t, 0x6000), None);
    }
}

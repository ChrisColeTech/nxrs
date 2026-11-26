//! CPU-written pages whose bytes are what the GPU copy already holds are not uploaded again.
//!
//! The write watch reports pages the CPU wrote, not pages that changed. Ocarina of Time rewrites ~50 MB a frame of
//! identical bytes into double-buffered mirrors in Kokiri Forest (98% unchanged, `NXGFX_UPLOAD_TRACE`), and the pool
//! sync uploaded all of it every frame. Each whole page uploaded from a mirror keeps a hash of the bytes uploaded; a
//! page written again with the same bytes is left out. A page's hash is forgotten when anything else changes the GPU
//! copy (a GPU write, another upload path, the pool going), so a skip only ever happens when the GPU copy is exactly
//! the last upload of those bytes. `NXGFX_UPLOAD_SKIP=0`: every written page is uploaded, as before.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const PAGE: u64 = 4096;

/// pool base -> (pool end, each page's hash as last uploaded; 0: unknown)
static H: Mutex<BTreeMap<u64, (u64, Vec<u64>)>> = Mutex::new(BTreeMap::new());
static SKIPPED: AtomicU64 = AtomicU64::new(0);
/// whole pages uploaded: no hash kept (first upload, or forgotten), or different bytes; bytes forgotten
static UNKNOWN: AtomicU64 = AtomicU64::new(0);
static DIFFER: AtomicU64 = AtomicU64::new(0);
static FORGOT: AtomicU64 = AtomicU64::new(0);

pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_UPLOAD_SKIP").as_deref() != Ok("0"))
}

/// xxHash64's round over 32-byte stripes, finished with its avalanche (never 0).
pub fn page_hash(b: &[u8]) -> u64 {
    const P1: u64 = 0x9E37_79B1_85EB_CA87;
    const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
    const P3: u64 = 0x1656_67B1_9E37_79F9;
    let round = |acc: u64, w: u64| acc.wrapping_add(w.wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1);
    let mut v = [P1.wrapping_add(P2), P2, 0, 0u64.wrapping_sub(P1)];
    let mut chunks = b.chunks_exact(32);
    for c in &mut chunks {
        for (k, lane) in v.iter_mut().enumerate() {
            *lane = round(*lane, u64::from_le_bytes(c[8 * k..8 * k + 8].try_into().unwrap()));
        }
    }
    let mut h = v[0]
        .rotate_left(1)
        .wrapping_add(v[1].rotate_left(7))
        .wrapping_add(v[2].rotate_left(12))
        .wrapping_add(v[3].rotate_left(18));
    for lane in v {
        h = (h ^ round(0, lane)).wrapping_mul(P1).wrapping_add(P3);
    }
    h = h.wrapping_add(b.len() as u64);
    for &x in chunks.remainder() {
        h = (h ^ (x as u64).wrapping_mul(P3)).rotate_left(11).wrapping_mul(P1);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(P2);
    h ^= h >> 29;
    h = h.wrapping_mul(P3);
    h ^= h >> 32;
    h.max(1)
}

/// The parts of [a, a + n) (CPU-written, in the pool [base, base + size)) to upload: whole pages whose bytes differ
/// from their last upload, and every partial page. The hashes of what is returned are kept as uploaded.
pub fn changed_runs(base: u64, size: u64, a: u64, n: u64) -> Vec<(u64, u64)> {
    if !on() {
        return vec![(a, n)];
    }
    let end = base + size;
    let mut h = H.lock().unwrap();
    let entry = h.entry(base).or_insert_with(|| (end, Vec::new()));
    if entry.0 != end {
        *entry = (end, Vec::new());
    }
    let pages = &mut entry.1;
    let first = base & !(PAGE - 1);
    let need = (end - first).div_ceil(PAGE) as usize;
    if pages.len() < need {
        pages.resize(need, 0);
    }
    let mut out: Vec<(u64, u64)> = Vec::new();
    let mut push = |s: u64, e: u64| match out.last_mut() {
        Some(l) if l.0 + l.1 == s => l.1 += e - s,
        _ => out.push((s, e - s)),
    };
    let mut p = a & !(PAGE - 1);
    while p < a + n {
        let (ps, pe) = (p.max(base), (p + PAGE).min(end));
        let (s, e) = (ps.max(a), pe.min(a + n));
        let i = ((p - first) / PAGE) as usize;
        if s == ps && e == pe {
            // SAFETY: the CPU-written guest bytes the upload would copy (committed memory of a registered pool)
            let b = unsafe { core::slice::from_raw_parts(s as *const u8, (e - s) as usize) };
            let x = page_hash(b);
            if pages[i] == x {
                SKIPPED.fetch_add(e - s, Ordering::Relaxed);
            } else {
                (if pages[i] == 0 { &UNKNOWN } else { &DIFFER }).fetch_add(e - s, Ordering::Relaxed);
                pages[i] = x;
                push(s, e);
            }
        } else if e > s {
            pages[i] = 0;
            push(s, e);
        }
        p += PAGE;
    }
    out
}

/// The GPU copy of [a, a + n) changes by other means (a GPU write, another upload): its pages' hashes are forgotten.
pub fn forget(a: u64, n: u64) {
    if !on() || n == 0 {
        return;
    }
    FORGOT.fetch_add(n, Ordering::Relaxed);
    if super::uptrace::on() {
        super::uptrace::note_forget(a, n);
    }
    let mut h = H.lock().unwrap();
    // every pool that overlaps the range (pools do not overlap: below the first that ends by `a`, none reaches it)
    let keys: Vec<u64> = h
        .range(..a + n)
        .rev()
        .take_while(|(_, (end, _))| *end > a)
        .map(|(k, _)| *k)
        .collect();
    for base in keys {
        let (_, pages) = h.get_mut(&base).unwrap();
        let first = base & !(PAGE - 1);
        let lo = (a.max(first) - first) / PAGE;
        let hi = ((a + n - first).div_ceil(PAGE)).min(pages.len() as u64);
        for x in pages.iter_mut().take(hi as usize).skip(lo as usize) {
            *x = 0;
        }
    }
}

/// A pool is made or goes: nothing is known of its GPU copy.
pub fn drop_pool(base: u64) {
    H.lock().unwrap().remove(&base);
}

/// The bytes left out since the last report, per frame.
pub fn report(frames: f64) -> String {
    if !on() {
        return String::new();
    }
    let mb = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / frames / 1048576.0;
    let (s, u, d, f) = (mb(&SKIPPED), mb(&UNKNOWN), mb(&DIFFER), mb(&FORGOT));
    format!("; {s:.2} MB a frame of CPU-written pages not uploaded (same bytes as their last upload), whole pages uploaded {u:.2} MB with no hash kept and {d:.2} MB changed, {f:.2} MB forgotten (other GPU-copy changes)")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page written again with its last upload's bytes is left out; changed, partial, or forgotten (GPU-written)
    /// pages are uploaded.
    #[test]
    fn unchanged_pages_are_not_uploaded_again() {
        let mut mem = vec![0u8; 5 * PAGE as usize];
        let base0 = mem.as_mut_ptr() as u64;
        // a page-aligned pool of 4 pages inside the allocation
        let base = (base0 + PAGE - 1) & !(PAGE - 1);
        let size = 4 * PAGE;
        drop_pool(base);
        assert_eq!(changed_runs(base, size, base, size), vec![(base, size)]);
        assert_eq!(changed_runs(base, size, base, size), vec![], "same bytes: nothing");
        unsafe { *((base + PAGE + 5) as *mut u8) = 9 };
        assert_eq!(changed_runs(base, size, base, size), vec![(base + PAGE, PAGE)]);
        forget(base + 2 * PAGE + 100, 8);
        assert_eq!(changed_runs(base, size, base, size), vec![(base + 2 * PAGE, PAGE)]);
        // a partial page is always uploaded, and its hash forgotten
        assert_eq!(changed_runs(base, size, base + 10, 20), vec![(base + 10, 20)]);
        assert_eq!(changed_runs(base, size, base, PAGE), vec![(base, PAGE)]);
        drop_pool(base);
        assert_eq!(changed_runs(base, size, base, PAGE), vec![(base, PAGE)]);
        drop_pool(base);
        assert_ne!(page_hash(&[0u8; 4096]), page_hash(&[0u8; 4095]));
        let mut a = [0u8; 4096];
        a[8] = 1;
        let mut b = [0u8; 4096];
        b[16] = 1;
        assert_ne!(page_hash(&a), page_hash(&b));
    }
}

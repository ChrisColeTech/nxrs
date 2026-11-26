//! CPU-written pages whose bytes are what the GPU copy already holds are not uploaded again.
//!
//! The write watch reports pages the CPU wrote, not pages that changed. Ocarina of Time rewrites ~50 MB a frame of
//! identical bytes into double-buffered mirrors in Kokiri Forest (98% unchanged, `NXGFX_UPLOAD_TRACE`), and the pool
//! sync uploaded all of it every frame. Each whole page the pool sync uploads keeps a hash of the bytes uploaded; a
//! page written again with the same bytes is left out. A skip only ever happens when the GPU copy is exactly the bytes
//! of that hash:
//!
//! - the hash is of the bytes staged: each page is copied once out of guest memory, hashed from that copy and
//!   uploaded from it (audit M1: hashing guest memory and copying it later could keep a hash of bytes never uploaded);
//! - any other change of the GPU copy forgets it (`forget`: other upload paths, a pool's buffer made or gone; pools
//!   laid over each other are all reached, audit H2);
//! - a GPU write noted by a submission (`gpu_writes`) parks the hash and stamps the page with that recording's epoch;
//!   when that submission's write-back lands (`landed`) and no later change restamped the page, the page gets the hash
//!   of the bytes the GPU left there: the parked one where the GPU did not change the page (its bytes equal the
//!   write-back's baseline), else the hash of the GPU's bytes. So pages bound writable every frame but rewritten by
//!   the CPU with the same bytes (OoT: ~46 MB a frame) are not sent again, and only the last writer re-arms;
//! - raw global memory (`gmem`) stores are not noted while recording: every pool a write log ever named is never
//!   skipped again (`raw_written`, audit H1), and the log's ranges are forgotten before the submission's write-backs
//!   land and before its events are published.
//!
//! `NXGFX_UPLOAD_SKIP=0` turns it off (every CPU-written byte uploaded, as before OOT-FOREST2).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const PAGE: u64 = 4096;

/// A page's GPU-copy knowledge: the hash of its bytes (0: unknown), the hash parked by a GPU-write note, the
/// recording epoch that noted it (0: none; `COPYING` while the pool sync copies it for an upload), and the hash of the
/// bytes a lazy write-back brought back from this pool's buffer into guest memory (0: none; `brought_back`).
#[derive(Clone, Copy, Default)]
struct Pg {
    hash: u64,
    kept: u64,
    stamp: u64,
    brought: u64,
}

/// `Pg::stamp` of a page `upload_changed` decided to upload and is copying with the lock released: its hash is kept
/// only if nothing changed the page's state meanwhile (any note, forget or raw log replaces the stamp).
const COPYING: u64 = u64::MAX;

struct PoolH {
    end: u64,
    /// raw global memory stores reach it: never skipped
    raw: bool,
    /// a lazy write-back's record was ever set in it (`raw_written` clears them only then)
    any_brought: bool,
    pages: Vec<Pg>,
}

#[derive(Default)]
struct State {
    /// pool base -> its pages
    pools: BTreeMap<u64, PoolH>,
    /// pools laid over another kept pool (pools may lie over each other): a range's owners are the pools walked down
    /// from it while they reach it, plus these (one that reaches past a pool ending below the range overlaps that pool)
    over: Vec<u64>,
    /// ranges a raw global memory write log named (start -> end), for pools whose entry comes later
    raw: BTreeMap<u64, u64>,
    /// the longest of `raw` (a lookup starts this far below the range, audit L4)
    raw_max: u64,
}

static H: Mutex<Option<State>> = Mutex::new(None);
static SKIPPED: AtomicU64 = AtomicU64::new(0);
/// whole pages uploaded: no hash kept (first upload, or forgotten), or different bytes; bytes forgotten; pages re-armed
/// at a landing (bytes)
static UNKNOWN: AtomicU64 = AtomicU64::new(0);
static DIFFER: AtomicU64 = AtomicU64::new(0);
static FORGOT: AtomicU64 = AtomicU64::new(0);
static REARMED: AtomicU64 = AtomicU64::new(0);
/// pages a lazy write-back brought back and the CPU did not change, left out (bytes)
static BROUGHT_SKIPPED: AtomicU64 = AtomicU64::new(0);
/// The recording epoch: moves at every submission (`next_epoch`); a GPU-write note stamps pages with the current one.
static EPOCH: AtomicU64 = AtomicU64::new(1);
/// Time in `landed` (the completion thread) and in `upload_changed` (copy, hash, decide; the uploads themselves are
/// not counted), ns.
static REARM_NS: AtomicU64 = AtomicU64::new(0);
static HASH_NS: AtomicU64 = AtomicU64::new(0);

struct Timer(std::time::Instant, &'static AtomicU64);
impl Drop for Timer {
    fn drop(&mut self) {
        self.1.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

pub fn on() -> bool {
    if cfg!(test) {
        return true; // the unit tests exercise the skip itself
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_UPLOAD_SKIP").as_deref() != Ok("0"))
}

/// The submission being recorded is complete (all its notes made): its epoch, for its landing (`landed`).
pub fn next_epoch() -> u64 {
    EPOCH.fetch_add(1, Ordering::AcqRel)
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

impl State {
    /// Every pool entry that overlaps [a, a + n), whatever lies between (nested pools, audit H2).
    fn owners(&self, a: u64, n: u64) -> Vec<u64> {
        let hi = a.saturating_add(n);
        let mut out: Vec<u64> = self
            .pools
            .range(..hi)
            .rev()
            .take_while(|(_, p)| p.end > a)
            .map(|(k, _)| *k)
            .collect();
        for &b in &self.over {
            if b < hi && !out.contains(&b) && self.pools.get(&b).is_some_and(|p| p.end > a) {
                out.push(b);
            }
        }
        out
    }
    /// A pool entry is made: whether it lies over another, or another over it (`over`).
    fn add(&mut self, base: u64, end: u64, raw: bool) {
        if let Some((&b, p)) = self.pools.range(..base).next_back() {
            if p.end > base && !self.over.contains(&b) {
                self.over.push(b);
            }
        }
        if let Some((&b, _)) = self.pools.range(base + 1..).next() {
            if b < end && !self.over.contains(&base) {
                self.over.push(base);
            }
        }
        self.pools.insert(base, PoolH { end, raw, any_brought: false, pages: Vec::new() });
    }
    /// The pool at `base` with a page entry for every page it touches.
    fn sized(&mut self, base: u64) -> Option<&mut PoolH> {
        let p = self.pools.get_mut(&base)?;
        let need = (p.end - (base & !(PAGE - 1))).div_ceil(PAGE) as usize;
        if p.pages.len() < need {
            p.pages.resize(need, Pg::default());
        }
        Some(p)
    }
    /// For each whole or partial page of [a, a + n) in the pool at `base`: f(page index, page start).
    fn each_page(&mut self, base: u64, a: u64, n: u64, mut f: impl FnMut(&mut Pg, u64)) {
        let Some(p) = self.pools.get_mut(&base) else { return };
        let first = base & !(PAGE - 1);
        let (s, e) = (a.max(base), (a + n).min(p.end));
        if e <= s {
            return;
        }
        let lo = ((s - first) / PAGE) as usize;
        let hi = (((e - first).div_ceil(PAGE)) as usize).min(p.pages.len());
        for i in lo..hi {
            f(&mut p.pages[i], first + i as u64 * PAGE);
        }
    }
    fn raw_overlaps(&self, lo: u64, hi: u64) -> bool {
        // only ranges starting less than the longest one below `lo` can reach it (audit L4: was every range below)
        self.raw.range(lo.saturating_sub(self.raw_max)..hi).any(|(_, &e)| e > lo)
    }
}

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    H.lock().unwrap()
}

/// The pool sync's upload of [a, a + n) (CPU-written, in the pool [base, base + size)): whole pages whose bytes differ
/// from the GPU copy's, and every partial page, go to `upload(address, bytes)` in runs; the bytes are copied once from
/// guest memory and hashed from that copy, and the hashes of what is uploaded are kept. Whole pages left out because
/// they still hold what a lazy write-back brought back from this pool's buffer (no CPU write since, `brought_back`)
/// are added to `back` (ascending): the caller does not report them as CPU writes (audit M2). Returns the bytes
/// uploaded.
///
/// Guest memory is never read with the hash lock held: a read of a page held on the device faults, and bringing it
/// back records into this table (audit L1). A batch's page states are looked at, the guest bytes of the pages that may
/// be left out hashed, the decision taken under the lock (only if those states did not change), the pages to upload
/// copied and hashed with the lock released, and their hashes kept only where nothing changed them meanwhile.
pub fn upload_changed(
    base: u64,
    size: u64,
    a: u64,
    n: u64,
    back: &mut Vec<u64>,
    mut upload: impl FnMut(u64, &[u8]),
) -> u64 {
    // pages are taken in batches of this much; each batch's runs are uploaded with the hash lock released (staging may
    // wait for a fence, and the completion thread takes this lock)
    const BATCH: u64 = 1 << 20;
    /// One page (or the part of it in the range) of a batch.
    struct Cand {
        pg: u64,
        s: u64,
        t: u64,
        whole: bool,
        /// the kept hash and the brought-back record as the batch first found them (0: none to compare)
        h: u64,
        br: u64,
        /// the hash of the guest bytes (of the copy, for a page uploaded)
        gh: u64,
        skip: bool,
    }
    thread_local! {
        static SCRATCH: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let end = base + size;
    let first = base & !(PAGE - 1);
    let (a, e) = (a.max(base), (a + n).min(end));
    if e <= a {
        return 0;
    }
    let mut sent = 0u64;
    let mut pg = a & !(PAGE - 1);
    // a large range's pages hashed on several threads first (OoT rewrites mirrors of 6-15 MB a frame: ~56 MB hashed
    // on the replay thread was 6.5 ms); the hash of guest memory only decides a skip, which uploads nothing
    let first_pg = pg;
    let pre: Vec<u64> = if e - a >= PAR_MIN { par_hashes(first_pg, e) } else { Vec::new() };
    // SAFETY (each guest read below): the CPU-written guest bytes of a registered pool (committed memory, as the upload
    // reads it); a page held on the device faults and is brought back first
    let guest = |s: u64, t: u64| unsafe { core::slice::from_raw_parts(s as *const u8, (t - s) as usize) };
    SCRATCH.with(|sc| {
        let mut buf = sc.borrow_mut();
        // (address, offset in buf, bytes) of each run of this batch
        let mut runs: Vec<(u64, usize, usize)> = Vec::new();
        let mut cand: Vec<Cand> = Vec::new();
        while pg < e {
            let b_end = (pg + BATCH).min(e);
            buf.clear();
            runs.clear();
            cand.clear();
            let t0 = Timer(std::time::Instant::now(), &HASH_NS);
            // 1. what the table knows of the batch's pages
            {
                let mut g = state();
                let st = g.get_or_insert_with(State::default);
                if !st.pools.get(&base).is_some_and(|p| p.end == end) {
                    let raw = st.raw_overlaps(base, end);
                    st.add(base, end, raw);
                }
                let p = st.sized(base).unwrap();
                let mut q = pg;
                while q < b_end {
                    let (ps, pe) = (q.max(base), (q + PAGE).min(end));
                    let (s, t) = (ps.max(a), pe.min(e));
                    let whole = s == ps && t == pe && t - s == PAGE;
                    let page = p.pages[((q - first) / PAGE) as usize];
                    let (h, br) = if whole { (if p.raw { 0 } else { page.hash }, page.brought) } else { (0, 0) };
                    cand.push(Cand { pg: q, s, t, whole, h, br, gh: 0, skip: false });
                    q += PAGE;
                }
            }
            // 2. the guest bytes of the pages that may be left out, hashed with the lock released
            for c in cand.iter_mut().filter(|c| c.h != 0 || c.br != 0) {
                c.gh = match pre.get(((c.pg - first_pg) / PAGE) as usize) {
                    Some(&h) if h != 0 => h,
                    _ => page_hash(guest(c.s, c.t)),
                };
            }
            // 3. left out where the page still is as step 1 found it and the guest bytes match; else marked for upload
            {
                let mut g = state();
                // (the entry may have gone meanwhile, a pool made or released: its pages are then uploaded, unknown)
                let mut p = g.as_mut().and_then(|st| st.pools.get_mut(&base)).filter(|p| p.end == end);
                for c in cand.iter_mut() {
                    let Some(p) = p.as_deref_mut() else { break };
                    let raw = p.raw;
                    // (an entry made again since step 1 may not reach this page yet: uploaded, unknown)
                    let Some(page) = p.pages.get_mut(((c.pg - first) / PAGE) as usize) else { continue };
                    // the "write" was this pool's lazy page coming back from the device, and the CPU has not changed it
                    if c.br != 0 && page.brought == c.br && c.gh == c.br {
                        page.brought = 0;
                        BROUGHT_SKIPPED.fetch_add(PAGE, Ordering::Relaxed);
                        back.push(c.pg);
                        c.skip = true;
                        continue;
                    }
                    // the bytes the GPU copy holds (read once, not copied: nothing is uploaded and the kept hash still
                    // describes the GPU copy, whatever the CPU writes meanwhile)
                    if c.h != 0 && !raw && page.hash == c.h && c.gh == c.h {
                        page.brought = 0;
                        SKIPPED.fetch_add(PAGE, Ordering::Relaxed);
                        c.skip = true;
                        continue;
                    }
                    if c.whole {
                        (if page.hash == 0 { &UNKNOWN } else { &DIFFER }).fetch_add(PAGE, Ordering::Relaxed);
                        *page = Pg { stamp: COPYING, ..Pg::default() };
                    } else {
                        *page = Pg::default();
                    }
                }
            }
            // 4. copied once, hashed from the copy and uploaded from it (audit M1), with the lock released
            for c in cand.iter_mut().filter(|c| !c.skip) {
                let at = buf.len();
                buf.extend_from_slice(guest(c.s, c.t));
                c.gh = if c.whole { page_hash(&buf[at..]) } else { 0 };
                match runs.last_mut() {
                    Some(r) if r.0 + r.2 as u64 == c.s => r.2 += (c.t - c.s) as usize,
                    _ => runs.push((c.s, at, (c.t - c.s) as usize)),
                }
            }
            // 5. the hash of what is uploaded kept, where no note, forget or raw log changed the page since step 3
            if cand.iter().any(|c| c.whole && !c.skip) {
                let mut g = state();
                if let Some(p) = g.as_mut().and_then(|st| st.pools.get_mut(&base)).filter(|p| p.end == end) {
                    for c in cand.iter().filter(|c| c.whole && !c.skip) {
                        if let Some(page) = p.pages.get_mut(((c.pg - first) / PAGE) as usize) {
                            if page.stamp == COPYING {
                                *page = Pg { hash: c.gh, ..Pg::default() };
                            }
                        }
                    }
                }
            }
            drop(t0);
            for &(at, o, len) in &runs {
                upload(at, &buf[o..o + len]);
                sent += len as u64;
            }
            pg = b_end;
        }
    });
    sent
}

/// Ranges from this size on are hashed on several threads (`par_hashes`).
const PAR_MIN: u64 = 4 << 20;

/// The hash of each whole page in [a, e) (a page-aligned, by page from `a`), on several threads; 0 for a page not
/// wholly inside. Persistent `vk page hash` threads take pieces of it with the caller (spawning threads per range cost
/// as much as the hashing saved).
fn par_hashes(a: u64, e: u64) -> Vec<u64> {
    let _t = Timer(std::time::Instant::now(), &HASH_NS);
    let n = ((e - a) / PAGE) as usize;
    let out: Vec<AtomicU64> = (0..n).map(|_| AtomicU64::new(0)).collect();
    par_for(n, &|k| {
        let p = a + k as u64 * PAGE;
        if p + PAGE <= e {
            // SAFETY: a whole page of the CPU-written range (committed guest memory of a registered pool)
            let h = page_hash(unsafe { core::slice::from_raw_parts(p as *const u8, PAGE as usize) });
            out[k].store(h, Ordering::Relaxed);
        }
    });
    out.into_iter().map(|h| h.into_inner()).collect()
}

/// f(0..n) on the caller and the persistent `vk page hash` threads, `PAR_PIECE` indices at a time; returns when every
/// call has returned.
fn par_for(n: usize, f: &(dyn Fn(usize) + Sync)) {
    // a raw pointer, not a reference: a worker may still hold the job after this returns (it then claims nothing and
    // never touches `f`); a panic in any call aborts (`ParJob::run`), so `done` never stops short of `n` with the caller
    // spinning, and the caller never unwinds while workers borrow its stack (audit L3)
    // SAFETY (lifetime erased): the threads call `f` only for indices they claimed below `n`, and each claimed call
    // returns before `done` counts it; this function returns only once `done` reached `n`, so no call outlives `f`
    let f: *const (dyn Fn(usize) + Sync + 'static) = unsafe { core::mem::transmute(f as *const (dyn Fn(usize) + Sync)) };
    let job = std::sync::Arc::new(ParJob {
        f,
        n,
        next: std::sync::atomic::AtomicUsize::new(0),
        done: std::sync::atomic::AtomicUsize::new(0),
    });
    hash_pool_start();
    {
        let mut j = HASH_JOB.lock().unwrap();
        j.0 += 1;
        j.1 = Some(job.clone());
    }
    HASH_CV.notify_all();
    job.run();
    while job.done.load(Ordering::Acquire) < n {
        std::hint::spin_loop();
    }
    let mut j = HASH_JOB.lock().unwrap();
    if j.1.as_ref().is_some_and(|x| std::sync::Arc::ptr_eq(x, &job)) {
        j.1 = None;
    }
}

struct ParJob {
    f: *const (dyn Fn(usize) + Sync),
    n: usize,
    next: std::sync::atomic::AtomicUsize,
    done: std::sync::atomic::AtomicUsize,
}

// SAFETY: `f` is a `Sync` closure, called only while `par_for` waits for it (see there)
unsafe impl Send for ParJob {}
unsafe impl Sync for ParJob {}

const PAR_PIECE: usize = 64;

impl ParJob {
    fn run(&self) {
        loop {
            let i = self.next.fetch_add(PAR_PIECE, Ordering::AcqRel);
            if i >= self.n {
                return;
            }
            let end = (i + PAR_PIECE).min(self.n);
            // SAFETY: a claimed index below `n`: `par_for` is still waiting, `f` alive
            let f = unsafe { &*self.f };
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (i..end).for_each(f))).is_err() {
                std::process::abort();
            }
            self.done.fetch_add(end - i, Ordering::AcqRel);
        }
    }
}

/// (job number, the job in progress)
static HASH_JOB: Mutex<(u64, Option<std::sync::Arc<ParJob>>)> = Mutex::new((0, None));
static HASH_CV: std::sync::Condvar = std::sync::Condvar::new();

fn hash_pool_start() {
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        let n = std::thread::available_parallelism().map(|n| n.get() / 4).unwrap_or(4).clamp(2, 6);
        for _ in 0..n {
            let _ = std::thread::Builder::new().name("vk page hash".into()).spawn(|| {
                let mut seen = 0u64;
                loop {
                    let job = {
                        let mut j = HASH_JOB.lock().unwrap();
                        while j.0 == seen || j.1.is_none() {
                            j = HASH_CV.wait(j).unwrap();
                        }
                        seen = j.0;
                        j.1.clone().unwrap()
                    };
                    job.run();
                }
            });
        }
    });
}

/// The GPU copy of [a, a + n) changes by other means (another upload path, a buffer made): its pages' hashes go.
pub fn forget(a: u64, n: u64) {
    if !on() || n == 0 {
        return;
    }
    FORGOT.fetch_add(n, Ordering::Relaxed);
    if super::uptrace::on() {
        super::uptrace::note_forget(a, n);
    }
    let mut g = state();
    let Some(st) = g.as_mut() else { return };
    for base in st.owners(a, n) {
        st.each_page(base, a, n, |pg, _| *pg = Pg::default());
    }
}

/// The submission being recorded writes [a, a + n) on the GPU (a noted write, landed at its completion): the pages'
/// hashes are parked and the pages stamped with this recording's epoch (`landed` brings them back).
pub fn gpu_writes(a: u64, n: u64) {
    if !on() || n == 0 {
        return;
    }
    let epoch = EPOCH.load(Ordering::Acquire);
    // the same range noted again in this recording (a storage buffer bound by draw after draw): its pages are stamped
    // with this epoch and unknown already (nothing in a recording sets a hash: the sync runs before it, landings belong
    // to earlier epochs; other upload paths only clear hashes)
    thread_local! {
        static RECENT: std::cell::Cell<[(u64, u64, u64); 8]> = const { std::cell::Cell::new([(0, 0, 0); 8]) };
    }
    if RECENT.with(|r| {
        let mut v = r.get();
        if v.contains(&(epoch, a, n)) {
            return true;
        }
        v.rotate_right(1);
        v[0] = (epoch, a, n);
        r.set(v);
        false
    }) {
        return;
    }
    FORGOT.fetch_add(n, Ordering::Relaxed);
    if super::uptrace::on() {
        super::uptrace::note_forget(a, n);
    }
    let mut g = state();
    let Some(st) = g.as_mut() else { return };
    for base in st.owners(a, n) {
        // (a lazy write-back's record goes: the GPU copy will hold other bytes, audit H1)
        st.each_page(base, a, n, |pg, _| {
            if pg.stamp != epoch {
                *pg = Pg { hash: 0, kept: pg.hash, stamp: epoch, brought: 0 };
            } else {
                pg.hash = 0;
                pg.brought = 0;
            }
        });
    }
}

/// A submission of recording epoch `epoch` completed: its write-back of [a, a + n) from the buffer of the pool at
/// `pool` holds the GPU's bytes `src`, and `before` (may be null) the same range as the GPU had it before the
/// submission's work. That pool's pages it noted and nothing changed since get the hash of what its GPU copy now holds;
/// other pools over the range (with buffers of their own, audit M1) only lose the stamp and stay unknown. Returns the
/// whole pages it found the GPU left unchanged (`src` equal to `before`), ascending: the landing need not compare them
/// again (`mem::land_changed`).
pub fn landed(pool: u64, a: u64, n: u64, src: *const u8, before: *const u8, epoch: u64) -> Vec<u64> {
    let mut same_pages = Vec::new();
    if !on() || n == 0 || src.is_null() {
        return same_pages;
    }
    let t0 = std::time::Instant::now();
    let _t = Timer(t0, &REARM_NS);
    // a large write-back's pages compared on several threads first (OoT: ~58 MB a frame, 6 ms on the completion thread
    // alone); 1: the GPU left the page as it was, 2: it changed it, 0: not a whole page of the range
    let lo_pg = a & !(PAGE - 1);
    let eq: Vec<std::sync::atomic::AtomicU8> = if n >= PAR_MIN && !before.is_null() {
        let pages = ((a + n - lo_pg).div_ceil(PAGE)) as usize;
        let v: Vec<std::sync::atomic::AtomicU8> = (0..pages).map(|_| Default::default()).collect();
        let (s0, b0) = (src as usize, before as usize);
        par_for(pages, &|k| {
            let p = lo_pg + k as u64 * PAGE;
            if p >= a && p + PAGE <= a + n {
                let o = (p - a) as usize;
                // SAFETY: the write-back's readback and baseline buffers hold n bytes from `a`
                let same = unsafe {
                    core::slice::from_raw_parts((s0 + o) as *const u8, PAGE as usize)
                        == core::slice::from_raw_parts((b0 + o) as *const u8, PAGE as usize)
                };
                v[k].store(if same { 1 } else { 2 }, Ordering::Relaxed);
            }
        });
        v
    } else {
        Vec::new()
    };
    let mut g = state();
    let Some(st) = g.as_mut() else { return same_pages };
    let mut rearmed = 0u64;
    for base in st.owners(a, n) {
        let end = st.pools[&base].end;
        if base != pool {
            // another pool's GPU copy: this write-back says nothing of its bytes (its own landing, if any, re-arms it)
            st.each_page(base, a, n, |pg, _| {
                if pg.stamp == epoch {
                    *pg = Pg::default();
                }
            });
            continue;
        }
        st.each_page(base, a, n, |pg, p| {
            // the GPU copy changed under any record of a lazy write-back (audit H1)
            pg.brought = 0;
            // whole pages of the pool inside the write-back only
            if pg.stamp != epoch || p < a || p + PAGE > a + n || p < base || p + PAGE > end {
                return;
            }
            let o = (p - a) as usize;
            // SAFETY: the write-back's readback (and baseline) buffers hold n bytes from `a`
            let s = unsafe { core::slice::from_raw_parts(src.add(o), PAGE as usize) };
            let same = !before.is_null()
                && pg.kept != 0
                && match eq.get(((p - lo_pg) / PAGE) as usize).map(|v| v.load(Ordering::Relaxed)) {
                    Some(1) => true,
                    Some(2) => false,
                    _ => s == unsafe { core::slice::from_raw_parts(before.add(o), PAGE as usize) },
                };
            pg.hash = if same { pg.kept } else { page_hash(s) };
            pg.kept = 0;
            pg.stamp = 0;
            rearmed += PAGE;
            if same {
                same_pages.push(p);
            }
        });
    }
    REARMED.fetch_add(rearmed, Ordering::Relaxed);
    same_pages.sort_unstable();
    same_pages
}

/// `lazy::bring_back` is about to put `bytes` (page `p` as the buffer of the pool at `pool` holds it) into guest
/// memory. Its copy reads as a CPU write (the watch is not reset there, MKW-RECORD L6); the pool's sync finding the page
/// still holding exactly those bytes leaves it out (`Pg::brought`, consumed): the CPU did not change it, and uploading
/// the device's older bytes back could overwrite GPU writes made since that are never written back (raw memory in
/// large pools). The record lives in the page's entry (audit H1): it goes with every change of that pool's GPU copy
/// (`forget`, `gpu_writes`, `landed`, `raw_written`, the pool's entry dropped) and with any upload or skip of the
/// page, so it never outlives the bytes it describes. Recorded before the copy, so a scan taking the copy's store
/// finds it. Called by a faulting thread: hashes before taking the lock, and nothing holding it reads guest memory.
pub fn brought_back(pool: u64, p: u64, bytes: &[u8]) {
    if !on() {
        return;
    }
    let h = page_hash(bytes);
    let mut g = state();
    let Some(st) = g.as_mut() else { return };
    // (a pool with no entry has no page the sync could leave out: its page is uploaded, a redundant upload)
    let Some(pl) = st.sized(pool) else { return };
    pl.any_brought = true;
    st.each_page(pool, p, PAGE, |pg, q| {
        if q == p {
            pg.brought = h;
        }
    });
}

/// The bring-back of page `p` (from the pool at `pool`) failed after `brought_back`: nothing was copied.
pub fn not_brought_back(pool: u64, p: u64) {
    if !on() {
        return;
    }
    if let Some(st) = state().as_mut() {
        st.each_page(pool, p, PAGE, |pg, _| pg.brought = 0);
    }
}

/// A raw global memory write log named [lo, hi): its hashes go, and pages there are never skipped again (the log comes
/// only at the submission's completion, after later syncs may have compared against bytes the shaders replaced).
pub fn raw_written(lo: u64, hi: u64) {
    if !on() || hi <= lo {
        return;
    }
    let mut g = state();
    let st = g.get_or_insert_with(State::default);
    if st.raw.get(&lo).is_none_or(|&e| e < hi) {
        st.raw.insert(lo, hi);
        st.raw_max = st.raw_max.max(hi - lo);
    }
    for base in st.owners(lo, hi - lo) {
        let p = st.pools.get_mut(&base).unwrap();
        if !p.raw {
            p.raw = true;
            st.each_page(base, lo, hi - lo, |pg, _| *pg = Pg::default());
        } else if p.any_brought {
            // a raw pool's hashes are never compared, but a lazy write-back's record is: the shaders changed the GPU
            // copy under it (audit H1)
            st.each_page(base, lo, hi - lo, |pg, _| pg.brought = 0);
        }
    }
}

/// A pool goes: nothing is known of its GPU copy, and the raw write logs inside it are forgotten (audit L4).
pub fn drop_pool(base: u64) {
    if let Some(st) = state().as_mut() {
        if let Some(p) = st.pools.remove(&base) {
            let end = p.end;
            let gone: Vec<u64> = st.raw.range(base..end).filter(|(_, &e)| e <= end).map(|(&s, _)| s).collect();
            for s in gone {
                st.raw.remove(&s);
            }
            if st.raw.is_empty() {
                st.raw_max = 0;
            }
        }
        st.over.retain(|&b| b != base);
    }
}

/// A pool's GPU copy is made anew (a buffer of its own, filled from guest memory): an empty entry, so a lazy
/// write-back's record has somewhere to go before the pool's first upload.
pub fn new_pool(base: u64, end: u64) {
    if !on() {
        return;
    }
    let mut g = state();
    let st = g.get_or_insert_with(State::default);
    st.pools.remove(&base);
    st.over.retain(|&b| b != base);
    let raw = st.raw_overlaps(base, end);
    st.add(base, end, raw);
}

/// The bytes left out since the last report, per frame.
pub fn report(frames: f64) -> String {
    if !on() {
        return String::new();
    }
    let mb = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / frames / 1048576.0;
    let (s, u, d, f, r) = (mb(&SKIPPED), mb(&UNKNOWN), mb(&DIFFER), mb(&FORGOT), mb(&REARMED));
    let ms = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / frames / 1e6;
    let (hm, rm) = (ms(&HASH_NS), ms(&REARM_NS));
    let bk = BROUGHT_SKIPPED.swap(0, Ordering::Relaxed) as f64 / frames / 1024.0;
    format!("; upload skip a frame: {s:.2} MB of CPU-written pages not uploaded (same bytes as the GPU copy), whole pages uploaded {u:.2} MB with no hash kept and {d:.2} MB changed, {f:.2} MB forgotten or GPU-written, {r:.2} MB re-armed at landings, {bk:.1} KB of pages brought back from the device unchanged; copy and hash {hm:.2} ms, re-arming {rm:.2} ms")
}

#[cfg(test)]
mod tests {
    use super::*;

    // the tests share the global state: one test at a time
    static SERIAL: Mutex<()> = Mutex::new(());

    /// A page-aligned pool of `pages` pages inside a fresh allocation (kept alive by the returned Vec).
    fn pool(pages: u64) -> (Vec<u8>, u64) {
        let mut mem = vec![0u8; ((pages + 1) * PAGE) as usize];
        let base = (mem.as_mut_ptr() as u64 + PAGE - 1) & !(PAGE - 1);
        (mem, base)
    }

    fn runs(base: u64, size: u64, a: u64, n: u64) -> Vec<(u64, u64)> {
        runs_back(base, size, a, n).0
    }

    /// The runs uploaded, and the pages left out as brought back unchanged.
    fn runs_back(base: u64, size: u64, a: u64, n: u64) -> (Vec<(u64, u64)>, Vec<u64>) {
        let mut out: Vec<(u64, u64)> = Vec::new();
        let mut back = Vec::new();
        upload_changed(base, size, a, n, &mut back, |at, b| match out.last_mut() {
            Some(l) if l.0 + l.1 == at => l.1 += b.len() as u64,
            _ => out.push((at, b.len() as u64)),
        });
        (out, back)
    }

    fn bytes(a: u64, n: u64) -> Vec<u8> {
        unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) }.to_vec()
    }

    fn set(a: u64, b: &[u8]) {
        unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), a as *mut u8, b.len()) };
    }

    /// A page written again with its last upload's bytes is left out; changed, partial, or forgotten pages are
    /// uploaded; the bytes handed to the upload are the bytes hashed.
    #[test]
    fn unchanged_pages_are_not_uploaded_again() {
        let _s = SERIAL.lock().unwrap();
        let (_m, base) = pool(4);
        let size = 4 * PAGE;
        drop_pool(base);
        assert_eq!(runs(base, size, base, size), vec![(base, size)]);
        assert_eq!(runs(base, size, base, size), vec![], "same bytes: nothing");
        unsafe { *((base + PAGE + 5) as *mut u8) = 9 };
        assert_eq!(runs(base, size, base, size), vec![(base + PAGE, PAGE)]);
        forget(base + 2 * PAGE + 100, 8);
        assert_eq!(runs(base, size, base, size), vec![(base + 2 * PAGE, PAGE)]);
        // a partial page is always uploaded, and its hash forgotten
        assert_eq!(runs(base, size, base + 10, 20), vec![(base + 10, 20)]);
        assert_eq!(runs(base, size, base, PAGE), vec![(base, PAGE)]);
        drop_pool(base);
        assert_eq!(runs(base, size, base, PAGE), vec![(base, PAGE)]);
        // what is uploaded is the copy that was hashed
        let mut seen = Vec::new();
        unsafe { *((base + 3 * PAGE) as *mut u8) = 7 };
        upload_changed(base, size, base + 3 * PAGE, PAGE, &mut Vec::new(), |_, b| seen.extend_from_slice(b));
        assert_eq!(seen[0], 7);
        drop_pool(base);
        assert_ne!(page_hash(&[0u8; 4096]), page_hash(&[0u8; 4095]));
        let mut a = [0u8; 4096];
        a[8] = 1;
        let mut b = [0u8; 4096];
        b[16] = 1;
        assert_ne!(page_hash(&a), page_hash(&b));
    }

    /// A large range (hashed on several threads first) skips and uploads as a small one does.
    #[test]
    fn a_large_range_is_hashed_in_parallel_alike() {
        let _s = SERIAL.lock().unwrap();
        let pages = (PAR_MIN / PAGE) + 3;
        let (_m, base) = pool(pages);
        let size = pages * PAGE;
        drop_pool(base);
        assert_eq!(runs(base, size, base, size), vec![(base, size)]);
        assert_eq!(runs(base, size, base, size), vec![]);
        unsafe { *((base + 700 * PAGE + 9) as *mut u8) = 3 };
        // also from a start inside a page: the partial first page is uploaded, page 700 is the only whole one changed
        assert_eq!(runs(base, size, base + 100, size - 100), vec![(base + 100, PAGE - 100), (base + 700 * PAGE, PAGE)]);
        // a large landing (compared on several threads): unchanged pages get their parked hash, a changed one the GPU's
        runs(base, size, base, size);
        let before: Vec<u8> = unsafe { core::slice::from_raw_parts(base as *const u8, size as usize) }.to_vec();
        let mut after = before.clone();
        after[(5 * PAGE) as usize] ^= 0xff;
        gpu_writes(base, size);
        let k = next_epoch();
        let same = landed(base, base, size, after.as_ptr(), before.as_ptr(), k);
        assert_eq!(same.len() as u64, pages - 1, "every page but the changed one");
        assert!(!same.contains(&(base + 5 * PAGE)));
        assert_eq!(runs(base, size, base, size), vec![(base + 5 * PAGE, PAGE)], "the guest lacks the GPU's change");
        drop_pool(base);
    }

    /// Audit H2: a forget reaches every pool over the range, also past a nested pool that ends below it.
    #[test]
    fn forget_reaches_a_pool_under_a_nested_one() {
        let _s = SERIAL.lock().unwrap();
        let (_m, base) = pool(8);
        let (outer, inner) = (base, base + PAGE);
        drop_pool(outer);
        drop_pool(inner);
        runs(outer, 8 * PAGE, outer, 8 * PAGE);
        runs(inner, PAGE, inner, PAGE);
        assert_eq!(runs(outer, 8 * PAGE, outer, 8 * PAGE), vec![]);
        // a GPU copy change at page 5 of the outer pool: the inner pool (ends at page 2) lies between in key order
        forget(outer + 5 * PAGE, 16);
        assert_eq!(runs(outer, 8 * PAGE, outer, 8 * PAGE), vec![(outer + 5 * PAGE, PAGE)]);
        drop_pool(outer);
        drop_pool(inner);
    }

    /// A GPU-written page gets its hash back at its own submission's landing: the parked one when the GPU left it as
    /// it was, the GPU's bytes' when it changed it; a later note or upload before the landing keeps it unknown.
    #[test]
    fn a_landing_rearms_only_the_last_writer() {
        let _s = SERIAL.lock().unwrap();
        let (_m, base) = pool(3);
        let size = 3 * PAGE;
        drop_pool(base);
        runs(base, size, base, size);
        let before: Vec<u8> = unsafe { core::slice::from_raw_parts(base as *const u8, size as usize) }.to_vec();
        // recording K notes the whole pool
        gpu_writes(base, size);
        let k = next_epoch();
        assert_eq!(runs(base, size, base, PAGE), vec![(base, PAGE)], "noted: unknown until it lands");
        // page 0 was uploaded again above (by the sync of K+1): K's landing must not touch it
        let mut after = before.clone();
        after[2 * PAGE as usize] = 0x55; // the GPU changed page 2
        landed(base, base, size, after.as_ptr(), before.as_ptr(), k);
        // page 1: unchanged and re-armed with the parked hash: the CPU rewriting the same bytes is skipped
        assert_eq!(runs(base, size, base + PAGE, PAGE), vec![]);
        // page 2: re-armed with the GPU's bytes: the guest still holds the old bytes, so it is uploaded
        assert_eq!(runs(base, size, base + 2 * PAGE, PAGE), vec![(base + 2 * PAGE, PAGE)]);
        // a note by K+1, then K+2's note before K+1 lands: K+1's landing re-arms nothing, K+2's does
        gpu_writes(base, size);
        let k1 = next_epoch();
        gpu_writes(base, size);
        let k2 = next_epoch();
        let now: Vec<u8> = unsafe { core::slice::from_raw_parts(base as *const u8, size as usize) }.to_vec();
        landed(base, base, size, now.as_ptr(), now.as_ptr(), k1);
        assert_eq!(runs(base, size, base + PAGE, PAGE), vec![(base + PAGE, PAGE)], "restamped by K+2");
        gpu_writes(base, size);
        let k3 = next_epoch();
        landed(base, base, size, now.as_ptr(), now.as_ptr(), k2);
        landed(base, base, size, now.as_ptr(), now.as_ptr(), k3);
        assert_eq!(runs(base, size, base, size), vec![], "K+3's landing re-armed every page");
        drop_pool(base);
    }

    /// L6: a page a lazy write-back brought back reads as written; while it holds exactly those bytes it is left out
    /// (also in a raw pool, whose GPU copy may be newer), once; a CPU change since is uploaded.
    #[test]
    fn a_page_brought_back_unchanged_is_not_uploaded() {
        let _s = SERIAL.lock().unwrap();
        let (_m, base) = pool(2);
        drop_pool(base);
        raw_written(base, base + 2 * PAGE);
        new_pool(base, base + 2 * PAGE);
        let page0 = bytes(base, PAGE);
        brought_back(base, base, &page0);
        assert_eq!(
            runs_back(base, 2 * PAGE, base, 2 * PAGE),
            (vec![(base + PAGE, PAGE)], vec![base]),
            "page 0 left out and reported back (not a CPU write), page 1 raw"
        );
        assert_eq!(runs(base, 2 * PAGE, base, PAGE), vec![(base, PAGE)], "consumed: a later write is uploaded");
        brought_back(base, base, &page0);
        unsafe { *(base as *mut u8) ^= 1 };
        assert_eq!(runs_back(base, 2 * PAGE, base, PAGE), (vec![(base, PAGE)], vec![]), "changed by the CPU since");
        // a pool with no entry keeps no record: its page is uploaded
        drop_pool(base);
        brought_back(base, base, &bytes(base, PAGE));
        assert_eq!(runs(base, 2 * PAGE, base, PAGE), vec![(base, PAGE)]);
        drop_pool(base);
        state().as_mut().unwrap().raw.clear();
    }

    /// Audit H1: a brought-back record never outlives the bytes it describes. A held page P (bytes D) comes back; the
    /// GPU writes G there and it lands (guest G, the GPU copy G); the CPU then writes D again: P must be uploaded (the
    /// old side table skipped it, guest hash == recorded hash, and the GPU kept G for good). Every other change of the
    /// GPU copy (forget, raw log, the entry made again) drops the record too, and an upload of the page consumes it.
    #[test]
    fn a_brought_back_record_goes_with_the_bytes_it_describes() {
        let _s = SERIAL.lock().unwrap();
        let (_m, base) = pool(2);
        let size = 2 * PAGE;
        drop_pool(base);
        new_pool(base, base + size);
        set(base, &[0xd0; PAGE as usize]);
        let d = bytes(base, PAGE);
        let g: Vec<u8> = vec![0x6e; PAGE as usize];
        assert_eq!(runs(base, size, base, size), vec![(base, size)]);
        // P brought back with D; no sync before the GPU writes it
        brought_back(base, base, &d);
        let before = bytes(base, size);
        gpu_writes(base, size);
        let k = next_epoch();
        set(base, &g); // the landing puts G into guest memory
        let after = bytes(base, size);
        landed(base, base, size, after.as_ptr(), before.as_ptr(), k);
        assert_eq!(runs(base, size, base, size), vec![], "guest G = the GPU copy G: nothing to send");
        set(base, &d); // the CPU writes D again
        assert_eq!(runs_back(base, size, base, PAGE), (vec![(base, PAGE)], vec![]), "a real CPU write: uploaded");
        // the record dropped by a note even before its landing
        brought_back(base, base, &d);
        gpu_writes(base, PAGE);
        next_epoch();
        assert_eq!(runs(base, size, base, PAGE), vec![(base, PAGE)], "noted since: uploaded");
        // by a forget (another upload path, e.g. a texture copy's source sync)
        brought_back(base, base, &d);
        forget(base, PAGE);
        assert_eq!(runs(base, size, base, PAGE), vec![(base, PAGE)], "forgotten since: uploaded");
        // by a raw log over the pool (also once the pool is raw)
        brought_back(base, base, &d);
        raw_written(base, base + size);
        assert_eq!(runs(base, size, base, PAGE), vec![(base, PAGE)], "raw store since: uploaded");
        brought_back(base, base, &d);
        raw_written(base, base + size);
        assert_eq!(runs(base, size, base, PAGE), vec![(base, PAGE)], "raw store since, raw pool: uploaded");
        // by the pool's GPU copy made anew
        brought_back(base, base, &d);
        new_pool(base, base + size);
        assert_eq!(runs(base, size, base, PAGE), vec![(base, PAGE)], "new GPU copy: uploaded");
        // a bring-back that failed after recording
        brought_back(base, base, &d);
        not_brought_back(base, base);
        assert_eq!(runs(base, size, base, PAGE), vec![(base, PAGE)]);
        drop_pool(base);
        assert!(state().as_ref().unwrap().raw.range(base..base + size).next().is_none(), "raw logs go with the pool");
    }

    /// Audit M1: a landing re-arms only the pool whose buffer was written back; another pool over the range (a
    /// buffer of its own) loses the stamp and stays unknown.
    #[test]
    fn a_landing_rearms_only_the_written_back_pool() {
        let _s = SERIAL.lock().unwrap();
        let (_m, base) = pool(4);
        let (outer, inner) = (base, base + PAGE);
        drop_pool(outer);
        drop_pool(inner);
        runs(outer, 4 * PAGE, outer, 4 * PAGE);
        runs(inner, 2 * PAGE, inner, 2 * PAGE);
        let before = bytes(inner, 2 * PAGE);
        gpu_writes(inner, 2 * PAGE);
        let k = next_epoch();
        let mut after = before.clone();
        after[0] ^= 0x5a; // the outer pool's buffer changed page 1
        set(inner, &after); // landed in guest memory
        landed(outer, inner, 2 * PAGE, after.as_ptr(), before.as_ptr(), k);
        assert_eq!(runs(outer, 4 * PAGE, inner, 2 * PAGE), vec![], "the outer buffer holds these bytes");
        assert_eq!(runs(inner, 2 * PAGE, inner, 2 * PAGE), vec![(inner, 2 * PAGE)], "the inner buffer: unknown");
        assert_eq!(runs(inner, 2 * PAGE, inner, 2 * PAGE), vec![], "then known again");
        drop_pool(outer);
        drop_pool(inner);
    }

    /// Audit H1: a pool a raw write log named is never skipped, also when its entry comes later; a pool that goes
    /// takes its raw logs with it (audit L4: a new pool over the memory is skipped again).
    #[test]
    fn raw_written_pools_are_always_uploaded() {
        let _s = SERIAL.lock().unwrap();
        let (_m, base) = pool(2);
        drop_pool(base);
        runs(base, 2 * PAGE, base, 2 * PAGE);
        raw_written(base, base + 2 * PAGE);
        assert_eq!(runs(base, 2 * PAGE, base, 2 * PAGE), vec![(base, 2 * PAGE)]);
        assert_eq!(runs(base, 2 * PAGE, base, 2 * PAGE), vec![(base, 2 * PAGE)]);
        drop_pool(base);
        assert_eq!(runs(base, 2 * PAGE, base, 2 * PAGE), vec![(base, 2 * PAGE)]);
        assert_eq!(runs(base, 2 * PAGE, base, 2 * PAGE), vec![], "a new pool: its own GPU copy, no raw store yet");
        drop_pool(base);
        raw_written(base, base + 2 * PAGE);
        assert_eq!(runs(base, 2 * PAGE, base, 2 * PAGE), vec![(base, 2 * PAGE)]);
        assert_eq!(runs(base, 2 * PAGE, base, 2 * PAGE), vec![(base, 2 * PAGE)], "an entry made after the log");
        drop_pool(base);
        state().as_mut().unwrap().raw.clear();
    }
}

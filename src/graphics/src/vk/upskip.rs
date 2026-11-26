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

/// A page's GPU-copy knowledge: the hash of its bytes (0: unknown), the hash parked by a GPU-write note, and the
/// recording epoch that noted it (0: none).
#[derive(Clone, Copy, Default)]
struct Pg {
    hash: u64,
    kept: u64,
    stamp: u64,
}

struct PoolH {
    end: u64,
    /// raw global memory stores reach it: never skipped
    raw: bool,
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
        self.pools.insert(base, PoolH { end, raw, pages: Vec::new() });
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
        // raw ranges are pool windows: they do not overlap each other much; the one starting last below `hi`
        self.raw.range(..hi).any(|(_, &e)| e > lo)
    }
}

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    H.lock().unwrap()
}

/// The pool sync's upload of [a, a + n) (CPU-written, in the pool [base, base + size)): whole pages whose bytes differ
/// from the GPU copy's, and every partial page, go to `upload(address, bytes)` in runs; the bytes are copied once from
/// guest memory and hashed from that copy, and the hashes of what is uploaded are kept. Returns the bytes uploaded.
pub fn upload_changed(base: u64, size: u64, a: u64, n: u64, mut upload: impl FnMut(u64, &[u8])) -> u64 {
    // pages are copied into this thread's scratch in batches of about this much; each batch's runs are uploaded with
    // the hash lock released (staging may wait for a fence, and the completion thread takes this lock)
    const BATCH: usize = 1 << 20;
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
    SCRATCH.with(|s| {
        let mut buf = s.borrow_mut();
        // (address, offset in buf, bytes) of each run of this batch
        let mut runs: Vec<(u64, usize, usize)> = Vec::new();
        while pg < e {
            buf.clear();
            runs.clear();
            {
                let _t = Timer(std::time::Instant::now(), &HASH_NS);
                let mut g = state();
                let st = g.get_or_insert_with(State::default);
                if !st.pools.get(&base).is_some_and(|p| p.end == end) {
                    let raw = st.raw_overlaps(base, end);
                    st.add(base, end, raw);
                }
                let p = st.pools.get_mut(&base).unwrap();
                let need = (end - first).div_ceil(PAGE) as usize;
                if p.pages.len() < need {
                    p.pages.resize(need, Pg::default());
                }
                let raw = p.raw;
                while pg < e && buf.len() < BATCH {
                    let (ps, pe) = (pg.max(base), (pg + PAGE).min(end));
                    let (s, t) = (ps.max(a), pe.min(e));
                    let i = ((pg - first) / PAGE) as usize;
                    let at = buf.len();
                    // SAFETY: the CPU-written guest bytes of a registered pool (committed memory, as the upload reads
                    // it)
                    let guest = unsafe { core::slice::from_raw_parts(s as *const u8, (t - s) as usize) };
                    let page = &mut p.pages[i];
                    // a whole page whose bytes hash as the GPU copy's is left out (read once, not copied: nothing
                    // is uploaded and the kept hash still describes the GPU copy, whatever the CPU writes meanwhile)
                    let whole = s == ps && t == pe;
                    let guest_hash = || match pre.get(((pg - first_pg) / PAGE) as usize) {
                        Some(&h) if h != 0 && t - s == PAGE => h,
                        _ => page_hash(guest),
                    };
                    // the "write" was a lazy page's return from the device, and the CPU has not changed it since
                    if whole && t - s == PAGE && still_brought_back(pg, guest_hash) {
                        BROUGHT_SKIPPED.fetch_add(PAGE, Ordering::Relaxed);
                        pg += PAGE;
                        continue;
                    }
                    if whole && !raw && page.hash != 0 && guest_hash() == page.hash {
                        SKIPPED.fetch_add(t - s, Ordering::Relaxed);
                        pg += PAGE;
                        continue;
                    }
                    // else copied once, hashed from the copy and uploaded from it (audit M1)
                    buf.extend_from_slice(guest);
                    let keep = if whole {
                        let x = page_hash(&buf[at..]);
                        if page.hash == x && !raw {
                            SKIPPED.fetch_add(t - s, Ordering::Relaxed);
                            false
                        } else {
                            (if page.hash == 0 { &UNKNOWN } else { &DIFFER }).fetch_add(t - s, Ordering::Relaxed);
                            *page = Pg { hash: x, kept: 0, stamp: 0 };
                            true
                        }
                    } else {
                        *page = Pg::default();
                        true
                    };
                    if !keep {
                        buf.truncate(at);
                    } else {
                        match runs.last_mut() {
                            Some(r) if r.0 + r.2 as u64 == s => r.2 += (t - s) as usize,
                            _ => runs.push((s, at, (t - s) as usize)),
                        }
                    }
                    pg += PAGE;
                }
            }
            for &(at, o, len) in &runs {
                upload(at, &buf[o..o + len]);
                sent += len as u64;
            }
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
    // SAFETY (lifetime): the threads call `f` only for indices they claimed below `n`, and each claimed call returns
    // before `done` counts it; this function returns only once `done` reached `n`, so no call outlives `f`
    let f: &'static (dyn Fn(usize) + Sync) = unsafe { core::mem::transmute(f) };
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
    f: &'static (dyn Fn(usize) + Sync),
    n: usize,
    next: std::sync::atomic::AtomicUsize,
    done: std::sync::atomic::AtomicUsize,
}

const PAR_PIECE: usize = 64;

impl ParJob {
    fn run(&self) {
        loop {
            let i = self.next.fetch_add(PAR_PIECE, Ordering::AcqRel);
            if i >= self.n {
                return;
            }
            let end = (i + PAR_PIECE).min(self.n);
            for k in i..end {
                (self.f)(k);
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
        st.each_page(base, a, n, |pg, _| {
            if pg.stamp != epoch {
                *pg = Pg { hash: 0, kept: pg.hash, stamp: epoch };
            } else {
                pg.hash = 0;
            }
        });
    }
}

/// A submission of recording epoch `epoch` completed: its write-back of [a, a + n) holds the GPU's bytes `src`, and
/// `before` (may be null) the same range as the GPU had it before the submission's work. Pages it noted and nothing
/// changed since get the hash of what the GPU copy now holds. Returns the whole pages it found the GPU left unchanged
/// (`src` equal to `before`), ascending: the landing need not compare them again (`mem::land_changed`).
pub fn landed(a: u64, n: u64, src: *const u8, before: *const u8, epoch: u64) -> Vec<u64> {
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
        st.each_page(base, a, n, |pg, p| {
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

/// Pages a lazy write-back brought back from the device (`lazy::bring_back`) -> the hash of the bytes it put in guest
/// memory. Its copy reads as a CPU write (the watch is not reset there, MKW-RECORD L6); a sync finding the page still
/// holding exactly those bytes leaves it out whatever else it knows: the CPU did not change it, and uploading the device's
/// older bytes back could overwrite GPU writes made since that are never written back (raw memory in large pools).
static BROUGHT: Mutex<Option<std::collections::HashMap<u64, u64>>> = Mutex::new(None);

/// `lazy::bring_back` put `bytes` (the device's page) into guest page `p`.
pub fn brought_back(p: u64, bytes: &[u8]) {
    let h = page_hash(bytes);
    let mut b = BROUGHT.lock().unwrap();
    let m = b.get_or_insert_with(std::collections::HashMap::new);
    // a page never synced again (its pool gone) is forgotten with the rest, rather than kept for ever
    if m.len() >= 1 << 16 {
        m.clear();
    }
    m.insert(p, h);
}

/// Whether the whole page at `p`, whose guest bytes hash `h`, still holds what `brought_back` put there (consumed).
fn still_brought_back(p: u64, h: impl FnOnce() -> u64) -> bool {
    let mut b = BROUGHT.lock().unwrap();
    let Some(m) = b.as_mut().filter(|m| !m.is_empty()) else { return false };
    m.remove(&p).is_some_and(|x| x == h())
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
    }
    for base in st.owners(lo, hi - lo) {
        let p = st.pools.get_mut(&base).unwrap();
        // (a raw pool's hashes are never compared: named again, nothing to do)
        if !p.raw {
            p.raw = true;
            st.each_page(base, lo, hi - lo, |pg, _| *pg = Pg::default());
        }
    }
}

/// A pool is made or goes: nothing is known of its GPU copy.
pub fn drop_pool(base: u64) {
    if let Some(st) = state().as_mut() {
        st.pools.remove(&base);
    }
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
        let mut out: Vec<(u64, u64)> = Vec::new();
        upload_changed(base, size, a, n, |at, b| match out.last_mut() {
            Some(l) if l.0 + l.1 == at => l.1 += b.len() as u64,
            _ => out.push((at, b.len() as u64)),
        });
        out
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
        upload_changed(base, size, base + 3 * PAGE, PAGE, |_, b| seen.extend_from_slice(b));
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
        let same = landed(base, size, after.as_ptr(), before.as_ptr(), k);
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
        landed(base, size, after.as_ptr(), before.as_ptr(), k);
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
        landed(base, size, now.as_ptr(), now.as_ptr(), k1);
        assert_eq!(runs(base, size, base + PAGE, PAGE), vec![(base + PAGE, PAGE)], "restamped by K+2");
        gpu_writes(base, size);
        let k3 = next_epoch();
        landed(base, size, now.as_ptr(), now.as_ptr(), k2);
        landed(base, size, now.as_ptr(), now.as_ptr(), k3);
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
        let page0: Vec<u8> = unsafe { core::slice::from_raw_parts(base as *const u8, PAGE as usize) }.to_vec();
        brought_back(base, &page0);
        assert_eq!(runs(base, 2 * PAGE, base, 2 * PAGE), vec![(base + PAGE, PAGE)], "page 0 left out, page 1 raw");
        assert_eq!(runs(base, 2 * PAGE, base, PAGE), vec![(base, PAGE)], "consumed: a later write is uploaded");
        brought_back(base, &page0);
        unsafe { *(base as *mut u8) ^= 1 };
        assert_eq!(runs(base, 2 * PAGE, base, PAGE), vec![(base, PAGE)], "changed by the CPU since: uploaded");
        drop_pool(base);
        state().as_mut().unwrap().raw.clear();
    }

    /// Audit H1: a pool a raw write log named is never skipped, also when its entry comes later.
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
        assert_eq!(runs(base, 2 * PAGE, base, 2 * PAGE), vec![(base, 2 * PAGE)], "a new entry over a raw range");
        drop_pool(base);
        state().as_mut().unwrap().raw.clear();
    }
}

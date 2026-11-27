//! Sealed pools (SCAN-COST): a mirrored pool the CPU has left alone for a while is made read-only and leaves every
//! write-watch scan, walk and per-pool pass.
//!
//! The write watch has no cheaper answer to "was this pool written?" than walking every page of it (~1.5 ms a GB here),
//! and Ocarina of Time keeps ~4.4 GB of mirrors, most never written after their load: ~11 ms a frame of scans. A sealed
//! pool costs nothing until it is written: the first store faults, the handler here opens the 64 KiB chunk around it
//! (read-write again), marks the pool woken and lets the store go on (the write watch records it as for any store).
//! Every sync takes the woken pools back into its scan first (`mem::drain_woken`), so a store into a sealed pool is
//! known the moment it is made: never missed, never later than a scan of every pool would have found it.
//!
//! The registry is ordered by page range and guarded by its own lock, which the fault handler takes on any thread
//! (one that holds the pool lock too): no holder of it waits for another lock or touches guest memory. The pool lock's
//! holder owns the pools' `sealed` flags; this module owns the protection and the queue of woken pools.
//!
//! Host writers that reset the protection without faulting (`VirtualAlloc(MEM_COMMIT)` over a committed read-only page
//! makes it read-write) tell it first (`host_write`). `NXGFX_SEAL=0`: off. Windows only (Linux: never sealed).
//!
//! **Chunk tracking** (USE-SYNC, `NXGFX_USE_SYNC`): a pool need not be quiet to be armed. Once armed whole it stays in
//! the registry for good ("tracked"): a store opens only its chunk, and a sync's scan asks the pool's **open chunks**
//! (`open_runs`), not its whole range. An open chunk the scans found unwritten for `quiet_chunk_frames` frames is closed
//! again (`close_quiet`: protected, then the caller takes the write watch over it); a chunk written frame after frame
//! stays open (hot) and is asked by every scan. The guarantee is the sealed pools' one, per chunk: a store into a closed
//! chunk opens it (and is recorded by the write watch) before it lands, and a scan reads the open chunks after it read
//! the replay queue's tail.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// The piece a store opens: one protection call (~3 us) and later one to close it again.
pub const CHUNK: u64 = 64 << 10;

struct Entry {
    end: u64,
    /// the pool's base (the woken queue names pools)
    pool: u64,
    /// read-only but for `open`, and not written since: the pool needs no scan
    armed: bool,
    /// chunks made read-write since the last arming (protected again at the next one), ascending
    open: Vec<u64>,
    /// for each of `open`: the frame it was opened or last found written (chunk tracking closes it when quiet)
    last: Vec<u64>,
    /// a first arming in pieces (a pool of hundreds of MB is protected over several passes): [todo, end) is not
    /// protected yet; `end` when it is all done
    todo: u64,
}

/// What `arm` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Arm {
    /// all of it read-only now, armed (the caller takes the write watch, then counts it sealed while `armed`)
    Done,
    /// a first piece more protected; the rest at the next call
    Part,
    /// not armed: another entry lies over the range
    Refused,
}

struct Reg {
    by_lo: BTreeMap<u64, Entry>,
    woken: Vec<u64>,
}

static REG: Mutex<Reg> = Mutex::new(Reg {
    by_lo: BTreeMap::new(),
    woken: Vec::new(),
});
static ANY_WOKEN: AtomicBool = AtomicBool::new(false);

/// Pools woken by a store (the fault handler), by a host write (`host_write`), found written without a fault (the
/// walker's check: a store the protection did not see, red), armed, bytes protected for that and its time, unsealed
/// whole (landings, pools gone or replaced).
pub static WAKES: AtomicU64 = AtomicU64::new(0);
pub static HOST_WAKES: AtomicU64 = AtomicU64::new(0);
pub static LEAKS: AtomicU64 = AtomicU64::new(0);
pub static ARMS: AtomicU64 = AtomicU64::new(0);
pub static ARM_B: AtomicU64 = AtomicU64::new(0);
pub static ARM_NS: AtomicU64 = AtomicU64::new(0);
pub static UNSEALS: AtomicU64 = AtomicU64::new(0);
/// The sealing passes' time under the pool lock (`mem::seal_some`).
pub static PASS_NS: AtomicU64 = AtomicU64::new(0);
/// Chunk tracking: chunks opened by a store or a host write, closed again when quiet, bytes of open chunks the scans
/// asked, and (verify mode) written pages found in closed chunks (a store the new path would have missed: red).
pub static OPENS: AtomicU64 = AtomicU64::new(0);
pub static CLOSES: AtomicU64 = AtomicU64::new(0);
pub static OPEN_SCAN_B: AtomicU64 = AtomicU64::new(0);
pub static MISSES: AtomicU64 = AtomicU64::new(0);
/// Moves whenever a chunk opens or closes (the pool walker keeps its list of runs until then).
pub static RUNS_GEN: AtomicU64 = AtomicU64::new(0);

pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    cfg!(windows) && *ON.get_or_init(|| std::env::var("NXGFX_SEAL").as_deref() != Ok("0"))
}

/// `NXGFX_USE_SYNC`: chunk tracking (`1`, the default; `verify` also checks every scan's closed chunks; `0`: whole
/// quiet pools only, as SCAN-COST left it). Needs sealing on.
fn mode() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| match std::env::var("NXGFX_USE_SYNC").as_deref() {
        Ok("0") => 0,
        Ok("verify") => 2,
        _ => 1,
    })
}

/// Chunk tracking on (`NXGFX_USE_SYNC`).
pub fn chunks() -> bool {
    on() && mode() != 0
}

/// Chunk tracking's verify mode (`NXGFX_USE_SYNC=verify`).
pub fn verify() -> bool {
    on() && mode() == 2
}

/// Frames an open chunk stays open after it was last found written (`NXGFX_USE_SYNC_QUIET`, default 8): a chunk written
/// every frame or every other frame stays open (hot: no fault per frame), one written once is closed again soon.
pub fn quiet_chunk_frames() -> u64 {
    static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NXGFX_USE_SYNC_QUIET")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8)
    })
}

/// Frames without a CPU write or a write-back landing before a pool is sealed (`NXGFX_SEAL_FRAMES`, default 60).
pub fn quiet_frames() -> u64 {
    static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NXGFX_SEAL_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60)
    })
}

/// Bytes newly protected per sealing pass (`NXGFX_SEAL_MB`, default 32; a pass runs at most once a frame).
pub fn budget() -> u64 {
    static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NXGFX_SEAL_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(32)
            << 20
    })
}

thread_local! {
    /// This thread holds the registry: a fault here is not ours to take (it would wait for itself).
    static HOLDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct Held(std::sync::MutexGuard<'static, Reg>);
impl Drop for Held {
    fn drop(&mut self) {
        HOLDING.with(|h| h.set(false));
    }
}

fn reg() -> Held {
    let g = REG.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    HOLDING.with(|h| h.set(true));
    Held(g)
}

/// The chunk of [lo, end) holding `a`.
fn chunk_of(lo: u64, end: u64, a: u64) -> (u64, u64) {
    let c = (a & !(CHUNK - 1)).max(lo);
    (c, ((a & !(CHUNK - 1)) + CHUNK).min(end))
}

/// Arms the pages [lo, hi) of the pool at `pool`: read-only from now (a pool sealed before: only the chunks opened
/// since), so its first store wakes it. A first arming protects at most `max` bytes a call (a pool of hundreds of MB is
/// protected over several calls; a store into a piece already protected faults and wakes it as any). When it is all
/// protected (`Arm::Done`) the caller takes the write watch over it (a store made before the protection) and counts
/// the pool sealed only while `armed(lo)` still holds. Also the bytes protected by this call.
pub fn arm(pool: u64, lo: u64, hi: u64, max: u64) -> (Arm, u64) {
    if !on() || hi <= lo {
        return (Arm::Refused, 0);
    }
    install();
    let t0 = std::time::Instant::now();
    let mut r = reg();
    // another extent at this start (a pool replaced without its release reaching here): unsealed whole first
    if r.0.by_lo.get(&lo).is_some_and(|e| e.end != hi || e.pool != pool) {
        let e = r.0.by_lo.remove(&lo).unwrap();
        open_all(lo, e.end);
    }
    // an entry over another part of the range: refused (sealed pools never share pages)
    let below = r.0.by_lo.range(..lo).next_back().is_some_and(|(_, e)| e.end > lo);
    let inside = r.0.by_lo.range(lo + 1..hi).next().is_some();
    if below || inside {
        return (Arm::Refused, 0);
    }
    let e = r.0.by_lo.entry(lo).or_insert(Entry {
        end: hi,
        pool,
        armed: true,
        open: Vec::new(),
        last: Vec::new(),
        todo: lo,
    });
    let mut ranges = Vec::new();
    let mut todo = e.todo;
    if todo < hi {
        let to = (todo + max.max(CHUNK)).min(hi);
        ranges.push((todo, to));
        todo = to;
    }
    let done = todo >= hi;
    if done {
        // the chunks a store or a host write opened since the last arming (or during this one) closed again
        ranges.extend(std::mem::take(&mut e.open).into_iter().map(|c| chunk_of(lo, hi, c)));
        e.last.clear();
        RUNS_GEN.fetch_add(1, Ordering::AcqRel);
    }
    let bytes = ranges.iter().map(|c| c.1 - c.0).sum();
    // the committed pages (the rest is committed later through `host_write`, which wakes the pool)
    for &(a, b) in &ranges {
        protect_some(a, b, false);
    }
    let e = r.0.by_lo.get_mut(&lo).unwrap();
    e.todo = todo;
    if done {
        e.armed = true;
        ARMS.fetch_add(1, Ordering::Relaxed);
    }
    ARM_B.fetch_add(bytes, Ordering::Relaxed);
    ARM_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    (if done { Arm::Done } else { Arm::Part }, bytes)
}

/// Whether the entry at `lo` is armed (sealed and not written since).
pub fn armed(lo: u64) -> bool {
    reg().0.by_lo.get(&lo).is_some_and(|e| e.armed)
}

/// A store at `a` faulted: the chunk holding it is opened and its pool woken. False when `a` is in no entry (another
/// handler's fault).
pub fn fault(a: u64) -> bool {
    if HOLDING.with(|h| h.get()) {
        return false;
    }
    let mut r = reg();
    let Reg { by_lo, woken } = &mut *r.0;
    let Some((&lo, e)) = by_lo.range_mut(..=a).next_back() else {
        return false;
    };
    if a >= e.end {
        return false;
    }
    // the page first: one that will not open (not committed) is not this handler's (the runtime commits it, which
    // wakes the pool through `host_write`); retried here it would fault for ever
    let pg = a & !4095;
    if !protect(pg, pg + 4096, true) {
        return false;
    }
    let (c, ce) = chunk_of(lo, e.end, a);
    protect_some(c, ce, true);
    open_chunk(e, c);
    if e.armed {
        e.armed = false;
        woken.push(e.pool);
        ANY_WOKEN.store(true, Ordering::Release);
        WAKES.fetch_add(1, Ordering::Relaxed);
    }
    true
}

/// Chunk `c` (its start, as `open` keeps it) of `e` counted open from this frame.
fn open_chunk(e: &mut Entry, c: u64) {
    let f = super::present::frames();
    match e.open.binary_search(&c) {
        Ok(i) => e.last[i] = f,
        Err(i) => {
            e.open.insert(i, c);
            e.last.insert(i, f);
            OPENS.fetch_add(1, Ordering::Relaxed);
            RUNS_GEN.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// The host is about to write or recommit [a, a + n) without the protection seeing it (a recommit makes committed pages
/// read-write): every entry over it is woken and its chunks there opened.
pub fn host_write(a: u64, n: u64) {
    if !on() || n == 0 || !installed() || HOLDING.with(|h| h.get()) {
        return;
    }
    let mut r = reg();
    let Reg { by_lo, woken } = &mut *r.0;
    let first = by_lo
        .range(..=a)
        .next_back()
        .map(|(&lo, _)| lo)
        .unwrap_or(a);
    for (&lo, e) in by_lo.range_mut(first..a.saturating_add(n)) {
        if e.end <= a {
            continue;
        }
        let mut at = a.max(lo);
        let end = a.saturating_add(n).min(e.end);
        while at < end {
            let (c, ce) = chunk_of(lo, e.end, at);
            protect_some(c, ce, true);
            open_chunk(e, c);
            at = ce;
        }
        if e.armed {
            e.armed = false;
            woken.push(e.pool);
            ANY_WOKEN.store(true, Ordering::Release);
            HOST_WAKES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The walker found the armed entry at `lo` written without a fault: woken (its writes are then taken by the next scan).
pub fn leaked(lo: u64) -> bool {
    let mut r = reg();
    let Reg { by_lo, woken } = &mut *r.0;
    match by_lo.get_mut(&lo) {
        Some(e) if e.armed => {
            e.armed = false;
            woken.push(e.pool);
            ANY_WOKEN.store(true, Ordering::Release);
            LEAKS.fetch_add(1, Ordering::Relaxed);
            true
        }
        _ => false,
    }
}

/// Every entry over [lo, hi) unsealed whole (read-write again, no entry): a landing is due on it, its pool is gone or
/// replaced. The pools those entries named (their `sealed` flags go with them).
pub fn remove(lo: u64, hi: u64) -> Vec<u64> {
    if !installed() {
        return vec![];
    }
    let mut r = reg();
    let first = r
        .0
        .by_lo
        .range(..=lo)
        .next_back()
        .filter(|(_, e)| e.end > lo)
        .map(|(&l, _)| l)
        .unwrap_or(lo);
    let keys: Vec<u64> = r.0.by_lo.range(first..hi.max(first + 1)).map(|(&l, _)| l).collect();
    let mut out = Vec::new();
    for l in keys {
        if let Some(e) = r.0.by_lo.remove(&l) {
            open_all(l, e.end);
            out.push(e.pool);
            UNSEALS.fetch_add(1, Ordering::Relaxed);
        }
    }
    out
}

/// The pools woken since the last call (the pool lock's holder clears their `sealed` flags).
pub fn take_woken() -> Vec<u64> {
    if !ANY_WOKEN.load(Ordering::Acquire) {
        return vec![];
    }
    let mut r = reg();
    ANY_WOKEN.store(false, Ordering::Release);
    std::mem::take(&mut r.0.woken)
}

/// Chunk tracking: the open chunks of every entry armed whole at least once, merged into runs: (entry start, run start,
/// run end), ascending. A tracked pool's scan asks these only (its closed chunks were not written since they closed: a
/// store would have opened them first).
pub fn open_runs() -> Vec<(u64, u64, u64)> {
    if !installed() {
        return vec![];
    }
    let r = reg();
    let mut out: Vec<(u64, u64, u64)> = Vec::new();
    for (&lo, e) in &r.0.by_lo {
        if e.todo < e.end {
            continue;
        }
        for &c in &e.open {
            let (a, b) = chunk_of(lo, e.end, c);
            match out.last_mut() {
                Some(l) if l.0 == lo && l.2 == a => l.2 = b,
                _ => out.push((lo, a, b)),
            }
        }
    }
    out
}

/// Chunk tracking: the entry at `lo` had CPU writes taken over `ranges` (address, bytes) at `frame`: the open chunks
/// they touch stay open (`close_quiet`).
pub fn stamp(lo: u64, ranges: &[(u64, u64)], frame: u64) {
    if ranges.is_empty() || !installed() {
        return;
    }
    let mut r = reg();
    let Some(e) = r.0.by_lo.get_mut(&lo) else {
        return;
    };
    for &(a, n) in ranges {
        let mut at = a.max(lo);
        let end = a.saturating_add(n).min(e.end);
        while at < end {
            let (c, ce) = chunk_of(lo, e.end, at);
            if let Ok(i) = e.open.binary_search(&c) {
                e.last[i] = e.last[i].max(frame);
            }
            at = ce;
        }
    }
}

/// Chunk tracking: the open chunks of the entry at `lo` not written for `quiet` frames by `frame` are protected again.
/// Returns them (the caller, holding the pool and watch locks, takes the write watch over them: a store made before the
/// protection) and whether the entry has no open chunk now (armed: its pool counts as sealed while no store wakes it).
/// A store into one after the protection faults (it waits for this call) and opens it again.
pub fn close_quiet(lo: u64, frame: u64, quiet: u64) -> (Vec<(u64, u64)>, bool) {
    if !installed() {
        return (vec![], false);
    }
    let mut r = reg();
    let Some(e) = r.0.by_lo.get_mut(&lo) else {
        return (vec![], false);
    };
    if e.todo < e.end {
        return (vec![], false);
    }
    let mut closed = Vec::new();
    let (mut open, mut last) = (Vec::with_capacity(e.open.len()), Vec::with_capacity(e.open.len()));
    for (&c, &l) in e.open.iter().zip(&e.last) {
        if frame.saturating_sub(l) >= quiet {
            let (a, b) = chunk_of(lo, e.end, c);
            protect_some(a, b, false);
            match closed.last_mut() {
                Some((_, pb)) if *pb == a => *pb = b,
                _ => closed.push((a, b)),
            }
            CLOSES.fetch_add(1, Ordering::Relaxed);
        } else {
            open.push(c);
            last.push(l);
        }
    }
    if !closed.is_empty() {
        RUNS_GEN.fetch_add(1, Ordering::AcqRel);
    }
    e.open = open;
    e.last = last;
    if e.open.is_empty() && !e.armed {
        e.armed = true;
        ARMS.fetch_add(1, Ordering::Relaxed);
    }
    (closed, e.armed)
}

/// Verify mode: the closed chunks of the entry at `lo` (armed whole at least once) as runs.
pub fn closed_runs(lo: u64) -> Vec<(u64, u64)> {
    if !installed() {
        return vec![];
    }
    let r = reg();
    let Some(e) = r.0.by_lo.get(&lo) else {
        return vec![];
    };
    if e.todo < e.end {
        return vec![];
    }
    let mut out = Vec::new();
    let mut at = lo;
    for &c in &e.open {
        let (a, b) = chunk_of(lo, e.end, c);
        if a > at {
            out.push((at, a));
        }
        at = at.max(b);
    }
    if at < e.end {
        out.push((at, e.end));
    }
    out
}

/// Verify mode: page `pg` of the entry at `lo` was found written in a chunk that is closed now (a store no fault saw):
/// counted, and its chunk opened so the scans ask it. False when its chunk is open now (a store opened it meanwhile:
/// no miss).
pub fn missed(lo: u64, pg: u64) -> bool {
    if !installed() {
        return false;
    }
    let mut r = reg();
    let Reg { by_lo, woken } = &mut *r.0;
    let Some(e) = by_lo.get_mut(&lo) else {
        return false;
    };
    if pg < lo || pg >= e.end {
        return false;
    }
    let (c, ce) = chunk_of(lo, e.end, pg);
    if e.open.binary_search(&c).is_ok() {
        return false;
    }
    protect_some(c, ce, true);
    open_chunk(e, c);
    if e.armed {
        e.armed = false;
        woken.push(e.pool);
        ANY_WOKEN.store(true, Ordering::Release);
    }
    MISSES.fetch_add(1, Ordering::Relaxed);
    true
}

/// Whether page `p` lies in an entry (lazy write-back's handler leaves such a fault to this one).
pub fn holds(p: u64) -> bool {
    if !installed() || HOLDING.with(|h| h.get()) {
        return false;
    }
    let r = reg();
    r.0.by_lo
        .range(..=p)
        .next_back()
        .is_some_and(|(_, e)| p < e.end)
}

/// (entries armed, their bytes) for the pools line.
pub fn armed_total() -> (u64, u64) {
    if !installed() {
        return (0, 0);
    }
    let r = reg();
    r.0.by_lo
        .iter()
        .filter(|(_, e)| e.armed)
        .fold((0, 0), |(n, b), (&lo, e)| (n + 1, b + (e.end - lo)))
}

/// The pools line's part.
pub fn summary(frames: f64) -> String {
    if !on() {
        return String::new();
    }
    let (n, b) = armed_total();
    let t = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / frames;
    let (w, h, l, a) = (t(&WAKES), t(&HOST_WAKES), LEAKS.load(Ordering::Relaxed), t(&ARMS));
    let mb = ARM_B.swap(0, Ordering::Relaxed) as f64 / frames / 1048576.0;
    let ms = ARM_NS.swap(0, Ordering::Relaxed) as f64 / frames / 1e6;
    let pass = PASS_NS.swap(0, Ordering::Relaxed) as f64 / frames / 1e6;
    let u = UNSEALS.load(Ordering::Relaxed);
    let chunks = if chunks() {
        let (on, ob) = open_total();
        let (o, c) = (t(&OPENS), t(&CLOSES));
        let sb = OPEN_SCAN_B.swap(0, Ordering::Relaxed) as f64 / frames / 1048576.0;
        format!(
            "; chunk tracking{}: {on} chunks open ({} MB), a frame {o:.1} opened, {c:.1} closed, {sb:.2} MB of open chunks scanned; {} written pages missed in closed chunks so far",
            if verify() { " (verify)" } else { "" },
            ob >> 20,
            MISSES.load(Ordering::Relaxed)
        )
    } else {
        String::new()
    };
    format!(
        "; sealed pools: {n} ({} MB) out of every scan; a frame {w:.2} woken by a store, {h:.2} by a host write, {a:.2} armed ({mb:.2} MB protected in {ms:.3} ms; sealing pass {pass:.3} ms under the pool lock); {u} unsealed whole so far, {l} found written without a fault so far{chunks}",
        b >> 20
    )
}

/// (open chunks, their bytes) for the pools line.
fn open_total() -> (u64, u64) {
    if !installed() {
        return (0, 0);
    }
    let r = reg();
    r.0.by_lo.iter().fold((0, 0), |(n, b), (&lo, e)| {
        let bytes: u64 = e.open.iter().map(|&c| {
            let (a, z) = chunk_of(lo, e.end, c);
            z - a
        }).sum();
        (n + e.open.len() as u64, b + bytes)
    })
}

static HANDLER: AtomicBool = AtomicBool::new(false);

fn installed() -> bool {
    HANDLER.load(Ordering::Acquire)
}

#[cfg(windows)]
fn install() {
    use windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler;
    if !HANDLER.swap(true, Ordering::SeqCst) {
        // the runtime's own commits (its lazy heap) wake what they make read-write, as the OS layer's do
        nxrt::mem::set_commit_hook(host_write);
        // first in the chain: the runtime's first-touch commit (a recommit, read-write) and crash reporter must not
        // see a sealed page's store
        unsafe { AddVectoredExceptionHandler(1, Some(veh)) };
    }
}
#[cfg(not(windows))]
fn install() {}

#[cfg(windows)]
unsafe extern "system" fn veh(
    ep: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    // SAFETY: the record the OS hands every vectored handler
    let er = unsafe { &*(*ep).ExceptionRecord };
    // a store (information 0 = 1) into a sealed page
    if er.ExceptionCode != EXCEPTION_ACCESS_VIOLATION
        || er.NumberParameters < 2
        || er.ExceptionInformation[0] != 1
    {
        return CONTINUE_SEARCH;
    }
    if fault(er.ExceptionInformation[1] as u64) {
        CONTINUE_EXECUTION
    } else {
        CONTINUE_SEARCH
    }
}

/// The committed pages of [lo, hi) read-write (`open`) or read-only: the whole range in one call, else chunk by chunk,
/// else page by page (a pool's memory may be committed in parts, 64 KiB at a time: the protection call refuses a range
/// with an uncommitted page in it). An uncommitted page needs no protection: it cannot be written before it is
/// committed, and a commit goes through `host_write` (the runtime's commit), which wakes the pool.
fn protect_some(lo: u64, hi: u64, open: bool) {
    if protect(lo, hi, open) {
        return;
    }
    // the committed runs, as the OS lists them
    for (a, b) in committed_runs(lo, hi) {
        protect(a, b, open);
    }
}

/// The committed runs of [lo, hi).
#[cfg(windows)]
fn committed_runs(lo: u64, hi: u64) -> Vec<(u64, u64)> {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    let mut out = Vec::new();
    let mut at = lo;
    while at < hi {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        // SAFETY: a query of this process's own address space
        if unsafe { VirtualQuery(at as *const _, &mut mbi, core::mem::size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
            break;
        }
        let e = (mbi.BaseAddress as u64 + mbi.RegionSize as u64).min(hi);
        if e <= at {
            break;
        }
        if mbi.State == MEM_COMMIT {
            out.push((at, e));
        }
        at = e;
    }
    out
}
#[cfg(not(windows))]
fn committed_runs(_lo: u64, _hi: u64) -> Vec<(u64, u64)> {
    Vec::new()
}

/// [lo, hi) read-write again (its entry goes, or never was).
fn open_all(lo: u64, hi: u64) {
    protect_some(lo, hi, true)
}

/// [a, b) read-write (`open`) or read-only.
#[cfg(windows)]
fn protect(a: u64, b: u64, open: bool) -> bool {
    use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_READONLY, PAGE_READWRITE};
    let mut old = 0u32;
    let p = if open { PAGE_READWRITE } else { PAGE_READONLY };
    // SAFETY: a protection change of guest pages this module sealed (or is about to); no Rust reference spans them
    b <= a || unsafe { VirtualProtect(a as *const _, (b - a) as usize, p, &mut old) } != 0
}
#[cfg(not(windows))]
fn protect(_a: u64, _b: u64, _open: bool) -> bool {
    false
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{
        GetWriteWatch, VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
    };
    const MEM_WRITE_WATCH: u32 = 0x20_0000;

    struct Alloc(u64, u64);
    impl Drop for Alloc {
        fn drop(&mut self) {
            remove(self.0, self.0 + self.1);
            unsafe { VirtualFree(self.0 as *mut _, 0, MEM_RELEASE) };
        }
    }
    fn alloc(n: u64) -> Alloc {
        let p = unsafe {
            VirtualAlloc(core::ptr::null(), n as usize, MEM_COMMIT | MEM_RESERVE | MEM_WRITE_WATCH, PAGE_READWRITE)
        };
        assert!(!p.is_null());
        Alloc(p as u64, n)
    }
    /// The written pages of [lo, hi), reset.
    fn written(lo: u64, hi: u64) -> Vec<u64> {
        let mut v = vec![0usize; 4096];
        let (mut n, mut g) = (v.len(), 0u32);
        let r = unsafe { GetWriteWatch(1, lo as *const _, (hi - lo) as usize, v.as_mut_ptr() as *mut _, &mut n, &mut g) };
        assert_eq!(r, 0);
        v[..n].iter().map(|&p| p as u64).collect()
    }
    /// Armed in one call.
    fn arm1(pool: u64, lo: u64, hi: u64) -> bool {
        arm(pool, lo, hi, u64::MAX >> 1).0 == Arm::Done
    }

    /// A first arming in pieces: each call protects at most its bytes; a store into a piece already protected wakes
    /// it (the pool is never counted sealed); once all of it is protected the chunks opened meanwhile are closed too and
    /// it is armed.
    #[test]
    fn a_large_pool_is_armed_in_pieces() {
        if !on() {
            return;
        }
        let m = alloc(4 * CHUNK);
        let (lo, hi) = (m.0, m.0 + m.1);
        written(lo, hi);
        assert_eq!(arm(lo, lo, hi, CHUNK), (Arm::Part, CHUNK));
        assert_eq!(arm(lo, lo, hi, CHUNK), (Arm::Part, CHUNK));
        // the first two chunks are read-only: a store there faults and wakes it
        unsafe { ((lo + 8) as *mut u8).write_volatile(1) };
        // (the woken queue is shared with other tests, which may drain it: the entry tells)
        assert!(!armed(lo), "woken");
        // past the protected pieces: no fault
        let wakes = WAKES.load(Ordering::Relaxed);
        unsafe { ((lo + 3 * CHUNK) as *mut u8).write_volatile(1) };
        assert_eq!(WAKES.load(Ordering::Relaxed), wakes);
        assert_eq!(arm(lo, lo, hi, CHUNK), (Arm::Part, CHUNK));
        // the last piece, and the chunk the store opened
        assert_eq!(arm(lo, lo, hi, CHUNK), (Arm::Done, 2 * CHUNK));
        assert!(armed(lo));
        assert_eq!(written(lo, hi), vec![lo, lo + 3 * CHUNK], "both stores in the write watch");
        unsafe { ((lo + 8) as *mut u8).write_volatile(2) };
        assert!(!armed(lo), "the opened chunk is read-only again");
    }

    /// A store into an armed range faults once, wakes its pool (queued for the syncs), opens only its chunk, lands in
    /// memory and is in the write watch; the next store into another chunk opens that one without waking again.
    /// Arming again protects only the opened chunks; a store wakes it again.
    #[test]
    fn a_store_into_a_sealed_pool_wakes_it_and_is_watched() {
        if !on() {
            return;
        }
        let m = alloc(4 * CHUNK);
        let (lo, hi) = (m.0, m.0 + m.1);
        let pool = lo + 0x10;
        written(lo, hi);
        assert!(arm1(pool, lo, hi));
        assert!(armed(lo));
        assert!(holds(lo + CHUNK));
        let wakes = WAKES.load(Ordering::Relaxed);
        unsafe { ((lo + CHUNK + 8) as *mut u64).write_volatile(0x1234) };
        assert!(!armed(lo), "the store woke it");
        assert!(WAKES.load(Ordering::Relaxed) > wakes);
        assert_eq!(unsafe { ((lo + CHUNK + 8) as *const u64).read_volatile() }, 0x1234);
        assert_eq!(written(lo, hi), vec![lo + CHUNK], "the store is in the write watch");
        {
            let r = reg();
            assert_eq!(r.0.by_lo[&lo].open, vec![lo + CHUNK], "only the store's chunk is open");
        }
        // another chunk: opened, the pool already woken
        unsafe { ((lo + 3 * CHUNK) as *mut u8).write_volatile(7) };
        {
            let r = reg();
            assert_eq!(r.0.by_lo[&lo].open, vec![lo + CHUNK, lo + 3 * CHUNK]);
        }
        // armed again: the open chunks are closed; a store wakes it once more
        assert!(arm1(pool, lo, hi));
        assert!(reg().0.by_lo[&lo].open.is_empty());
        written(lo, hi);
        unsafe { (lo as *mut u8).write_volatile(1) };
        assert!(!armed(lo));
        assert_eq!(written(lo, hi), vec![lo]);
    }

    /// Chunk tracking: stores open only their chunks (listed by `open_runs`, adjacent ones as one run); a chunk found
    /// written stays open, a quiet one closes (read-only: the next store faults and opens it again); with none open the
    /// entry is armed again. `closed_runs` is the rest; a page written in a closed chunk without a fault is a miss (its
    /// chunk opens), one in an open chunk is not.
    #[test]
    fn chunks_open_at_a_store_and_close_when_quiet() {
        if !on() {
            return;
        }
        let m = alloc(4 * CHUNK);
        let (lo, hi) = (m.0, m.0 + m.1);
        assert!(arm1(lo, lo, hi));
        written(lo, hi);
        unsafe { ((lo + CHUNK + 8) as *mut u8).write_volatile(1) };
        unsafe { ((lo + 2 * CHUNK + 8) as *mut u8).write_volatile(1) };
        let mine = || open_runs().into_iter().filter(|r| r.0 == lo).collect::<Vec<_>>();
        assert_eq!(mine(), vec![(lo, lo + CHUNK, lo + 3 * CHUNK)], "two adjacent open chunks: one run");
        assert_eq!(closed_runs(lo), vec![(lo, lo + CHUNK), (lo + 3 * CHUNK, hi)]);
        let f0 = super::super::present::frames();
        // the first chunk found written 10 frames on; at that frame with 5 quiet frames the second closes
        stamp(lo, &[(lo + CHUNK + 8, 1)], f0 + 10);
        let (closed, armed_now) = close_quiet(lo, f0 + 10, 5);
        assert_eq!(closed, vec![(lo + 2 * CHUNK, lo + 3 * CHUNK)]);
        assert!(!armed_now, "one chunk still open");
        assert_eq!(mine(), vec![(lo, lo + CHUNK, lo + 2 * CHUNK)]);
        // closed again: a store faults and opens it
        let opens = OPENS.load(Ordering::Relaxed);
        unsafe { ((lo + 2 * CHUNK + 16) as *mut u8).write_volatile(2) };
        assert!(OPENS.load(Ordering::Relaxed) > opens);
        assert_eq!(mine(), vec![(lo, lo + CHUNK, lo + 3 * CHUNK)]);
        // a page written in an open chunk is no miss; one written past the protection (a write no fault saw) is
        assert!(!missed(lo, lo + CHUNK));
        assert!(protect(lo + 3 * CHUNK, lo + 3 * CHUNK + 4096, true));
        unsafe { ((lo + 3 * CHUNK) as *mut u8).write_volatile(3) };
        let misses = MISSES.load(Ordering::Relaxed);
        assert!(missed(lo, lo + 3 * CHUNK));
        assert!(MISSES.load(Ordering::Relaxed) > misses);
        assert_eq!(mine(), vec![(lo, lo + CHUNK, hi)], "the missed page's chunk is open now");
        // everything quiet: all closed, armed again (sealed), nothing open
        let (closed, armed_now) = close_quiet(lo, f0 + 1000, 5);
        assert_eq!(closed, vec![(lo + CHUNK, hi)]);
        assert!(armed_now && armed(lo));
        assert!(mine().is_empty());
        assert_eq!(closed_runs(lo), vec![(lo, hi)]);
    }

    /// A host write announced before it (a recommit) wakes the pool and opens the chunks it covers; `remove` unseals the
    /// whole range (stores go through without faults) and names the pool.
    #[test]
    fn host_writes_and_removal_leave_nothing_sealed() {
        if !on() {
            return;
        }
        let m = alloc(4 * CHUNK);
        let (lo, hi) = (m.0, m.0 + m.1);
        assert!(arm1(lo, lo, hi));
        let hw = HOST_WAKES.load(Ordering::Relaxed);
        host_write(lo + CHUNK - 8, 16);
        assert!(!armed(lo));
        assert!(HOST_WAKES.load(Ordering::Relaxed) > hw);
        assert_eq!(reg().0.by_lo[&lo].open, vec![lo, lo + CHUNK], "both chunks the write spans");
        assert!(arm1(lo, lo, hi));
        assert_eq!(remove(lo, hi), vec![lo]);
        assert!(!holds(lo));
        let wakes = WAKES.load(Ordering::Relaxed);
        unsafe { ((lo + 2 * CHUNK) as *mut u8).write_volatile(1) };
        assert_eq!(WAKES.load(Ordering::Relaxed), wakes, "read-write again: no fault");
    }

    /// A range over another entry is refused; the walker's check wakes an armed entry once. A range committed in part
    /// is armed over its committed pages (a store there wakes it); its other pages are committed later through
    /// `host_write` (the runtime's commit), which wakes it too, and the page is written without a fault.
    #[test]
    fn overlapping_and_uncommitted_ranges_are_not_sealed() {
        if !on() {
            return;
        }
        let m = alloc(4 * CHUNK);
        let (lo, hi) = (m.0, m.0 + m.1);
        assert!(arm1(lo, lo + CHUNK, lo + 3 * CHUNK));
        assert!(!arm1(lo, lo, lo + 2 * CHUNK), "over an entry");
        assert!(!arm1(lo, lo + 2 * CHUNK, hi), "inside an entry");
        assert!(leaked(lo + CHUNK));
        assert!(!leaked(lo + CHUNK), "once");
        remove(lo, hi);
        // reserved but not committed past the first page
        let r = unsafe { VirtualAlloc(core::ptr::null(), (2 * CHUNK) as usize, MEM_RESERVE | MEM_WRITE_WATCH, PAGE_READWRITE) } as u64;
        assert!(r != 0);
        assert!(!unsafe { VirtualAlloc(r as *mut _, 4096, MEM_COMMIT, PAGE_READWRITE) }.is_null());
        assert!(arm1(r, r, r + 2 * CHUNK));
        assert!(holds(r));
        unsafe { (r as *mut u8).write_volatile(1) };
        assert!(!armed(r), "the committed page is sealed: the store woke it");
        assert!(arm1(r, r, r + 2 * CHUNK));
        // a commit of the rest, announced as the runtime does
        host_write(r + CHUNK, 4096);
        assert!(!armed(r), "the commit woke it");
        assert!(!unsafe { VirtualAlloc((r + CHUNK) as *mut _, 4096, MEM_COMMIT, PAGE_READWRITE) }.is_null());
        let wakes = WAKES.load(Ordering::Relaxed);
        unsafe { ((r + CHUNK) as *mut u8).write_volatile(1) };
        assert_eq!(WAKES.load(Ordering::Relaxed), wakes, "committed read-write: no fault");
        remove(r, r + 2 * CHUNK);
        unsafe { VirtualFree(r as *mut _, 0, MEM_RELEASE) };
    }
}

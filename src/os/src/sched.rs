//! Virtual cores: Horizon runs at most one thread per core, the best ready one, and games rely on it (a
//! higher-priority thread started on the caller's core runs before the caller goes on; per-core data is keyed by
//! the core number without locks). Guest threads run on free host threads, so each guest thread holds one of the
//! application's cores (three on Switch 1) while it runs guest code, as mkw-recomp's kernel does:
//!
//! * it takes a core when it starts and gives it back where Horizon switches threads: the blocking SVCs (waits,
//!   locks, condition variables, address waits, sleep), yields, ExitThread, and host functions that block
//!   (`without_core`). The runtime's kernel tells it through `nxrt::kernel::Scheduler` (thread start and exit,
//!   blocking enter and leave, priority and core mask changes, GetCurrentProcessorNumber; installed by `init`);
//! * a free core goes to the best waiting thread that may run there (priority, then first come); a thread prefers
//!   its ideal core and migrates within its affinity mask;
//! * preemption at checkpoints: after every SVC os answers (`svc.rs`), and at every pass of a marked spin-wait loop
//!   (the runtime's `rt_spin_gate`, `Scheduler::spin_pass`),
//!   a thread that holds a core a better thread waits for hands it over and queues again;
//! * a waiter that has waited 2 ms for a core held by a worse thread (20 ms for an equal or better one: a holder
//!   spinning in code with no checkpoint) takes the core; the holder gives up its claim at its next checkpoint;
//! * GetCurrentProcessorNumber answers the core the thread holds.
//!
//! `NXOS_FREE_CORES=1` turns the gate off (every thread runs whenever it is ready).

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_CORES: usize = 8; // a Switch 2 program has 6 (its NPDM's cores 0-5)
const STEAL_WORSE: Duration = Duration::from_millis(2);
const STEAL_ANY: Duration = Duration::from_millis(20);

#[derive(Clone, Debug)]
struct Th {
    prio: i32,
    ideal: usize,
    mask: u64,
    /// The core it holds.
    core: Option<usize>,
    waiting: bool,
    wseq: u64,
    /// A waiter took its core: it gives up the claim at its next checkpoint.
    displaced: bool,
    handle: u32,
    alive: bool,
    /// Host functions it is inside (`host_enter` depth): a holder there runs no guest code, so taking its core
    /// lets no two threads run guest code on one core.
    host: std::sync::Arc<AtomicU64>,
    /// Mirror of `displaced` for `host_leave` (no lock on the hot path).
    disp: std::sync::Arc<AtomicBool>,
}

struct State {
    threads: Vec<Th>,
    owner: [Option<usize>; MAX_CORES],
    ncores: usize,
    seq: u64,
}

static ST: Mutex<State> = Mutex::new(State {
    threads: Vec::new(),
    owner: [None; MAX_CORES],
    ncores: 3,
    seq: 0,
});
static CV: Condvar = Condvar::new();
static OFF: OnceLock<bool> = OnceLock::new();
pub static HANDOFFS: AtomicU64 = AtomicU64::new(0);
pub static STEALS: AtomicU64 = AtomicU64::new(0);
/// Steals from an equal or better holder (no Horizon equivalent: such a holder keeps its core until it blocks).
pub static STEALS_ANY: AtomicU64 = AtomicU64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static ME: Cell<Option<usize>> = const { Cell::new(None) };
    static ME_HOST: std::cell::RefCell<Option<(std::sync::Arc<AtomicU64>, std::sync::Arc<AtomicBool>)>> = const { std::cell::RefCell::new(None) };
}

/// A holder running guest code keeps its core this long against any waiter (Horizon would pause it; here it would run
/// on beside the taker): past it the waiter takes the core anyway (a holder spinning with no checkpoint). 200 ms broke
/// Donkey Kong Bananza: its loaders run longer than that between SVCs.
const STEAL_GUEST: Duration = Duration::from_millis(5000);
pub static STEALS_GUEST: AtomicU64 = AtomicU64::new(0);

/// `NXOS_STEAL_ANY_MS=<ms>` (debugging): how long a waiter waits before it takes a core from an equal or better holder
/// (default 20; 0 = never, as Horizon, where an equal priority holder keeps its core until it blocks or yields).
fn steal_any() -> Option<Duration> {
    static V: OnceLock<Option<Duration>> = OnceLock::new();
    *V.get_or_init(|| {
        match std::env::var("NXOS_STEAL_ANY_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            Some(0) => None,
            Some(ms) => Some(Duration::from_millis(ms)),
            None => Some(STEAL_ANY),
        }
    })
}

/// `NXOS_STEAL_WORSE_MS=<ms>` (debugging): how long a waiter waits before it takes a core from a worse holder (default
/// 2; 0 = never: with `NXOS_CORES=1` and `NXOS_STEAL_ANY_MS=0` no two guest threads ever run at once).
fn steal_worse() -> Option<Duration> {
    static V: OnceLock<Option<Duration>> = OnceLock::new();
    *V.get_or_init(|| {
        match std::env::var("NXOS_STEAL_WORSE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            Some(0) => None,
            Some(ms) => Some(Duration::from_millis(ms)),
            None => Some(STEAL_WORSE),
        }
    })
}

fn off() -> bool {
    *OFF.get_or_init(|| std::env::var("NXOS_FREE_CORES").is_ok_and(|v| v == "1"))
}

/// Configures the core count from the process core mask and installs the scheduler (before any thread starts).
pub fn init(core_mask: u64) {
    // `NXOS_CORES=<n>` (debugging): fewer virtual cores than the process mask gives (1 = every guest thread in turn)
    let want = std::env::var("NXOS_CORES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n >= 1);
    let n = want
        .unwrap_or(core_mask.count_ones() as usize)
        .clamp(1, MAX_CORES);
    ST.lock().unwrap().ncores = n;
    STARTED.store(true, Ordering::Relaxed);
    nxrt::kernel::set_scheduler(&Cores);
    crate::log!(
        "scheduler: {n} virtual cores{}",
        if off() {
            " are OFF (NXOS_FREE_CORES=1): threads run freely"
        } else {
            ", the best ready thread on each; checkpoints at SVCs and spin waits"
        }
    );
}

fn new_thread(st: &mut State, prio: i32, ideal: i32, mask: u64, handle: u32) -> usize {
    let all = (1u64 << st.ncores) - 1;
    let ideal = if ideal < 0 || ideal as usize >= st.ncores {
        0
    } else {
        ideal as usize
    };
    let mask = if mask & all == 0 {
        1 << ideal
    } else {
        mask & all
    };
    st.threads.push(Th {
        prio,
        ideal,
        mask,
        core: None,
        waiting: false,
        wseq: 0,
        displaced: false,
        handle,
        alive: true,
        host: Default::default(),
        disp: Default::default(),
    });
    st.threads.len() - 1
}

/// Whether thread `i` is the best waiter for core `c`.
fn best_waiter(st: &State, c: usize) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (k, t) in st.threads.iter().enumerate() {
        if !t.waiting || t.mask >> c & 1 == 0 {
            continue;
        }
        best = match best {
            None => Some(k),
            Some(b) => {
                let bt = &st.threads[b];
                if t.prio < bt.prio || (t.prio == bt.prio && t.wseq < bt.wseq) {
                    Some(k)
                } else {
                    Some(b)
                }
            }
        };
    }
    best
}

fn try_take(st: &mut State, i: usize) -> bool {
    let (ideal, mask) = (st.threads[i].ideal, st.threads[i].mask);
    let order = std::iter::once(ideal).chain((0..st.ncores).filter(move |&c| c != ideal));
    for c in order {
        if mask >> c & 1 == 0 || st.owner[c].is_some() {
            continue;
        }
        if best_waiter(st, c) == Some(i) {
            st.owner[c] = Some(i);
            let t = &mut st.threads[i];
            t.core = Some(c);
            t.waiting = false;
            t.displaced = false;
            t.disp.store(false, Ordering::SeqCst);
            return true;
        }
    }
    false
}

fn acquire(i: usize) {
    let mut st = ST.lock().unwrap();
    st.seq += 1;
    let s = st.seq;
    {
        let t = &mut st.threads[i];
        if t.core.is_some() && !t.displaced {
            return;
        }
        t.core = None;
        t.waiting = true;
        t.wseq = s;
    }
    // the profiler's samples count this as a wait for a core (back to the state before on any return)
    struct Mark(u32);
    impl Drop for Mark {
        fn drop(&mut self) {
            nxrt::profile::mark(self.0);
        }
    }
    let _mark = Mark(nxrt::profile::mark(nxrt::profile::W_CORE));
    let t0 = Instant::now();
    // wait accounting: the time until a core is taken (any return below)
    struct Acct(Instant);
    impl Drop for Acct {
        fn drop(&mut self) {
            if nxrt::waitacct::on() {
                nxrt::waitacct::add(
                    nxrt::waitacct::K_CORE,
                    0,
                    self.0.elapsed().as_nanos() as u64,
                    nxrt::waitacct::BY_NONE,
                );
            }
        }
    }
    let _acct = Acct(t0);
    loop {
        if try_take(&mut st, i) {
            return;
        }
        let waited = t0.elapsed();
        if steal_worse().is_some_and(|d| waited >= d) || steal_any().is_some_and(|d| waited >= d) {
            // a core the waiter may use, held by a thread worse than it (or any holder after STEAL_ANY)
            let me = st.threads[i].clone();
            let pick = (0..st.ncores)
                .filter(|&c| me.mask >> c & 1 != 0)
                .find(|&c| {
                    st.owner[c].is_some_and(|o| {
                        let t = &st.threads[o];
                        let in_host = t.host.load(Ordering::Relaxed) > 0 || waited >= STEAL_GUEST;
                        in_host
                            && ((t.prio > me.prio && steal_worse().is_some_and(|d| waited >= d))
                                || steal_any().is_some_and(|d| waited >= d))
                    }) && best_waiter(&st, c) == Some(i)
                });
            // the holder may be leaving its host function right now: it reads `disp` after it drops `host` (SeqCst on
            // both sides), so either it sees the flag and waits for a core, or this sees it back in guest code and
            // leaves it the core (before STEAL_GUEST)
            let pick = pick.filter(|&c| {
                let o = st.owner[c].unwrap();
                st.threads[o].disp.store(true, Ordering::SeqCst);
                let ok = st.threads[o].host.load(Ordering::SeqCst) > 0 || waited >= STEAL_GUEST;
                if !ok {
                    st.threads[o]
                        .disp
                        .store(st.threads[o].displaced, Ordering::SeqCst);
                }
                ok
            });
            if let Some(c) = pick {
                if let Some(o) = st.owner[c] {
                    st.threads[o].displaced = true;
                    st.threads[o].disp.store(true, Ordering::SeqCst);
                    if st.threads[o].host.load(Ordering::Relaxed) == 0
                        && STEALS_GUEST.fetch_add(1, Ordering::Relaxed) < 16
                    {
                        crate::log!(
                            "scheduler: thread {:#x} took core {c} from thread {:#x}, which ran guest code with no checkpoint for {:.0} ms",
                            me.handle,
                            st.threads[o].handle,
                            waited.as_secs_f64() * 1e3
                        );
                    }
                    if st.threads[o].prio <= me.prio
                        && STEALS_ANY.fetch_add(1, Ordering::Relaxed) < 16
                    {
                        crate::log!(
                            "scheduler: thread {:#x} (priority {}) took core {c} from thread {:#x} (priority {}) after {:.1} ms: the holder runs on until its next checkpoint",
                            me.handle,
                            me.prio,
                            st.threads[o].handle,
                            st.threads[o].prio,
                            waited.as_secs_f64() * 1e3
                        );
                    }
                }
                st.owner[c] = Some(i);
                let t = &mut st.threads[i];
                t.core = Some(c);
                t.waiting = false;
                t.displaced = false;
                t.disp.store(false, Ordering::SeqCst);
                STEALS.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        st = CV.wait_timeout(st, Duration::from_micros(500)).unwrap().0;
    }
}

fn release(i: usize) {
    let mut st = ST.lock().unwrap();
    let t = &mut st.threads[i];
    let core = t.core.take();
    let displaced = std::mem::replace(&mut t.displaced, false);
    t.disp.store(false, Ordering::SeqCst);
    if let Some(c) = core {
        if !displaced && st.owner[c] == Some(i) {
            st.owner[c] = None;
        }
    }
    drop(st);
    CV.notify_all();
}

/// The calling thread's scheduler id, if it is a guest thread the scheduler knows.
pub fn me() -> Option<usize> {
    ME.get()
}

/// Gives the core up around `f` (a host function that blocks, as a guest thread blocked in the kernel would).
pub fn without_core<R>(f: impl FnOnce() -> R) -> R {
    // wait accounting: the host function the thread blocks in (its guest state names it)
    let t0 = nxrt::waitacct::on().then(Instant::now);
    let r = match ME.get().filter(|_| !off()) {
        Some(i) => {
            release(i);
            let r = f();
            host_wait_done(t0);
            acquire(i);
            return r;
        }
        None => f(),
    };
    host_wait_done(t0);
    r
}

/// `waitacct`: a blocking host function's wait (since `t0`) ended, keyed by the host function's address.
fn host_wait_done(t0: Option<Instant>) {
    let Some(t0) = t0 else { return };
    // SAFETY: the calling thread's own guest state
    let pc = nxrt::kernel::current_cpu().map_or(0, |c| unsafe { (*c).pc });
    nxrt::waitacct::add(
        nxrt::waitacct::K_HOST,
        pc,
        t0.elapsed().as_nanos() as u64,
        nxrt::waitacct::BY_NONE,
    );
}

/// A checkpoint: hand the core to a better waiter (or anyone, with `to_any`), or take a core back after a steal.
pub fn checkpoint(to_any: bool) {
    let Some(i) = ME.get() else { return };
    if off() {
        return;
    }
    let yield_now = {
        let st = ST.lock().unwrap();
        let t = &st.threads[i];
        match t.core {
            _ if t.displaced => true,
            None => true,
            Some(c) => best_waiter(&st, c).is_some_and(|w| to_any || st.threads[w].prio < t.prio),
        }
    };
    if yield_now {
        HANDOFFS.fetch_add(1, Ordering::Relaxed);
        release(i);
        acquire(i);
    }
}

/// The core the calling thread holds (GetCurrentProcessorNumber).
pub fn current_core() -> Option<usize> {
    let i = ME.get()?;
    ST.lock().unwrap().threads[i].core
}

fn by_handle(h: u32) -> Option<usize> {
    let st = ST.lock().unwrap();
    if h == 0xFFFF_8000 {
        return ME.get();
    }
    st.threads.iter().position(|t| t.handle == h && t.alive)
}

/// The main thread's priority and ideal core (main.npdm), for its start.
static MAIN: OnceLock<(i32, i32)> = OnceLock::new();

pub fn set_main(prio: i32, core: i32) {
    let _ = MAIN.set((prio, core));
}

/// The scheduler is on and the calling thread is one of its guest threads.
pub fn active() -> bool {
    !off() && STARTED.load(Ordering::Relaxed) && ME.get().is_some()
}

/// CreateThread made thread `handle` (not started yet): the scheduler's copy of its priority and ideal core.
pub fn created(handle: u32, prio: i32, core: i32) {
    let mut st = ST.lock().unwrap();
    new_thread(&mut st, prio, core, 0, handle);
}

/// A guest thread starts on this host thread: it takes a core before its entry runs. A thread os did not see
/// created is the main thread (main.npdm's priority and core).
fn start(handle: u32) {
    let i = {
        let mut st = ST.lock().unwrap();
        match st
            .threads
            .iter()
            .position(|t| t.handle == handle && t.alive)
        {
            Some(i) => i,
            None => {
                let (prio, core) = MAIN.get().copied().unwrap_or((44, 0));
                new_thread(&mut st, prio, core, 0, handle)
            }
        }
    };
    ME.set(Some(i));
    let h = {
        let st = ST.lock().unwrap();
        (st.threads[i].host.clone(), st.threads[i].disp.clone())
    };
    ME_HOST.with(|m| *m.borrow_mut() = Some(h));
    if !off() {
        acquire(i);
    }
}

fn finish(i: usize) {
    if !off() {
        release(i);
    }
    ST.lock().unwrap().threads[i].alive = false;
}

/// The runtime kernel's view of the virtual cores.
struct Cores;

impl nxrt::kernel::Scheduler for Cores {
    fn thread_start(&self, handle: u32, _core: i32) {
        start(handle);
    }
    fn thread_exit(&self, _handle: u32) {
        if let Some(i) = ME.take() {
            finish(i);
        }
    }
    fn block_enter(&self) {
        if active() {
            release(ME.get().unwrap());
        }
    }
    fn block_leave(&self) {
        if active() {
            acquire(ME.get().unwrap());
        }
    }
    fn priority_changed(&self, handle: u32, prio: i32) {
        note_priority(handle, prio);
    }
    fn affinity_changed(&self, handle: u32, core: i32, mask: u64) {
        note_core_mask(handle, core, mask);
    }
    fn current_processor(&self) -> Option<u32> {
        current_core().map(|c| c as u32)
    }
    /// A pass of a marked spin-wait loop: give the core to anyone waiting for it (the runtime then lets the host
    /// run others).
    fn spin_pass(&self) {
        checkpoint(true);
    }
    fn host_enter(&self) {
        ME_HOST.with(|m| {
            if let Some((h, _)) = m.borrow().as_ref() {
                h.fetch_add(1, Ordering::SeqCst);
            }
        });
    }
    /// Back to guest code: a thread whose core was taken while it was in the host function waits for a core
    /// first (Horizon: it was preempted), so two threads never run guest code on one core.
    fn host_leave(&self) {
        let back = ME_HOST.with(|m| match m.borrow().as_ref() {
            Some((h, d)) => h.fetch_sub(1, Ordering::SeqCst) == 1 && d.load(Ordering::SeqCst),
            None => false,
        });
        if back && !off() {
            checkpoint(false);
        }
    }
    fn host_suspend(&self) -> bool {
        let inside = ME_HOST.with(|m| {
            m.borrow()
                .as_ref()
                .is_some_and(|(h, _)| h.load(Ordering::SeqCst) > 0)
        });
        if inside {
            self.host_leave();
        }
        inside
    }
}

/// SetThreadPriority / SetThreadCoreMask applied by the kernel: the scheduler's copy follows the kernel's.
pub fn note_priority(h: u32, prio: i32) {
    if let Some(i) = by_handle(h) {
        ST.lock().unwrap().threads[i].prio = prio;
        CV.notify_all();
    }
}

pub fn note_core_mask(h: u32, ideal: i32, mask: u64) {
    let Some(i) = by_handle(h) else { return };
    let mut st = ST.lock().unwrap();
    let all = (1u64 << st.ncores) - 1;
    let t = &mut st.threads[i];
    if ideal == -2 {
        t.ideal = 0;
        t.mask = 1;
    } else {
        if mask & all != 0 {
            t.mask = mask & all;
        }
        if ideal >= 0 && (ideal as usize) < MAX_CORES {
            t.ideal = ideal as usize;
        }
        if t.mask >> t.ideal & 1 == 0 {
            t.ideal = t.mask.trailing_zeros() as usize;
        }
    }
    drop(st);
    CV.notify_all();
}

pub fn stats() -> String {
    let st = ST.lock().unwrap();
    let held: Vec<String> = st.owner[..st.ncores]
        .iter()
        .map(|o| o.map_or("-".into(), |i| format!("{:#x}", st.threads[i].handle)))
        .collect();
    let waiting = st.threads.iter().filter(|t| t.waiting).count();
    format!(
        "cores [{}], {waiting} waiting, {} handoffs, {} steals ({} from an equal or better holder, {} from a holder in guest code)",
        held.join(" "),
        HANDOFFS.load(Ordering::Relaxed),
        STEALS.load(Ordering::Relaxed),
        STEALS_ANY.load(Ordering::Relaxed),
        STEALS_GUEST.load(Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_waiter_and_take() {
        let mut st = State {
            threads: vec![],
            owner: [None; MAX_CORES],
            ncores: 3,
            seq: 0,
        };
        let a = new_thread(&mut st, 44, 0, 0, 1);
        let b = new_thread(&mut st, 30, 0, 0, 2);
        let m = new_thread(&mut st, 40, 1, 0b111, 3);
        for (k, s) in [(a, 1), (b, 2), (m, 3)] {
            st.threads[k].waiting = true;
            st.threads[k].wseq = s;
        }
        assert_eq!(best_waiter(&st, 0), Some(b), "priority 30 first");
        assert!(!try_take(&mut st, a), "a better waiter wants core 0");
        assert!(try_take(&mut st, b));
        assert_eq!(st.owner[0], Some(b));
        assert!(try_take(&mut st, m), "core 1 is m's ideal");
        assert_eq!(st.threads[m].core, Some(1));
        assert!(
            !try_take(&mut st, a),
            "core 0 is held and a may run nowhere else"
        );
    }
}

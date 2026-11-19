//! Virtual cores: Horizon runs at most one thread per core, the best ready one, and games rely on it (a
//! higher-priority thread started on the caller's core runs before the caller goes on; per-core data is keyed by
//! the core number without locks). Guest threads run on free host threads, so each guest thread holds one of the
//! application's cores (three on Switch 1) while it runs guest code, as mkw-recomp's kernel does:
//!
//! * it takes a core when it starts (CreateThread's entry goes through `thread_entry`) and gives it back where
//!   Horizon switches threads: the blocking SVCs (waits, locks, condition variables, address waits, sleep), yields,
//!   ExitThread, and host functions that block (`without_core`);
//! * a free core goes to the best waiting thread that may run there (priority, then first come); a thread prefers
//!   its ideal core and migrates within its affinity mask;
//! * preemption at checkpoints: after every SVC, and at every pass of a marked spin-wait loop (`nxos_spin_gate`),
//!   a thread that holds a core a better thread waits for hands it over and queues again;
//! * a waiter that has waited 2 ms for a core held by a worse thread (20 ms for an equal or better one: a holder
//!   spinning in code with no checkpoint) takes the core; the holder gives up its claim at its next checkpoint;
//! * GetCurrentProcessorNumber answers the core the thread holds.
//!
//! `NXOS_FREE_CORES=1` turns the gate off (every thread runs whenever it is ready).

use nxrt::cpu::Cpu;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_CORES: usize = 4;
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
    entry: u64,
    arg: u64,
    alive: bool,
}

struct State {
    threads: Vec<Th>,
    owner: [Option<usize>; MAX_CORES],
    ncores: usize,
    seq: u64,
}

static ST: Mutex<State> = Mutex::new(State { threads: Vec::new(), owner: [None; MAX_CORES], ncores: 3, seq: 0 });
static CV: Condvar = Condvar::new();
static OFF: OnceLock<bool> = OnceLock::new();
pub static HANDOFFS: AtomicU64 = AtomicU64::new(0);
pub static STEALS: AtomicU64 = AtomicU64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static ME: Cell<Option<usize>> = const { Cell::new(None) };
}

fn off() -> bool {
    *OFF.get_or_init(|| std::env::var("NXOS_FREE_CORES").is_ok_and(|v| v == "1"))
}

/// Configures the core count from the process core mask (before any thread starts).
pub fn init(core_mask: u64) {
    let n = (core_mask.count_ones() as usize).clamp(1, MAX_CORES);
    ST.lock().unwrap().ncores = n;
    STARTED.store(true, Ordering::Relaxed);
    crate::log!(
        "scheduler: {n} virtual cores{}",
        if off() { " are OFF (NXOS_FREE_CORES=1): threads run freely" } else { ", the best ready thread on each; checkpoints at SVCs and spin waits" }
    );
}

fn new_thread(st: &mut State, prio: i32, ideal: i32, mask: u64, handle: u32, entry: u64, arg: u64) -> usize {
    let all = (1u64 << st.ncores) - 1;
    let ideal = if ideal < 0 || ideal as usize >= st.ncores { 0 } else { ideal as usize };
    let mask = if mask & all == 0 { 1 << ideal } else { mask & all };
    st.threads.push(Th { prio, ideal, mask, core: None, waiting: false, wseq: 0, displaced: false, handle, entry, arg, alive: true });
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
    let t0 = Instant::now();
    loop {
        if try_take(&mut st, i) {
            return;
        }
        let waited = t0.elapsed();
        if waited >= STEAL_WORSE {
            // a core the waiter may use, held by a thread worse than it (or any holder after STEAL_ANY)
            let me = st.threads[i].clone();
            let pick = (0..st.ncores).filter(|&c| me.mask >> c & 1 != 0).find(|&c| {
                st.owner[c].is_some_and(|o| st.threads[o].prio > me.prio || waited >= STEAL_ANY) && best_waiter(&st, c) == Some(i)
            });
            if let Some(c) = pick {
                if let Some(o) = st.owner[c] {
                    st.threads[o].displaced = true;
                }
                st.owner[c] = Some(i);
                let t = &mut st.threads[i];
                t.core = Some(c);
                t.waiting = false;
                t.displaced = false;
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
    match ME.get().filter(|_| !off()) {
        Some(i) => {
            release(i);
            let r = f();
            acquire(i);
            r
        }
        None => f(),
    }
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

/// The main thread joins the scheduler (on itself).
pub fn enter_main(handle: u32, prio: i32, core: i32) {
    let i = {
        let mut st = ST.lock().unwrap();
        new_thread(&mut st, prio, core, 0, handle, 0, 0)
    };
    ME.set(Some(i));
    if !off() {
        acquire(i);
    }
}

/// A new thread's first function: takes a core, then runs the guest entry with its argument.
unsafe fn thread_entry(c: *mut Cpu) {
    let i = (*c).x[0] as usize;
    ME.set(Some(i));
    let (entry, arg) = {
        let st = ST.lock().unwrap();
        (st.threads[i].entry, st.threads[i].arg)
    };
    if !off() {
        acquire(i);
    }
    (*c).x[0] = arg;
    let r = nxrt::boundary::call_guest(c, entry);
    finish(i);
    if r == nxrt::boundary::GuestCall::Returned {
        crate::log!("[RED] scheduler: a guest thread's entry returned (Horizon requires ExitThread)");
    }
}

fn finish(i: usize) {
    if !off() {
        release(i);
    }
    ST.lock().unwrap().threads[i].alive = false;
}

/// Hands an SVC to the runtime (threads, synchronization, exit, debug output) and passes its shim status on,
/// with the core given up around the blocking ones.
pub unsafe fn delegate(c: *mut Cpu, imm: u32, pc: u64) -> u32 {
    let me = ME.get().filter(|_| !off() && STARTED.load(Ordering::Relaxed));
    match (imm, me) {
        (0x0A, Some(i)) => {
            finish(i);
            nxrt::svc::nxrt_svc_impl(c, imm, pc)
        }
        (0x0B, Some(i)) => {
            // SleepThread: 0 / -1 / -2 are yields (to the same priority, with migration, to anyone)
            let ns = (*c).x[0] as i64;
            if ns <= 0 {
                checkpoint(ns == -2 || ns == -1);
                std::thread::yield_now();
                (*c).x[0] = 0;
                return nxrt::asm::ST_RETURN;
            }
            release(i);
            let r = nxrt::svc::nxrt_svc_impl(c, imm, pc);
            acquire(i);
            r
        }
        (0x18 | 0x1A | 0x1C | 0x34, Some(i)) => {
            release(i);
            let r = nxrt::svc::nxrt_svc_impl(c, imm, pc);
            acquire(i);
            r
        }
        _ => nxrt::svc::nxrt_svc_impl(c, imm, pc),
    }
}

/// CreateThread(-> w1 handle; x1 entry, x2 argument, x3 stack top, w4 priority, w5 core): the thread starts in
/// `thread_entry`, which takes a core first.
pub unsafe fn create_thread(c: *mut Cpu, pc: u64) -> u32 {
    let (entry, arg, prio, core) = ((*c).x[1], (*c).x[2], (*c).x[4] as i32, (*c).x[5] as i32);
    let core = if core == -2 { 0 } else { core };
    (*c).x[5] = core as u32 as u64;
    let i = {
        let mut st = ST.lock().unwrap();
        new_thread(&mut st, prio, core, 0, 0, entry, arg)
    };
    (*c).x[1] = crate::hle::addr("nxos thread entry", thread_entry);
    (*c).x[2] = i as u64;
    let r = nxrt::svc::nxrt_svc_impl(c, 0x08, pc);
    if (*c).x[0] == 0 {
        ST.lock().unwrap().threads[i].handle = (*c).x[1] as u32;
    } else {
        ST.lock().unwrap().threads[i].alive = false;
    }
    r
}

/// SetThreadPriority(w0 handle, w1 priority) / SetThreadCoreMask(w0 handle, w1 ideal core, x2 mask): the
/// scheduler's copy follows the kernel's.
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

/// One pass of a marked spin-wait loop: give the core to anyone waiting for it, else let the host run others.
#[no_mangle]
pub extern "C" fn nxos_spin_gate(_c: *mut Cpu, _head: u64) {
    nxrt::boundary::guard("nxos_spin_gate", || {
        checkpoint(true);
        std::thread::yield_now();
    })
}

pub fn stats() -> String {
    let st = ST.lock().unwrap();
    let held: Vec<String> = st.owner[..st.ncores].iter().map(|o| o.map_or("-".into(), |i| format!("{:#x}", st.threads[i].handle))).collect();
    let waiting = st.threads.iter().filter(|t| t.waiting).count();
    format!("cores [{}], {waiting} waiting, {} handoffs, {} steals", held.join(" "), HANDOFFS.load(Ordering::Relaxed), STEALS.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_waiter_and_take() {
        let mut st = State { threads: vec![], owner: [None; MAX_CORES], ncores: 3, seq: 0 };
        let a = new_thread(&mut st, 44, 0, 0, 1, 0, 0);
        let b = new_thread(&mut st, 30, 0, 0, 2, 0, 0);
        let m = new_thread(&mut st, 40, 1, 0b111, 3, 0, 0);
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
        assert!(!try_take(&mut st, a), "core 0 is held and a may run nowhere else");
    }
}

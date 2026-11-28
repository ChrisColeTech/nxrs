//! The minimal Horizon kernel: threads (one host thread per guest thread, running on the guest stack the caller
//! gives), locks, condition variables, address arbitration, events and WaitSynchronization.
//!
//! Semantics are the public Horizon kernel's (KConditionVariable / KAddressArbiter / KSynchronization):
//!   Threads    0x08 CreateThread 0x09 StartThread 0x0A ExitThread 0x0B SleepThread 0x0C/0x0D priority
//!              0x0E/0x0F core mask 0x10 GetCurrentProcessorNumber 0x25 GetThreadId 0x19 CancelSynchronization
//!   Locks      0x1A ArbitrateLock 0x1B ArbitrateUnlock (tag word = owner handle | 0x40000000 has waiters)
//!   Condition  0x1C WaitProcessWideKeyAtomic 0x1D SignalProcessWideKey
//!   Address    0x34 WaitForAddress 0x35 SignalToAddress
//!   Events     0x45 CreateEvent 0x11 SignalEvent 0x12 ClearEvent 0x17 ResetSignal 0x18 WaitSynchronization
//!              0x16 CloseHandle
//! All waits hang on one kernel lock with one condition variable per thread; a waiter is picked in kernel order
//! (numerically lower priority first, then first come). Correctness first; scheduling (virtual cores, preemption)
//! comes with the games.

use crate::cpu::Cpu;
use crate::mem::Space;
use std::cell::Cell;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

pub const R_OK: u64 = 0;
pub const R_TIMEOUT: u64 = 0xEA01;
pub const R_CANCELLED: u64 = 0xEC01;
pub const R_OUT_OF_RANGE: u64 = 0xEE01;
pub const R_BAD_HANDLE: u64 = 0xE401;
pub const R_BAD_STATE: u64 = 0xFA01;
pub const R_INVALID_ENUM: u64 = 0xF001;
pub const WAITERS_BIT: u32 = 0x4000_0000;
pub const H_CUR_THREAD: u32 = 0xFFFF_8000;
const H_FIRST: u32 = 0x1000;
const MAX_SYNC: usize = 64;
/// Host stack of a guest thread: lifted code runs on it, so guest recursion depth costs host stack. On the web it
/// is the linear-memory shadow stack, which holds only locals whose address is taken (the calls themselves go on
/// the engine's own stack), and every thread's comes out of the one 16 GiB memory beside the guest window: 4 MiB.
/// Chrome on Windows commits all of it; Anime Girls' 24 guest threads used at most ~24 KiB each of the 32 MiB they
/// had (768 MiB committed, under 1 MiB touched; WEB-WMEM).
pub const HOST_STACK: usize = if cfg!(target_os = "emscripten") { 4 << 20 } else { 256 << 20 };

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Obj {
    Free,
    Thread(usize),
    EventR(usize),
    EventW(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SyncRef {
    Thread(usize),
    Event(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wait {
    None,
    Lock,
    CondVar,
    Address,
    Sync,
}

struct Event {
    signaled: bool,
    refs: u32,
}

struct Thread {
    handle: u32,
    id: u64,
    entry: u64,
    arg: u64,
    stack_top: u64,
    prio: i32,
    core: i32,
    mask: u64,
    started: bool,
    exited: bool,
    closed: bool,
    cancel: bool,
    refs: u32,
    wkind: Wait,
    woke: bool,
    waddr: u64,
    wkey: u64,
    wtag: u32,
    wres: u64,
    wseq: u64,
    wobjs: Vec<SyncRef>,
    widx: Option<usize>,
    /// Who ended the current wait (`waitacct`: a guest handle, a host thread, or `BY_NONE`), and whether a
    /// condition variable's signaller set it (the lock's later handoff then does not overwrite it).
    wby: u32,
    wsig: bool,
    /// When the current wait times out (None: no time limit), for the thread dump.
    wuntil: Option<Instant>,
    cv: Arc<Condvar>,
    tls: u64,
    cpu: *mut Cpu,
    /// Its host thread still runs (from spawn until `thread_main` returns): the object, its `Cpu` and its TLS
    /// region stay allocated until then, whatever the guest does with the handle.
    host_running: bool,
}

unsafe impl Send for Thread {}

#[derive(Default)]
struct State {
    handles: Vec<Obj>,
    threads: Vec<Option<Thread>>,
    events: Vec<Option<Event>>,
    next_id: u64,
    wseq: u64,
}

pub struct Kernel {
    g: Mutex<State>,
    pub space: &'static Space,
}

static KERNEL: OnceLock<Kernel> = OnceLock::new();

/// What a scheduler (the os layer's virtual cores) is told and asked. Every method has a do-nothing default, so a
/// scheduler implements what it needs. None is called with the kernel lock held.
pub trait Scheduler: Sync {
    /// A guest thread starts running on this host thread (before its entry), with its ideal core.
    fn thread_start(&self, _handle: u32, _core: i32) {}
    /// A guest thread is done (after ExitThread or its entry returned), on its own host thread.
    fn thread_exit(&self, _handle: u32) {}
    /// The calling guest thread is about to block in an SVC (sleep, a lock, a condition variable, an address wait,
    /// WaitSynchronization): its core can go to another thread.
    fn block_enter(&self) {}
    /// It is back from the blocking SVC and wants its core again (may wait for one).
    fn block_leave(&self) {}
    /// SetThreadPriority was applied to a thread.
    fn priority_changed(&self, _handle: u32, _prio: i32) {}
    /// SetThreadCoreMask was applied to a thread.
    fn affinity_changed(&self, _handle: u32, _core: i32, _mask: u64) {}
    /// GetCurrentProcessorNumber of the calling thread; None: the thread's ideal core.
    fn current_processor(&self) -> Option<u32> {
        None
    }
    /// A pass of a marked spin-wait loop (`rt_spin_gate`, before it yields the host core): a checkpoint.
    fn spin_pass(&self) {}
    /// The calling guest thread enters / leaves a host function (`nxrt_host_invoke`): while it is in one, another
    /// thread taking its core runs no guest code beside guest code of that core.
    fn host_enter(&self) {}
    fn host_leave(&self) {}
    /// `host_leave` when the thread is inside a host function (a guest call from it); whether it was.
    fn host_suspend(&self) -> bool {
        false
    }
}

struct NoScheduler;
impl Scheduler for NoScheduler {}

static SCHEDULER: OnceLock<&'static dyn Scheduler> = OnceLock::new();

/// Installs the scheduler (once, before guest threads start).
pub fn set_scheduler(s: &'static dyn Scheduler) {
    if SCHEDULER.set(s).is_err() {
        crate::rt::fatal("kernel: a scheduler is installed twice");
    }
}

pub(crate) fn sched() -> &'static dyn Scheduler {
    SCHEDULER.get().copied().unwrap_or(&NoScheduler)
}

thread_local! {
    /// The guest thread running on this host thread (its slot), if any.
    static CUR: Cell<Option<usize>> = const { Cell::new(None) };
    /// This thread's own guest state (lock-free, for the crash reporter).
    static CPU: Cell<*mut Cpu> = const { Cell::new(core::ptr::null_mut()) };
}

/// The calling host thread's guest state, if it runs a guest thread.
pub fn current_cpu() -> Option<*mut Cpu> {
    let c = CPU.get();
    (!c.is_null()).then_some(c)
}

pub fn get() -> &'static Kernel {
    KERNEL
        .get()
        .unwrap_or_else(|| crate::rt::fatal("kernel used before process start"))
}

pub fn init(space: &'static Space) -> &'static Kernel {
    if KERNEL
        .set(Kernel {
            g: Mutex::new(State {
                next_id: 0x51,
                ..Default::default()
            }),
            space,
        })
        .is_err()
    {
        crate::rt::fatal("kernel initialised twice");
    }
    crate::process::startup(space);
    get()
}

/// The kernel, once a process started.
pub fn try_get() -> Option<&'static Kernel> {
    KERNEL.get()
}

fn ld32(a: u64) -> u32 {
    unsafe {
        (*(a as *const core::sync::atomic::AtomicU32)).load(core::sync::atomic::Ordering::SeqCst)
    }
}
fn ld64(a: u64) -> u64 {
    unsafe {
        (*(a as *const core::sync::atomic::AtomicU64)).load(core::sync::atomic::Ordering::SeqCst)
    }
}
fn st32(a: u64, v: u32) {
    unsafe {
        (*(a as *const core::sync::atomic::AtomicU32))
            .store(v, core::sync::atomic::Ordering::SeqCst)
    }
}
fn cas32(a: u64, exp: &mut u32, v: u32) -> bool {
    use core::sync::atomic::{AtomicU32, Ordering::SeqCst};
    match unsafe { (*(a as *const AtomicU32)).compare_exchange(*exp, v, SeqCst, SeqCst) } {
        Ok(_) => true,
        Err(cur) => {
            *exp = cur;
            false
        }
    }
}

impl State {
    fn h_new(&mut self, o: Obj) -> u32 {
        if let Some(i) = self.handles.iter().position(|h| *h == Obj::Free) {
            self.handles[i] = o;
            return H_FIRST + 1 + i as u32;
        }
        self.handles.push(o);
        H_FIRST + self.handles.len() as u32
    }
    fn h_get(&self, h: u32) -> Obj {
        let i = h.wrapping_sub(H_FIRST + 1) as usize;
        self.handles.get(i).copied().unwrap_or(Obj::Free)
    }
    fn h_free(&mut self, h: u32) {
        let i = h.wrapping_sub(H_FIRST + 1) as usize;
        if let Some(s) = self.handles.get_mut(i) {
            *s = Obj::Free;
        }
    }
    fn t(&mut self, i: usize) -> &mut Thread {
        self.threads[i].as_mut().unwrap()
    }
    /// The thread a handle names; the pseudo-handle is the caller.
    fn thread_of(&self, h: u32) -> Option<usize> {
        if h == H_CUR_THREAD {
            return CUR.get();
        }
        match self.h_get(h) {
            Obj::Thread(i) => Some(i),
            _ => None,
        }
    }

    fn pick(&self, kind: Wait, a: u64) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, t) in self.threads.iter().enumerate() {
            let Some(t) = t else { continue };
            if t.wkind != kind
                || (if kind == Wait::CondVar {
                    t.wkey
                } else {
                    t.waddr
                }) != a
            {
                continue;
            }
            let better = match best {
                None => true,
                Some(b) => {
                    let b = self.threads[b].as_ref().unwrap();
                    t.prio < b.prio || (t.prio == b.prio && t.wseq < b.wseq)
                }
            };
            if better {
                best = Some(i);
            }
        }
        best
    }

    /// The calling thread as the one that ends a wait (`waitacct`; only when it is on).
    fn waker(&self) -> u32 {
        match CUR.get().and_then(|c| self.threads.get(c)?.as_ref()) {
            Some(t) => t.handle,
            None => crate::waitacct::waker(),
        }
    }

    fn wake(&mut self, i: usize, res: u64) {
        if crate::waitacct::on() && !self.t(i).wsig {
            let by = self.waker();
            self.t(i).wby = by;
        }
        let t = self.t(i);
        t.wkind = Wait::None;
        t.woke = true;
        t.wres = res;
        t.cv.notify_one();
    }

    /// Releases the lock at addr: to the first waiter in kernel order, or to nobody (zero).
    fn lock_handoff(&mut self, addr: u64) {
        let Some(w) = self.pick(Wait::Lock, addr) else {
            st32(addr, 0);
            return;
        };
        let more = self.threads.iter().enumerate().any(|(i, t)| {
            i != w
                && t.as_ref()
                    .is_some_and(|t| t.wkind == Wait::Lock && t.waddr == addr)
        });
        let tag = self.t(w).wtag;
        st32(addr, tag | if more { WAITERS_BIT } else { 0 });
        self.wake(w, 0);
    }

    /// Whether the owner a lock word names cannot release it: no owner, an
    /// exited thread, or a thread itself waiting for this lock (or on a condition variable over it). The pseudo-handle
    /// and anything that is not a thread are left alone: never steal from a real owner.
    fn owner_is_stale(&self, v: u32, addr: u64) -> bool {
        let o = v & !WAITERS_BIT;
        if o == 0 {
            return true;
        }
        if o == H_CUR_THREAD {
            return false;
        }
        let Obj::Thread(i) = self.h_get(o) else {
            return false;
        };
        let Some(t) = self.threads[i].as_ref() else {
            return false;
        };
        t.exited || (matches!(t.wkind, Wait::Lock | Wait::CondVar) && t.waddr == addr)
    }

    fn lock_waiters(&self, addr: u64, except: Option<usize>) -> bool {
        self.threads.iter().enumerate().any(|(i, t)| {
            Some(i) != except
                && t.as_ref()
                    .is_some_and(|t| t.wkind == Wait::Lock && t.waddr == addr)
        })
    }

    /// The sweep for lock waiters stuck behind a stale owner or whose waiters bit was cleared (then the owner unlocks
    /// in user mode without calling the kernel), run each frame. The stale
    /// lock goes to its first waiter in kernel order; a lost bit is set again. What it repaired: (address, what, the
    /// word it found).
    fn heal_locks(&mut self) -> Vec<(u64, &'static str, u32)> {
        let mut notes = vec![];
        for _ in 0..64 {
            let mut changed = false;
            for i in 0..self.threads.len() {
                let Some(t) = self.threads[i].as_ref() else {
                    continue;
                };
                if t.wkind != Wait::Lock {
                    continue;
                }
                let a = t.waddr;
                let mut v = ld32(a);
                if self.owner_is_stale(v, a) {
                    let Some(n) = self.pick(Wait::Lock, a) else {
                        continue;
                    };
                    let nv = self.t(n).wtag
                        | if self.lock_waiters(a, Some(n)) {
                            WAITERS_BIT
                        } else {
                            0
                        };
                    let found = v;
                    if cas32(a, &mut v, nv) {
                        self.wake(n, 0);
                        notes.push((a, "stale owner", found));
                        changed = true;
                        break;
                    }
                } else if v & WAITERS_BIT == 0 {
                    let found = v;
                    if cas32(a, &mut v, found | WAITERS_BIT) {
                        notes.push((a, "lost waiter flag", found));
                    }
                }
            }
            if !changed {
                break;
            }
        }
        notes
    }

    /// Makes waiter i take the lock at addr: free -> it gets it and wakes; else it becomes a lock waiter.
    fn lock_acquire_for(&mut self, i: usize, addr: u64, tag: u32) {
        let mut v = ld32(addr);
        loop {
            if v == 0 {
                if cas32(addr, &mut v, tag) {
                    self.wake(i, 0);
                    return;
                }
                continue;
            }
            let nv = v | WAITERS_BIT;
            if cas32(addr, &mut v, nv) {
                break;
            }
        }
        self.wseq += 1;
        let s = self.wseq;
        let t = self.t(i);
        t.wkind = Wait::Lock;
        t.waddr = addr;
        t.wtag = tag;
        t.wseq = s;
    }

    fn signaled(&self, r: SyncRef) -> bool {
        match r {
            SyncRef::Thread(i) => self.threads[i].as_ref().is_some_and(|t| t.exited),
            SyncRef::Event(e) => self.events[e].as_ref().is_some_and(|e| e.signaled),
        }
    }

    fn wake_sync_waiters(&mut self, obj: SyncRef) {
        for i in 0..self.threads.len() {
            let hit = match &self.threads[i] {
                Some(t) if t.wkind == Wait::Sync => t.wobjs.iter().position(|o| *o == obj),
                _ => None,
            };
            if let Some(k) = hit {
                self.t(i).widx = Some(k);
                self.wake(i, 0);
            }
        }
    }

    fn sync_retain(&mut self, r: SyncRef) {
        match r {
            SyncRef::Thread(i) => self.t(i).refs += 1,
            SyncRef::Event(e) => self.events[e].as_mut().unwrap().refs += 1,
        }
    }
    fn sync_release(&mut self, r: SyncRef) {
        match r {
            SyncRef::Thread(i) => {
                self.t(i).refs -= 1;
                release(self.thread_maybe_free(i));
            }
            SyncRef::Event(e) => self.event_put(e),
        }
    }
    fn event_put(&mut self, e: usize) {
        let ev = self.events[e].as_mut().unwrap();
        ev.refs -= 1;
        if ev.refs == 0 {
            self.events[e] = None;
        }
    }
    /// Frees thread i once nothing can reach it: its handle closed, the guest thread exited, no wait holds it, and
    /// its host thread finished (the exit path still runs on its stack and `Cpu` after ExitThread).
    fn thread_maybe_free(&mut self, i: usize) -> Option<(u64, *mut Cpu)> {
        let t = self.t(i);
        if t.closed && t.exited && t.refs == 0 && !t.host_running {
            let r = (t.tls, t.cpu);
            self.threads[i] = None;
            return Some(r);
        }
        None
    }

    /// The guest thread i ended (ExitThread, or its host thread ending): waiters on it are released. Idempotent.
    fn mark_exited(&mut self, i: usize) {
        if !self.t(i).exited {
            self.t(i).exited = true;
            self.wake_sync_waiters(SyncRef::Thread(i));
        }
    }
}

/// A repaired lock: the first 20 logged, then every 1024th.
fn heal_note(addr: u64, what: &str, v: u32) {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 20 || n % 1024 == 0 {
        crate::rt::log(&format!(
            "kernel: mutex {addr:#x} repaired ({what}, value {v:#x}) [{n} times]"
        ));
    }
}

impl Kernel {
    /// The stuck-mutex healer: the host calls it each frame. How many locks it repaired.
    pub fn heal_mutexes(&self) -> usize {
        let notes = lock(&self.g).heal_locks();
        for (a, what, v) in &notes {
            heal_note(*a, what, *v);
        }
        notes.len()
    }
}

/// Releases what `thread_maybe_free` gave back.
fn release(freed: Option<(u64, *mut Cpu)>) {
    if let Some((tls, cpu)) = freed {
        get().space.free_tls(tls);
        // SAFETY: leaked from a Box at creation; its host thread has finished, nothing else holds it.
        unsafe { drop(Box::from_raw(cpu)) };
    }
}

/// The kernel lock. It never poisons: a panic anywhere in the runtime ends the process (`boundary::guard`, which
/// every entry point and every runtime thread runs under), so no later caller can see a half-done update.
fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Blocks thread i (holding the lock) until woken or timed out (ns < 0: forever, 0: immediate timeout).
fn block<'a>(
    mut g: MutexGuard<'a, State>,
    i: usize,
    kind: Wait,
    ns: i64,
) -> (MutexGuard<'a, State>, u64) {
    g.wseq += 1;
    let s = g.wseq;
    let t = g.t(i);
    t.wkind = kind;
    t.woke = false;
    t.wseq = s;
    t.wby = crate::waitacct::BY_NONE;
    t.wsig = false;
    if ns == 0 {
        t.wkind = Wait::None;
        return (g, R_TIMEOUT);
    }
    let cv = t.cv.clone();
    let deadline = (ns > 0).then(|| Instant::now() + Duration::from_nanos(ns as u64));
    t.wuntil = deadline;
    while !g.t(i).woke {
        match deadline {
            None => g = cv.wait(g).unwrap_or_else(PoisonError::into_inner),
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    let t = g.t(i);
                    t.wkind = Wait::None;
                    return (g, R_TIMEOUT);
                }
                g = cv
                    .wait_timeout(g, d - now)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
        }
    }
    let r = g.t(i).wres;
    (g, r)
}

/// `waitacct`: thread i's wait (`kind` on `key`, since `t0`) ended; who ended it is the thread's `wby`.
fn acct(g: &mut State, i: usize, kind: u8, key: u64, t0: Option<Instant>) {
    if let Some(t0) = t0 {
        let by = g.t(i).wby;
        crate::waitacct::add(kind, key, t0.elapsed().as_nanos() as u64, by);
    }
}

/// The start of a wait when the accounting is on.
fn acct_t0() -> Option<Instant> {
    crate::waitacct::on().then(Instant::now)
}

fn cur(svc: u32) -> usize {
    CUR.get().unwrap_or_else(|| {
        crate::rt::fatal(&format!(
            "kernel: svc {svc:#x} on a host thread (no guest thread)"
        ))
    })
}

/// The calling host thread's guest thread handle, if it is one.
/// Every guest thread's state for a stall report (os's `threads` control command): handle, priority, core, what it
/// waits for, and its guest frames (x30, then the x29 chain) as of its last SVC.
pub fn dump_threads() -> String {
    let Some(k) = KERNEL.get() else {
        return "no kernel\n".into();
    };
    let g = lock(&k.g);
    let mut out = String::new();
    for t in g.threads.iter().flatten() {
        if t.exited {
            continue;
        }
        let w = match t.wkind {
            Wait::None => "running or sleeping".to_string(),
            // the lock word holds the owner's handle (bit 30: waiters)
            Wait::Lock if crate::mem::readable(t.waddr, 4) => {
                format!("lock {:#x} (word {:#x})", t.waddr, unsafe {
                    *(t.waddr as *const u32)
                })
            }
            Wait::Lock => format!("lock {:#x}", t.waddr),
            Wait::CondVar => format!("condvar key {:#x} (lock {:#x})", t.wkey, t.waddr),
            Wait::Address => format!("address {:#x}", t.waddr),
            Wait::Sync => format!("sync on {} objects", t.wobjs.len()),
        };
        let until = match (t.wkind, t.wuntil) {
            (Wait::None, _) => String::new(),
            (_, None) => " (no time limit)".into(),
            (_, Some(d)) => format!(
                " (time limit in {} ms)",
                d.saturating_duration_since(Instant::now()).as_millis()
            ),
        };
        out.push_str(&format!(
            "thread {:#x} prio {} core {} entry {:#x}: {w}{until}",
            t.handle, t.prio, t.core, t.entry
        ));
        if !t.cpu.is_null() {
            let c = unsafe { &*t.cpu };
            out.push_str(&format!("; pc {:#x}", c.pc));
            if let Some(n) = crate::hostfn::name_of(c.pc) {
                out.push_str(&format!(" ({n})"));
            }
            out.push_str(&format!(" lr {:#x} frames", c.x[30]));
            let mut fp = c.x[29];
            // NXRS_DUMP_FRAMES=<ret>[,<ret>..]: the 16 words at the frame record of a frame returning there (its
            // saved registers: a stall's loop counters)
            let peek: Vec<u64> = std::env::var("NXRS_DUMP_FRAMES")
                .map(|v| {
                    v.split(',')
                        .filter_map(|s| {
                            u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut words = String::new();
            for _ in 0..12 {
                if fp == 0 || fp & 7 != 0 || !crate::mem::readable(fp, 16) {
                    break;
                }
                let ret = unsafe { *((fp + 8) as *const u64) };
                out.push_str(&format!(" {ret:#x}"));
                if peek.contains(&ret) && crate::mem::readable(fp, 128) {
                    words.push_str(&format!("; at {ret:#x} fp {fp:#x}:"));
                    for i in 0..16u64 {
                        words.push_str(&format!(" {:x}", unsafe { *((fp + i * 8) as *const u64) }));
                    }
                }
                fp = unsafe { *(fp as *const u64) };
            }
            out.push_str(&words);
        }
        out.push('\n');
    }
    out
}

pub fn current_handle() -> Option<u32> {
    let i = CUR.get()?;
    lock(&get().g).threads[i].as_ref().map(|t| t.handle)
}

impl Kernel {
    /// A new event as svcCreateEvent makes it: (write handle, read handle). Host code signals it with
    /// [`Kernel::signal_event`]; the guest waits on the read handle.
    /// The process's live guest threads (GetResourceLimitCurrentValue's thread count).
    pub fn thread_count(&self) -> usize {
        lock(&self.g).threads.iter().filter(|t| t.is_some()).count()
    }

    pub fn create_event(&self) -> (u32, u32) {
        let mut g = lock(&self.g);
        let e = g
            .events
            .iter()
            .position(|e| e.is_none())
            .unwrap_or_else(|| {
                g.events.push(None);
                g.events.len() - 1
            });
        g.events[e] = Some(Event {
            signaled: false,
            refs: 2,
        });
        (g.h_new(Obj::EventW(e)), g.h_new(Obj::EventR(e)))
    }

    /// Signals an event (either handle): its waiters wake. R_OK or R_BAD_HANDLE.
    pub fn signal_event(&self, h: u32) -> u64 {
        let mut g = lock(&self.g);
        match g.h_get(h) {
            Obj::EventW(e) | Obj::EventR(e) => {
                g.events[e].as_mut().unwrap().signaled = true;
                g.wake_sync_waiters(SyncRef::Event(e));
                R_OK
            }
            _ => R_BAD_HANDLE,
        }
    }

    /// Clears an event (either handle). R_OK or R_BAD_HANDLE.
    pub fn clear_event(&self, h: u32) -> u64 {
        let mut g = lock(&self.g);
        match g.h_get(h) {
            Obj::EventW(e) | Obj::EventR(e) => {
                g.events[e].as_mut().unwrap().signaled = false;
                R_OK
            }
            _ => R_BAD_HANDLE,
        }
    }

    /// Whether an event (either handle) is signaled; None for another handle.
    pub fn event_signaled(&self, h: u32) -> Option<bool> {
        let g = lock(&self.g);
        match g.h_get(h) {
            Obj::EventW(e) | Obj::EventR(e) => Some(g.events[e].as_ref().unwrap().signaled),
            _ => None,
        }
    }

    /// Closes a handle (svcCloseHandle).
    pub fn close_handle(&self, h: u32) {
        let mut c = Cpu::new_boxed();
        c.x[0] = h as u64;
        svc_inner(&mut c, 0x16);
    }

    /// Creates a guest thread (not started); returns its handle.
    pub fn create_thread(&self, entry: u64, arg: u64, stack_top: u64, prio: i32, core: i32) -> u32 {
        let tls = self.space.alloc_tls();
        let cpu = Box::into_raw(Cpu::new_boxed());
        let mut g = lock(&self.g);
        let id = g.next_id;
        g.next_id += 1;
        let slot = g
            .threads
            .iter()
            .position(|t| t.is_none())
            .unwrap_or_else(|| {
                g.threads.push(None);
                g.threads.len() - 1
            });
        let core = if core >= 0 { core } else { 0 };
        let handle = g.h_new(Obj::Thread(slot));
        g.threads[slot] = Some(Thread {
            handle,
            id,
            entry,
            arg,
            stack_top: stack_top & !0xF,
            prio,
            core,
            mask: 1 << core,
            started: false,
            exited: false,
            closed: false,
            cancel: false,
            refs: 0,
            wkind: Wait::None,
            woke: false,
            waddr: 0,
            wkey: 0,
            wtag: 0,
            wuntil: None,
            wres: 0,
            wseq: 0,
            wobjs: vec![],
            widx: None,
            wby: 0,
            wsig: false,
            cv: Arc::new(Condvar::new()),
            tls,
            cpu,
            host_running: true,
        });
        drop(g);
        let r = std::thread::Builder::new()
            .name(format!("guest {handle:#x}"))
            .stack_size(HOST_STACK)
            .spawn(move || thread_main(slot));
        if let Err(e) = r {
            crate::rt::fatal(&format!(
                "kernel: host thread for guest thread {handle:#x}: {e}"
            ));
        }
        if crate::rt::trace("thread") {
            crate::rt::log(&format!("CreateThread {handle:#x} entry {} arg {arg:#x} sp {stack_top:#x} prio {prio} core {core}", crate::dispatch::guest_name(entry)));
        }
        handle
    }

    pub fn start_thread(&self, h: u32) -> u64 {
        let mut g = lock(&self.g);
        match g.h_get(h) {
            Obj::Thread(i) => {
                let t = g.t(i);
                t.started = true;
                t.cv.notify_one();
                R_OK
            }
            _ => R_BAD_HANDLE,
        }
    }

    /// The calling thread ends: its waiters are released (the host stack then unwinds to the thread's base,
    /// `boundary::exit_jump`).
    pub fn exit_current(&self) {
        lock(&self.g).mark_exited(cur(0x0A));
    }

    /// The end of thread i's host thread (the last thing `thread_main` does with it).
    fn host_finished(&self, i: usize) {
        let mut g = lock(&self.g);
        g.mark_exited(i);
        g.t(i).host_running = false;
        let freed = g.thread_maybe_free(i);
        drop(g);
        release(freed);
    }

    /// The main thread: created and started at once; returns (handle, its Cpu).
    pub fn create_main(&self, entry: u64, stack_top: u64, prio: i32, core: i32) -> u32 {
        let h = self.create_thread(entry, 0, stack_top, prio, core);
        let mut g = lock(&self.g);
        let Obj::Thread(i) = g.h_get(h) else {
            unreachable!()
        };
        let t = g.t(i);
        t.arg = 0;
        // Horizon's main thread gets x0 = 0 and x1 = its own handle; thread_main sets x1 for the main thread.
        t.started = true;
        t.cv.notify_one();
        h
    }
}

fn thread_main(slot: usize) {
    crate::boundary::guard("a guest thread's host thread", || thread_body(slot));
}

fn thread_body(slot: usize) {
    CUR.set(Some(slot));
    let k = get();
    let (cpu, entry, arg, sp, tls, handle, main);
    {
        let mut g = lock(&k.g);
        loop {
            let t = g.t(slot);
            if t.started || t.closed {
                break;
            }
            let cv = t.cv.clone();
            g = cv.wait(g).unwrap_or_else(PoisonError::into_inner);
        }
        let t = g.t(slot);
        if !t.started {
            drop(g);
            k.host_finished(slot);
            return;
        }
        (cpu, entry, arg, sp, tls, handle) = (t.cpu, t.entry, t.arg, t.stack_top, t.tls, t.handle);
        main = t.id == 0x51;
        // The thread's ID at ThreadLocalRegion+0x110: SDK 22's InternalCriticalSection compares a mutex's owner
        // word with the 32-bit ID it reads there (mrs tpidrro_el0; ldr w8, [x8, #0x110]). Left 0, it equals an
        // unlocked mutex's owner, every mutex reads as held by the caller and the SDK's allocator aborts at its
        // first lock (Ocarina of Time, SDK 22.2.8).
        // SAFETY: the thread's own TLS region, 0x200 bytes, committed.
        unsafe { ((tls + 0x110) as *mut u32).write_unaligned(t.id as u32) };
    }
    let core = lock(&k.g).t(slot).core;
    crate::waitacct::thread_start(handle, entry);
    sched().thread_start(handle, core);
    crate::profile::add_current(handle, cpu);
    CPU.set(cpu);
    // SAFETY: this thread owns its Cpu until it exits.
    unsafe {
        let c = &mut *cpu;
        c.x[0] = arg;
        if main {
            c.x[1] = handle as u64;
        }
        c.sp = sp;
        c.tpidrro = tls;
        c.thread = slot as *mut _;
        match crate::boundary::call_guest(c, entry) {
            crate::boundary::GuestCall::Returned => {
                if main {
                    crate::process::exit(0);
                }
                crate::rt::log(&format!("kernel: guest thread {handle:#x}'s entry returned (Horizon requires ExitThread)"));
            }
            crate::boundary::GuestCall::Exit => {}
            crate::boundary::GuestCall::Escaped => {
                crate::rt::fatal("kernel: a guest jump left a thread's entry function")
            }
        }
        crate::boundary::take_pending();
    }
    CPU.set(core::ptr::null_mut());
    crate::profile::remove(handle);
    sched().thread_exit(handle);
    k.host_finished(slot);
}

/// The kernel's SVCs. Returns false for a number this file does not handle. The blocking ones (sleep, locks,
/// condition variables, address waits, WaitSynchronization) are bracketed by the scheduler's `block_enter` /
/// `block_leave`.
pub fn svc(c: &mut Cpu, imm: u32) -> bool {
    let blocking = matches!(imm, 0x0B | 0x18 | 0x1A | 0x1C | 0x34);
    if !blocking {
        return svc_inner(c, imm);
    }
    sched().block_enter();
    let t0 = std::time::Instant::now();
    crate::profile::mark(match imm {
        0x0B => crate::profile::W_SLEEP,
        0x18 => crate::profile::W_SYNC,
        0x1A => crate::profile::W_MUTEX,
        0x1C => crate::profile::W_CONDVAR,
        _ => crate::profile::W_ADDRESS,
    });
    let r = svc_inner(c, imm);
    crate::profile::mark(crate::profile::RUN);
    let us = t0.elapsed().as_micros() as u64;
    sched().block_leave();
    // metrics (waits by kind): how many waits and how long, per second in metrics-*.csv
    let (n, t) = wait_counters(imm);
    nxconfig::metrics::add(n, 1);
    nxconfig::metrics::add(t, us);
    r
}

/// The metrics counters of a blocking SVC's kind: (waits, microseconds waited).
fn wait_counters(
    imm: u32,
) -> (
    &'static std::sync::atomic::AtomicU64,
    &'static std::sync::atomic::AtomicU64,
) {
    use nxconfig::metrics::counter;
    type C2 = (
        &'static std::sync::atomic::AtomicU64,
        &'static std::sync::atomic::AtomicU64,
    );
    static C: OnceLock<[C2; 5]> = OnceLock::new();
    let c = C.get_or_init(|| {
        [
            (counter("wait_sleep"), counter("wait_sleep_us")),
            (counter("wait_sync"), counter("wait_sync_us")),
            (counter("wait_mutex"), counter("wait_mutex_us")),
            (counter("wait_condvar"), counter("wait_condvar_us")),
            (counter("wait_address"), counter("wait_address_us")),
        ]
    });
    c[match imm {
        0x0B => 0,
        0x18 => 1,
        0x1A => 2,
        0x1C => 3,
        _ => 4,
    }]
}

/// `NXRS_SLEEP_SPIN_US` (default 1000; 0 = the host's sleep alone): the last part of a SleepThread that is waited by
/// yielding instead of the host's timer.
fn sleep_spin() -> Duration {
    static V: OnceLock<Duration> = OnceLock::new();
    *V.get_or_init(|| {
        let us = std::env::var("NXRS_SLEEP_SPIN_US")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(1000);
        Duration::from_micros(us)
    })
}

/// SleepThread: as long as asked. Horizon wakes a sleeper on its tick (microseconds); the host's sleep wakes on its
/// timer, ~0.6 ms past the time asked on this machine (Windows' high-resolution waitable timer). Ocarina of Time's job
/// threads and its main thread back off with 20 us sleeps ~3 times a frame each and slept ~0.58 ms each (OOT-WAIT): a
/// host sleep for all but the last `sleep_spin()` of it, then yields until the time is up.
pub(crate) fn sleep_thread(ns: u64) {
    let spin = sleep_spin();
    let want = Duration::from_nanos(ns);
    if spin.is_zero() {
        std::thread::sleep(want);
        return;
    }
    let until = Instant::now() + want;
    if want > spin {
        std::thread::sleep(want - spin);
    }
    while Instant::now() < until {
        std::thread::yield_now();
    }
}

fn svc_inner(c: &mut Cpu, imm: u32) -> bool {
    let k = get();
    match imm {
        0x08 => {
            let h = k.create_thread(c.x[1], c.x[2], c.x[3], c.x[4] as i32, c.x[5] as i32);
            c.x[0] = R_OK;
            c.x[1] = h as u64;
        }
        0x09 => c.x[0] = k.start_thread(c.x[0] as u32),
        0x0B => {
            let ns = c.x[0] as i64;
            if ns > 0 {
                let t0 = acct_t0();
                sleep_thread(ns as u64);
                if let Some(t0) = t0 {
                    // keyed by the time asked for: what the host's sleep makes of it
                    crate::waitacct::add(
                        crate::waitacct::K_SLEEP,
                        ns as u64,
                        t0.elapsed().as_nanos() as u64,
                        crate::waitacct::BY_NONE,
                    );
                }
            } else {
                std::thread::yield_now();
            }
        }
        0x0C => {
            let g = lock(&k.g);
            let t = g.thread_of(c.x[1] as u32);
            c.x[0] = if t.is_some() { R_OK } else { R_BAD_HANDLE };
            c.x[1] = t.map_or(0, |i| g.threads[i].as_ref().unwrap().prio as u64);
        }
        0x0D => {
            let mut g = lock(&k.g);
            match g.thread_of(c.x[0] as u32) {
                Some(i) => {
                    g.t(i).prio = c.x[1] as i32;
                    let h = g.t(i).handle;
                    drop(g);
                    sched().priority_changed(h, c.x[1] as i32);
                    c.x[0] = R_OK;
                }
                None => c.x[0] = R_BAD_HANDLE,
            }
        }
        0x0E => {
            let g = lock(&k.g);
            let t = g
                .thread_of(c.x[2] as u32)
                .map(|i| g.threads[i].as_ref().unwrap());
            c.x[0] = if t.is_some() { R_OK } else { R_BAD_HANDLE };
            c.x[1] = t.map_or(0, |t| t.core as u64);
            c.x[2] = t.map_or(0, |t| t.mask);
        }
        0x0F => {
            let mut g = lock(&k.g);
            match g.thread_of(c.x[0] as u32) {
                Some(i) => {
                    let (ideal, mask) = (c.x[1] as i32, c.x[2]);
                    let t = g.t(i);
                    if ideal >= 0 {
                        t.core = ideal;
                    } else if ideal == -2 {
                        t.core = 0;
                    }
                    t.mask = if ideal == -2 {
                        1
                    } else if mask != 0 {
                        mask
                    } else {
                        t.mask
                    };
                    let (h, core, mask) = (t.handle, t.core, t.mask);
                    drop(g);
                    sched().affinity_changed(h, core, mask);
                    c.x[0] = R_OK;
                }
                None => c.x[0] = R_BAD_HANDLE,
            }
        }
        0x10 => {
            if let Some(n) = sched().current_processor() {
                c.x[0] = n as u64;
            } else {
                let g = lock(&k.g);
                c.x[0] = CUR
                    .get()
                    .map_or(0, |i| g.threads[i].as_ref().unwrap().core as u64);
            }
        }
        0x25 => {
            let g = lock(&k.g);
            let t = g.thread_of(c.x[1] as u32);
            c.x[0] = if t.is_some() { R_OK } else { R_BAD_HANDLE };
            c.x[1] = t.map_or(0, |i| g.threads[i].as_ref().unwrap().id);
        }
        0x1A => {
            // ArbitrateLock(w0 owner handle, x1 address, w2 this thread's tag)
            let i = cur(imm);
            let (addr, tag) = (c.x[1], c.x[2] as u32);
            let mut g = lock(&k.g);
            let mut r = R_OK;
            let mut v = ld32(addr);
            if v == (c.x[0] as u32 | WAITERS_BIT) && g.owner_is_stale(v, addr) {
                // already released: hand it to the caller, the waiters bit kept if anyone
                // else is queued; the caller rereads the word
                let nv = tag
                    | if g.lock_waiters(addr, None) {
                        WAITERS_BIT
                    } else {
                        0
                    };
                let found = v;
                if cas32(addr, &mut v, nv) {
                    heal_note(addr, "on request", found);
                }
            } else if v == (c.x[0] as u32 | WAITERS_BIT) {
                let t = g.t(i);
                t.waddr = addr;
                t.wtag = tag;
                let t0 = acct_t0();
                (g, r) = block(g, i, Wait::Lock, -1);
                acct(&mut g, i, crate::waitacct::K_MUTEX, addr, t0);
            }
            drop(g);
            c.x[0] = r;
        }
        0x1B => {
            lock(&k.g).lock_handoff(c.x[0]);
            c.x[0] = R_OK;
        }
        0x1C => {
            // WaitProcessWideKeyAtomic(x0 lock address, x1 condition key, w2 tag, x3 timeout). The key is set
            // before the lock is released: a signaller that reads key 0 in user mode skips svc 0x1D.
            let i = cur(imm);
            let (addr, key, tag, ns) = (c.x[0], c.x[1], c.x[2] as u32, c.x[3] as i64);
            let mut g = lock(&k.g);
            st32(key, 1);
            {
                let t = g.t(i);
                t.wkey = key;
                t.waddr = addr;
                t.wtag = tag;
            }
            g.lock_handoff(addr);
            let t0 = acct_t0();
            let (mut g, r) = block(g, i, Wait::CondVar, ns);
            acct(&mut g, i, crate::waitacct::K_CONDVAR, key, t0);
            drop(g);
            c.x[0] = r;
        }
        0x1D => {
            // SignalProcessWideKey(x0 key, w1 count; <= 0: all). Each woken waiter retakes its lock.
            let (key, cnt) = (c.x[0], c.x[1] as i32);
            let mut g = lock(&k.g);
            let mut n = 0;
            while cnt <= 0 || n < cnt {
                let Some(w) = g.pick(Wait::CondVar, key) else {
                    break;
                };
                let (a, tag) = (g.t(w).waddr, g.t(w).wtag);
                if crate::waitacct::on() {
                    // the signaller ended this wait, not the lock's later handoff
                    let by = g.waker();
                    g.t(w).wby = by;
                    g.t(w).wsig = true;
                }
                g.lock_acquire_for(w, a, tag);
                n += 1;
            }
            let left = g.threads.iter().any(|t| {
                t.as_ref()
                    .is_some_and(|t| t.wkind == Wait::CondVar && t.wkey == key)
            });
            if !left {
                st32(key, 0);
            }
            c.x[0] = R_OK;
        }
        0x34 => {
            // WaitForAddress(x0 address, w1 type, x2 value, x3 timeout): 0 wait if less than, 1 decrement if less
            // than then wait, 2 wait if equal, 3 wait if the 64-bit value is equal
            let i = cur(imm);
            let (addr, ty, val, ns) = (c.x[0], c.x[1] as u32, c.x[2] as i32, c.x[3] as i64);
            let mut g = lock(&k.g);
            let mut v = ld32(addr);
            let mut ok = match ty {
                3 => ld64(addr) == c.x[2],
                2 => v as i32 == val,
                _ => (v as i32) < val,
            };
            if ok && ty == 1 {
                while !{
                    let nv = v.wrapping_sub(1);
                    cas32(addr, &mut v, nv)
                } {
                    if (v as i32) >= val {
                        ok = false;
                        break;
                    }
                }
            }
            let r = if !ok {
                R_BAD_STATE
            } else {
                g.t(i).waddr = addr;
                let r;
                let t0 = acct_t0();
                (g, r) = block(g, i, Wait::Address, ns);
                acct(&mut g, i, crate::waitacct::K_ADDRESS, addr, t0);
                r
            };
            drop(g);
            c.x[0] = r;
        }
        0x35 => {
            // SignalToAddress(x0 address, w1 type, w2 value, w3 count): 0 wake; 1 increment if equal and wake; 2 if
            // equal, modify by the waiter count and wake
            let (addr, ty, val, cnt) = (c.x[0], c.x[1] as u32, c.x[2] as i32, c.x[3] as i32);
            let mut g = lock(&k.g);
            let nw = g
                .threads
                .iter()
                .filter(|t| {
                    t.as_ref()
                        .is_some_and(|t| t.wkind == Wait::Address && t.waddr == addr)
                })
                .count() as i32;
            let mut r = R_OK;
            if ty != 0 {
                let mut nv = val.wrapping_add(1);
                if ty == 2 {
                    nv = if cnt <= 0 {
                        if nw > 0 {
                            val - 2
                        } else {
                            val + 1
                        }
                    } else if nw == 0 {
                        val + 1
                    } else if nw <= cnt {
                        val - 1
                    } else {
                        val
                    };
                }
                let mut e = val as u32;
                if !cas32(addr, &mut e, nv as u32) {
                    r = R_BAD_STATE;
                }
            }
            if r == R_OK {
                let mut n = 0;
                while cnt <= 0 || n < cnt {
                    let Some(w) = g.pick(Wait::Address, addr) else {
                        break;
                    };
                    g.wake(w, 0);
                    n += 1;
                }
            }
            c.x[0] = r;
        }
        0x45 => {
            let (w, r) = k.create_event();
            c.x[0] = R_OK;
            c.x[1] = w as u64;
            c.x[2] = r as u64;
        }
        0x11 | 0x12 | 0x17 => {
            // SignalEvent(write end) / ClearEvent(either end) / ResetSignal(read end, error if not signaled)
            let mut g = lock(&k.g);
            let e = match (imm, g.h_get(c.x[0] as u32)) {
                (0x11, Obj::EventW(e))
                | (0x17, Obj::EventR(e))
                | (0x12, Obj::EventW(e) | Obj::EventR(e)) => Some(e),
                _ => None,
            };
            c.x[0] = match e {
                None => R_BAD_HANDLE,
                Some(e) if imm == 0x11 => {
                    g.events[e].as_mut().unwrap().signaled = true;
                    g.wake_sync_waiters(SyncRef::Event(e));
                    R_OK
                }
                Some(e) if imm == 0x17 && !g.events[e].as_ref().unwrap().signaled => R_BAD_STATE,
                Some(e) => {
                    g.events[e].as_mut().unwrap().signaled = false;
                    R_OK
                }
            };
        }
        0x18 => {
            // WaitSynchronization(-> w1 index; x1 handle array, w2 count, x3 timeout)
            let i = cur(imm);
            let (n, ns) = (c.x[2] as usize, c.x[3] as i64);
            if n > MAX_SYNC {
                c.x[0] = R_OUT_OF_RANGE;
                c.x[1] = 0;
                return true;
            }
            let hs: Vec<u32> = (0..n)
                .map(|k| unsafe { *((c.x[1] + 4 * k as u64) as *const u32) })
                .collect();
            let mut g = lock(&k.g);
            let mut objs = vec![];
            for &h in &hs {
                match g.h_get(h) {
                    Obj::Thread(t) => objs.push(SyncRef::Thread(t)),
                    Obj::EventR(e) => objs.push(SyncRef::Event(e)),
                    _ => break,
                }
            }
            let (mut r, mut idx) = (R_BAD_HANDLE, None);
            if objs.len() == n {
                for &o in &objs {
                    g.sync_retain(o);
                }
                idx = objs.iter().position(|&o| g.signaled(o));
                if idx.is_some() {
                    r = R_OK;
                } else if ns == 0 {
                    r = R_TIMEOUT;
                } else if g.t(i).cancel {
                    g.t(i).cancel = false;
                    r = R_CANCELLED;
                } else {
                    g.t(i).wobjs = objs.clone();
                    g.t(i).widx = None;
                    let t0 = acct_t0();
                    (g, r) = block(g, i, Wait::Sync, ns);
                    idx = g.t(i).widx;
                    // the object that ended the wait (the first, on a timeout)
                    let h = hs.get(idx.unwrap_or(0)).copied().unwrap_or(0);
                    acct(&mut g, i, crate::waitacct::K_SYNC, h as u64, t0);
                    g.t(i).wobjs.clear();
                }
                for &o in &objs {
                    g.sync_release(o);
                }
            }
            c.x[0] = r;
            c.x[1] = idx.unwrap_or(0) as u64;
        }
        0x19 => {
            let mut g = lock(&k.g);
            c.x[0] = match g.thread_of(c.x[0] as u32) {
                Some(t) => {
                    if g.t(t).wkind == Wait::Sync {
                        g.wake(t, R_CANCELLED);
                    } else {
                        g.t(t).cancel = true;
                    }
                    R_OK
                }
                None => R_BAD_HANDLE,
            };
        }
        0x16 => {
            let h = c.x[0] as u32;
            let mut g = lock(&k.g);
            match g.h_get(h) {
                Obj::EventR(e) | Obj::EventW(e) => {
                    g.h_free(h);
                    g.event_put(e);
                }
                Obj::Thread(t) => {
                    g.h_free(h);
                    g.t(t).closed = true;
                    g.t(t).cv.notify_one();
                    release(g.thread_maybe_free(t));
                }
                Obj::Free => {}
            }
            c.x[0] = R_OK;
        }
        // SetThreadActivity(w0 handle, w1 pause / run): accepted, the thread keeps running. Unity's
        // garbage collector pauses every thread around GetThreadContext3.
        0x32 => {
            let g = lock(&k.g);
            c.x[0] = if g.thread_of(c.x[0] as u32).is_some() {
                R_OK
            } else {
                R_BAD_HANDLE
            };
        }
        // GetThreadContext3(x0 ThreadContext, w1 handle): the thread's registers as its `Cpu` holds them (x0-x28,
        // fp, lr, sp, pc, pstate, v0-v31, fpcr, fpsr, tpidr; 0x320 bytes)
        0x33 => {
            let g = lock(&k.g);
            let cpu = g
                .thread_of(c.x[1] as u32)
                .map(|i| g.threads[i].as_ref().unwrap().cpu)
                .filter(|p| !p.is_null());
            c.x[0] = match cpu {
                Some(p) => {
                    let mut b = [0u8; 0x320];
                    // SAFETY: the thread's Cpu stays allocated while the kernel lock is held and the thread object lives
                    let t = unsafe { &*p };
                    for i in 0..31 {
                        b[8 * i..8 * i + 8].copy_from_slice(&t.x[i].to_le_bytes());
                    }
                    b[0xf8..0x100].copy_from_slice(&t.sp.to_le_bytes());
                    b[0x100..0x108].copy_from_slice(&t.pc.to_le_bytes());
                    b[0x108..0x10c].copy_from_slice(&(t.nzcv as u32).to_le_bytes());
                    for i in 0..32 {
                        b[0x110 + 16 * i..0x120 + 16 * i].copy_from_slice(&t.v[i].to_le_bytes());
                    }
                    b[0x310..0x314].copy_from_slice(&(t.fpcr as u32).to_le_bytes());
                    b[0x314..0x318].copy_from_slice(&(t.fpsr as u32).to_le_bytes());
                    b[0x318..0x320].copy_from_slice(&t.tpidr.to_le_bytes());
                    // SAFETY: the guest's output buffer
                    unsafe {
                        core::ptr::copy_nonoverlapping(b.as_ptr(), c.x[0] as *mut u8, b.len())
                    };
                    R_OK
                }
                None => R_BAD_HANDLE,
            };
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_threads(n: usize) -> State {
        let mut s = State {
            next_id: 1,
            ..Default::default()
        };
        for p in 0..n {
            let h = s.h_new(Obj::Thread(p));
            s.threads.push(Some(Thread {
                handle: h,
                id: p as u64,
                entry: 0,
                arg: 0,
                stack_top: 0,
                prio: 44,
                core: 0,
                mask: 1,
                started: true,
                exited: false,
                closed: false,
                cancel: false,
                refs: 0,
                wkind: Wait::None,
                woke: false,
                waddr: 0,
                wkey: 0,
                wtag: 0,
                wuntil: None,
                wres: 0,
                wseq: 0,
                wobjs: vec![],
                widx: None,
                wby: 0,
                wsig: false,
                cv: Arc::new(Condvar::new()),
                tls: 0,
                cpu: core::ptr::null_mut(),
                host_running: true,
            }));
        }
        s
    }

    fn wait_on(s: &mut State, i: usize, kind: Wait, a: u64, prio: i32, tag: u32) {
        s.wseq += 1;
        let q = s.wseq;
        let t = s.t(i);
        t.wkind = kind;
        t.woke = false;
        t.prio = prio;
        t.wtag = tag;
        t.wseq = q;
        if kind == Wait::CondVar {
            t.wkey = a;
        } else {
            t.waddr = a;
        }
    }

    /// ExitThread, WaitSynchronization + CloseHandle by another thread, then the host thread's end, in either order:
    /// the object is freed exactly once, only after both the handle is closed and the host thread is done, and a
    /// second "exited" is harmless. (The race behind a 1-in-5 `threads` failure: the object was freed at
    /// CloseHandle while its host thread still ran the exit path, which then unwrapped a freed or reused slot.)
    #[test]
    fn thread_object_outlives_its_host_thread() {
        for close_first in [true, false] {
            let mut s = state_with_threads(2);
            s.t(1).wkind = Wait::Sync;
            s.t(1).wobjs = vec![SyncRef::Thread(0)];
            s.mark_exited(0); // ExitThread
            assert!(s.t(1).woke, "the joiner is released at ExitThread");
            let close = |s: &mut State| {
                s.t(0).closed = true;
                s.thread_maybe_free(0)
            };
            let host_end = |s: &mut State| {
                s.mark_exited(0);
                s.t(0).host_running = false;
                s.thread_maybe_free(0)
            };
            let (first, second) = if close_first {
                (close(&mut s), host_end(&mut s))
            } else {
                (host_end(&mut s), close(&mut s))
            };
            assert!(
                first.is_none(),
                "freed too early (close_first {close_first})"
            );
            assert!(second.is_some() && s.threads[0].is_none());
            assert!(s.threads[1].is_some(), "the other thread is untouched");
        }
    }

    /// OOT-WAIT: SleepThread sleeps as long as asked, not the host timer's ~0.6 ms: 20 us sleeps (Ocarina of Time's
    /// job threads' backoff) average well under 0.3 ms (the host's sleep alone: ~0.58 ms each), and a longer one is
    /// never short.
    #[test]
    fn sleep_thread_sleeps_as_long_as_asked() {
        let t0 = Instant::now();
        for _ in 0..20 {
            sleep_thread(20_000);
        }
        let each = t0.elapsed() / 20;
        assert!(each >= Duration::from_micros(20) && each < Duration::from_micros(300), "20 us sleeps took {each:?} each");
        let t0 = Instant::now();
        sleep_thread(3_000_000);
        let e = t0.elapsed();
        assert!(e >= Duration::from_millis(3) && e < Duration::from_millis(5), "a 3 ms sleep took {e:?}");
    }

    #[test]
    fn handles() {
        let mut s = state_with_threads(2);
        assert_eq!(s.threads[0].as_ref().unwrap().handle, 0x1001);
        assert_eq!(s.h_get(0x1002), Obj::Thread(1));
        s.h_free(0x1001);
        assert_eq!(s.h_get(0x1001), Obj::Free);
        assert_eq!(s.h_new(Obj::EventR(0)), 0x1001, "a freed handle is reused");
        assert_eq!(s.h_get(0x5000), Obj::Free);
    }

    #[test]
    fn lock_handoff_in_kernel_order() {
        let mut s = state_with_threads(3);
        let word = Box::new(0u32);
        let a = &*word as *const u32 as u64;
        st32(a, 0x1001 | WAITERS_BIT); // held by 0x1001 with waiters
        wait_on(&mut s, 1, Wait::Lock, a, 44, 0x1002);
        wait_on(&mut s, 2, Wait::Lock, a, 30, 0x1003); // better priority, later
        s.lock_handoff(a);
        assert_eq!(
            ld32(a),
            0x1003 | WAITERS_BIT,
            "priority 30 first; 0x1002 still waits"
        );
        assert!(s.t(2).woke && s.t(2).wkind == Wait::None);
        s.lock_handoff(a);
        assert_eq!(ld32(a), 0x1002, "last waiter: no waiters bit");
        s.lock_handoff(a);
        assert_eq!(ld32(a), 0, "free");
    }

    /// The stuck-mutex healer: a lock held by an exited thread or by one of its
    /// own waiters goes to the best waiter; a lost waiters bit is set again; a live owner is never robbed.
    #[test]
    fn stuck_mutexes_are_healed() {
        let mut s = state_with_threads(5); // handles 0x1001..0x1005
        let words = Box::new([0u32; 3]);
        let (a, b, c) = (
            &words[0] as *const u32 as u64,
            &words[1] as *const u32 as u64,
            &words[2] as *const u32 as u64,
        );
        // a: its owner 0x1001 exited; waiters 0x1002 (priority 44) and 0x1003 (priority 30)
        st32(a, 0x1001 | WAITERS_BIT);
        s.t(0).exited = true;
        wait_on(&mut s, 1, Wait::Lock, a, 44, 0x1002);
        wait_on(&mut s, 2, Wait::Lock, a, 30, 0x1003);
        // b: held by 0x1004 (alive, running), waiter 0x1005, the waiters bit lost
        st32(b, 0x1004);
        wait_on(&mut s, 4, Wait::Lock, b, 44, 0x1005);
        // c: a live owner with no kernel waiter: not visited
        st32(c, 0x1004 | WAITERS_BIT);
        let notes = s.heal_locks();
        assert_eq!(
            notes,
            vec![
                (a, "stale owner", 0x1001 | WAITERS_BIT),
                (b, "lost waiter flag", 0x1004)
            ]
        );
        assert_eq!(
            ld32(a),
            0x1003 | WAITERS_BIT,
            "priority 30 takes it, 0x1002 still waits"
        );
        assert!(s.t(2).woke && s.t(2).wkind == Wait::None && s.t(1).wkind == Wait::Lock);
        assert_eq!(
            ld32(b),
            0x1004 | WAITERS_BIT,
            "the lost bit set again; the live owner keeps the lock"
        );
        assert!(s.t(4).wkind == Wait::Lock);
        assert_eq!(ld32(c), 0x1004 | WAITERS_BIT);
        assert!(s.heal_locks().is_empty(), "nothing left to repair");
        // an owner waiting on the very lock it holds is stale; the pseudo-handle and non-threads are left alone
        assert!(s.owner_is_stale(0x1005 | WAITERS_BIT, b) && s.owner_is_stale(WAITERS_BIT, b));
        assert!(
            !s.owner_is_stale(H_CUR_THREAD | WAITERS_BIT, b)
                && !s.owner_is_stale(0x5000, b)
                && !s.owner_is_stale(0x1003, a)
        );
    }

    #[test]
    fn condvar_signal_retakes_the_lock() {
        let mut s = state_with_threads(3);
        let words = Box::new([0u32; 2]);
        let (lock, key) = (
            &words[0] as *const u32 as u64,
            &words[1] as *const u32 as u64,
        );
        st32(key, 1);
        for i in [1usize, 2] {
            wait_on(&mut s, i, Wait::CondVar, key, 44, 0x1001 + i as u32);
            s.t(i).waddr = lock;
        }
        st32(lock, 0x1001); // the signaller holds the lock
        let w = s.pick(Wait::CondVar, key).unwrap();
        assert_eq!(w, 1, "first come at equal priority");
        let (a, tag) = (s.t(w).waddr, s.t(w).wtag);
        s.lock_acquire_for(w, a, tag);
        assert_eq!(
            s.t(1).wkind,
            Wait::Lock,
            "the lock is taken: it now waits for it"
        );
        assert_eq!(ld32(lock), 0x1001 | WAITERS_BIT);
        s.lock_handoff(lock); // the signaller unlocks
        assert_eq!(ld32(lock), 0x1002);
        assert!(s.t(1).woke);
        let w = s.pick(Wait::CondVar, key).unwrap();
        st32(lock, 0);
        let (a, tag) = (s.t(w).waddr, s.t(w).wtag);
        s.lock_acquire_for(w, a, tag);
        assert_eq!(
            ld32(lock),
            0x1003,
            "a free lock goes to the woken waiter at once"
        );
        assert!(s.t(2).woke);
    }

    #[test]
    fn sync_waiters_and_events() {
        let mut s = state_with_threads(2);
        s.events.push(Some(Event {
            signaled: false,
            refs: 2,
        }));
        s.t(1).wkind = Wait::Sync;
        s.t(1).wobjs = vec![SyncRef::Thread(0), SyncRef::Event(0)];
        assert!(!s.signaled(SyncRef::Event(0)));
        s.events[0].as_mut().unwrap().signaled = true;
        s.wake_sync_waiters(SyncRef::Event(0));
        assert!(s.t(1).woke);
        assert_eq!(s.t(1).widx, Some(1));
        s.event_put(0);
        s.event_put(0);
        assert!(s.events[0].is_none(), "both ends closed");
    }

    #[test]
    fn block_times_out_and_wakes() {
        let m = Arc::new(Mutex::new(state_with_threads(1)));
        let g = m.lock().unwrap();
        let t0 = Instant::now();
        let (g, r) = block(g, 0, Wait::Address, 5_000_000);
        assert_eq!(r, R_TIMEOUT);
        assert!(t0.elapsed() >= Duration::from_millis(5));
        assert_eq!(g.threads[0].as_ref().unwrap().wkind, Wait::None);
        let (g, r) = block(g, 0, Wait::Address, 0);
        assert_eq!(r, R_TIMEOUT);
        drop(g);
        let m2 = m.clone();
        let waker = std::thread::spawn(move || loop {
            let mut g = m2.lock().unwrap();
            if g.t(0).wkind == Wait::Address {
                g.wake(0, 7);
                break;
            }
            drop(g);
            std::thread::yield_now();
        });
        let g = m.lock().unwrap();
        let (_g, r) = block(g, 0, Wait::Address, -1);
        assert_eq!(r, 7);
        waker.join().unwrap();
    }
}

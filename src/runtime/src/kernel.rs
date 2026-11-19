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
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
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
/// Host stack of a guest thread: lifted code runs on it, so guest recursion depth costs host stack.
pub const HOST_STACK: usize = 256 << 20;

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
    cv: Arc<Condvar>,
    tls: u64,
    cpu: *mut Cpu,
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
    KERNEL.get().unwrap_or_else(|| crate::rt::fatal("kernel used before process start"))
}

pub fn init(space: &'static Space) -> &'static Kernel {
    if KERNEL.set(Kernel { g: Mutex::new(State { next_id: 0x51, ..Default::default() }), space }).is_err() {
        crate::rt::fatal("kernel initialised twice");
    }
    get()
}

fn ld32(a: u64) -> u32 {
    unsafe { (*(a as *const core::sync::atomic::AtomicU32)).load(core::sync::atomic::Ordering::SeqCst) }
}
fn ld64(a: u64) -> u64 {
    unsafe { (*(a as *const core::sync::atomic::AtomicU64)).load(core::sync::atomic::Ordering::SeqCst) }
}
fn st32(a: u64, v: u32) {
    unsafe { (*(a as *const core::sync::atomic::AtomicU32)).store(v, core::sync::atomic::Ordering::SeqCst) }
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
            if t.wkind != kind || (if kind == Wait::CondVar { t.wkey } else { t.waddr }) != a {
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

    fn wake(&mut self, i: usize, res: u64) {
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
            i != w && t.as_ref().is_some_and(|t| t.wkind == Wait::Lock && t.waddr == addr)
        });
        let tag = self.t(w).wtag;
        st32(addr, tag | if more { WAITERS_BIT } else { 0 });
        self.wake(w, 0);
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
                self.thread_maybe_free(i);
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
    fn thread_maybe_free(&mut self, i: usize) {
        let t = self.t(i);
        if t.closed && t.exited && t.refs == 0 {
            let (tls, cpu) = (t.tls, t.cpu);
            self.threads[i] = None;
            get().space.free_tls(tls);
            // SAFETY: the Cpu was leaked from a Box at creation and no host thread uses it any more.
            unsafe { drop(Box::from_raw(cpu)) };
        }
    }
}

/// Blocks thread i (holding the lock) until woken or timed out (ns < 0: forever, 0: immediate timeout).
fn block<'a>(mut g: MutexGuard<'a, State>, i: usize, kind: Wait, ns: i64) -> (MutexGuard<'a, State>, u64) {
    g.wseq += 1;
    let s = g.wseq;
    let t = g.t(i);
    t.wkind = kind;
    t.woke = false;
    t.wseq = s;
    if ns == 0 {
        t.wkind = Wait::None;
        return (g, R_TIMEOUT);
    }
    let cv = t.cv.clone();
    let deadline = (ns > 0).then(|| Instant::now() + Duration::from_nanos(ns as u64));
    while !g.t(i).woke {
        match deadline {
            None => g = cv.wait(g).unwrap(),
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    let t = g.t(i);
                    t.wkind = Wait::None;
                    return (g, R_TIMEOUT);
                }
                g = cv.wait_timeout(g, d - now).unwrap().0;
            }
        }
    }
    let r = g.t(i).wres;
    (g, r)
}

fn cur(svc: u32) -> usize {
    CUR.get().unwrap_or_else(|| crate::rt::fatal(&format!("kernel: svc {svc:#x} on a host thread (no guest thread)")))
}

/// The calling host thread's guest thread handle, if it is one.
pub fn current_handle() -> Option<u32> {
    let i = CUR.get()?;
    get().g.lock().unwrap().threads[i].as_ref().map(|t| t.handle)
}

impl Kernel {
    /// Creates a guest thread (not started); returns its handle.
    pub fn create_thread(&self, entry: u64, arg: u64, stack_top: u64, prio: i32, core: i32) -> u32 {
        let tls = self.space.alloc_tls();
        let cpu = Box::into_raw(Cpu::new_boxed());
        let mut g = self.g.lock().unwrap();
        let id = g.next_id;
        g.next_id += 1;
        let slot = g.threads.iter().position(|t| t.is_none()).unwrap_or_else(|| {
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
            wres: 0,
            wseq: 0,
            wobjs: vec![],
            widx: None,
            cv: Arc::new(Condvar::new()),
            tls,
            cpu,
        });
        drop(g);
        let r = std::thread::Builder::new()
            .name(format!("guest {handle:#x}"))
            .stack_size(HOST_STACK)
            .spawn(move || thread_main(slot));
        if let Err(e) = r {
            crate::rt::fatal(&format!("kernel: host thread for guest thread {handle:#x}: {e}"));
        }
        if crate::rt::trace("thread") {
            crate::rt::log(&format!("CreateThread {handle:#x} entry {} arg {arg:#x} sp {stack_top:#x} prio {prio} core {core}", crate::dispatch::guest_name(entry)));
        }
        handle
    }

    pub fn start_thread(&self, h: u32) -> u64 {
        let mut g = self.g.lock().unwrap();
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
        self.finish(cur(0x0A));
    }

    fn finish(&self, i: usize) {
        let mut g = self.g.lock().unwrap();
        if g.t(i).exited {
            return;
        }
        g.t(i).exited = true;
        g.wake_sync_waiters(SyncRef::Thread(i));
        g.thread_maybe_free(i);
    }

    /// The main thread: created and started at once; returns (handle, its Cpu).
    pub fn create_main(&self, entry: u64, stack_top: u64, prio: i32, core: i32) -> u32 {
        let h = self.create_thread(entry, 0, stack_top, prio, core);
        let mut g = self.g.lock().unwrap();
        let Obj::Thread(i) = g.h_get(h) else { unreachable!() };
        let t = g.t(i);
        t.arg = 0;
        // Horizon's main thread gets x0 = 0 and x1 = its own handle; thread_main sets x1 for the main thread.
        t.started = true;
        t.cv.notify_one();
        h
    }
}

fn thread_main(slot: usize) {
    CUR.set(Some(slot));
    let k = get();
    let (cpu, entry, arg, sp, tls, handle, main);
    {
        let mut g = k.g.lock().unwrap();
        loop {
            let t = g.t(slot);
            if t.started || t.closed {
                break;
            }
            let cv = t.cv.clone();
            g = cv.wait(g).unwrap();
        }
        let t = g.t(slot);
        if !t.started {
            drop(g);
            k.finish(slot);
            return;
        }
        (cpu, entry, arg, sp, tls, handle) = (t.cpu, t.entry, t.arg, t.stack_top, t.tls, t.handle);
        main = t.id == 0x51;
    }
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
            crate::boundary::GuestCall::Escaped => crate::rt::fatal("kernel: a guest jump left a thread's entry function"),
        }
        crate::boundary::take_pending();
    }
    k.finish(slot);
}

/// The kernel's SVCs. Returns false for a number this file does not handle.
pub fn svc(c: &mut Cpu, imm: u32) -> bool {
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
                std::thread::sleep(Duration::from_nanos(ns as u64));
            } else {
                std::thread::yield_now();
            }
        }
        0x0C => {
            let g = k.g.lock().unwrap();
            let t = g.thread_of(c.x[1] as u32);
            c.x[0] = if t.is_some() { R_OK } else { R_BAD_HANDLE };
            c.x[1] = t.map_or(0, |i| g.threads[i].as_ref().unwrap().prio as u64);
        }
        0x0D => {
            let mut g = k.g.lock().unwrap();
            match g.thread_of(c.x[0] as u32) {
                Some(i) => {
                    g.t(i).prio = c.x[1] as i32;
                    c.x[0] = R_OK;
                }
                None => c.x[0] = R_BAD_HANDLE,
            }
        }
        0x0E => {
            let g = k.g.lock().unwrap();
            let t = g.thread_of(c.x[2] as u32).map(|i| g.threads[i].as_ref().unwrap());
            c.x[0] = if t.is_some() { R_OK } else { R_BAD_HANDLE };
            c.x[1] = t.map_or(0, |t| t.core as u64);
            c.x[2] = t.map_or(0, |t| t.mask);
        }
        0x0F => {
            let mut g = k.g.lock().unwrap();
            match g.thread_of(c.x[0] as u32) {
                Some(i) => {
                    let (ideal, mask) = (c.x[1] as i32, c.x[2]);
                    let t = g.t(i);
                    if ideal >= 0 {
                        t.core = ideal;
                    } else if ideal == -2 {
                        t.core = 0;
                    }
                    t.mask = if ideal == -2 { 1 } else if mask != 0 { mask } else { t.mask };
                    c.x[0] = R_OK;
                }
                None => c.x[0] = R_BAD_HANDLE,
            }
        }
        0x10 => {
            let g = k.g.lock().unwrap();
            c.x[0] = CUR.get().map_or(0, |i| g.threads[i].as_ref().unwrap().core as u64);
        }
        0x25 => {
            let g = k.g.lock().unwrap();
            let t = g.thread_of(c.x[1] as u32);
            c.x[0] = if t.is_some() { R_OK } else { R_BAD_HANDLE };
            c.x[1] = t.map_or(0, |i| g.threads[i].as_ref().unwrap().id);
        }
        0x1A => {
            // ArbitrateLock(w0 owner handle, x1 address, w2 this thread's tag)
            let i = cur(imm);
            let (addr, tag) = (c.x[1], c.x[2] as u32);
            let mut g = k.g.lock().unwrap();
            let mut r = R_OK;
            if ld32(addr) == (c.x[0] as u32 | WAITERS_BIT) {
                let t = g.t(i);
                t.waddr = addr;
                t.wtag = tag;
                (g, r) = block(g, i, Wait::Lock, -1);
            }
            drop(g);
            c.x[0] = r;
        }
        0x1B => {
            k.g.lock().unwrap().lock_handoff(c.x[0]);
            c.x[0] = R_OK;
        }
        0x1C => {
            // WaitProcessWideKeyAtomic(x0 lock address, x1 condition key, w2 tag, x3 timeout). The key is set
            // before the lock is released: a signaller that reads key 0 in user mode skips svc 0x1D.
            let i = cur(imm);
            let (addr, key, tag, ns) = (c.x[0], c.x[1], c.x[2] as u32, c.x[3] as i64);
            let mut g = k.g.lock().unwrap();
            st32(key, 1);
            {
                let t = g.t(i);
                t.wkey = key;
                t.waddr = addr;
                t.wtag = tag;
            }
            g.lock_handoff(addr);
            let (g, r) = block(g, i, Wait::CondVar, ns);
            drop(g);
            c.x[0] = r;
        }
        0x1D => {
            // SignalProcessWideKey(x0 key, w1 count; <= 0: all). Each woken waiter retakes its lock.
            let (key, cnt) = (c.x[0], c.x[1] as i32);
            let mut g = k.g.lock().unwrap();
            let mut n = 0;
            while cnt <= 0 || n < cnt {
                let Some(w) = g.pick(Wait::CondVar, key) else { break };
                let (a, tag) = (g.t(w).waddr, g.t(w).wtag);
                g.lock_acquire_for(w, a, tag);
                n += 1;
            }
            let left = g.threads.iter().any(|t| t.as_ref().is_some_and(|t| t.wkind == Wait::CondVar && t.wkey == key));
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
            let mut g = k.g.lock().unwrap();
            let mut v = ld32(addr);
            let mut ok = match ty {
                3 => ld64(addr) == c.x[2],
                2 => v as i32 == val,
                _ => (v as i32) < val,
            };
            if ok && ty == 1 {
                while !{ let nv = v.wrapping_sub(1); cas32(addr, &mut v, nv) } {
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
                (g, r) = block(g, i, Wait::Address, ns);
                r
            };
            drop(g);
            c.x[0] = r;
        }
        0x35 => {
            // SignalToAddress(x0 address, w1 type, w2 value, w3 count): 0 wake; 1 increment if equal and wake; 2 if
            // equal, modify by the waiter count and wake
            let (addr, ty, val, cnt) = (c.x[0], c.x[1] as u32, c.x[2] as i32, c.x[3] as i32);
            let mut g = k.g.lock().unwrap();
            let nw = g.threads.iter().filter(|t| t.as_ref().is_some_and(|t| t.wkind == Wait::Address && t.waddr == addr)).count() as i32;
            let mut r = R_OK;
            if ty != 0 {
                let mut nv = val.wrapping_add(1);
                if ty == 2 {
                    nv = if cnt <= 0 {
                        if nw > 0 { val - 2 } else { val + 1 }
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
                    let Some(w) = g.pick(Wait::Address, addr) else { break };
                    g.wake(w, 0);
                    n += 1;
                }
            }
            c.x[0] = r;
        }
        0x45 => {
            let mut g = k.g.lock().unwrap();
            let e = g.events.iter().position(|e| e.is_none()).unwrap_or_else(|| {
                g.events.push(None);
                g.events.len() - 1
            });
            g.events[e] = Some(Event { signaled: false, refs: 2 });
            let (w, r) = (g.h_new(Obj::EventW(e)), g.h_new(Obj::EventR(e)));
            c.x[0] = R_OK;
            c.x[1] = w as u64;
            c.x[2] = r as u64;
        }
        0x11 | 0x12 | 0x17 => {
            // SignalEvent(write end) / ClearEvent(either end) / ResetSignal(read end, error if not signaled)
            let mut g = k.g.lock().unwrap();
            let e = match (imm, g.h_get(c.x[0] as u32)) {
                (0x11, Obj::EventW(e)) | (0x17, Obj::EventR(e)) | (0x12, Obj::EventW(e) | Obj::EventR(e)) => Some(e),
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
            let hs: Vec<u32> = (0..n).map(|k| unsafe { *((c.x[1] + 4 * k as u64) as *const u32) }).collect();
            let mut g = k.g.lock().unwrap();
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
                    (g, r) = block(g, i, Wait::Sync, ns);
                    idx = g.t(i).widx;
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
            let mut g = k.g.lock().unwrap();
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
            let mut g = k.g.lock().unwrap();
            match g.h_get(h) {
                Obj::EventR(e) | Obj::EventW(e) => {
                    g.h_free(h);
                    g.event_put(e);
                }
                Obj::Thread(t) => {
                    g.h_free(h);
                    g.t(t).closed = true;
                    g.t(t).cv.notify_one();
                    g.thread_maybe_free(t);
                }
                Obj::Free => {}
            }
            c.x[0] = R_OK;
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_threads(n: usize) -> State {
        let mut s = State { next_id: 1, ..Default::default() };
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
                wres: 0,
                wseq: 0,
                wobjs: vec![],
                widx: None,
                cv: Arc::new(Condvar::new()),
                tls: 0,
                cpu: core::ptr::null_mut(),
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
        assert_eq!(ld32(a), 0x1003 | WAITERS_BIT, "priority 30 first; 0x1002 still waits");
        assert!(s.t(2).woke && s.t(2).wkind == Wait::None);
        s.lock_handoff(a);
        assert_eq!(ld32(a), 0x1002, "last waiter: no waiters bit");
        s.lock_handoff(a);
        assert_eq!(ld32(a), 0, "free");
    }

    #[test]
    fn condvar_signal_retakes_the_lock() {
        let mut s = state_with_threads(3);
        let words = Box::new([0u32; 2]);
        let (lock, key) = (&words[0] as *const u32 as u64, &words[1] as *const u32 as u64);
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
        assert_eq!(s.t(1).wkind, Wait::Lock, "the lock is taken: it now waits for it");
        assert_eq!(ld32(lock), 0x1001 | WAITERS_BIT);
        s.lock_handoff(lock); // the signaller unlocks
        assert_eq!(ld32(lock), 0x1002);
        assert!(s.t(1).woke);
        let w = s.pick(Wait::CondVar, key).unwrap();
        st32(lock, 0);
        let (a, tag) = (s.t(w).waddr, s.t(w).wtag);
        s.lock_acquire_for(w, a, tag);
        assert_eq!(ld32(lock), 0x1003, "a free lock goes to the woken waiter at once");
        assert!(s.t(2).woke);
    }

    #[test]
    fn sync_waiters_and_events() {
        let mut s = state_with_threads(2);
        s.events.push(Some(Event { signaled: false, refs: 2 }));
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

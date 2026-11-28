//! the guest threads: one host thread per guest thread, its own Context and VFP bank, parked on a
//! condvar while an svc blocks it. The scheduler hooks mirror nxrt's `Scheduler` shape (design
//! only — the code does not transfer).

use crate::context::{self, Context};
use crate::Process3ds;
use std::sync::{Arc, Condvar, Mutex};

/// what a scheduler is told and asked; every method has a do-nothing default, none is called
/// with the kernel lock held
pub trait Scheduler: Sync {
    /// a guest thread starts running (before its entry)
    fn thread_start(&self, _handle: u32, _priority: i32) {}
    /// a guest thread is done (ExitThread or its entry returned)
    fn thread_exit(&self, _handle: u32) {}
    /// the calling guest thread is about to block in an svc
    fn block_enter(&self) {}
    /// it is back and wants its core again
    fn block_leave(&self) {}
    /// SetThreadPriority was applied
    fn priority_changed(&self, _handle: u32, _priority: i32) {}
}

pub struct NoScheduler;
impl Scheduler for NoScheduler {}

/// the thread-local guest state the C callbacks reach through ctx.user
pub struct ThreadState {
    pub process: *const Process3ds,
    pub thread: usize,
    /// the VFP bank and fpscr the ctx points into (owned here so the addresses stay put)
    pub vfp: Box<[u32; 32]>,
    pub fpscr: u32,
    pub ctx: Context,
}

// safe: the process outlives every thread (start() joins them), and nothing moves a ThreadState
unsafe impl Send for ThreadState {}

impl ThreadState {
    /// the state and the context wired to each other, heap-anchored by the caller's Box
    pub fn new(process: *const Process3ds, thread: usize, tls: u32) -> Box<ThreadState> {
        let mut state = Box::new(ThreadState {
            process,
            thread,
            vfp: Box::new([0u32; 32]),
            fpscr: 0,
            ctx: plain_context(tls),
        });
        state.ctx.vfp = state.vfp.as_mut_ptr();
        state.ctx.fpscr = &mut state.fpscr;
        state.ctx.user = state.as_ref() as *const ThreadState as *mut core::ffi::c_void;
        state
    }

    /// # Safety
    ///
    /// ctx must be one this host handed out, its user naming a live ThreadState
    pub unsafe fn of<'a>(ctx: *mut Context) -> &'a mut ThreadState {
        unsafe { &mut *((*ctx).user as *mut ThreadState) }
    }

    /// the process this thread runs in
    pub fn process(&self) -> &Process3ds {
        // safe: the process outlives the boot
        unsafe { &*self.process }
    }
}

/// the register state a guest thread starts with
pub fn plain_context(tls: u32) -> Context {
    Context {
        r: [0; 16],
        n: 0,
        z: 0,
        c: 0,
        v: 0,
        q: 0,
        thumb: 0,
        ge: 0,
        exclusive: 0,
        budget: 0,
        exit: 0,
        svc: 0,
        depth: 0,
        exclusive_address: 0,
        tls,
        read_pages: std::ptr::null(),
        write_pages: std::ptr::null(),
        vfp: std::ptr::null_mut(),
        fpscr: std::ptr::null_mut(),
        host: std::ptr::null(),
        user: std::ptr::null_mut(),
        stack_limit: 0,
        tick_end: 0,
        tick_last: 0,
        tick_floor: 0,
        tick_cost: 0,
        tick_at: 0,
        ticks: 0,
        wait_at: 0,
        release_at: 0,
        held: 0,
        holding: 0,
        held_floor: context::NOTHING_HELD,
        done: 0,
        trail: [0; context::TRAIL],
    }
}

/// how a thread waits: nothing, a fixed sleep, or until something signals it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    None,
    /// SleepThread: wake after the timeout (a negative one waits until signaled)
    Sleep { until_signaled: bool },
    /// a synchronization object (a mutex's wait list, an event, an arbiter slot)
    Object { objects: [u32; 8], count: usize, timeout: Option<u64> },
    /// the thread asked to exit
    Exit,
}

/// a guest thread as the kernel sees it
pub struct GuestThread {
    pub handle: u32,
    pub id: u32,
    /// the current priority, settable from an svc on another thread
    pub priority: std::sync::atomic::AtomicI32,
    pub entry: u32,
    pub arg: u32,
    pub stack_top: u32,
    pub tls: u32,
    pub state: Mutex<ThreadRun>,
    pub wake: Condvar,
}

/// the run state behind the kernel lock
pub struct ThreadRun {
    pub started: bool,
    pub exited: bool,
    pub waiting: Waiting,
    /// what signaled the last wait (its object, or none for a timeout)
    pub woke_by: Option<u32>,
    /// the context lives beside it in the ThreadState the host thread owns
    pub state: Option<*mut ThreadState>,
}

impl GuestThread {
    /// parks the host thread until the wait ends or the timeout passes; the object that
    /// signaled it, or `None` for a timeout (or a wait ended without a signal)
    pub fn park_until(&self, timeout: Option<std::time::Duration>) -> Option<u32> {
        let deadline = timeout.map(|left| std::time::Instant::now() + left);
        let mut state = self.state.lock().unwrap();
        loop {
            if state.woke_by.is_some() {
                break;
            }
            if state.waiting == Waiting::None {
                // the wait ended without a signal (an exit, a timeout the kernel recorded)
                break;
            }
            match deadline {
                Some(at) => {
                    let now = std::time::Instant::now();
                    if now >= at {
                        break;
                    }
                    let (woken, _) = self
                        .wake
                        .wait_timeout_while(state, at - now, |s| s.woke_by.is_none() && s.waiting != Waiting::None)
                        .unwrap();
                    state = woken;
                    if state.woke_by.is_none() && std::time::Instant::now() >= at {
                        break;
                    }
                }
                None => {
                    state = self
                        .wake
                        .wait_while(state, |s| s.woke_by.is_none() && s.waiting != Waiting::None)
                        .unwrap();
                }
            }
        }
        state.waiting = Waiting::None;
        state.woke_by.take()
    }

    /// signals a parked thread (its wait is satisfied by `object`)
    pub fn signal(&self, object: u32) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.waiting == Waiting::None || state.woke_by.is_some() {
            return false;
        }
        state.woke_by = Some(object);
        state.waiting = Waiting::None;
        self.wake.notify_all();
        true
    }

    /// is the thread runnable right now
    pub fn runnable(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.started && !state.exited && state.waiting == Waiting::None
    }
}

/// the kernel's wake channels, shared with the thread objects
pub type WakeGate = Arc<(Mutex<bool>, Condvar)>;

/// a plain gate a waiter sleeps on (the boot uses one to wait for the boot to end)
pub fn gate() -> WakeGate {
    Arc::new((Mutex::new(false), Condvar::new()))
}

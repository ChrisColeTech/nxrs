//! runtime3ds: the guest CPU host for 3DS titles. The generated C (translator3ds) is written
//! against recomp.h v8; this crate is a host implementing that interface: the run loop over the
//! Entry tables, page-table memory, one host thread per guest thread, the kernel svc table, an
//! ARM11/Thumb-1/VFP interpreter for what the recompiler left, and the hot-set/miss recorder.
//! docs/waves/3DS-W2.md holds the pinned svc/clock/memory facts; the donor's docs/interface.md
//! is the ABI's prose.

pub mod context;
pub mod dispatch;
pub mod hostfast;
pub mod interp;
pub mod mem;
pub mod prof;
pub mod run;
pub mod svc;
pub mod thread;

use context::Context;
use dispatch::Library;
use mem::Memory;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use thread::{GuestThread, ThreadRun, ThreadState, Waiting};

/// how the boot is limited: the wall clock cap decides a clean stop before any instruction
/// budget could
#[derive(Clone)]
pub struct BootOptions {
    /// instructions per slice (the guest tick window follows it)
    pub slice: i32,
    /// wall clock before the boot stops cleanly
    pub time_cap: std::time::Duration,
    /// total instructions before the boot stops cleanly
    pub tick_cap: u64,
    /// the main thread's priority (retail applications start theirs at 0x30)
    pub main_priority: i32,
    /// log every slice's threads (off: only the svc and gap lines)
    pub trace_slices: bool,
}

impl Default for BootOptions {
    fn default() -> BootOptions {
        BootOptions {
            slice: 200_000,
            time_cap: std::time::Duration::from_secs(120),
            tick_cap: 200_000_000,
            main_priority: 0x30,
            trace_slices: false,
        }
    }
}

/// the counters the boot report reads
#[derive(Default)]
pub struct Stats3ds {
    pub svcs: AtomicU64,
    pub interpreted: AtomicU64,
    pub recompiled: AtomicU64,
    pub ticks_answered_in_place: AtomicU64,
    pub gaps: AtomicU64,
}

/// the kernel's objects behind handles
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Object {
    Thread(usize),
    Mutex(usize),
    Semaphore(usize),
    Event(usize),
    Arbiter(usize),
    MemBlock(usize),
    Timer(usize),
}

struct Mutex3ds {
    /// the thread that holds it, with its count
    locked_by: Option<usize>,
    count: u32,
}

struct Event3ds {
    signaled: bool,
    /// 0 oneshot, 1 sticky, 2 pulse (3dbrew ResetType)
    reset: u32,
}

struct Semaphore3ds {
    count: i32,
    max: i32,
}

struct Arbiter3ds {
    /// the waiting thread slots, keyed by the address they arbitrate
    waiters: BTreeMap<u32, Vec<usize>>,
}

struct Kernel {
    threads: Vec<Option<Arc<GuestThread>>>,
    mutexes: Vec<Mutex3ds>,
    events: Vec<Event3ds>,
    semaphores: Vec<Semaphore3ds>,
    arbiters: Vec<Arbiter3ds>,
    blocks: Vec<(u32, u32)>,
    handles: BTreeMap<u32, Object>,
    next_handle: u32,
    next_thread_id: u32,
}

/// a 3DS title's guest process: the code, the memory, the threads, the kernel objects, the
/// svc/gap log. One per boot; `start` runs it headless.
pub struct Process3ds {
    pub library: Mutex<Library>,
    pub memory: Arc<Memory>,
    kernel: Mutex<Kernel>,
    pub stats: Stats3ds,
    cache_ops: AtomicU64,
    tick_counter: AtomicU64,
    /// the hot-set / miss recorder
    pub recorder: Arc<prof::Recorder>,
    gaps: Mutex<Vec<(String, u64)>>,
    log: Mutex<Vec<String>>,
    stop: Mutex<Option<String>>,
    options: BootOptions,
    scheduler: &'static dyn thread::Scheduler,
    /// the boot's start, for the wall clock cap
    began: std::time::Instant,
}

// safe: the process is shared with the guest threads through the ThreadStates; everything
// mutable sits behind locks
unsafe impl Send for Process3ds {}
unsafe impl Sync for Process3ds {}

/// how the boot ended
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// the guest called ExitProcess
    ExitProcess,
    /// the guest hit svcBreak (an assert), the log carries it
    Break(String),
    /// the wall clock cap
    Time,
    /// the instruction cap
    Ticks,
    /// every thread ended and none can run again
    Idle,
}

/// what a boot reports
#[derive(Debug, Clone)]
pub struct BootReport {
    pub program_id: u64,
    pub describe: String,
    pub stop: Stop,
    /// the guest ticks run
    pub ticks: u64,
    /// instructions the recompiled C ran, and the interpreter's (the coverage window)
    pub recompiled: u64,
    pub interpreted: u64,
    /// misses the interpreter logged, and how many sites
    pub misses: u64,
    pub miss_sites: usize,
    pub svcs: u64,
    /// the gap lines, deduped, with counts
    pub gaps: Vec<(String, u64)>,
    /// the boot log (entry, threads, svc Breaks, OutputDebugString)
    pub log: Vec<String>,
    /// threads that were created and ran at least one slice
    pub threads_ran: usize,
    /// the ticks the code answered itself (the fast path working)
    pub ticks_answered_in_place: u64,
    pub faults: u64,
    pub mmio_reads: u64,
    pub mmio_writes: u64,
    pub watched_writes: u64,
    pub cache_ops: u64,
}

impl Process3ds {
    /// builds a process over the loaded code and the mapped memory; the boot options set the
    /// slice and the caps
    pub fn new(library: Library, memory: Arc<Memory>, options: BootOptions) -> Process3ds {
        Process3ds {
            library: Mutex::new(library),
            memory,
            kernel: Mutex::new(Kernel {
                threads: Vec::new(),
                mutexes: Vec::new(),
                events: Vec::new(),
                semaphores: Vec::new(),
                arbiters: Vec::new(),
                blocks: Vec::new(),
                handles: BTreeMap::new(),
                next_handle: 0x1000,
                next_thread_id: 1,
            }),
            stats: Stats3ds::default(),
            cache_ops: AtomicU64::new(0),
            tick_counter: AtomicU64::new(0),
            recorder: Arc::new(prof::Recorder::new()),
            gaps: Mutex::new(Vec::new()),
            log: Mutex::new(Vec::new()),
            stop: Mutex::new(None),
            options,
            scheduler: &thread::NoScheduler,
            began: std::time::Instant::now(),
        }
    }

    /// installs the scheduler hooks (before the threads start)
    pub fn set_scheduler(&mut self, scheduler: &'static dyn thread::Scheduler) {
        self.scheduler = scheduler;
    }

    /// the guest tick clock: one tick per instruction, claimed in slice windows
    pub fn tick_claim(&self, slice: u64) -> u64 {
        self.tick_counter.fetch_add(slice, Ordering::Relaxed)
    }

    /// has the boot stopped, and why
    pub fn stopped(&self) -> bool {
        self.stop.lock().unwrap().is_some()
    }

    fn stop_reason(&self) -> Option<String> {
        self.stop.lock().unwrap().clone()
    }

    /// ends the boot cleanly, waking every parked thread
    pub fn stop_boot(&self, reason: String) {
        let mut stop = self.stop.lock().unwrap();
        if stop.is_some() {
            return;
        }
        *stop = Some(reason.clone());
        self.say(reason);
        let kernel = self.kernel.lock().unwrap();
        for thread in kernel.threads.iter().flatten() {
            thread.signal(u32::MAX);
        }
    }

    /// a line in the boot log
    pub fn say(&self, line: String) {
        self.log.lock().unwrap().push(line);
    }

    /// a gap line: deduped, counted (the gap ledger shape os's watch.rs keeps)
    pub fn gap(&self, line: &str) {
        self.stats.gaps.fetch_add(1, Ordering::Relaxed);
        let mut gaps = self.gaps.lock().unwrap();
        for (known, count) in gaps.iter_mut() {
            if known == line {
                *count += 1;
                return;
            }
        }
        gaps.push((line.to_owned(), 1));
    }

    /// a fresh per-thread miss-log handle
    pub fn miss_log(&self) -> prof::Handle {
        prof::Handle::new(&self.recorder)
    }

    /// the wait a thread's kernel record names, if any
    pub fn waiting_of(&self, thread: usize) -> Option<Waiting> {
        let kernel = self.kernel.lock().unwrap();
        let thread = kernel.threads.get(thread).and_then(|t| t.as_ref())?;
        let run = thread.state.lock().unwrap();
        (!run.exited).then_some(run.waiting)
    }

    /// the svcs the code answered itself, added up from the settles
    pub fn apply_settled(&self, state: &mut ThreadState, settled: hostfast::Settled) {
        self.stats.ticks_answered_in_place.fetch_add(settled.ticks_answered as u64, Ordering::Relaxed);
        if settled.releases_to_apply > 0 {
            // the code dropped the lent mutex `holding` times; the kernel takes them back
            self.apply_releases(state.thread, settled.releases_to_apply);
        }
        let _ = settled.trail_entries;
    }

    fn apply_releases(&self, thread: usize, mut count: u32) {
        let targets: Vec<usize> = {
            let kernel = self.kernel.lock().unwrap();
            kernel
                .mutexes
                .iter()
                .enumerate()
                .filter(|(_, mutex)| mutex.locked_by == Some(thread))
                .map(|(index, _)| index)
                .collect()
        };
        for index in targets {
            if count == 0 {
                return;
            }
            let (handle, freed) = {
                let mut kernel = self.kernel.lock().unwrap();
                let mutex = &mut kernel.mutexes[index];
                let taken = count.min(mutex.count);
                mutex.count -= taken;
                count -= taken;
                let freed = mutex.count == 0;
                if freed {
                    mutex.locked_by = None;
                }
                let handle = kernel
                    .handles
                    .iter()
                    .find(|(_, object)| **object == Object::Mutex(index))
                    .map(|(&handle, _)| handle)
                    .unwrap_or(0);
                (handle, freed)
            };
            if freed {
                self.wake_mutex_waiters(handle);
            }
        }
    }

    /// the svc entry the Host callback reaches
    pub fn svc_exec(&self, state: &mut ThreadState, number: u32) {
        svc::dispatch(self, state, number);
    }

    /// the bytes mapped in the application region (the GetSystemInfo answer the heap sizing uses)
    pub fn used_application_bytes(&self) -> u32 {
        self.memory.regions().iter().map(|&(_, size, ..)| size).sum()
    }

    // ---------- threads ----------

    /// the main guest thread at entry, then every guest thread: one host thread each
    pub fn boot(&self, entry: u32, stack_top: u32, stack_size: u32) -> Result<(), String> {
        self.memory
            .map(
                0x1FF8_2000,
                0x1000 * 64,
                mem::Perm::Rw,
                mem::State::Private,
                mem::Kind::Tls,
            )
            .map_err(|e| format!("the TLS region did not map, {e}"))?;
        let index = self.create_thread(entry, 0, stack_top, self.options.main_priority)
            .ok_or("the main thread did not start")?;
        self.say(format!("main thread at {entry:#x}, stack {stack_top:#x}-{:#x}, priority {:#x}", stack_top - stack_size, self.options.main_priority));
        let _ = index;
        Ok(())
    }

    /// registers a thread and starts its host thread; returns the handle
    fn create_thread(&self, entry: u32, arg: u32, stack_top: u32, priority: i32) -> Option<u32> {
        let mut kernel = self.kernel.lock().unwrap();
        let index = kernel.threads.len();
        let id = kernel.next_thread_id;
        kernel.next_thread_id += 1;
        let tls = 0x1FF8_2000 + (index as u32 + 1) * 0x200;
        let thread = Arc::new(GuestThread {
            handle: kernel.next_handle,
            id,
            priority: std::sync::atomic::AtomicI32::new(priority),
            entry,
            arg,
            stack_top,
            tls,
            state: Mutex::new(ThreadRun {
                started: false,
                exited: false,
                waiting: Waiting::None,
                woke_by: None,
                state: None,
            }),
            wake: std::sync::Condvar::new(),
        });
        let handle = kernel.next_handle;
        kernel.next_handle += 1;
        kernel.handles.insert(handle, Object::Thread(index));
        kernel.threads.push(Some(thread));
        drop(kernel);
        let process: *const Process3ds = self;
        // safe: the process outlives the boot, which waits on every thread before returning
        let mut state = ThreadState::new(process, index, tls);
        let ctx = &mut state.ctx;
        ctx.r[13] = stack_top;
        ctx.r[14] = 0xFFFF_FFFF;
        ctx.r[15] = entry;
        ctx.thumb = (entry & 1) as u8;
        let run_state = ThreadRun {
            started: false,
            exited: false,
            waiting: Waiting::None,
            woke_by: None,
            state: Some(Box::into_raw(state)),
        };
        {
            let kernel = self.kernel.lock().unwrap();
            let thread = kernel.threads[index].as_ref().unwrap();
            *thread.state.lock().unwrap() = run_state;
        }
        let process_ptr = self as *const Process3ds;
        let spawned = {
            let process_ptr = process_ptr as usize;
            std::thread::Builder::new()
                .name(format!("guest-{id}"))
                .stack_size(16 << 20)
                .spawn(move || thread_main(process_ptr, index))
                .is_ok()
        };
        spawned.then_some(handle)
    }

    /// spawns a guest thread from svc CreateThread
    pub fn spawn_guest(&self, entry: u32, arg: u32, stack_top: u32, priority: i32) -> Option<u32> {
        let handle = self.create_thread(entry, arg, stack_top, priority);
        if let Some(handle) = handle {
            self.say(format!("thread {handle:#x} created at {entry:#x}, arg {arg:#x}, stack {stack_top:#x}, priority {priority}"));
            self.scheduler.thread_start(handle, priority);
        }
        handle
    }

    /// marks a thread exited (svc ExitThread)
    pub fn exit_thread(&self, index: usize, at: u32) {
        self.say(format!("thread {index} exited at {at:#x}"));
        self.scheduler.thread_exit(index as u32);
        let kernel = self.kernel.lock().unwrap();
        if let Some(thread) = kernel.threads.get(index).and_then(|t| t.as_ref()) {
            let mut run = thread.state.lock().unwrap();
            run.exited = true;
            run.waiting = Waiting::None;
        }
    }

    fn thread_of(&self, index: usize) -> Option<Arc<GuestThread>> {
        self.kernel.lock().unwrap().threads.get(index).and_then(|t| t.as_ref()).cloned()
    }

    /// parks the calling thread (its own host thread) for the wait; wakes when it ends
    pub fn park_thread(&self, index: usize, waiting: Waiting, ns: Option<u64>) {
        let Some(thread) = self.thread_of(index) else { return };
        {
            let mut run = thread.state.lock().unwrap();
            run.waiting = waiting;
        }
        self.scheduler.block_enter();
        let timeout = ns.map(|ns| {
            // the guest sleeps in nanoseconds; a slice of host time stands in for them
            std::time::Duration::from_nanos(ns.min(2_000_000_000) as u64 / 100)
        });
        let woke = thread.park_until(timeout);
        self.scheduler.block_leave();
        let mut run = thread.state.lock().unwrap();
        run.waiting = Waiting::None;
        run.woke_by = None;
        let _ = woke;
    }

    /// the thread's kernel id (svc GetThreadId)
    pub fn thread_id_of(&self, handle: u32) -> Option<u32> {
        let kernel = self.kernel.lock().unwrap();
        match kernel.handles.get(&handle)? {
            Object::Thread(index) => Some(kernel.threads.get(*index).and_then(|t| t.as_ref()).map_or(0, |t| t.id)),
            _ => None,
        }
    }

    pub fn priority_of(&self, handle: u32) -> Option<i32> {
        let kernel = self.kernel.lock().unwrap();
        match kernel.handles.get(&handle)? {
            Object::Thread(index) => {
                kernel.threads.get(*index).and_then(|t| t.as_ref()).map(|t| t.priority.load(Ordering::Relaxed))
            }
            _ => None,
        }
    }

    pub fn set_priority(&self, handle: u32, priority: i32) -> bool {
        let kernel = self.kernel.lock().unwrap();
        match kernel.handles.get(&handle) {
            Some(Object::Thread(index)) => {
                if let Some(thread) = kernel.threads.get(*index).and_then(|t| t.as_ref()) {
                    thread.priority.store(priority, Ordering::Relaxed);
                }
                self.scheduler.priority_changed(handle, priority);
                true
            }
            _ => false,
        }
    }

    // ---------- synchronization objects ----------

    pub fn create_mutex(&self, initial: bool, owner: usize) -> u32 {
        let mut kernel = self.kernel.lock().unwrap();
        let index = kernel.mutexes.len();
        kernel.mutexes.push(Mutex3ds { locked_by: initial.then_some(owner), count: initial as u32 });
        let handle = kernel.next_handle;
        kernel.next_handle += 1;
        kernel.handles.insert(handle, Object::Mutex(index));
        handle
    }

    /// releases one hold; says whether the kernel did it
    pub fn release_mutex(&self, handle: u32, thread: usize) -> bool {
        let index = {
            let kernel = self.kernel.lock().unwrap();
            match kernel.handles.get(&handle) {
                Some(Object::Mutex(index)) => *index,
                _ => return false,
            }
        };
        let mut kernel = self.kernel.lock().unwrap();
        let mutex = &mut kernel.mutexes[index];
        if mutex.locked_by != Some(thread) {
            return false;
        }
        mutex.count -= 1;
        let freed = mutex.count == 0;
        if freed {
            mutex.locked_by = None;
        }
        drop(kernel);
        if freed {
            self.wake_mutex_waiters(handle);
        }
        true
    }

    /// wakes threads parked on a mutex handle that just freed
    fn wake_mutex_waiters(&self, handle: u32) {
        let kernel = self.kernel.lock().unwrap();
        for thread in kernel.threads.iter().flatten() {
            let mut run = thread.state.lock().unwrap();
            if let Waiting::Object { objects, count, .. } = run.waiting {
                if objects[..count].contains(&handle) {
                    run.woke_by = Some(handle);
                    run.waiting = Waiting::None;
                    drop(run);
                    thread.wake.notify_all();
                }
            }
        }
    }

    pub fn create_semaphore(&self, initial: u32, max: u32) -> u32 {
        let mut kernel = self.kernel.lock().unwrap();
        let index = kernel.semaphores.len();
        kernel.semaphores.push(Semaphore3ds { count: initial as i32, max: max as i32 });
        let handle = kernel.next_handle;
        kernel.next_handle += 1;
        kernel.handles.insert(handle, Object::Semaphore(index));
        handle
    }

    pub fn release_semaphore(&self, handle: u32, count: u32) -> Option<u32> {
        let mut kernel = self.kernel.lock().unwrap();
        let index = match kernel.handles.get(&handle)? {
            Object::Semaphore(index) => *index,
            _ => return None,
        };
        let semaphore = &mut kernel.semaphores[index];
        let old = semaphore.count as u32;
        semaphore.count = (semaphore.count + count as i32).min(semaphore.max);
        Some(old)
    }

    pub fn create_event(&self, reset: u32) -> u32 {
        let mut kernel = self.kernel.lock().unwrap();
        let index = kernel.events.len();
        kernel.events.push(Event3ds { signaled: false, reset });
        let handle = kernel.next_handle;
        kernel.next_handle += 1;
        kernel.handles.insert(handle, Object::Event(index));
        handle
    }

    pub fn signal_event(&self, handle: u32) -> bool {
        let index = {
            let kernel = self.kernel.lock().unwrap();
            match kernel.handles.get(&handle) {
                Some(Object::Event(index)) => *index,
                _ => return false,
            }
        };
        {
            let mut kernel = self.kernel.lock().unwrap();
            kernel.events[index].signaled = true;
        }
        // wake the waiters; a oneshot resets as exactly one goes through
        let mut kernel = self.kernel.lock().unwrap();
        let mut woke_any = false;
        for thread in kernel.threads.iter().flatten() {
            let mut run = thread.state.lock().unwrap();
            if let Waiting::Object { objects, count, .. } = run.waiting {
                if objects[..count].contains(&handle) {
                    run.woke_by = Some(handle);
                    run.waiting = Waiting::None;
                    drop(run);
                    thread.wake.notify_all();
                    woke_any = true;
                }
            }
        }
        if woke_any && kernel.events[index].reset == 0 {
            kernel.events[index].signaled = false;
        }
        true
    }

    pub fn clear_event(&self, handle: u32) -> bool {
        let mut kernel = self.kernel.lock().unwrap();
        let index = match kernel.handles.get(&handle) {
            Some(Object::Event(index)) => *index,
            _ => return false,
        };
        kernel.events[index].signaled = false;
        true
    }

    pub fn create_timer(&self, _reset: u32) -> u32 {
        let mut kernel = self.kernel.lock().unwrap();
        let index = kernel.events.len();
        kernel.events.push(Event3ds { signaled: false, reset: 1 });
        let handle = kernel.next_handle;
        kernel.next_handle += 1;
        kernel.handles.insert(handle, Object::Timer(index));
        handle
    }

    pub fn create_memory_block(&self, base: u32, size: u32) -> u32 {
        let mut kernel = self.kernel.lock().unwrap();
        kernel.blocks.push((base, size));
        let index = kernel.blocks.len() - 1;
        let handle = kernel.next_handle;
        kernel.next_handle += 1;
        kernel.handles.insert(handle, Object::MemBlock(index));
        handle
    }

    pub fn create_arbiter(&self) -> u32 {
        let mut kernel = self.kernel.lock().unwrap();
        let index = kernel.arbiters.len();
        kernel.arbiters.push(Arbiter3ds { waiters: BTreeMap::new() });
        let handle = kernel.next_handle;
        kernel.next_handle += 1;
        kernel.handles.insert(handle, Object::Arbiter(index));
        handle
    }

    /// svc ArbitrateAddress SIGNAL: wake `count` threads waiting at the address
    pub fn arbitrate_signal(&self, handle: u32, address: u32, count: u32) {
        let mut kernel = self.kernel.lock().unwrap();
        let index = match kernel.handles.get(&handle) {
            Some(Object::Arbiter(index)) => *index,
            _ => return,
        };
        let Some(waiters) = kernel.arbiters[index].waiters.get_mut(&address) else { return };
        let wake = waiters.drain(..count.min(waiters.len() as u32) as usize).collect::<Vec<_>>();
        for thread_index in wake {
            if let Some(thread) = kernel.threads.get(thread_index).and_then(|t| t.as_ref()) {
                let mut run = thread.state.lock().unwrap();
                run.woke_by = Some(handle);
                run.waiting = Waiting::None;
                drop(run);
                thread.wake.notify_all();
            }
        }
    }

    /// svc ArbitrateAddress wait forms: park at the address until a signal (or pass if it
    /// already holds)
    pub fn park_arbiter(&self, thread: usize, handle: u32, address: u32, value: u32, decrement: bool, timeout: Option<u64>) {
        let current = self.memory.read32(address);
        let passes = current.wrapping_sub(decrement as u32) < value;
        if decrement {
            self.memory.write32(address, current.wrapping_sub(1));
        }
        if passes {
            return;
        }
        {
            let mut kernel = self.kernel.lock().unwrap();
            if let Some(Object::Arbiter(index)) = kernel.handles.get(&handle).copied() {
                kernel.arbiters[index].waiters.entry(address).or_default().push(thread);
            }
        }
        self.park_thread(thread, Waiting::Object { objects: [handle, 0, 0, 0, 0, 0, 0, 0], count: 1, timeout }, timeout);
    }

    /// svc WaitSynchronization: an event checks its signal, a mutex its freedom, anything else
    /// waits until signaled; answers r0 (and r1 for the N form's index)
    pub fn park_wait(&self, thread: usize, handles: &[u32], ctx: &mut Context, ns: u64) -> bool {
        // something already satisfied?
        for (i, &handle) in handles.iter().enumerate() {
            if self.wait_ready(handle, thread) {
                ctx.r[0] = svc::R_OK;
                ctx.r[1] = i as u32;
                return false;
            }
        }
        if ns == u64::MAX {
            // forever: still parks, the boot's stop wakes it
        }
        let timeout = (ns != u64::MAX).then_some(ns);
        let mut objects = [0u32; 8];
        let take = handles.len().min(8);
        objects[..take].copy_from_slice(&handles[..take]);
        self.park_thread(thread, Waiting::Object { objects, count: take, timeout }, timeout);
        ctx.r[0] = svc::R_TIMEOUT;
        ctx.r[1] = 0;
        true
    }

    /// is the object ready right now (a signaled event, a free-or-own mutex)
    fn wait_ready(&self, handle: u32, thread: usize) -> bool {
        let kernel = self.kernel.lock().unwrap();
        match kernel.handles.get(&handle) {
            Some(Object::Event(index)) => kernel.events[*index].signaled,
            Some(Object::Mutex(index)) => {
                kernel.mutexes[*index].locked_by.is_none() || kernel.mutexes[*index].locked_by == Some(thread)
            }
            Some(Object::Semaphore(index)) => kernel.semaphores[*index].count > 0,
            Some(Object::Thread(index)) => {
                kernel.threads.get(*index).and_then(|t| t.as_ref()).is_none_or(|t| t.state.lock().unwrap().exited)
            }
            Some(Object::Timer(index)) => kernel.events[*index].signaled,
            _ => false,
        }
    }

    pub fn close_handle(&self, handle: u32) -> bool {
        self.kernel.lock().unwrap().handles.remove(&handle).is_some()
    }

    pub fn duplicate_handle(&self, handle: u32) -> Option<u32> {
        let object = *self.kernel.lock().unwrap().handles.get(&handle)?;
        let mut kernel = self.kernel.lock().unwrap();
        let copy = kernel.next_handle;
        kernel.next_handle += 1;
        kernel.handles.insert(copy, object);
        Some(copy)
    }

    /// the report
    pub fn report(&self, program_id: u64, describe: String) -> BootReport {
        let stop_reason = self.stop_reason();
        let stop = match stop_reason {
            Some(reason) if reason.contains("ExitProcess") => Stop::ExitProcess,
            Some(reason) if reason.contains("svcBreak") => Stop::Break(reason),
            Some(reason) if reason.contains("instruction cap") => Stop::Ticks,
            Some(reason) if reason.contains("wall clock") => Stop::Time,
            Some(other) => Stop::Break(other),
            None => Stop::Idle,
        };
        let misses = self.recorder.addresses();
        let stats = self.memory.stats();
        BootReport {
            program_id,
            describe,
            stop,
            ticks: self.tick_counter.load(Ordering::Relaxed),
            recompiled: self.stats.recompiled.load(Ordering::Relaxed),
            interpreted: self.stats.interpreted.load(Ordering::Relaxed),
            misses: self.recorder.total(),
            miss_sites: misses.len(),
            svcs: self.stats.svcs.load(Ordering::Relaxed),
            gaps: self.gaps.lock().unwrap().clone(),
            log: self.log.lock().unwrap().clone(),
            threads_ran: self.threads_ran(),
            ticks_answered_in_place: self.stats.ticks_answered_in_place.load(Ordering::Relaxed),
            faults: stats.faults,
            mmio_reads: stats.mmio_reads,
            mmio_writes: stats.mmio_writes,
            watched_writes: stats.watched_writes,
            cache_ops: self.cache_ops.load(Ordering::Relaxed),
        }
    }

    fn threads_ran(&self) -> usize {
        self.kernel.lock().unwrap().threads.iter().flatten().filter(|t| t.state.lock().unwrap().started).count()
    }

    /// marks a thread's first slice (the thread itself calls this)
    pub fn mark_started(&self, index: usize) {
        let kernel = self.kernel.lock().unwrap();
        if let Some(thread) = kernel.threads.get(index).and_then(|t| t.as_ref()) {
            let mut run = thread.state.lock().unwrap();
            run.started = true;
            run.waiting = Waiting::None;
        }
    }
}

/// the host thread's loop for one guest thread: slices, parks, exits
fn thread_main(process_ptr: usize, index: usize) {
    // safe: the process outlives the boot (start waits on it), the ThreadState stays put
    let process: &Process3ds = unsafe { &*(process_ptr as *const Process3ds) };
    let state_ptr = {
        let kernel = process.kernel.lock().unwrap();
        let thread = kernel.threads[index].as_ref().unwrap();
        let run = thread.state.lock().unwrap();
        let state = run.state.unwrap();
        drop(run);
        state
    };
    let state: &'static mut ThreadState = unsafe { Box::leak(Box::from_raw(state_ptr)) };
    state.ctx.read_pages = process.memory.read_table();
    state.ctx.write_pages = process.memory.write_table();
    state.ctx.host = &run::HOST;
    process.mark_started(index);
    process.say(format!("thread {index} entered at {:#x}", state.ctx.r[15]));
    loop {
        if process.stopped() {
            return;
        }
        match run::run_slice(process, state, process.options.slice) {
            run::SliceOutcome::Continue => {
                if process.tick_counter.load(Ordering::Relaxed) > process.options.tick_cap {
                    process.stop_boot("instruction cap reached".to_owned());
                    return;
                }
                if process.began.elapsed() > process.options.time_cap {
                    process.stop_boot("wall clock cap reached".to_owned());
                    return;
                }
                std::thread::yield_now();
            }
            run::SliceOutcome::Parked => {
                // the thread's kernel record says why; park here until it ends
                let waiting = process.waiting_of(index);
                match waiting {
                    Some(Waiting::Sleep { until_signaled }) => {
                        let thread = process.thread_of(index).unwrap();
                        thread.park_until(if until_signaled { None } else { Some(std::time::Duration::from_millis(1)) });
                    }
                    Some(Waiting::Object { timeout, .. }) => {
                        let thread = process.thread_of(index).unwrap();
                        thread.park_until(
                            timeout.map(|ns| std::time::Duration::from_nanos(ns.min(2_000_000_000) / 100)),
                        );
                    }
                    Some(Waiting::Exit) | Some(Waiting::None) | None => {
                        // exited or the boot stopped
                        return;
                    }
                }
            }
            run::SliceOutcome::Stopped => return,
        }
        if process.waiting_of(index).is_none() && process.stopped() {
            return;
        }
    }
}

/// boots a process headless: the main thread, then every guest thread, until the caps or the
/// guest's own exit; waits for the boot to end and reports
pub fn start(process: &Process3ds, program_id: u64, entry: u32, stack_top: u32, stack_size: u32) -> Result<BootReport, String> {
    process.boot(entry, stack_top, stack_size)?;
    let deadline = std::time::Instant::now() + process.options.time_cap;
    loop {
        if process.stopped() {
            break;
        }
        let alive = process
            .kernel
            .lock()
            .unwrap()
            .threads
            .iter()
            .flatten()
            .any(|t| !t.state.lock().unwrap().exited);
        if !alive {
            process.stop_boot("every guest thread ended".to_owned());
            break;
        }
        if std::time::Instant::now() > deadline {
            process.stop_boot("wall clock cap reached".to_owned());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // give the guest threads a moment to notice, then report
    std::thread::sleep(std::time::Duration::from_millis(50));
    Ok(process.report(program_id, format!("{}", process.library.lock().unwrap().describe())))
}

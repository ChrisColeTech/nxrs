//! The CPU profiler (the ini's `profile=1`): a sampling thread stops each guest thread about a thousand times a
//! second, reads where its host code is, and counts the guest function that code was lifted from; every 10 seconds
//! it writes `profile.txt` beside the executable (the functions that took the most samples, with their share).
//!
//! It also splits each guest thread's time (`Win::report`, a `profile:` log line block every 10 s): a sample of a
//! thread blocked in a kernel wait (`mark`, set around the blocking SVCs and the scheduler's wait for a core) counts
//! for that wait without stopping the thread; a running one is stopped and counts as lifted guest code, or as the
//! host function its guest state names (`Cpu::pc` while a host function runs; an SVC's guest pc otherwise), split
//! into running engine code, blocked in the OS (the sample's host pc in ntdll / win32u: a host lock, condition
//! variable or sleep) and other modules (the driver, the C runtime).
//!
//! A web build has no sampling (a page cannot stop another thread): only the thread bookkeeping and `mark` remain,
//! so the sampler's own pieces are unused there.
#![cfg_attr(target_os = "emscripten", allow(dead_code, unused_imports))]

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::Debug::{GetThreadContext, CONTEXT};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, ResumeThread, SuspendThread,
};

/// CONTEXT_AMD64 | CONTEXT_CONTROL
#[cfg(windows)]
const CONTEXT_CONTROL: u32 = 0x0010_0001;

/// A guest thread's state as the samples see it: what `mark` set, and its guest state (for `Cpu::pc`).
pub struct Slot {
    state: AtomicU32,
    cpu: AtomicU64,
}

/// `mark` states: running (guest code or a host function), else the wait it is blocked in.
pub const RUN: u32 = 0;
pub const W_SLEEP: u32 = 1;
pub const W_SYNC: u32 = 2;
pub const W_MUTEX: u32 = 3;
pub const W_CONDVAR: u32 = 4;
pub const W_ADDRESS: u32 = 5;
/// Waiting for a virtual core (the os scheduler).
pub const W_CORE: u32 = 6;
const WAIT_NAMES: [&str; 7] = ["run", "sleep", "sync", "mutex", "condvar", "address", "core"];

thread_local! {
    static MARK: Cell<Option<&'static Slot>> = const { Cell::new(None) };
}

/// The calling guest thread's state for the samples (one of the `W_*` waits or `RUN`); the previous one. Nothing
/// when the profiler is off.
pub fn mark(s: u32) -> u32 {
    if !on() {
        return RUN;
    }
    MARK.with(|m| m.get().map_or(RUN, |sl| sl.state.swap(s, Ordering::Relaxed)))
}

fn new_slot(cpu: usize) -> &'static Slot {
    let s: &'static Slot = Box::leak(Box::new(Slot {
        state: AtomicU32::new(RUN),
        cpu: AtomicU64::new(cpu as u64),
    }));
    MARK.with(|m| m.set(Some(s)));
    s
}

#[cfg(windows)]
struct Threads(Vec<(u32, usize, &'static Slot)>);
#[cfg(target_os = "linux")]
struct Threads(Vec<(u32, crate::linux::HostThread, &'static Slot)>);
#[cfg(target_os = "emscripten")]
struct Threads(Vec<(u32, (), &'static Slot)>);
// SAFETY: the handles are plain kernel handles, usable from any thread.
unsafe impl Send for Threads {}

fn threads() -> &'static Mutex<Threads> {
    static T: OnceLock<Mutex<Threads>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(Threads(vec![])))
}

static ON: OnceLock<bool> = OnceLock::new();

/// Whether the profiler runs (the ini's `profile`, read once at start-up).
pub fn on() -> bool {
    *ON.get().unwrap_or(&false)
}

/// The calling host thread runs guest thread `handle`: kept for the profiler's samples and the per-thread CPU
/// figures (`thread_cpu`); `cpu` is its guest state.
#[cfg(windows)]
pub fn add_current(handle: u32, cpu: *mut crate::cpu::Cpu) {
    let slot = new_slot(cpu as usize);
    let mut d: HANDLE = core::ptr::null_mut();
    // SAFETY: duplicating our own thread's pseudo-handle into a real one
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            GetCurrentThread(),
            GetCurrentProcess(),
            &mut d,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok != 0 {
        threads()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .0
            .push((handle, d as usize, slot));
    }
}

/// Guest thread `handle` is done on this host thread.
#[cfg(windows)]
pub fn remove(handle: u32) {
    MARK.with(|m| m.set(None));
    let mut t = threads().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(i) = t.0.iter().position(|(h, ..)| *h == handle) {
        let (_, d, _) = t.0.remove(i);
        // SAFETY: our duplicated handle
        unsafe { CloseHandle(d as HANDLE) };
    }
}

/// Every live guest thread's CPU time so far (user + kernel, seconds), by its handle (per-thread guest CPU).
#[cfg(windows)]
pub fn thread_cpu() -> Vec<(u32, f64)> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetThreadTimes;
    let t = threads().lock().unwrap_or_else(|p| p.into_inner());
    t.0.iter()
        .filter_map(|&(h, d, _)| {
            let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) =
                unsafe { core::mem::zeroed() };
            // SAFETY: our duplicated thread handle and our own FILETIMEs
            let ok = unsafe { GetThreadTimes(d as HANDLE, &mut c, &mut e, &mut k, &mut u) } != 0;
            let ft = |f: FILETIME| {
                ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7
            };
            ok.then(|| (h, ft(k) + ft(u)))
        })
        .collect()
}

/// The host pc of a thread: suspended, read, resumed, nothing else in between (it may hold any lock).
#[cfg(windows)]
fn pc_of(h: HANDLE) -> Option<u64> {
    // SAFETY: a thread handle with suspend and context rights; the CONTEXT is 16-aligned by its type
    unsafe {
        if SuspendThread(h) == u32::MAX {
            return None;
        }
        let mut cx: CONTEXT = core::mem::zeroed();
        cx.ContextFlags = CONTEXT_CONTROL;
        let ok = GetThreadContext(h, &mut cx) != 0;
        ResumeThread(h);
        ok.then_some(cx.Rip)
    }
}

/// Starts the profiler when `on` (once).
pub fn start(on: bool) {
    if ON.set(on).is_err() || !on {
        return;
    }
    let out = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("profile.txt")))
        .unwrap_or_else(|| "profile.txt".into());
    // a page cannot stop or read another thread's registers: no sampling profiler on the web
    #[cfg(target_os = "emscripten")]
    {
        crate::rt::log(&format!("profile: not available in web builds ({} not written)", out.display()));
        return;
    }
    #[cfg(not(target_os = "emscripten"))]
    crate::rt::log(&format!("profile: active ({} every 10 s)", out.display()));
    #[cfg(not(target_os = "emscripten"))]
    std::thread::Builder::new()
        .name("profiler".into())
        .spawn(move || {
            let mut counts: HashMap<u64, u64> = HashMap::new();
            let (mut total, mut outside) = (0u64, 0u64);
            let t0 = Instant::now();
            let mut last = Instant::now();
            let ranges = Ranges::get();
            let mut win = Win::default();
            let mut cpu_was: HashMap<u32, f64> = thread_cpu().into_iter().collect();
            loop {
                std::thread::sleep(Duration::from_millis(1));
                win.rounds += 1;
                let hs: Vec<_> = threads()
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .0
                    .iter()
                    .map(|&(h, th, s)| (h, th, s))
                    .collect();
                let table = crate::dispatch::table();
                for (h, th, slot) in hs {
                    // a thread blocked in a kernel wait counts for it without being stopped
                    let st = slot.state.load(Ordering::Relaxed);
                    if st != RUN {
                        total += 1;
                        outside += 1;
                        win.add(h, Where::Wait(st.min(W_CORE)));
                        continue;
                    }
                    #[cfg(windows)]
                    let rip = pc_of(th as HANDLE);
                    #[cfg(target_os = "linux")]
                    let rip = crate::linux::thread_pc(th);
                    let Some(rip) = rip else { continue };
                    total += 1;
                    match table.and_then(|t| t.guest_of_host(rip)).filter(|&(_, d)| d < 0x10_0000) {
                        Some((g, _)) => {
                            *counts.entry(g).or_insert(0) += 1;
                            win.add(h, Where::Guest(g));
                        }
                        None => {
                            outside += 1;
                            // SAFETY: the thread's own Cpu, live while it is registered; a racy read of one word
                            let pc = unsafe {
                                core::ptr::read_volatile(core::ptr::addr_of!(
                                    (*(slot.cpu.load(Ordering::Relaxed) as *const crate::cpu::Cpu)).pc
                                ))
                            };
                            win.add(h, Where::Host(pc, ranges.class(rip)));
                        }
                    }
                }
                if last.elapsed() >= Duration::from_secs(10) && total > 0 {
                    let secs = last.elapsed().as_secs_f64();
                    last = Instant::now();
                    let cpu_now: HashMap<u32, f64> = thread_cpu().into_iter().collect();
                    for line in win.report(&cpu_was, &cpu_now, secs, &host_name) {
                        crate::rt::log(&line);
                    }
                    cpu_was = cpu_now;
                    win = Win::default();
                    let mut v: Vec<(&u64, &u64)> = counts.iter().collect();
                    v.sort_by(|a, b| b.1.cmp(a.1));
                    let mut s = format!(
                        "CPU profile after {:.0} s: {total} samples of the guest threads ({:.1}% outside lifted code: host functions, waits)\n",
                        t0.elapsed().as_secs_f64(),
                        100.0 * outside as f64 / total as f64
                    );
                    for (g, n) in v.into_iter().take(200) {
                        s.push_str(&format!("{:6.2}% {:8}  {}\n", 100.0 * *n as f64 / total as f64, n, crate::dispatch::guest_name(*g)));
                    }
                    let _ = std::fs::write(&out, s);
                }
            }
        })
        .ok();
}

/// The calling host thread runs guest thread `handle` (web: kept for `mark` and the thread list; nothing samples it).
#[cfg(target_os = "emscripten")]
pub fn add_current(handle: u32, cpu: *mut crate::cpu::Cpu) {
    let slot = new_slot(cpu as usize);
    threads()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .0
        .push((handle, (), slot));
}

#[cfg(target_os = "emscripten")]
pub fn remove(handle: u32) {
    threads()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .0
        .retain(|t| t.0 != handle);
}

/// Per-thread CPU time is not visible to a page: none on the web.
#[cfg(target_os = "emscripten")]
pub fn thread_cpu() -> Vec<(u32, f64)> {
    vec![]
}

/// The calling host thread runs guest thread `handle` (Linux: its pthread, sampled with SIGPROF).
#[cfg(target_os = "linux")]
pub fn add_current(handle: u32, cpu: *mut crate::cpu::Cpu) {
    let slot = new_slot(cpu as usize);
    threads()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .0
        .push((handle, crate::linux::current_thread(), slot));
}

#[cfg(target_os = "linux")]
pub fn remove(handle: u32) {
    MARK.with(|m| m.set(None));
    let mut t = threads().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(i) = t.0.iter().position(|(h, ..)| *h == handle) {
        t.0.remove(i);
    }
}

/// Every live guest thread's CPU time so far (user + system, seconds), by its handle.
#[cfg(target_os = "linux")]
pub fn thread_cpu() -> Vec<(u32, f64)> {
    let t = threads().lock().unwrap_or_else(|p| p.into_inner());
    t.0.iter()
        .filter_map(|&(h, th, _)| crate::linux::thread_cpu_seconds(th).map(|s| (h, s)))
        .collect()
}

/// Where a sample found a thread: blocked in a kernel wait (`W_*`), in lifted guest code (the function), or outside
/// it (the guest pc: a host function's address or an SVC's pc; and the host code's class).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    Wait(u32),
    Guest(u64),
    Host(u64, Class),
}

/// The host code a sample outside lifted code was in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    /// this image's code (the engine)
    Engine,
    /// ntdll / win32u: blocked or yielding in the OS (a host lock, condition variable, sleep)
    Os,
    /// another module (the graphics driver, the C runtime)
    Other,
}

const CLASS_NAMES: [&str; 3] = ["engine", "os-blocked", "other dll"];

/// Address ranges of the modules a host pc is classed by.
#[derive(Default)]
struct Ranges {
    own: (u64, u64),
    os: Vec<(u64, u64)>,
}

impl Ranges {
    #[cfg(windows)]
    fn get() -> Ranges {
        use windows_sys::Win32::System::LibraryLoader::{
            GetModuleHandleA, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        };
        // SAFETY: a loaded image's PE headers
        let span = |base: u64| unsafe {
            if base == 0 {
                return (0, 0);
            }
            let lfanew = *((base + 0x3c) as *const u32) as u64;
            // OptionalHeader.SizeOfImage: NT signature 4 + file header 20 + 56
            let size = *((base + lfanew + 4 + 20 + 56) as *const u32) as u64;
            (base, base + size)
        };
        let mut m = core::ptr::null_mut();
        // SAFETY: a lookup by address of our own code; the refcount is left alone
        let own = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                Ranges::get as *const () as *const u16,
                &mut m,
            )
        };
        let own = if own != 0 { span(m as u64) } else { (0, 0) };
        let os = [b"ntdll.dll\0".as_slice(), b"win32u.dll\0".as_slice()]
            .iter()
            // SAFETY: a lookup of an already loaded module by name
            .map(|n| span(unsafe { GetModuleHandleA(n.as_ptr()) } as u64))
            .filter(|r| r.1 > 0)
            .collect();
        Ranges { own, os }
    }

    #[cfg(target_os = "linux")]
    fn get() -> Ranges {
        Ranges::default()
    }

    fn class(&self, rip: u64) -> Class {
        if self.os.iter().any(|&(lo, hi)| rip >= lo && rip < hi) {
            Class::Os
        } else if (rip >= self.own.0 && rip < self.own.1) || self.own.1 == 0 {
            Class::Engine
        } else {
            Class::Other
        }
    }
}

/// A host function's name, else the guest place of an SVC.
fn host_name(pc: u64) -> String {
    crate::hostfn::name_of(pc).unwrap_or_else(|| format!("svc at {}", crate::dispatch::guest_name(pc)))
}

/// One thread's samples in a report window.
#[derive(Default)]
struct Per {
    n: u64,
    wait: [u64; 7],
    guest: u64,
    host: [u64; 3],
    keys: HashMap<(u64, Class), u64>,
}

/// The samples of a report window, by thread.
#[derive(Default)]
struct Win {
    rounds: u64,
    per: HashMap<u32, Per>,
    guest: HashMap<u64, u64>,
}

impl Win {
    fn add(&mut self, h: u32, w: Where) {
        let p = self.per.entry(h).or_default();
        p.n += 1;
        match w {
            Where::Wait(s) => p.wait[s as usize] += 1,
            Where::Guest(g) => {
                p.guest += 1;
                *self.guest.entry(g).or_default() += 1;
            }
            Where::Host(pc, c) => {
                p.host[c as usize] += 1;
                *p.keys.entry((pc, c)).or_default() += 1;
            }
        }
    }

    /// The `profile:` lines: the busiest threads (most samples not in a kernel wait), each one's split as shares of
    /// its samples (= of wall time) with its CPU time from the OS, its top host functions; the window's top guest
    /// functions (share of all guest-code samples).
    fn report(
        &self,
        cpu_was: &HashMap<u32, f64>,
        cpu_now: &HashMap<u32, f64>,
        secs: f64,
        name: &dyn Fn(u64) -> String,
    ) -> Vec<String> {
        let mut out = vec![format!(
            "profile: thread split over {secs:.1} s ({} sample rounds, {} threads seen)",
            self.rounds,
            self.per.len()
        )];
        let mut v: Vec<(&u32, &Per)> = self.per.iter().collect();
        let busy = |p: &Per| p.n - p.wait.iter().sum::<u64>();
        v.sort_by(|a, b| busy(b.1).cmp(&busy(a.1)).then(a.0.cmp(b.0)));
        for (h, p) in v.into_iter().take(10) {
            if busy(p) == 0 {
                break;
            }
            let pct = |c: u64| 100.0 * c as f64 / p.n.max(1) as f64;
            let cpu = match (cpu_was.get(h), cpu_now.get(h)) {
                (Some(a), Some(b)) => format!("{:.0}%", 100.0 * (b - a) / secs.max(1e-9)),
                _ => "?".into(),
            };
            let mut s = format!(
                "profile: thread {h:#x} cpu {cpu}: guest {:.1}%, {} {:.1}%, {} {:.1}%, {} {:.1}%",
                pct(p.guest),
                CLASS_NAMES[0],
                pct(p.host[0]),
                CLASS_NAMES[1],
                pct(p.host[1]),
                CLASS_NAMES[2],
                pct(p.host[2])
            );
            s.push_str("; waits");
            for (k, &c) in p.wait.iter().enumerate().skip(1) {
                if c > 0 {
                    s.push_str(&format!(" {} {:.1}%", WAIT_NAMES[k], pct(c)));
                }
            }
            let mut keys: Vec<(&(u64, Class), &u64)> = p.keys.iter().collect();
            keys.sort_by(|a, b| b.1.cmp(a.1).then(a.0 .0.cmp(&b.0 .0)));
            s.push_str("; host:");
            for ((pc, c), n) in keys.into_iter().take(6) {
                s.push_str(&format!(" {} [{}] {:.1}%,", name(*pc), CLASS_NAMES[*c as usize], pct(*n)));
            }
            out.push(s.trim_end_matches(',').to_string());
        }
        let all: u64 = self.guest.values().sum();
        let mut g: Vec<(&u64, &u64)> = self.guest.iter().collect();
        g.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let mut s = format!("profile: guest code {all} samples, top:");
        for (f, n) in g.into_iter().take(12) {
            s.push_str(&format!(
                " {} {:.1}%,",
                crate::dispatch::guest_name(*f),
                100.0 * *n as f64 / all.max(1) as f64
            ));
        }
        out.push(s.trim_end_matches(',').to_string());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The split: waits counted by kind, guest and host samples by class, the busiest thread first, shares of the
    /// thread's samples.
    #[test]
    fn thread_split_report() {
        let mut w = Win { rounds: 4, ..Default::default() };
        for x in [Where::Wait(W_ADDRESS), Where::Guest(0x7000_0000), Where::Host(0x10, Class::Os), Where::Host(0x10, Class::Os)] {
            w.add(1, x);
        }
        w.add(2, Where::Wait(W_CORE));
        w.add(2, Where::Guest(0x7000_0000));
        let was = HashMap::from([(1u32, 1.0f64)]);
        let now = HashMap::from([(1u32, 1.5f64)]);
        let r = w.report(&was, &now, 1.0, &|pc| format!("f{pc:x}"));
        assert_eq!(r.len(), 4, "{r:?}");
        assert!(r[1].starts_with("profile: thread 0x1 cpu 50%: guest 25.0%, engine 0.0%, os-blocked 50.0%"), "{}", r[1]);
        assert!(r[1].contains("waits address 25.0%") && r[1].contains("f10 [os-blocked] 50.0%"), "{}", r[1]);
        assert!(r[2].contains("thread 0x2 cpu ?") && r[2].contains("core 50.0%"), "{}", r[2]);
        assert!(r[3].contains("guest code 2 samples"), "{}", r[3]);
    }
}

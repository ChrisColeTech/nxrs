//! The CPU profiler (the ini's `profile=1`): a sampling thread stops each guest thread about a thousand times a
//! second, reads where its host code is, and counts the guest function that code was lifted from; every 10 seconds
//! it writes `profile.txt` beside the executable (the functions that took the most samples, with their share), as
//! Turbo's runtime does.

use std::collections::HashMap;
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

#[cfg(windows)]
struct Threads(Vec<(u32, usize)>);
#[cfg(target_os = "linux")]
struct Threads(Vec<(u32, crate::linux::HostThread)>);
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
/// figures (`thread_cpu`).
#[cfg(windows)]
pub fn add_current(handle: u32) {
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
            .push((handle, d as usize));
    }
}

/// Guest thread `handle` is done on this host thread.
#[cfg(windows)]
pub fn remove(handle: u32) {
    let mut t = threads().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(i) = t.0.iter().position(|(h, _)| *h == handle) {
        let (_, d) = t.0.remove(i);
        // SAFETY: our duplicated handle
        unsafe { CloseHandle(d as HANDLE) };
    }
}

/// Every live guest thread's CPU time so far (user + kernel, seconds), by its handle (Turbo kernel.c's per-thread
/// guest CPU).
#[cfg(windows)]
pub fn thread_cpu() -> Vec<(u32, f64)> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetThreadTimes;
    let t = threads().lock().unwrap_or_else(|p| p.into_inner());
    t.0.iter()
        .filter_map(|&(h, d)| {
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
    crate::rt::log(&format!("profile: active ({} every 10 s)", out.display()));
    std::thread::Builder::new()
        .name("profiler".into())
        .spawn(move || {
            let mut counts: HashMap<u64, u64> = HashMap::new();
            let (mut total, mut outside) = (0u64, 0u64);
            let t0 = Instant::now();
            let mut last = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(1));
                #[cfg(windows)]
                let pcs: Vec<u64> = {
                    let hs: Vec<usize> = threads().lock().unwrap_or_else(|p| p.into_inner()).0.iter().map(|(_, h)| *h).collect();
                    hs.iter().filter_map(|&h| pc_of(h as HANDLE)).collect()
                };
                #[cfg(target_os = "linux")]
                let pcs: Vec<u64> = {
                    let hs: Vec<crate::linux::HostThread> = threads().lock().unwrap_or_else(|p| p.into_inner()).0.iter().map(|(_, h)| *h).collect();
                    hs.into_iter().filter_map(crate::linux::thread_pc).collect()
                };
                let table = crate::dispatch::table();
                for rip in pcs {
                    total += 1;
                    match table.and_then(|t| t.guest_of_host(rip)).filter(|&(_, d)| d < 0x10_0000) {
                        Some((g, _)) => *counts.entry(g).or_insert(0) += 1,
                        None => outside += 1,
                    }
                }
                if last.elapsed() >= Duration::from_secs(10) && total > 0 {
                    last = Instant::now();
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

/// The calling host thread runs guest thread `handle` (Linux: its pthread, sampled with SIGPROF).
#[cfg(target_os = "linux")]
pub fn add_current(handle: u32) {
    threads()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .0
        .push((handle, crate::linux::current_thread()));
}

#[cfg(target_os = "linux")]
pub fn remove(handle: u32) {
    let mut t = threads().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(i) = t.0.iter().position(|(h, _)| *h == handle) {
        t.0.remove(i);
    }
}

/// Every live guest thread's CPU time so far (user + system, seconds), by its handle.
#[cfg(target_os = "linux")]
pub fn thread_cpu() -> Vec<(u32, f64)> {
    let t = threads().lock().unwrap_or_else(|p| p.into_inner());
    t.0.iter()
        .filter_map(|&(h, th)| crate::linux::thread_cpu_seconds(th).map(|s| (h, s)))
        .collect()
}

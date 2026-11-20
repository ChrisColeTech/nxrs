//! `metrics=1` (or `NXRS_METRICS=1`): once a second a row in `metrics-<time>.csv` beside the game: seconds, frames
//! presented and FPS, process CPU (percent of one core) and private memory, then every engine counter's change over
//! that second (`nxconfig::metrics`: dropped work, waits, ...; a `#` line names the columns again when a counter
//! appears). Per-frame rows graphics pushes go to `frames-<time>.csv`. Off, nothing runs.

use std::io::Write;
use std::sync::Mutex;
#[cfg(windows)]
use windows_sys::Win32::Foundation::FILETIME;
#[cfg(windows)]
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

struct M {
    f: std::fs::File,
    frames: Option<std::fs::File>,
    names: Vec<&'static str>,
    last: Vec<u64>,
    t0: std::time::Instant,
    last_t: f64,
    last_cpu: f64,
    last_frames: u64,
}
static M_: Mutex<Option<M>> = Mutex::new(None);

/// The process's CPU time (user + system), seconds.
#[cfg(target_os = "linux")]
pub fn cpu_seconds() -> f64 {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
}

/// The process's memory in MB (resident + swapped out: Linux's nearest to Windows' private bytes).
#[cfg(target_os = "linux")]
pub fn private_mb() -> f64 {
    let (rss, swap) = nxrt::linux::process_memory();
    (rss + swap) as f64 / 1048576.0
}

#[cfg(windows)]
pub fn cpu_seconds() -> f64 {
    unsafe {
        let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = std::mem::zeroed();
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
        ft(k) + ft(u)
    }
}

#[cfg(windows)]
pub fn private_mb() -> f64 {
    unsafe {
        let mut pm: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        pm.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(GetCurrentProcess(), &mut pm as *mut _ as *mut _, pm.cb);
        pm.PrivateUsage as f64 / 1048576.0
    }
}

pub fn start() {
    if !nxconfig::metrics::enabled() {
        return;
    }
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // NXRS_METRICS=<folder>: the files go there (the harness's run folder), else beside the game
    let dir = match std::env::var("NXRS_METRICS") {
        Ok(v) if v.len() > 1 => std::path::PathBuf::from(v),
        _ => crate::game().exe_dir.clone(),
    };
    let _ = std::fs::create_dir_all(&dir);
    let dir = &dir;
    let p = dir.join(format!("metrics-{t}.csv"));
    match std::fs::File::create(&p) {
        Ok(f) => {
            let frames = std::fs::File::create(dir.join(format!("frames-{t}.csv"))).ok();
            crate::flog!("metrics: {} each second, frames-{t}.csv each frame", p.display());
            *M_.lock().unwrap() = Some(M { f, frames, names: vec![], last: vec![], t0: std::time::Instant::now(), last_t: 0.0, last_cpu: cpu_seconds(), last_frames: 0 });
        }
        Err(e) => crate::flog!("[RED] metrics: cannot write {}: {e}", p.display()),
    }
}

/// Once a second from the front-end loop.
pub fn tick() {
    let mut g = M_.lock().unwrap();
    let Some(m) = g.as_mut() else { return };
    let t = m.t0.elapsed().as_secs_f64();
    let dt = (t - m.last_t).max(1e-3);
    let cpu = cpu_seconds();
    let frames = nxos::nvn::presents();
    let snap = nxconfig::metrics::snapshot();
    let names: Vec<&'static str> = snap.iter().map(|(k, _)| *k).collect();
    if names != m.names {
        let _ = writeln!(m.f, "#seconds,frames,fps,cpu_percent,private_mb{}", names.iter().map(|n| format!(",{n}")).collect::<String>());
        m.last.resize(names.len(), 0);
        m.names = names;
    }
    let mut row = format!("{t:.1},{frames},{:.1},{:.0},{:.0}", (frames - m.last_frames) as f64 / dt, (cpu - m.last_cpu) / dt * 100.0, private_mb());
    for (i, (_, v)) in snap.iter().enumerate() {
        row.push_str(&format!(",{}", v - m.last[i]));
        m.last[i] = *v;
    }
    let _ = writeln!(m.f, "{row}");
    if let Some(f) = m.frames.as_mut() {
        for r in nxconfig::metrics::take_frames() {
            let _ = writeln!(f, "{r}");
        }
    }
    m.last_t = t;
    m.last_cpu = cpu;
    m.last_frames = frames;
}

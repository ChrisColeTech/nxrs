//! A host thread that watches the run: a heartbeat line with the guest's progress every few seconds
//! (`NXOS_HEARTBEAT` seconds, default 10, 0 = none), and the end of a measured run: `NXOS_EXIT_AFTER=<seconds>`
//! ends the process after that long, and `NXOS_EXIT_AFTER_SUBMIT=<seconds>` that long after the first NVN queue
//! submit, both with the gap summary. A stall (presents stopped for 15 s after the first) logs every guest thread's
//! state once per stall (`nxrt::kernel::dump_threads`); the control command `threads` prints it any time.

use std::time::{Duration, Instant};

fn env_secs(k: &str) -> Option<f64> {
    std::env::var(k).ok().and_then(|v| v.parse().ok())
}

pub fn run() {
    let t0 = Instant::now();
    let beat = env_secs("NXOS_HEARTBEAT").unwrap_or(10.0);
    let after = env_secs("NXOS_EXIT_AFTER");
    let after_submit = env_secs("NXOS_EXIT_AFTER_SUBMIT");
    let mut last = Instant::now();
    nxconfig::ctl::register(
        "threads",
        ": every guest thread, what it waits for and its guest frames",
        |_| nxrt::kernel::dump_threads(),
    );
    nxconfig::ctl::register("peek", "<address hex> [bytes hex, at most 0x1000]: guest memory as hex words (readable pages only)", |a| {
        let h = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok();
        let (Some(at), n) = (a.first().and_then(|s| h(s)), a.get(1).and_then(|s| h(s)).unwrap_or(0x40).min(0x1000)) else {
            return "peek <address hex> [bytes hex]".into();
        };
        let info = crate::mem::get().query(at);
        if info.perm & 1 == 0 || at + n > info.addr + info.size {
            return format!("{at:#x}: not readable (state {:#x}, perm {:#x})", info.state, info.perm);
        }
        crate::guest::bytes(at, n)
            .chunks(16)
            .enumerate()
            .map(|(i, c)| {
                let w: Vec<String> = c.chunks(4).map(|x| format!("{:08x}", u32::from_le_bytes([x[0], *x.get(1).unwrap_or(&0), *x.get(2).unwrap_or(&0), *x.get(3).unwrap_or(&0)]))).collect();
                format!("{:#x}: {}
", at + 16 * i as u64, w.join(" "))
            })
            .collect()
    });
    register_debug_commands();
    let mut cpu = ThreadCpu::default();
    let mut last_cpu = Instant::now();
    let mut stall = Stall::default();
    loop {
        let p = crate::nvn::presents();
        // a game paused by the host (the web page's menu) shows no new image on purpose
        if crate::clock::held() {
            stall = Stall::default();
        }
        if let Some(sec) = stall.tick(p, Instant::now()) {
            crate::log!("warning: {sec} s without a new image (frames={p}, GPU submits={}); the guest threads:", crate::nvn::submits());
            for l in nxrt::kernel::dump_threads().lines() {
                crate::log!("  {l}");
            }
        }
        std::thread::sleep(Duration::from_millis(100));
        if last_cpu.elapsed().as_secs_f64() >= 1.0 {
            last_cpu = Instant::now();
            cpu.tick();
        }
        if beat > 0.0 && last.elapsed().as_secs_f64() >= beat {
            last = Instant::now();
            let svcs: u64 = crate::svc::counts().iter().map(|c| c.1).sum();
            crate::log!(
                "heartbeat: {svcs} svcs, {} NVN calls, {} submits, {} presents, {} gaps; {}",
                crate::nvn::calls(),
                crate::nvn::submits(),
                crate::nvn::presents(),
                crate::log::gaps().len(),
                crate::sched::stats()
            );
            if crate::log::secs() as u64 % 60 < beat as u64 + 1 {
                // where the process's private memory is: the guest's heap and physical maps (committed pages)
                let (heap, alias) = nxrt::kernel::get().space.committed();
                crate::log!(
                    "memory: guest heap {} MB committed, physical maps {} MB",
                    heap >> 20,
                    alias >> 20
                );
                crate::log!("memory layout: {}", nxrt::kernel::get().space.layout());
                for l in crate::services::audio::report() {
                    crate::log!("{l}");
                }
            }
        }
        let done = after.is_some_and(|s| t0.elapsed().as_secs_f64() >= s)
            || after_submit.is_some_and(|s| crate::nvn::first_submit_age().is_some_and(|a| a >= s));
        if done {
            crate::log!("run ends here (NXOS_EXIT_AFTER / NXOS_EXIT_AFTER_SUBMIT)");
            crate::nvn::report();
            for l in crate::services::audio::report() {
                crate::log!("{l}");
            }
            crate::log::report_gaps("end of run");
            nxrt::process::exit(0);
        }
    }
}

fn hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

/// The control pipe's debugging commands on the os side (nxnative DM_CTL): symbols, memory to a
/// file, the environment, the guest clock's speed, focus and touch gestures for scripted runs.
pub fn register_debug_commands() {
    crate::cheats::register_commands();
    register_gap_command();
    nxconfig::ctl::register(
        "wpage",
        "<page hex>: log each write to a guest page and the function that made it (up to 600)",
        |a| match a.first().and_then(|s| hex(s)) {
            Some(p) => crate::wwatch::arm(p, p + 0x1000),
            None => "wpage <page hex>
"
            .into(),
        },
    );
    nxconfig::ctl::register(
        "wtrace",
        "<from hex> <to hex>: log each write to a guest range (at most 64 MB, up to 600)",
        |a| match (
            a.first().and_then(|s| hex(s)),
            a.get(1).and_then(|s| hex(s)),
        ) {
            (Some(lo), Some(hi)) => crate::wwatch::arm(lo, hi),
            _ => "wtrace <from hex> <to hex>
"
            .into(),
        },
    );
    nxconfig::ctl::register("wclear", ": stop every write watch", |_| {
        crate::wwatch::disarm()
    });
    nxconfig::ctl::register("addr", "<mangled name>: the guest address a loaded module defines it at (and its host takeover, if any)", |a| {
        let Some(n) = a.first() else { return "addr <mangled name>\n".into() };
        match crate::hle::sym(n) {
            Some(v) => format!("{n} = {v:#x} ({})\n", nxrt::dispatch::guest_name(v)),
            None => format!("{n}: not defined by the loaded modules\n"),
        }
    });
    nxconfig::ctl::register(
        "memdump",
        "<address hex> <bytes hex> <file>: guest memory into a file (readable pages only)",
        |a| {
            let (Some(at), Some(n), Some(f)) = (
                a.first().and_then(|s| hex(s)),
                a.get(1).and_then(|s| hex(s)),
                a.get(2),
            ) else {
                return "memdump <address hex> <bytes hex> <file>\n".into();
            };
            let n = n.min(256 << 20);
            let (mut out, mut p) = (Vec::with_capacity(n as usize), at);
            while p < at + n {
                let info = crate::mem::get().query(p);
                let end = (info.addr + info.size).min(at + n);
                if info.perm & 1 == 0
                    || info.state == crate::mem::ST_FREE
                    || !nxrt::mem::readable(p, end - p)
                {
                    break;
                }
                out.extend_from_slice(crate::guest::bytes(p, end - p));
                p = end;
            }
            match std::fs::write(f, &out) {
                Ok(()) => format!(
                    "{:#x} bytes from {at:#x} into {f}{}\n",
                    out.len(),
                    if (out.len() as u64) < n {
                        " (stopped at an unreadable page)"
                    } else {
                        ""
                    }
                ),
                Err(e) => format!("{f}: {e}\n"),
            }
        },
    );
    nxconfig::ctl::register(
        "env",
        "<NAME>: an environment variable of the game's process",
        |a| match a.first() {
            Some(n) => format!("{n}={}\n", std::env::var(n).unwrap_or_default()),
            None => "env <NAME>\n".into(),
        },
    );
    nxconfig::ctl::register(
        "speed",
        "[rate]: the guest clock's rate (1 = real time, 0.05..4); none: the rate now",
        |a| {
            if let Some(r) = a.first().and_then(|v| v.parse::<f64>().ok()) {
                crate::clock::set_rate(r);
            }
            format!("clock rate {}\n", crate::clock::rate())
        },
    );
    nxconfig::ctl::register(
        "focus",
        "<1|2|3>: the window's focus as the game sees it (1 in focus, 2 out, 3 background)",
        |a| match a.first().and_then(|v| v.parse::<i32>().ok()) {
            Some(st @ 1..=3) => {
                crate::services::oe::set_focus(st);
                format!("focus {}\n", crate::services::oe::focus())
            }
            _ => format!(
                "focus {} (focus <1|2|3> to change it)\n",
                crate::services::oe::focus()
            ),
        },
    );
    nxconfig::ctl::register("drag", "<x0> <y0> <x1> <y1> [seconds]: a finger dragged on the touch screen (1280x720), 0.3 s by default", |a| {
        let v: Vec<u32> = a.iter().take(4).filter_map(|s| s.parse().ok()).collect();
        if v.len() < 4 {
            return "drag <x0> <y0> <x1> <y1> [seconds]\n".into();
        }
        let secs = a.get(4).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.3).clamp(0.05, 10.0);
        let (x0, y0, x1, y1) = (v[0] as f64, v[1] as f64, v[2] as f64, v[3] as f64);
        std::thread::spawn(move || {
            let steps = (secs * 120.0) as u32;
            for i in 0..=steps {
                let t = i as f64 / steps.max(1) as f64;
                crate::input::set_touch(true, (x0 + (x1 - x0) * t) as u32, (y0 + (y1 - y0) * t) as u32);
                std::thread::sleep(Duration::from_secs_f64(secs / steps.max(1) as f64));
            }
            crate::input::set_touch(false, x1 as u32, y1 as u32);
        });
        format!("dragging {x0} {y0} -> {x1} {y1} over {secs} s\n")
    });
    // two fingers either side of a centre: their spread from r0 to r1 (pinch), or their angle from a0 to a1 (rotate)
    let two = |cx: f64, cy: f64, r: (f64, f64), deg: (f64, f64), secs: f64| {
        std::thread::spawn(move || {
            let steps = ((secs * 120.0) as u32).max(1);
            let mut last = [(0, 0); 2];
            for i in 0..=steps {
                let t = i as f64 / steps as f64;
                last = crate::input::two_fingers(
                    cx,
                    cy,
                    r.0 + (r.1 - r.0) * t,
                    deg.0 + (deg.1 - deg.0) * t,
                );
                for (k, &(x, y)) in last.iter().enumerate() {
                    crate::input::set_finger(k, true, x, y);
                }
                std::thread::sleep(Duration::from_secs_f64(secs / steps as f64));
            }
            for (k, &(x, y)) in last.iter().enumerate() {
                crate::input::set_finger(k, false, x, y);
            }
        });
    };
    nxconfig::ctl::register("pinch", "<x> <y> <r0> <r1> [seconds]: two fingers either side of (x, y) spread from r0 to r1 px (1280x720), 0.5 s by default", move |a| {
        let v: Vec<f64> = a.iter().take(4).filter_map(|s| s.parse().ok()).collect();
        if v.len() < 4 {
            return "pinch <x> <y> <r0> <r1> [seconds]\n".into();
        }
        let secs = a.get(4).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.5).clamp(0.05, 10.0);
        two(v[0], v[1], (v[2], v[3]), (0.0, 0.0), secs);
        format!("pinching at {} {}: {} -> {} px over {secs} s\n", v[0], v[1], v[2], v[3])
    });
    nxconfig::ctl::register("rotate", "<x> <y> <r> <deg0> <deg1> [seconds]: two fingers r px either side of (x, y) turned from deg0 to deg1, 0.5 s by default", move |a| {
        let v: Vec<f64> = a.iter().take(5).filter_map(|s| s.parse().ok()).collect();
        if v.len() < 5 {
            return "rotate <x> <y> <r> <deg0> <deg1> [seconds]\n".into();
        }
        let secs = a.get(5).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.5).clamp(0.05, 10.0);
        two(v[0], v[1], (v[2], v[2]), (v[3], v[4]), secs);
        format!("rotating at {} {}: {} -> {} degrees over {secs} s\n", v[0], v[1], v[3], v[4])
    });
}

fn register_gap_command() {
    nxconfig::ctl::register(
        "gaps",
        ": unimplemented services, commands, SVCs, imports and NVN calls met so far",
        |_| crate::log::gap_summary(),
    );
}

/// The stall dumps (the front loop's): presents stopped after the first one -> a thread dump after 6 s
/// (people close a frozen game before 15), then one every further 15 s without an image, at most 4 in a run; a new
/// image starts the count again. Not before the first image (the boot's loading is not a stall).
#[derive(Default)]
struct Stall {
    seen: u64,
    since: Option<Instant>,
    dumps: u32,
}

impl Stall {
    /// The seconds of the dump due now, if one is.
    fn tick(&mut self, presents: u64, now: Instant) -> Option<u32> {
        if presents != self.seen || self.since.is_none() {
            self.seen = presents;
            self.since = Some(now);
            return None;
        }
        let sec = if self.dumps == 0 { 6 } else { 15 };
        if presents == 0
            || self.dumps >= 4
            || now.duration_since(self.since?).as_secs() < sec as u64
        {
            return None;
        }
        self.dumps += 1;
        self.since = Some(now);
        Some(sec)
    }
}

/// Guest CPU per thread: each second, every guest thread's CPU time since the last second goes to
/// the metrics as `cpu_t<handle>_ms` and the total as `guest_cpu_ms` (metrics=1); the busiest threads of the last
/// 10 s are logged.
#[derive(Default)]
struct ThreadCpu {
    last: std::collections::HashMap<u32, f64>,
    window: std::collections::HashMap<u32, f64>,
    ticks: u32,
}

impl ThreadCpu {
    fn tick(&mut self) {
        let on = nxconfig::metrics::enabled();
        let mut total = 0.0;
        for (h, t) in nxrt::profile::thread_cpu() {
            let d = (t - self.last.insert(h, t).unwrap_or(t)).max(0.0);
            total += d;
            *self.window.entry(h).or_default() += d;
            if on {
                static NAMES: std::sync::Mutex<Vec<(u32, &'static std::sync::atomic::AtomicU64)>> =
                    std::sync::Mutex::new(Vec::new());
                let mut n = NAMES.lock().unwrap();
                let c = match n.iter().find(|x| x.0 == h) {
                    Some(x) => x.1,
                    None => {
                        let name: &'static str =
                            Box::leak(format!("cpu_t{h:x}_ms").into_boxed_str());
                        let c = nxconfig::metrics::counter(name);
                        n.push((h, c));
                        c
                    }
                };
                nxconfig::metrics::add(c, (d * 1000.0) as u64);
            }
        }
        if on {
            crate::metric!("guest_cpu_ms", (total * 1000.0) as u64);
        }
        self.ticks += 1;
        if self.ticks % 10 == 0 {
            let mut v: Vec<(u32, f64)> = self.window.drain().collect();
            v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let busy: Vec<String> = v
                .iter()
                .take(4)
                .filter(|x| x.1 > 0.05)
                .map(|(h, t)| format!("{h:#x} {:.0}%", t * 10.0))
                .collect();
            if !busy.is_empty() {
                crate::log!(
                    "cpu: busiest guest threads over 10 s (percent of a core): {}",
                    busy.join(", ")
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first stall dump after 6 s, then every 15 s, at most 4 in a run; a new image restarts the
    /// count; nothing before the first image.
    #[test]
    fn stall_dump_schedule() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut st = Stall::default();
        assert_eq!(st.tick(0, at(0.0)), None);
        assert_eq!(st.tick(0, at(30.0)), None, "no image yet: loading");
        assert_eq!(st.tick(5, at(30.0)), None);
        assert_eq!(st.tick(5, at(35.9)), None);
        assert_eq!(st.tick(5, at(36.0)), Some(6));
        assert_eq!(st.tick(5, at(50.0)), None);
        assert_eq!(st.tick(5, at(51.0)), Some(15));
        assert_eq!(st.tick(6, at(52.0)), None, "an image");
        assert_eq!(st.tick(6, at(66.9)), None);
        assert_eq!(
            st.tick(6, at(67.0)),
            Some(15),
            "the 6 s dump is only the first"
        );
        assert_eq!(st.tick(6, at(82.0)), Some(15));
        assert_eq!(st.tick(6, at(200.0)), None, "four dumps a run");
    }

    #[test]
    fn gaps_control_reports_live_counts() {
        crate::log::gap(crate::log::Gap::Command, "watch-test-command", "");
        register_gap_command();
        let reply = nxconfig::ctl::run("gaps");
        assert!(
            reply
                .lines()
                .next()
                .is_some_and(|l| l.starts_with("gaps distinct ") && l.contains(" hits ")),
            "{reply}"
        );
        assert!(
            reply.lines().any(|l| l == "Command\t1\twatch-test-command"),
            "{reply}"
        );
    }
}

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
    nxconfig::ctl::register("threads", ": every guest thread, what it waits for and its guest frames", |_| nxrt::kernel::dump_threads());
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
    let (mut seen, mut since, mut dumped) = (0u64, Instant::now(), false);
    loop {
        let p = crate::nvn::presents();
        if p != seen {
            (seen, since, dumped) = (p, Instant::now(), false);
        } else if p > 0 && !dumped && since.elapsed().as_secs() >= 15 {
            dumped = true;
            crate::log!("stall: no present for 15 s (frame {p}); the guest threads:");
            for l in nxrt::kernel::dump_threads().lines() {
                crate::log!("  {l}");
            }
        }
        std::thread::sleep(Duration::from_millis(100));
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
                crate::log!("memory: guest heap {} MB committed, physical maps {} MB", heap >> 20, alias >> 20);
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

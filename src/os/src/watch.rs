//! A host thread that watches the run: a heartbeat line with the guest's progress every few seconds
//! (`NXOS_HEARTBEAT` seconds, default 10, 0 = none), and the end of a measured run: `NXOS_EXIT_AFTER=<seconds>`
//! ends the process after that long, and `NXOS_EXIT_AFTER_SUBMIT=<seconds>` that long after the first NVN queue
//! submit, both with the gap summary.

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
    loop {
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
        }
        let done = after.is_some_and(|s| t0.elapsed().as_secs_f64() >= s)
            || after_submit.is_some_and(|s| crate::nvn::first_submit_age().is_some_and(|a| a >= s));
        if done {
            crate::log!("run ends here (NXOS_EXIT_AFTER / NXOS_EXIT_AFTER_SUBMIT)");
            crate::nvn::report();
            crate::log::report_gaps("end of run");
            nxrt::process::exit(0);
        }
    }
}

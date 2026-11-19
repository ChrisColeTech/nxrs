//! Named counters any part bumps (dropped work, waits, presents...), read once a second by the metrics writer
//! (`metrics=1`: `metrics-<time>.csv` beside the log) and by the control pipe's `counters` command. A counter is a
//! static atomic: bumping it costs one relaxed add.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static NAMES: Mutex<Vec<(&'static str, &'static AtomicU64)>> = Mutex::new(Vec::new());

/// The counter `name` (made on first use; keep the reference, the lookup takes a lock).
pub fn counter(name: &'static str) -> &'static AtomicU64 {
    let mut n = NAMES.lock().unwrap();
    if let Some((_, c)) = n.iter().find(|(k, _)| *k == name) {
        return c;
    }
    let c: &'static AtomicU64 = Box::leak(Box::new(AtomicU64::new(0)));
    n.push((name, c));
    c
}

pub fn add(c: &AtomicU64, n: u64) {
    c.fetch_add(n, Ordering::Relaxed);
}

/// Every counter's (name, value), in registration order.
pub fn snapshot() -> Vec<(&'static str, u64)> {
    NAMES.lock().unwrap().iter().map(|(k, c)| (*k, c.load(Ordering::Relaxed))).collect()
}

/// Whether metrics were asked for (`metrics=1` in the ini, or `NXRS_METRICS`).
pub fn enabled() -> bool {
    crate::flag("metrics") || std::env::var("NXRS_METRICS").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Per-frame rows (`frames-<time>.csv`): graphics pushes one line per present while `enabled()` (its own columns,
/// first a header line starting with `#`); the front end writes them.
static FRAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn frame(row: String) {
    let mut f = FRAMES.lock().unwrap();
    if f.len() < 100_000 {
        f.push(row);
    }
}

/// The rows pushed since the last call.
pub fn take_frames() -> Vec<String> {
    std::mem::take(&mut *FRAMES.lock().unwrap())
}

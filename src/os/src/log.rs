//! The os layer's log: `[os] ...` lines on stderr (the runtime writes `[nxrt]`), with a tag for lines that report a
//! gap (`UNIMPL`, `[RED]`), a once-per-key filter, and the summary of every gap met, printed at exit and on demand.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::sync::Mutex;
use std::sync::OnceLock;

static T0: OnceLock<std::time::Instant> = OnceLock::new();

/// Seconds since the os layer started, for the log's time column.
pub fn secs() -> f64 {
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

pub fn line(msg: &str) {
    let mut e = std::io::stderr().lock();
    let _ = e.write_all(format!("[os {:8.3}] {msg}\n", secs()).as_bytes());
}

#[macro_export]
macro_rules! log {
    ($($a:tt)*) => { $crate::log::line(&format!($($a)*)) };
}

static ONCE: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// True the first time `key` is seen.
pub fn first(key: &str) -> bool {
    let mut g = ONCE.lock().unwrap();
    g.get_or_insert_with(HashSet::new).insert(key.to_string())
}

#[macro_export]
macro_rules! log_once {
    ($key:expr, $($a:tt)*) => { if $crate::log::first(&$key) { $crate::log::line(&format!($($a)*)) } };
}

/// What kind of gap a guest call met.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Gap {
    /// An import no module and no host function provides (a logging stub answered 0).
    Import,
    /// A service the host does not serve (sm refused it).
    Service,
    /// A command of a served service with no handler (answered success, zeroed output).
    Command,
    /// An SVC the host has no handler for (answered success).
    Svc,
    /// An NVN entry point bound to the logging stub.
    Nvn,
    /// Anything else worth listing at exit.
    Other,
}

static GAPS: Mutex<BTreeMap<(Gap, String), u64>> = Mutex::new(BTreeMap::new());

/// Records a gap: the first time it is logged with `UNIMPL`, later hits are counted.
/// A metrics counter (`nxconfig::metrics`), looked up once per call site.
#[macro_export]
macro_rules! metric {
    ($name:literal, $n:expr) => {{
        static C: std::sync::OnceLock<&'static std::sync::atomic::AtomicU64> = std::sync::OnceLock::new();
        nxconfig::metrics::add(C.get_or_init(|| nxconfig::metrics::counter($name)), $n as u64);
    }};
}

pub fn gap(kind: Gap, what: &str, detail: &str) {
    // dropped work: every request answered by a stand-in (an unimplemented command, SVC or service)
    crate::metric!("gap_answers", 1);
    let mut g = GAPS.lock().unwrap();
    let n = g.entry((kind, what.to_string())).or_insert(0);
    *n += 1;
    if *n == 1 {
        drop(g);
        line(&format!("UNIMPL {kind:?} {what}{}{detail}", if detail.is_empty() { "" } else { ": " }));
    }
}

/// Every gap met so far, with its hit count.
pub fn gaps() -> Vec<(Gap, String, u64)> {
    GAPS.lock().unwrap().iter().map(|((k, w), n)| (*k, w.clone(), *n)).collect()
}

/// The summary block: one line per gap.
pub fn report_gaps(why: &str) {
    let g = gaps();
    line(&format!("gap summary ({why}): {} distinct", g.len()));
    for (k, w, n) in g {
        line(&format!("  gap {k:?} {w} x{n}"));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn once_and_gaps() {
        assert!(super::first("log-test-key"));
        assert!(!super::first("log-test-key"));
        super::gap(super::Gap::Other, "log-test-gap", "");
        super::gap(super::Gap::Other, "log-test-gap", "");
        assert!(super::gaps().iter().any(|(_, w, n)| w == "log-test-gap" && *n == 2));
    }
}

/// A diagnostic line from instrumented generated code (a debugging aid: `void nxos_dbg(uint64_t tag, ...6 values)`).
#[no_mangle]
pub extern "C" fn nxos_dbg(tag: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) {
    line(&format!("DBG {tag:#x}: {a:#x} {b:#x} {c:#x} {d:#x} {e:#x} {f:#x}"));
}

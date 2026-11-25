//! Opt-in frame-chain trace (`NXGFX_CHAIN_TRACE=<start s>[:<length s>]`, diagnosis): for a short stretch from that
//! log time on, the game's queue ops, the replay thread's work on them, their GPU completion and the game's sync waits
//! are noted with their times, then written to the log in one go (`[vk] chain ...` lines). It shows which fence a
//! game thread waits for and what the wait stands behind. Off: one `OnceLock` read per note.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

fn window() -> Option<(f64, f64)> {
    static W: OnceLock<Option<(f64, f64)>> = OnceLock::new();
    *W.get_or_init(|| {
        let v = std::env::var("NXGFX_CHAIN_TRACE").ok()?;
        let mut it = v.split(':');
        let s: f64 = it.next()?.trim().parse().ok()?;
        let n: f64 = it.next().and_then(|x| x.trim().parse().ok()).unwrap_or(0.6);
        Some((s, s + n))
    })
}

struct Ev {
    t: f64,
    th: u64,
    what: &'static str,
    a: u64,
    b: u64,
}

static EVS: Mutex<Vec<Ev>> = Mutex::new(Vec::new());
static NAMES: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
static DUMPED: AtomicBool = AtomicBool::new(false);
static NEXT_TH: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static TH: u64 = {
        let id = NEXT_TH.fetch_add(1, Ordering::Relaxed);
        let name = std::thread::current().name().unwrap_or("").to_string();
        NAMES.lock().unwrap().push((id, name));
        id
    };
}

/// Whether notes are being taken now.
pub fn on() -> bool {
    let Some((s, e)) = window() else { return false };
    let t = crate::log::elapsed();
    if t > e {
        dump();
        return false;
    }
    t >= s
}

/// Notes `what` with two values on the calling thread.
pub fn note(what: &'static str, a: u64, b: u64) {
    if !on() {
        return;
    }
    let t = crate::log::elapsed();
    let th = TH.with(|v| *v);
    EVS.lock().unwrap().push(Ev { t, th, what, a, b });
}

fn dump() {
    if DUMPED.swap(true, Ordering::AcqRel) {
        return;
    }
    let evs = std::mem::take(&mut *EVS.lock().unwrap());
    let names = NAMES.lock().unwrap().clone();
    crate::vklog!("chain trace: {} notes, threads {names:?}", evs.len());
    let t0 = evs.first().map(|e| e.t).unwrap_or(0.0);
    for e in evs {
        crate::vklog!(
            "chain {:9.3} th{} {} {:#x} {:#x}",
            (e.t - t0) * 1e3,
            e.th,
            e.what,
            e.a,
            e.b
        );
    }
}

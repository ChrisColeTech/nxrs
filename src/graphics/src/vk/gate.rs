//! The gate of parallel segment translation (MKW-RECORD step 4, `exec::run_segmented`).
//!
//! A stream's segments are translated at the same time on several threads, each into a child submission of its own.
//! Reading shared state (images, pools, tables) is safe from any of them. Changes of shared state come in two kinds:
//!
//! - First-use initialization whose commands all go into the child's upload buffer (`pass_pre`: a pool's buffer made
//!   or bound, CPU writes uploaded, an image made and given its first content, a lazy registration made, a twin
//!   copied, the waiting pool images prepared). Every upload buffer of a submission runs before all of its main
//!   buffers, as the serial path's single one does, and a dropped child's upload buffer is still submitted: any
//!   segment may make these.
//! - Changes whose effect depends on where in the stream they are made (`pass`: an image's first GPU write, which ends
//!   its uploads from storage; a storage upgrade, copied at its place in the main buffer). Only the oldest segment
//!   still being translated makes one, and only when every older segment's result stands: the *frontier*. Any other
//!   segment that needs one gives up at once (the caller changes nothing and returns what it would for a missing
//!   object), and the join translates it again on the replay thread, in order. A segment whose translation overlapped
//!   an older segment's such change may have read the state before it: its result is thrown away and translated again
//!   too (`Run::stands`). A segment that made one is never translated again.
//!
//! The gate never waits: a segment that may not change shared state gives up instead, so no thread ever waits while it
//! holds the images' or the pools' lock. `NXGFX_TRANSLATE_GATE=all` (reference mode): every call of a gated function
//! counts as a change.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

/// One segment's state in a parallel run.
#[derive(Clone, Copy, Default)]
struct Seg {
    /// 0 waiting, 1 being translated, 2 translated, 3 given up
    state: u8,
    /// the run's change epoch when its translation started
    start: u64,
    /// the epoch its changes ended at (0: it changed nothing)
    fin: u64,
    /// it may change shared state (it was the frontier when it first asked)
    changes: bool,
}

struct St {
    epoch: u64,
    /// segments [0, frontier) are translated and their results stand
    frontier: usize,
    /// the latest end of a change among them
    maxfin: u64,
    seg: Vec<Seg>,
}

/// The gate of one parallel run of a stream's segments.
pub struct Run {
    st: Mutex<St>,
}

thread_local! {
    /// the run and the segment this thread translates (null: not in a parallel run)
    static CTX: Cell<(*const Run, usize)> = const { Cell::new((core::ptr::null(), 0)) };
    /// changes of shared state counted on this thread (`change`), in parallel runs or not
    static CHANGES: Cell<u64> = const { Cell::new(0) };
    /// this thread's segment of a parallel run has given up
    static GIVEN_UP: Cell<bool> = const { Cell::new(false) };
}

/// Gate statistics, per frame in the `[vk] translate` line.
pub static ASKED: AtomicU64 = AtomicU64::new(0);
pub static GAVE_UP: AtomicU64 = AtomicU64::new(0);

/// `NXGFX_TRANSLATE_GATE=all`: every gated call counts as a change (correctness reference for A/B runs).
pub fn all() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_TRANSLATE_GATE").as_deref() == Ok("all"))
}

/// Whether this thread is translating a segment of a parallel run.
pub fn parallel() -> bool {
    CTX.with(|c| !c.get().0.is_null())
}

/// Changes counted on this thread so far (a serial translation compares two readings: did it change anything).
pub fn changes() -> u64 {
    CHANGES.with(|c| c.get())
}

/// Before a change of shared state; `need`: whether this call changes it (all calls count with
/// `NXGFX_TRANSLATE_GATE=all`). True: go on and change it. False: this thread's segment has given up (it will be
/// translated again); the caller changes nothing.
#[track_caller]
pub fn pass(need: bool) -> bool {
    if !need && !all() {
        return true;
    }
    CHANGES.with(|c| c.set(c.get() + 1));
    let (r, k) = CTX.with(|c| c.get());
    if r.is_null() {
        return true;
    }
    // SAFETY: CTX names a run only between `Run::enter` and `Run::leave` on this thread, while the run lives
    let run = unsafe { &*r };
    let ok = run.ask(k);
    let at = std::panic::Location::caller();
    let mut s = SITES.lock().unwrap();
    let e = s.entry((at.file(), at.line())).or_default();
    e.0 += 1;
    e.1 += !ok as u64;
    ok
}

/// Where parallel segments asked the gate: (file, line) -> (asked, gave up); the most asked in the translate line.
static SITES: Mutex<std::collections::BTreeMap<(&'static str, u32), (u64, u64)>> =
    Mutex::new(std::collections::BTreeMap::new());

/// The sites that asked most since the last call, as text (and forgets them).
pub fn sites() -> String {
    let mut v: Vec<_> = std::mem::take(&mut *SITES.lock().unwrap()).into_iter().collect();
    v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    v.iter()
        .take(6)
        .map(|((f, l), (n, u))| {
            format!("{}:{l} {n} ({u} gave up)", f.rsplit(['/', '\\']).next().unwrap_or(f))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Before a change of shared state whose commands all go into the submission's upload buffer (`Sub::pre`) and whose
/// result does not depend on when in the stream it is made: first-use initialization (a pool's buffer made or its
/// granules bound, CPU writes uploaded, an image made and given its first content, a lazy registration made, a twin
/// copied). Every upload buffer of a submission runs before all of its main buffers, as the serial path's single one
/// does, and a dropped child's upload buffer is still submitted, so any segment may make it. Gated as `pass` only in
/// the reference mode (`NXGFX_TRANSLATE_GATE=all`).
#[track_caller]
pub fn pass_pre(need: bool) -> bool {
    if all() {
        return pass(need);
    }
    true
}

/// Counts a change made without asking (a serial translation's: `changes`).
pub fn count() {
    CHANGES.with(|c| c.set(c.get() + 1));
}

/// Whether this thread's segment has given up (the translation of the rest of it is wasted).
pub fn given_up() -> bool {
    GIVEN_UP.with(|c| c.get())
}

impl Run {
    pub fn new(n: usize) -> Run {
        Run {
            st: Mutex::new(St {
                epoch: 0,
                frontier: 0,
                maxfin: 0,
                seg: vec![Seg::default(); n],
            }),
        }
    }

    /// This thread starts translating segment `k`.
    pub fn enter(&self, k: usize) {
        {
            let mut st = self.st.lock().unwrap();
            let e = st.epoch;
            let s = &mut st.seg[k];
            s.state = 1;
            s.start = e;
        }
        CTX.with(|c| c.set((self as *const Run, k)));
        GIVEN_UP.with(|c| c.set(false));
    }

    /// This thread is done with segment `k` (translated, or given up).
    pub fn leave(&self, k: usize) {
        CTX.with(|c| c.set((core::ptr::null(), 0)));
        GIVEN_UP.with(|c| c.set(false));
        crate::log::mute(false);
        let mut st = self.st.lock().unwrap();
        if st.seg[k].changes {
            st.epoch += 1;
            st.seg[k].fin = st.epoch;
        }
        if st.seg[k].state == 1 {
            st.seg[k].state = 2;
        }
        st.advance();
    }

    fn ask(&self, k: usize) -> bool {
        ASKED.fetch_add(1, Relaxed);
        let mut st = self.st.lock().unwrap();
        if st.seg[k].changes {
            return true;
        }
        if st.seg[k].state == 3 {
            return false;
        }
        if st.frontier == k && st.seg[k].start >= st.maxfin {
            st.seg[k].changes = true;
            return true;
        }
        st.seg[k].state = 3;
        GIVEN_UP.with(|c| c.set(true));
        // what the rest of this record skips because of it is not wrong: no red lines until `leave`
        crate::log::mute(true);
        GAVE_UP.fetch_add(1, Relaxed);
        false
    }

    /// At the join, in stream order: whether segment `k`'s parallel result stands. `later`: the latest change epoch of
    /// the segments before it (start at 0; `u64::MAX` once a segment translated again at the join changed shared state,
    /// which every parallel result was made before).
    pub fn stands(&self, k: usize, later: &mut u64) -> bool {
        let st = self.st.lock().unwrap();
        let s = st.seg[k];
        let ok = s.state == 2 && s.start >= *later;
        if ok {
            *later = (*later).max(s.fin);
        }
        ok
    }
}

impl St {
    fn advance(&mut self) {
        while self.frontier < self.seg.len() {
            let s = self.seg[self.frontier];
            if s.state != 2 || s.start < self.maxfin {
                break;
            }
            self.maxfin = self.maxfin.max(s.fin);
            self.frontier += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_in(r: &Run, k: usize, f: impl FnOnce()) {
        r.enter(k);
        f();
        r.leave(k);
    }

    /// The oldest segment may change shared state; a younger one running beside it may not, and gives up.
    #[test]
    fn only_the_frontier_changes() {
        let r = Run::new(3);
        r.enter(0);
        assert!(pass(true));
        r.leave(0);
        r.enter(2);
        assert!(!pass(true), "segment 1 is not translated yet");
        assert!(given_up());
        r.leave(2);
        run_in(&r, 1, || assert!(pass(false)));
        let mut later = 0;
        assert!(r.stands(0, &mut later));
        assert!(r.stands(1, &mut later), "1 started after 0's changes ended");
        assert!(!r.stands(2, &mut later), "2 gave up");
    }

    /// A segment translated while an older one changed shared state read the state before the change: it does not
    /// stand, and it may not change anything itself.
    #[test]
    fn overlap_with_an_older_change_is_stale() {
        let r = Run::new(2);
        r.enter(0);
        assert!(pass(true));
        // segment 1 starts on another thread while 0 is still being translated (simulated on this one)
        r.st.lock().unwrap().seg[1] = Seg { state: 1, start: 0, fin: 0, changes: false };
        r.leave(0);
        CTX.with(|c| c.set((&r as *const Run, 1)));
        assert!(!pass(true), "its start is before 0's change ended");
        r.leave(1);
        let mut later = 0;
        assert!(r.stands(0, &mut later));
        assert!(!r.stands(1, &mut later));
    }

    /// A segment that started after the older segments' changes ended stands, and may change state itself.
    #[test]
    fn a_segment_after_the_changes_stands() {
        let r = Run::new(2);
        run_in(&r, 0, || assert!(pass(true)));
        run_in(&r, 1, || assert!(pass(true)));
        let mut later = 0;
        assert!(r.stands(0, &mut later));
        assert!(r.stands(1, &mut later));
        // a change at the join (a segment translated again) makes every later parallel result stale
        let mut later = u64::MAX;
        assert!(!r.stands(1, &mut later));
    }

    /// Calls that change nothing pass without asking; outside a run every change passes.
    #[test]
    fn no_change_needs_no_gate() {
        let r = Run::new(2);
        r.enter(1);
        assert!(pass(false));
        assert!(!given_up());
        r.leave(1);
        let before = changes();
        assert!(pass(true));
        assert_eq!(changes(), before + 1);
    }
}

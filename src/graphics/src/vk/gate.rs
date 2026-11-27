//! The gate of parallel segment translation (MKW-RECORD step 4, `exec::run_segmented`).
//!
//! A stream's segments are translated at the same time on several threads, each into a child submission of its own.
//! Reading shared state (images, pools, tables) is safe from any of them. Changes of shared state come in two kinds:
//!
//! - First-use initialization whose commands all go into the upload buffer (`pass_pre`: a pool's buffer made or
//!   bound, CPU writes uploaded, an image made and given its first content, a lazy registration made, a twin copied,
//!   the waiting pool images prepared). The segments of a parallel run share one upload buffer, the parent's, recorded
//!   under `pre_lock` in the order the changes are made (they depend on each other: audit H1); it runs before every
//!   main buffer, as the serial path's single one does: any segment may make these.
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
    /// live images taken over by a new texture object in this run (`image::ensure_image`): old serial -> (the segment
    /// that took it over, the new serial). A segment before that one in the stream that uses the old object after the
    /// takeover uses the image where it is now (`taken_later`, audit M1).
    takeovers: Mutex<std::collections::HashMap<u64, (usize, u64)>>,
    /// texture views the run's segments looked up (`view_get` / `view_put`): key -> (the earliest segment that made
    /// it, the view), shared by its lanes as the serial path's one submission shares `Sub::memo_views`
    views: Mutex<std::collections::HashMap<ViewKey, (usize, u64, u64)>>,
    /// `St::epoch`, readable without its lock: a view kept at an older epoch is not used (a change of shared state
    /// since may have changed what the lookup finds)
    epoch_now: AtomicU64,
    /// (MKW-AHEAD) each segment's part, in a run over several (`with_parts`); empty: one submission
    parts: Vec<usize>,
}

/// `NXGFX_VIEW_SHARE=1`: later segments reuse views earlier segments looked up. Off by default (lead, 2026-11-26):
/// ~3 % of lookups hit, no measured gain, and it touches segment ordering (where the dark-turn fault came from).
fn share_views() -> bool {
    if cfg!(test) {
        return true; // the unit tests exercise the sharing itself
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_VIEW_SHARE").as_deref() == Ok("1"))
}

/// `exec::tex_view`'s memo key: (texture ID, type and image format, storage, the tables' generation).
pub type ViewKey = (i32, u32, bool, u64);

/// Views taken from the run's shared memo (`view_get`), and lookups that went on to the images (misses), for the
/// translate line.
pub static VIEW_HITS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// A view an earlier segment of this thread's run (or this one) looked up with this key: the serial translation of
/// the stream would have found it in its submission's memo at this point too (a segment after the one that made it).
/// None outside a run.
pub fn view_get(key: &ViewKey) -> Option<u64> {
    let (r, k) = seg_ctx();
    if r.is_null() || !share_views() {
        return None;
    }
    // SAFETY: as in `pass`
    let run = unsafe { &*r };
    let now = run.epoch_now.load(Relaxed);
    let v = run.views.lock().unwrap().get(key).filter(|e| e.0 <= k && e.2 == now).map(|e| e.1);
    VIEW_HITS[v.is_none() as usize].fetch_add(1, Relaxed);
    v
}

/// A view this thread's segment looked up (`view_get`'s other half). Not kept from a segment that gave up (it bound
/// a stand-in where the stream order may not), nor from one that overlapped an older segment's change of shared
/// state (its lookups may predate the change; it is translated again at the join).
pub fn view_put(key: ViewKey, view: u64) {
    let (r, k) = seg_ctx();
    if r.is_null() || given_up() || !share_views() {
        return;
    }
    // SAFETY: as in `pass`
    let run = unsafe { &*r };
    let epoch = {
        let st = run.st.lock().unwrap();
        // a parallel segment that started before the latest change (a segment translated again at the join runs
        // after every parallel one, in order: its lookups are current)
        if CTX.with(|c| !c.get().0.is_null()) && st.seg[k].start != st.epoch {
            return;
        }
        st.epoch
    };
    let mut m = run.views.lock().unwrap();
    match m.get(&key) {
        Some(e) if e.0 <= k && e.2 == epoch => {}
        _ => {
            m.insert(key, (k, view, epoch));
        }
    }
}

thread_local! {
    /// the run and the segment this thread translates (null: not in a parallel run)
    static CTX: Cell<(*const Run, usize)> = const { Cell::new((core::ptr::null(), 0)) };
    /// changes of shared state counted on this thread (`change`), in parallel runs or not
    static CHANGES: Cell<u64> = const { Cell::new(0) };
    /// this thread's segment of a parallel run has given up
    static GIVEN_UP: Cell<bool> = const { Cell::new(false) };
    /// the run and the segment translated again at its join on this thread (`Run::redo`): takeovers only
    static JOIN: Cell<(*const Run, usize)> = const { Cell::new((core::ptr::null(), 0)) };
}

/// The run and segment of this thread: a parallel segment's, or one translated again at the join.
fn seg_ctx() -> (*const Run, usize) {
    let c = CTX.with(|c| c.get());
    if !c.0.is_null() {
        return c;
    }
    JOIN.with(|c| c.get())
}

/// The live image of texture object `old` was taken over by object `new` (`image::ensure_image`) in this thread's
/// segment of a parallel run (or its translation again at the join).
pub fn note_takeover(old: u64, new: u64) {
    let (r, k) = seg_ctx();
    if r.is_null() {
        return;
    }
    // SAFETY: as in `pass`: the context names a run only while it lives
    let run = unsafe { &*r };
    run.takeovers.lock().unwrap().insert(old, (k, new));
}

/// The object that took over `old`'s image, when a later segment of this thread's run did: this thread's use of `old`
/// comes before the takeover in the stream, so it uses that image (audit M1).
pub fn taken_later(old: u64) -> Option<u64> {
    let (r, k) = seg_ctx();
    if r.is_null() {
        return None;
    }
    // SAFETY: as in `pass`
    let run = unsafe { &*r };
    let t = run.takeovers.lock().unwrap();
    t.get(&old).filter(|e| e.0 > k).map(|e| e.1)
}

/// Uses of a taken-over object's image by a segment before the takeover (`taken_later`), for the translate line.
pub static TAKEN_LATER: AtomicU64 = AtomicU64::new(0);

/// Gate statistics, per frame in the `[vk] translate` line.
pub static ASKED: AtomicU64 = AtomicU64::new(0);
pub static GAVE_UP: AtomicU64 = AtomicU64::new(0);
/// (MKW-AHEAD) segments of a later part that gave up at a first-use initialization (`pass_pre`)
pub static PRE_GAVE_UP: AtomicU64 = AtomicU64::new(0);

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
/// copied). A parallel run's segments record these into one shared upload buffer under `pre_lock` (taken by the
/// caller after this), in the order they are made, and it runs before all of the submission's main buffers, as the
/// serial path's single one does, so any segment may make it. Gated as `pass` only in the reference mode
/// (`NXGFX_TRANSLATE_GATE=all`).
///
/// (MKW-AHEAD) In a run over several parts of a chain, each part a submission of its own (`Run::with_parts`), a part's
/// upload buffer runs before its own main buffers only, after the parts before it: such a change is made only by a
/// segment of the oldest part still being translated (the frontier's); a segment of a later part gives up.
#[track_caller]
pub fn pass_pre(need: bool) -> bool {
    if all() {
        return pass(need);
    }
    let (r, k) = CTX.with(|c| c.get());
    if r.is_null() {
        return true;
    }
    // SAFETY: as in `pass`
    let run = unsafe { &*r };
    run.pre_ok(k)
}

/// The upload buffer every segment of a parallel run records into (audit H1). The first-use initializations depend on
/// each other (a pool uploaded whole, then a page of it the CPU wrote since; an image made and uploaded, then copied
/// into a twin): they must run in the order they were made, not in segment order. Each is made under the lock of what
/// it changes (the pools' or the images'), and its commands are recorded under this lock taken inside that one, so the
/// shared buffer holds them in the order they were made. Lock order: the images' or the pools' lock, then this; a
/// holder takes neither of those (nor this again: `pre_lock` counts nested takes on this thread).
static PRE_MU: Mutex<()> = Mutex::new(());

thread_local! {
    /// `pre_lock`s held on this thread (the outermost holds `PRE_MU`)
    static PRE_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Held while commands are recorded into the shared upload buffer (`pre_lock`).
pub struct PreGuard {
    counted: bool,
    _held: Option<std::sync::MutexGuard<'static, ()>>,
}

impl Drop for PreGuard {
    fn drop(&mut self) {
        if self.counted {
            PRE_DEPTH.with(|d| d.set(d.get() - 1));
        }
    }
}

/// Before recording into the upload buffer (`Sub::pre`) on a translation lane of a parallel run, after the gate passed
/// (`pass_pre`) and under the images' or the pools' lock: the run's segments share one upload buffer (`queue::share_pre`),
/// recorded by one thread at a time, in the order the changes are made. Nothing to hold outside a parallel run.
pub fn pre_lock() -> PreGuard {
    if !parallel() {
        return PreGuard {
            counted: false,
            _held: None,
        };
    }
    let d = PRE_DEPTH.with(|d| {
        let v = d.get();
        d.set(v + 1);
        v
    });
    PreGuard {
        counted: true,
        _held: (d == 0).then(|| lock(&PRE_MU, 3)),
    }
}

/// `PRE_MU` itself, for the replay thread making a child submission during a parallel run: its upload buffer comes from
/// the command pool of the shared one (`queue::new_sub`), which another lane may be recording into.
pub fn pre_pool_lock() -> std::sync::MutexGuard<'static, ()> {
    PRE_MU.lock().unwrap()
}

/// Whether this thread may record into a shared upload buffer now (outside a parallel run: always).
pub fn pre_held() -> bool {
    !parallel() || PRE_DEPTH.with(|d| d.get() > 0)
}

/// Time parallel segments waited for shared locks (diagnosis): the pool lock in `note_gpu_write`, in `resolve`, the
/// images' lock (views, targets), the shared upload buffer's (`pre_lock`), in ns.
pub static LOCK_NS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Takes `m`, timing the wait into `LOCK_NS[k]` on a translation lane.
pub fn lock<'a, T>(m: &'a Mutex<T>, k: usize) -> std::sync::MutexGuard<'a, T> {
    if !parallel() {
        return m.lock().unwrap();
    }
    if let Ok(g) = m.try_lock() {
        return g;
    }
    let t = std::time::Instant::now();
    let g = m.lock().unwrap();
    LOCK_NS[k].fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
    g
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
            takeovers: Mutex::new(Default::default()),
            views: Mutex::new(Default::default()),
            epoch_now: AtomicU64::new(0),
            parts: Vec::new(),
        }
    }

    /// (MKW-AHEAD) A run over the segments of several parts, each submitted on its own: `parts[k]` is segment `k`'s.
    pub fn with_parts(parts: Vec<usize>) -> Run {
        let mut r = Run::new(parts.len());
        r.parts = parts;
        r
    }

    /// `pass_pre` for segment `k`: always in a one-submission run; in a run over parts, only for a segment of the
    /// frontier's part (else it gives up, as `ask`).
    fn pre_ok(&self, k: usize) -> bool {
        if self.parts.is_empty() {
            return true;
        }
        let mut st = self.st.lock().unwrap();
        let f = st.frontier.min(self.parts.len() - 1);
        if self.parts[k] <= self.parts[f] && st.seg[k].state != 3 {
            return true;
        }
        PRE_GAVE_UP.fetch_add(1, Relaxed);
        if st.seg[k].state == 3 {
            return false;
        }
        st.seg[k].state = 3;
        drop(st);
        GIVEN_UP.with(|c| c.set(true));
        crate::log::mute(true);
        GAVE_UP.fetch_add(1, Relaxed);
        false
    }

    /// This thread translates segment `k` again at the join, in order (`taken_later` knows its place); `None` ends it.
    pub fn redo(&self, k: Option<usize>) {
        JOIN.with(|c| {
            c.set(match k {
                Some(k) => (self as *const Run, k),
                None => (core::ptr::null(), 0),
            })
        });
    }

    /// A segment translated again at the join changed shared state: views looked up before are not used again.
    pub fn changed_at_join(&self) {
        let mut st = self.st.lock().unwrap();
        st.epoch += 1;
        self.epoch_now.store(st.epoch, Relaxed);
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
            self.epoch_now.store(st.epoch, Relaxed);
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

    /// A view one segment looked up is found by itself and later segments of the run, never by an earlier one (the
    /// serial order would not have looked it up yet), nor after a change of shared state, nor from a segment that gave
    /// up; outside a run there is nothing shared.
    #[test]
    fn views_are_shared_with_later_segments_only() {
        let key = (7, 1, false, 3);
        assert_eq!(view_get(&key), None);
        let r = Run::new(4);
        run_in(&r, 1, || {
            assert_eq!(view_get(&key), None);
            view_put(key, 0x99);
            assert_eq!(view_get(&key), Some(0x99));
        });
        run_in(&r, 0, || assert_eq!(view_get(&key), None, "segment 0 comes before 1"));
        run_in(&r, 2, || assert_eq!(view_get(&key), Some(0x99)));
        // a segment that gave up keeps nothing
        let other = (8, 1, false, 3);
        let r2 = Run::new(2);
        r2.enter(1);
        assert!(!pass(true), "segment 0 is not translated yet");
        view_put(other, 0x55);
        r2.leave(1);
        run_in(&r2, 1, || assert_eq!(view_get(&other), None));
        // a change of shared state at the join: what was looked up before is not used again
        r.changed_at_join();
        run_in(&r, 2, || assert_eq!(view_get(&key), None));
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

    /// The shared upload buffer's lock (audit H1): held by one lane at a time while it records, taken again on the same
    /// thread without waiting, and not taken at all outside a parallel run.
    #[test]
    fn the_shared_upload_buffer_is_recorded_by_one_thread_at_a_time() {
        {
            let g = pre_lock();
            assert!(g._held.is_none(), "outside a run nothing is shared");
            assert!(pre_held());
        }
        let r = Run::new(2);
        r.enter(0);
        assert!(!pre_held(), "a lane records into the shared buffer only under its lock");
        {
            let a = pre_lock();
            assert!(a._held.is_some());
            let b = pre_lock();
            assert!(b._held.is_none(), "nested on the same thread");
            assert!(pre_held());
            assert!(PRE_MU.try_lock().is_err(), "another lane waits");
            drop(b);
            assert!(pre_held());
        }
        assert!(!pre_held());
        assert!(PRE_MU.try_lock().is_ok());
        r.leave(0);
    }

    /// A takeover made by a segment is seen by the segments before it in the stream only (audit M1), also when one is
    /// translated again at the join; outside a run there is none.
    #[test]
    fn a_takeover_is_seen_by_earlier_segments_only() {
        assert_eq!(taken_later(7), None);
        let r = Run::new(3);
        run_in(&r, 1, || note_takeover(7, 9));
        run_in(&r, 0, || assert_eq!(taken_later(7), Some(9), "segment 0 comes first: the image is 9's now"));
        run_in(&r, 1, || assert_eq!(taken_later(7), None, "its own takeover"));
        run_in(&r, 2, || assert_eq!(taken_later(7), None, "after the takeover: 7 is gone, as in order"));
        r.redo(Some(0));
        assert_eq!(taken_later(7), Some(9));
        r.redo(None);
        assert_eq!(taken_later(7), None);
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

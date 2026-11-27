//! Samples-passed (occlusion) reports: how far the replay may stay behind a game that reads them, and what the game
//! asks and when its answers land (OOT-OCC).
//!
//! A report's record {samples, time} is written into the game's memory when the submission that replays it completes
//! on the GPU (`queue.rs` `Post::OcclReport`). With the asynchronous replay that is up to `NXGFX_ASYNC_PRESENTS`
//! presents after the game queued it. `NXGFX_OCCL_LAG` is how many presents the replay may stay behind while a game uses
//! these reports (a report in the last 120 presents):
//!
//! * 0: each present waits until every op queued before it has been replayed (the rule MK8D's black stretches brought,
//!   `queue.rs` `occl_sync`): the game reads this frame's results in its next frame, as without asynchronous replay,
//!   but the game's thread and the replay thread take turns (Ocarina of Time's render thread spent 83 % of its time
//!   there, OOT-DESC).
//! * 1 (default): a present waits while one present is still queued: the game builds frame n + 1 while the replay
//!   translates frame n, and reads the results of frame n − 1 (the most recent ones completed), one frame later than
//!   with 0. `NXGFX_OCCL_SYNC=0` turns the wait off altogether (the asynchronous replay's own limits).
//!
//! Until a report has landed, its memory holds the previous answer at that address. When the game has rewritten that
//! memory itself since (to mark it pending), the previous answer is put back at submit (`NXGFX_OCCL_EARLY`, default 1:
//! a game polling for its result reads the most recent completed one instead of waiting for the replay); an address
//! never answered is left alone.
//!
//! The diagnosis line (every 600 presents while reports are in use) gives reports and resets a frame, the addresses,
//! the draws between a reset and its report, render conditions on report addresses, the share of zero results and of
//! results that changed between zero and non-zero, how many presents after its submission each result landed, and the
//! game's own rewrites of report memory.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// `NXGFX_OCCL_LAG`: presents the replay may stay behind while occlusion results are in use (0: none, the old rule).
pub fn lag() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NXGFX_OCCL_LAG")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1)
    })
}

/// `NXGFX_OCCL_EARLY`: the previous answer put back at submit over memory the game rewrote.
fn early() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("NXGFX_OCCL_EARLY").as_deref() != Ok("0"))
}

/// Whether a present must wait for the replay, `queued` presents being queued and `behind` ops not replayed: the
/// lag rule above (only while occlusion results are in use; the caller checks that).
pub fn present_waits(lag: u64, queued: u64, behind: u64) -> bool {
    if lag == 0 {
        behind > 0
    } else {
        queued >= lag
    }
}

/// One report address: the record last written there by us (landing or put back), its last landed samples, and the
/// guest presents at which its reports not landed yet were submitted.
#[derive(Default)]
struct Slot {
    written: Option<[u64; 2]>,
    landed: Option<u64>,
    sent: VecDeque<u64>,
}

#[derive(Default)]
struct Stats {
    reports: u64,
    resets: u64,
    conds: u64,
    draws_counted: u64,
    landed: u64,
    zero: u64,
    flips: u64,
    late: [u64; 4],
    rewrote_submit: u64,
    rewrote_land: u64,
    put_back: u64,
}

#[derive(Default)]
struct St {
    slots: HashMap<u64, Slot>,
    s: Stats,
    from: u64,
}

static ST: Mutex<Option<St>> = Mutex::new(None);
static PRESENTS: AtomicU64 = AtomicU64::new(0);
/// The game's presents' wait for the replay in `aq_wait_room` (ns, cumulative).
pub static PRESENT_WAIT_NS: AtomicU64 = AtomicU64::new(0);
/// The present wait and the replay thread's busy time at the last diagnosis line.
static LAST_NS: Mutex<(u64, u64)> = Mutex::new((0, 0));

/// The records of one submission, read on the game's thread before it is queued: its samples-passed reports, resets,
/// draws and render conditions in order (`op` codes of `nvn::obj::op`).
#[derive(Default)]
pub struct Walk {
    reports: Vec<u64>,
    resets: u64,
    conds: Vec<u64>,
    draws: u64,
    draws_counted: u64,
    counting: bool,
}

impl Walk {
    pub fn record(&mut self, opc: u32, r: &[u8]) {
        use crate::nvn::obj::op;
        let at = |o: usize| -> u64 {
            r.get(o..o + 8)
                .map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
        };
        match opc {
            op::RESET_COUNTER if at(8) as u32 == crate::nvn::types::counter::SAMPLES_PASSED => {
                self.resets += 1;
                self.counting = true;
                self.draws = 0;
            }
            op::REPORT_COUNTER if at(8) as u32 == crate::nvn::types::counter::SAMPLES_PASSED => {
                self.reports.push(at(16));
                self.draws_counted += self.draws;
                self.draws = 0;
            }
            op::RENDER_COND => {
                let a = at(16);
                if a != 0 {
                    self.conds.push(a);
                }
            }
            op::DRAW | op::DRAW_IDX | op::MULTI_DRAW | op::DRAW_TEX | op::MESH if self.counting => {
                self.draws += 1
            }
            _ => {}
        }
    }
}

/// A submission's walk, on the game's thread before the submission is queued: notes its reports and puts the previous
/// answer back where the game rewrote report memory (`NXGFX_OCCL_EARLY`).
pub fn submitted(w: &Walk) {
    if w.reports.is_empty() && w.resets == 0 {
        return;
    }
    let now = PRESENTS.load(Ordering::Relaxed);
    let mut g = ST.lock().unwrap();
    let st = g.get_or_insert_with(St::default);
    st.s.reports += w.reports.len() as u64;
    st.s.resets += w.resets;
    st.s.draws_counted += w.draws_counted;
    st.s.conds += w.conds.iter().filter(|a| st.slots.contains_key(a)).count() as u64;
    for &a in &w.reports {
        if a == 0 {
            continue;
        }
        if st.slots.len() >= 1 << 16 && !st.slots.contains_key(&a) {
            continue;
        }
        let slot = st.slots.entry(a).or_default();
        if let Some(rec) = slot.written {
            // SAFETY: the game's report memory (16 bytes; the replay writes it the same way)
            let cur = unsafe { core::ptr::read_unaligned(a as *const [u64; 2]) };
            if cur != rec {
                st.s.rewrote_submit += 1;
                if early() {
                    super::counters::write_record(a, rec[0], rec[1]);
                    st.s.put_back += 1;
                }
            }
        }
        if slot.sent.len() >= 16 {
            slot.sent.pop_front();
        }
        slot.sent.push_back(now);
    }
}

/// A report at `addr` lands with `samples` and time `t` (the completion thread, instead of writing it itself).
pub fn land(addr: u64, samples: u64, t: u64) {
    let mut g = ST.lock().unwrap();
    let st = g.get_or_insert_with(St::default);
    let now = PRESENTS.load(Ordering::Relaxed);
    let slot = st.slots.entry(addr).or_default();
    if let Some(rec) = slot.written {
        // SAFETY: as in `submitted`
        let cur = unsafe { core::ptr::read_unaligned(addr as *const [u64; 2]) };
        if cur != rec {
            st.s.rewrote_land += 1;
        }
    }
    super::counters::write_record(addr, samples, t);
    slot.written = Some([samples, t]);
    if let Some(prev) = slot.landed {
        if (prev == 0) != (samples == 0) {
            st.s.flips += 1;
        }
    }
    slot.landed = Some(samples);
    if let Some(sent) = slot.sent.pop_front() {
        st.s.late[(now.saturating_sub(sent) as usize).min(3)] += 1;
    }
    st.s.landed += 1;
    st.s.zero += (samples == 0) as u64;
}

/// Every guest present (the game's thread, nvnQueuePresentTexture): the diagnosis line every 600 presents.
pub fn note_present() {
    let n = PRESENTS.fetch_add(1, Ordering::Relaxed) + 1;
    if n % 600 != 0 {
        return;
    }
    let mut g = ST.lock().unwrap();
    let Some(st) = g.as_mut() else { return };
    let frames = (n - st.from).max(1) as f64;
    let s = std::mem::take(&mut st.s);
    st.from = n;
    let (wait, busy) = (
        PRESENT_WAIT_NS.load(Ordering::Relaxed),
        super::queue::aq_ns(),
    );
    let (w0, b0) = std::mem::replace(&mut *LAST_NS.lock().unwrap(), (wait, busy));
    if s.reports == 0 && s.landed == 0 {
        return;
    }
    let pct = |a: u64, b: u64| 100.0 * a as f64 / b.max(1) as f64;
    crate::vklog!(
        "occlusion, per frame over the last {frames:.0}: presents waited {:.2} ms for the replay, the replay thread busy {:.2} ms; {:.1} samples-passed reports at {} addresses in all, {:.1} resets, {:.1} draws counted a report, {:.1} render conditions on report addresses; landed {:.1}: {:.0} % zero, {:.1} % changed between zero and not; landed after 0 / 1 / 2 / 3+ presents: {} / {} / {} / {}; the game rewrote report memory before its submit {} times ({} put back), before its landing {} (NXGFX_OCCL_LAG={}, OCCL_EARLY {})",
        (wait - w0) as f64 / 1e6 / frames,
        (busy - b0) as f64 / 1e6 / frames,
        s.reports as f64 / frames,
        st.slots.len(),
        s.resets as f64 / frames,
        s.draws_counted as f64 / s.reports.max(1) as f64,
        s.conds as f64 / frames,
        s.landed as f64 / frames,
        pct(s.zero, s.landed),
        pct(s.flips, s.landed),
        s.late[0],
        s.late[1],
        s.late[2],
        s.late[3],
        s.rewrote_submit,
        s.put_back,
        s.rewrote_land,
        lag(),
        early() as u8
    );
}

#[cfg(test)]
mod tests {
    /// Lag 0 waits until nothing is behind; lag n waits while n presents are queued.
    #[test]
    fn the_lag_decides_when_a_present_waits() {
        assert!(super::present_waits(0, 0, 1));
        assert!(!super::present_waits(0, 2, 0));
        assert!(!super::present_waits(1, 0, 40), "the previous frame replayed: the game goes on");
        assert!(super::present_waits(1, 1, 3), "the previous frame still queued");
        assert!(!super::present_waits(2, 1, 3));
    }

    /// A landed record is put back at the next submit only where the game rewrote it, and never-answered memory is
    /// left alone.
    #[test]
    fn the_previous_answer_is_put_back_over_a_rewrite() {
        let mut mem = Box::new([0u64; 4]);
        let a = mem.as_mut_ptr() as u64;
        let b = a + 16;
        let walk = |addrs: &[u64]| {
            let mut w = super::Walk::default();
            for &x in addrs {
                let mut r = vec![0u8; 24];
                r[8..16].copy_from_slice(
                    &(crate::nvn::types::counter::SAMPLES_PASSED as u64).to_le_bytes(),
                );
                r[16..24].copy_from_slice(&x.to_le_bytes());
                w.record(crate::nvn::obj::op::REPORT_COUNTER, &r);
            }
            w
        };
        super::submitted(&walk(&[a, b]));
        mem[2] = 7;
        super::submitted(&walk(&[b]));
        assert_eq!(mem[2], 7, "never answered: left alone");
        super::land(a, 1234, 99);
        assert_eq!(mem[0..2], [1234, 99]);
        super::submitted(&walk(&[a]));
        assert_eq!(mem[0..2], [1234, 99], "not rewritten: nothing to do");
        mem[0] = 0;
        mem[1] = 0;
        super::submitted(&walk(&[a]));
        if super::early() {
            assert_eq!(mem[0..2], [1234, 99], "rewritten by the game: the previous answer is back");
        }
        super::land(a, 0, 100);
        assert_eq!(mem[0..2], [0, 100]);
    }
}

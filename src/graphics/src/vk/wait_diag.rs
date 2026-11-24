//! Opt-in CPU-side wait ledger (NXGFX_WAIT_DIAG=1). No driver calls or guest-memory reads on the observer.
//! Each mark is synchronized; separate subsystem snapshots are observations, not one atomic global snapshot.
//! At most eight reports, four waits and eight parked queues per report. Never resolves a fence or retries work.

use super::{Gpu, AQ, ASYNC, RECEIPT};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_WAIT_DIAG").as_deref() == Ok("1"))
}

#[derive(Clone, Debug)]
struct Mark {
    phase: &'static str,
    at: Instant,
    // Queue/sequence for the async op; queue/submission for recording; fence/0 for completion.
    ids: [u64; 2],
    record: Option<(usize, u32, Option<[u8; 8]>)>,
}

#[derive(Clone, Debug)]
struct Waiting {
    id: u64,
    sync: usize,
    timeout: u64,
    fence: Option<u64>,
    phase: &'static str,
    at: Instant,
}

#[derive(Default)]
struct State {
    next: u64,
    waits: Vec<Waiting>,
    marks: [Option<Mark>; 3],
}

static STATE: Mutex<State> = Mutex::new(State {
    next: 0,
    waits: Vec::new(),
    marks: [None, None, None],
});

/// Async op / recording / completion are separate lanes: completion can advance during replay.
pub fn mark(lane: usize, phase: &'static str, ids: [u64; 2]) {
    if enabled() {
        STATE.lock().unwrap().marks[lane] = Some(Mark {
            phase,
            at: Instant::now(),
            ids,
            record: None,
        });
    }
}

pub fn phase(phase: &'static str) {
    if enabled() {
        if let Some(m) = &mut STATE.lock().unwrap().marks[1] {
            m.phase = phase;
            m.at = Instant::now();
        }
    }
}

pub fn record(offset: usize, op: u32, cs: Option<[u8; 8]>) {
    if enabled() {
        if let Some(m) = &mut STATE.lock().unwrap().marks[1] {
            m.phase = "record";
            m.at = Instant::now();
            m.record = Some((offset, op, cs));
        }
    }
}

pub struct Wait(Option<u64>);

impl Wait {
    pub fn new(sync: usize, timeout: u64) -> Self {
        if !enabled() || timeout == 0 {
            return Self(None);
        }
        let id = {
            let mut s = STATE.lock().unwrap();
            if s.waits.len() == 16 {
                return Self(None);
            }
            s.next += 1;
            let id = s.next;
            s.waits.push(Waiting {
                id,
                sync,
                timeout,
                fence: None,
                phase: "enter",
                at: Instant::now(),
            });
            id
        };
        static START: Once = Once::new();
        START.call_once(|| {
            if let Err(e) = std::thread::Builder::new()
                .name("nvn wait diagnostic".into())
                .spawn(observe)
            {
                crate::vklog!("wait-diag: observer could not start: {e}");
            }
        });
        Self(Some(id))
    }

    pub fn set(&self, phase: &'static str, fence: Option<u64>) {
        if let Some(id) = self.0 {
            if let Some(w) = STATE.lock().unwrap().waits.iter_mut().find(|w| w.id == id) {
                w.phase = phase;
                w.fence = fence;
            }
        }
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        if let Some(id) = self.0 {
            STATE.lock().unwrap().waits.retain(|w| w.id != id);
        }
    }
}

fn observe() {
    let mut reports = 0;
    while reports < 8 {
        std::thread::sleep(Duration::from_secs(2));
        let (waits, marks) = {
            let Ok(s) = STATE.try_lock() else { continue };
            let waits: Vec<_> = s
                .waits
                .iter()
                .filter(|w| w.at.elapsed() >= Duration::from_secs(2))
                .take(4)
                .cloned()
                .collect();
            (waits, s.marks.clone())
        };
        if waits.is_empty() {
            continue;
        }
        let Some(g) = Gpu::try_get() else { continue };
        reports += 1;
        let last = g.q.last_fence();
        let done = g.q.done.try_lock().ok().map(|d| *d);
        let aq = AQ.get().map(|a| {
            (
                a.tail.load(std::sync::atomic::Ordering::Acquire),
                a.done.load(std::sync::atomic::Ordering::Acquire),
            )
        });
        let recording = g.q.recording.load(std::sync::atomic::Ordering::Acquire);
        let mem_free = g.mem.try_lock().is_ok();
        let images_free = g.images.try_lock().is_ok();
        let vkq_free = g.q.vkq.try_lock().is_ok();
        crate::vklog!("wait-diag {reports}/8: last={last} done={done:?} aq(tail,done)={aq:?} recording={recording} locks_available mem={mem_free} images={images_free} vkq={vkq_free}");
        for (lane, m) in marks.iter().enumerate() {
            if let Some(m) = m {
                crate::vklog!(
                    "wait-diag lane={lane} phase={} age_ms={} ids={:x?} record(offset,op,cs)={:x?}",
                    m.phase,
                    m.at.elapsed().as_millis(),
                    m.ids,
                    m.record
                );
            }
        }
        for w in waits {
            // The fence is the waiter's own saved value, never a fresh read of a mutable guest sync.
            let mut target = w.fence;
            if let Some(f) = target.filter(|f| f & RECEIPT == 0 && f & ASYNC != 0) {
                target = AQ.get().and_then(|a| {
                    a.fences
                        .try_lock()
                        .ok()
                        .and_then(|m| m.get(&(f & !ASYNC)).copied())
                });
            }
            let physical = target.and_then(|f| {
                if f & RECEIPT == 0 {
                    Some(f)
                } else {
                    g.q.receipts
                        .try_lock()
                        .ok()
                        .and_then(|m| m.get(&f).copied())
                }
            });
            crate::vklog!("wait-diag sync={:#x} phase={} age_ms={} timeout={:#x} saved_fence={:x?} exact_target={target:x?} physical={physical:?} (None: unresolved, evicted, or lock busy)", w.sync, w.phase, w.at.elapsed().as_millis(), w.timeout, w.fence);
        }
        // Never hold an operational lock while logging or taking another operational lock.
        let parked = g.q.sub.try_lock().ok().map(|s| {
            (
                s.parked.len(),
                s.parked
                    .iter()
                    .take(8)
                    .map(|(q, p)| {
                        // The first 24 bytes include a WAIT_EVENT's address/mode/value, without touching that address.
                        (
                            *q,
                            p.wait_receipt,
                            p.receipts.iter().take(8).copied().collect::<Vec<_>>(),
                            p.stream.len(),
                            p.stream.iter().take(24).copied().collect::<Vec<_>>(),
                            p.presents.len(),
                            p.later.len(),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        });
        let signals =
            g.q.pending_signals
                .try_lock()
                .ok()
                .map(|s| s.iter().take(8).map(|(a, v)| (*a, *v)).collect::<Vec<_>>());
        crate::vklog!("wait-diag parked(count,queue,wait,receipts,bytes,head,presents,later)={parked:x?}; pending_signals={signals:x?}");
    }
}

//! Hot pools: a mirrored pool the CPU rewrites and the GPU writes on most frames becomes imported host memory.
//!
//! A Mirror pays for every CPU write (the write watch takes the page, `upskip` hashes it, a changed page is uploaded)
//! and for every GPU write (the range is compared on the GPU, its changed pages copied back and merged into guest
//! memory at the fence, under the pool lock the replay thread's syncs wait on). Ocarina of Time rewrites ~57 MB a frame
//! of pages with the bytes the GPU copy already holds, inside ~67 MB a frame of ranges its shaders write (OOT-FPS,
//! OOT-REPLAY2): hashing, comparing and landing all of it every frame. Imported (`Placement::Host`,
//! VK_EXT_external_memory_host) the GPU reads and writes the guest's pages themselves, as the console's GPU does: no
//! upload, no hash, no write-back. The write watch still asks such a pool (the texture check needs CPU-written texture
//! bytes); GPU writes into imported pages are not write-watch writes (DIRECT-MEM's GPU test).
//!
//! What decides: per pool, the frames of the last 64 in which the CPU wrote at least `cpu_kb` of it and the frames in
//! which a write-back of it landed. A pool over both counts (`cpu_frames`, `wb_frames`), at least `min_kb` large and
//! at most `max_mb`, is switched at the next sync that finds none of its write-backs in flight (`mem`). Pools the CPU
//! writes but the GPU only reads stay mirrored by default (`wb_frames`): DIRECT-MEM measured that per-draw constant
//! pools read over PCIe cost more GPU time than their sync saved (MKW +3.3 ms a frame).
//!
//! OoT's are whole pools of 6-15 MB (flags 0x24, CPU and GPU cached, as 3.3 GB of its other pools: the flags cannot
//! tell them apart), several of them written every other frame (double-buffered): hence 24 of 64, not a majority.
//!
//! `NXGFX_HOST_HOT=0`: no pool switches (every pool stays as placed). `NXGFX_HOST_HOT_FRAMES` (24), `_WB_FRAMES` (16),
//! `_CPU_KB` (256), `_MIN_KB` (1024), `_MAX_MB` (256): the thresholds. Each switch and each refusal is logged.

use std::sync::atomic::{AtomicU64, Ordering};

/// The rule's thresholds.
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    /// frames of the last 64 with at least `cpu_kb` of CPU writes
    pub cpu_frames: u32,
    /// frames of the last 64 with a write-back landed (0: not asked)
    pub wb_frames: u32,
    pub cpu_kb: u64,
    pub min_kb: u64,
    pub max_mb: u64,
}

impl Default for Rule {
    fn default() -> Rule {
        Rule { cpu_frames: 24, wb_frames: 16, cpu_kb: 256, min_kb: 1024, max_mb: 256 }
    }
}

fn env_u64(k: &str, d: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

/// The rule, None when switched off (`NXGFX_HOST_HOT=0`).
pub fn rule() -> Option<Rule> {
    static R: std::sync::OnceLock<Option<Rule>> = std::sync::OnceLock::new();
    *R.get_or_init(|| {
        if std::env::var("NXGFX_HOST_HOT").as_deref() == Ok("0") {
            return None;
        }
        let d = Rule::default();
        Some(Rule {
            cpu_frames: env_u64("NXGFX_HOST_HOT_FRAMES", d.cpu_frames as u64).min(64) as u32,
            wb_frames: env_u64("NXGFX_HOST_HOT_WB_FRAMES", d.wb_frames as u64).min(64) as u32,
            cpu_kb: env_u64("NXGFX_HOST_HOT_CPU_KB", d.cpu_kb),
            min_kb: env_u64("NXGFX_HOST_HOT_MIN_KB", d.min_kb),
            max_mb: env_u64("NXGFX_HOST_HOT_MAX_MB", d.max_mb),
        })
    })
}

/// One pool's recent history: bit k of a mask is the frame `k` frames before `frame`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Heat {
    frame: u64,
    /// CPU-written bytes in `frame`
    cur: u64,
    /// frames with at least the rule's `cpu_kb` of CPU writes
    cpu: u64,
    /// frames with a write-back landed
    wb: u64,
    /// a switch was tried (made or refused): never tried again
    pub tried: bool,
}

fn aged(mask: u64, from: u64, to: u64) -> u64 {
    match to.saturating_sub(from) {
        d if d >= 64 => 0,
        d => mask << d,
    }
}

impl Heat {
    fn roll(&mut self, frame: u64) {
        if frame > self.frame {
            self.cpu = aged(self.cpu, self.frame, frame);
            self.wb = aged(self.wb, self.frame, frame);
            self.cur = 0;
            self.frame = frame;
        }
    }

    /// The CPU wrote `bytes` of the pool, found at a sync in `frame`.
    pub fn note_cpu(&mut self, frame: u64, bytes: u64, r: &Rule) {
        self.roll(frame);
        if frame < self.frame {
            return;
        }
        self.cur += bytes;
        if self.cur >= r.cpu_kb << 10 {
            self.cpu |= 1;
        }
    }

    /// A write-back of the pool landed in `frame`.
    pub fn note_wb(&mut self, frame: u64) {
        self.roll(frame);
        if frame == self.frame {
            self.wb |= 1;
        }
    }

    /// (frames with CPU writes, frames with write-backs) of the 64 up to `frame`.
    pub fn counts(&self, frame: u64) -> (u32, u32) {
        (
            aged(self.cpu, self.frame, frame).count_ones(),
            aged(self.wb, self.frame, frame).count_ones(),
        )
    }

    /// Whether a pool of `size` bytes with this history is to be imported at `frame` (`buf_max`: the device's largest
    /// buffer; an import is one buffer).
    pub fn hot(&self, frame: u64, size: u64, buf_max: u64, r: &Rule) -> bool {
        if self.tried || size < r.min_kb << 10 || size > r.max_mb << 20 || size > buf_max {
            return false;
        }
        let (c, w) = self.counts(frame);
        c >= r.cpu_frames.max(1) && w >= r.wb_frames
    }
}

/// Pools switched, imports refused, and the bytes switched (the pools line).
pub static SWITCHED: AtomicU64 = AtomicU64::new(0);
pub static SWITCHED_B: AtomicU64 = AtomicU64::new(0);
pub static REFUSED: AtomicU64 = AtomicU64::new(0);
/// Syncs that found a hot pool with its write-back in flight (tried again at the next).
pub static WAITED: AtomicU64 = AtomicU64::new(0);

/// The pools line's part.
pub fn summary() -> String {
    if rule().is_none() {
        return String::new();
    }
    format!(
        "; hot pools imported: {} ({} MB), {} refused, {} syncs found one in flight",
        SWITCHED.load(Ordering::Relaxed),
        SWITCHED_B.load(Ordering::Relaxed) >> 20,
        REFUSED.load(Ordering::Relaxed),
        WAITED.load(Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1 << 20;

    #[test]
    fn a_pool_the_cpu_and_the_gpu_write_on_most_frames_is_hot() {
        let r = Rule::default();
        let mut h = Heat::default();
        for f in 100..140 {
            h.note_cpu(f, 300 << 10, &r);
            if f % 2 == 0 {
                h.note_wb(f);
            }
        }
        assert_eq!(h.counts(139), (40, 20));
        assert!(h.hot(139, 16 * MB, 4096 * MB, &r));
        // a later look: the frames age out of the 64
        assert!(!h.hot(139 + 40, 16 * MB, 4096 * MB, &r));
        assert!(!h.hot(139 + 64, 16 * MB, 4096 * MB, &r));
        assert_eq!(h.counts(139 + 64), (0, 0));
    }

    #[test]
    fn small_writes_constant_pools_and_cpu_only_pools_stay_mirrored() {
        let r = Rule::default();
        // CPU writes under the per-frame floor, even from several syncs of one frame, until they add up
        let mut h = Heat::default();
        for f in 0..64 {
            h.note_cpu(f, 100 << 10, &r);
            h.note_wb(f);
        }
        assert_eq!(h.counts(63).0, 0, "100 KB a frame is under the floor");
        for f in 64..128 {
            h.note_cpu(f, 100 << 10, &r);
            h.note_cpu(f, 100 << 10, &r);
            h.note_cpu(f, 100 << 10, &r);
            h.note_wb(f);
        }
        assert_eq!(h.counts(127), (64, 64), "three syncs of 100 KB in one frame add up");
        assert!(h.hot(127, 2 * MB, 4096 * MB, &r));
        // too small (per-draw constants), too large, larger than one buffer
        assert!(!h.hot(127, 512 << 10, 4096 * MB, &r));
        assert!(!h.hot(127, 300 * MB, 4096 * MB, &r));
        assert!(!h.hot(127, 64 * MB, 32 * MB, &r));
        // the CPU writes it every frame, the GPU never: stays
        let mut c = Heat::default();
        for f in 0..64 {
            c.note_cpu(f, MB, &r);
        }
        assert!(!c.hot(63, 8 * MB, 4096 * MB, &r));
        // unless the rule does not ask for write-backs
        assert!(c.hot(63, 8 * MB, 4096 * MB, &Rule { wb_frames: 0, ..r }));
    }

    #[test]
    fn a_pool_tried_once_is_not_tried_again() {
        let r = Rule::default();
        let mut h = Heat::default();
        for f in 0..64 {
            h.note_cpu(f, MB, &r);
            h.note_wb(f);
        }
        assert!(h.hot(63, 8 * MB, 4096 * MB, &r));
        h.tried = true;
        assert!(!h.hot(63, 8 * MB, 4096 * MB, &r));
    }

    #[test]
    fn notes_of_an_older_frame_do_not_count_for_a_newer_one() {
        let r = Rule::default();
        let mut h = Heat::default();
        h.note_cpu(10, MB, &r);
        h.note_cpu(9, MB, &r);
        h.note_wb(9);
        assert_eq!(h.counts(10), (1, 0));
    }
}

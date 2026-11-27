//! Render pass segments (WEB-W10c). Vulkan lets a pass sample its own depth/stencil attachment while its draws do
//! not write that aspect; WebGPU allows it only on an attachment declared read-only for the aspect
//! (`depthReadOnly` / `stencilReadOnly`, no load/store ops, no pipeline writing it), and a pass is one
//! synchronization scope. The replay therefore plays a Vulkan pass as segments: a segment that samples an aspect and
//! never writes it gets that aspect read-only; a draw that would mix the two in one segment ends it, and a
//! continuation pass (every attachment loaded, nothing cleared) carries on with the same attachments.
//!
//! Aspects are indexed 0 (depth) and 1 (stencil). The functions on the `PASS_BEGIN` payload edit it in place:
//! `[colour count][per colour: view, resolve, clear, store, int, clear value x4][view, depth mode, stencil mode,
//! depth clear, stencil clear]`, all 32-bit words; a mode is 0 (no such aspect), 1 load, 2 clear, 3 read-only.

/// What one segment has done to the pass's depth/stencil attachment, per aspect.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub sampled: [bool; 2],
    pub written: [bool; 2],
}

pub const MODE_LOAD: u32 = 1;
pub const MODE_CLEAR: u32 = 2;
pub const MODE_READ_ONLY: u32 = 3;

const COLOR_BYTES: usize = 36;

impl Seg {
    /// A segment starting with these modes (a clear writes its aspect).
    pub fn starting(modes: [u32; 2]) -> Seg {
        Seg { sampled: [false; 2], written: modes.map(|m| m == MODE_CLEAR) }
    }

    /// Whether a draw (or clear) sampling `samples` and writing `writes` must start a new segment first: it samples
    /// an aspect this segment wrote, or writes one it sampled.
    pub fn conflicts(&self, samples: [bool; 2], writes: [bool; 2]) -> bool {
        (0..2).any(|i| (samples[i] && self.written[i]) || (writes[i] && self.sampled[i]))
    }

    pub fn add(&mut self, samples: [bool; 2], writes: [bool; 2]) {
        for i in 0..2 {
            self.sampled[i] |= samples[i];
            self.written[i] |= writes[i];
        }
    }

    /// The aspects to declare read-only when the segment ends: sampled, never written.
    pub fn read_only(&self) -> [bool; 2] {
        [0, 1].map(|i| self.sampled[i] && !self.written[i])
    }
}

fn word(p: &[u8], at: usize) -> u32 {
    p.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn set_word(p: &mut [u8], at: usize, v: u32) {
    if let Some(b) = p.get_mut(at..at + 4) {
        b.copy_from_slice(&v.to_le_bytes());
    }
}

fn colors(p: &[u8]) -> usize {
    word(p, 0) as usize
}

/// Byte offset of the depth mode word (the stencil mode follows it).
fn ds_modes_at(p: &[u8]) -> usize {
    4 + colors(p) * COLOR_BYTES + 4
}

/// The depth and stencil modes of a pass begin payload.
pub fn modes(p: &[u8]) -> [u32; 2] {
    let at = ds_modes_at(p);
    [word(p, at), word(p, at + 4)]
}

/// Declares the aspects `ro` read-only (those the attachment has).
pub fn mark_read_only(p: &mut [u8], ro: [bool; 2]) {
    let at = ds_modes_at(p);
    for (i, r) in ro.into_iter().enumerate() {
        if r && word(p, at + 4 * i) != 0 {
            set_word(p, at + 4 * i, MODE_READ_ONLY);
        }
    }
}

/// Every colour attachment stored: a segment followed by a continuation keeps what it drew for it.
pub fn keep_stores(p: &mut [u8]) {
    for c in 0..colors(p) {
        set_word(p, 4 + c * COLOR_BYTES + 12, 1);
    }
}

/// The begin payload of a continuation segment: the same attachments, every one loaded (no clears).
pub fn continuation(p: &[u8]) -> Vec<u8> {
    let mut q = p.to_vec();
    for c in 0..colors(p) {
        set_word(&mut q, 4 + c * COLOR_BYTES + 8, 0);
    }
    let at = ds_modes_at(p);
    for i in 0..2 {
        if word(&q, at + 4 * i) != 0 {
            set_word(&mut q, at + 4 * i, MODE_LOAD);
        }
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A begin payload: colours (clear, store) and the depth/stencil modes.
    fn payload(cols: &[(u32, u32)], modes: [u32; 2]) -> Vec<u8> {
        let mut w = vec![cols.len() as u32];
        for (i, &(clear, store)) in cols.iter().enumerate() {
            w.extend_from_slice(&[10 + i as u32, 0, clear, store, 0, 1, 2, 3, 4]);
        }
        w.extend_from_slice(&[99, modes[0], modes[1], 0x3f80_0000, 7]);
        w.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn words(p: &[u8]) -> Vec<u32> {
        p.chunks(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    }

    #[test]
    fn a_segment_that_only_samples_depth_declares_it_read_only() {
        let mut s = Seg::starting([MODE_LOAD, 0]);
        // a draw testing depth without writing it, sampling the attachment (MK8D's 1920x1080 depth32float)
        assert!(!s.conflicts([true, false], [false, false]));
        s.add([true, false], [false, false]);
        assert_eq!(s.read_only(), [true, false]);
        let mut p = payload(&[(0, 1)], [MODE_LOAD, 0]);
        mark_read_only(&mut p, s.read_only());
        assert_eq!(modes(&p), [MODE_READ_ONLY, 0], "no stencil aspect stays absent");
    }

    #[test]
    fn depth_and_stencil_are_decided_separately() {
        let mut s = Seg::starting([MODE_LOAD, MODE_LOAD]);
        s.add([true, false], [false, true]);
        assert_eq!(s.read_only(), [true, false]);
        let mut p = payload(&[], [MODE_LOAD, MODE_LOAD]);
        mark_read_only(&mut p, s.read_only());
        assert_eq!(modes(&p), [MODE_READ_ONLY, MODE_LOAD]);
    }

    #[test]
    fn writing_then_sampling_splits_and_the_continuation_loads_everything() {
        let mut s = Seg::starting([MODE_CLEAR, 0]);
        assert_eq!(s.written, [true, false], "a clear writes its aspect");
        s.add([false, false], [true, false]);
        // the next draw samples depth: a new segment first
        assert!(s.conflicts([true, false], [false, false]));
        let mut first = payload(&[(1, 0), (0, 1)], [MODE_CLEAR, 0]);
        mark_read_only(&mut first, s.read_only());
        keep_stores(&mut first);
        let fw = words(&first);
        assert_eq!((fw[3], fw[4]), (1, 1), "the first colour keeps its clear and is now stored");
        assert_eq!(modes(&first), [MODE_CLEAR, 0], "a written aspect is never read-only");
        let cont = continuation(&first);
        let cw = words(&cont);
        assert_eq!((cw[3], cw[12]), (0, 0), "no colour is cleared again");
        assert_eq!(modes(&cont), [MODE_LOAD, 0]);
        assert_eq!(cw[0], 2);
        assert_eq!(cw[cw.len() - 1], 7, "views and clear values are kept");
        let mut s2 = Seg::starting(modes(&cont));
        s2.add([true, false], [false, false]);
        assert_eq!(s2.read_only(), [true, false]);
    }

    #[test]
    fn sampling_then_writing_splits() {
        let mut s = Seg::starting([MODE_LOAD, 0]);
        s.add([true, false], [false, false]);
        assert!(s.conflicts([false, false], [true, false]));
        assert!(!s.conflicts([true, false], [false, false]));
        assert!(!s.conflicts([false, true], [false, true]), "the other aspect is free");
    }
}

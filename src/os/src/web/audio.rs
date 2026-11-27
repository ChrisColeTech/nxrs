//! The audio ring the page's AudioWorklet plays (`src/front/web/page/audio-worklet.js`): 48 kHz float stereo frames in linear
//! memory, written by the engine's audio device thread (`media/sink.rs`), read by the worklet.
//!
//! Header (32-bit words): `[0]` frames written, `[1]` frames read (free-running counters; fill = written - read),
//! `[2]` capacity in frames (a power of two), `[3]` heartbeat (the worklet's process calls), `[4]` underrun frames
//! (silence the worklet played because the ring was short), `[5]` underrun events, `[6]` the page's AudioContext
//! state (0 not started, 1 running, 2 suspended or closed), `[7]` its sample rate. Frames from word 16, two f32 bits
//! each, at `(index & (capacity - 1)) * 2`.

use std::sync::atomic::{AtomicU32, Ordering::*};

const HEADER: usize = 16;
pub const CAPACITY: u32 = 8192;

pub struct Ring {
    w: &'static [AtomicU32],
}

impl Ring {
    /// Allocates the ring and gives it to the page.
    pub fn publish() -> Ring {
        let w = super::block(HEADER + CAPACITY as usize * 2);
        w[2].store(CAPACITY, Release);
        // SAFETY: the ring lives for the process
        unsafe { super::nxweb_audio_publish(w.as_ptr() as *const u32, CAPACITY) };
        Ring { w }
    }
    pub fn fill(&self) -> u32 {
        self.w[0]
            .load(Acquire)
            .wrapping_sub(self.w[1].load(Acquire))
    }
    pub fn heartbeat(&self) -> u32 {
        self.w[3].load(Relaxed)
    }
    /// (underrun frames, underrun events)
    pub fn underruns(&self) -> (u32, u32) {
        (self.w[4].load(Relaxed), self.w[5].load(Relaxed))
    }
    pub fn context(&self) -> (u32, u32) {
        (self.w[6].load(Acquire), self.w[7].load(Relaxed))
    }
    /// Appends frames (the caller keeps the fill within the capacity).
    pub fn push(&self, frames: &[[f32; 2]]) {
        let mut at = self.w[0].load(Relaxed);
        for f in frames {
            let i = HEADER + ((at & (CAPACITY - 1)) * 2) as usize;
            self.w[i].store(f[0].to_bits(), Relaxed);
            self.w[i + 1].store(f[1].to_bits(), Relaxed);
            at = at.wrapping_add(1);
        }
        self.w[0].store(at, Release);
    }
}

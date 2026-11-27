//! The command ring between the replay and the page's GPU worker (`src/web/gpu-worker.js` reads it).
//!
//! One static block of linear memory: a 256-byte header of 32-bit words (`H_*`) and a byte ring of packets
//! `[op u32, payload length u32, payload padded to 4]`. Producers (any engine thread) append under one lock and wake
//! the worker (`Atomics.waitAsync` on `H_HEAD`); the worker advances `H_TAIL` as it consumes. A packet that does not
//! fit before the ring's end is preceded by a wrap packet (op 0) or, with under 8 bytes left, an implied wrap.
//!
//! Completion: a `SIGNAL` names two words and a value; the worker writes the value into the second (queued) word once it
//! has handed the submission to the GPU, and into the first once the GPU finished it, bumping `H_GEN` and notifying it;
//! engine threads waiting for any value sleep on `H_GEN` (`wait_until`).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

pub const MAGIC: u32 = 0x4e58_4746;
pub const DATA: usize = 32 << 20;

pub const H_MAGIC: usize = 0;
pub const H_HEAD: usize = 1;
pub const H_TAIL: usize = 2;
/// the worker: 0 not started, 1 ready, 2 failed (no WebGPU)
pub const H_STATE: usize = 3;
pub const H_CANVAS_W: usize = 4;
pub const H_CANVAS_H: usize = 5;
pub const H_ERRORS: usize = 6;
pub const H_PRESENTS: usize = 7;
pub const H_GEN: usize = 8;
pub const H_DATA_SIZE: usize = 9;
/// adapter features the device uses: `F_*`
pub const H_FEATURES: usize = 10;
pub const H_MAX_STORAGE_PER_STAGE: usize = 11;
pub const H_MAX_UNIFORM_PER_STAGE: usize = 12;
pub const H_MAX_TEXTURES_PER_STAGE: usize = 13;
pub const H_MAX_BIND_GROUPS: usize = 14;
/// maxStorageBufferBindingSize in MiB
pub const H_MAX_STORAGE_MB: usize = 15;
pub const H_MAX_TEX_2D: usize = 16;
/// submissions the worker finished
pub const H_DONE: usize = 17;
/// the last submission's execute time on the worker (µs)
pub const H_EXEC_US: usize = 18;
/// the interval between the last two presents (µs)
pub const H_FRAME_US: usize = 19;
/// the worker is waiting for packets (producers notify only then)
pub const H_SLEEPING: usize = 20;

pub const F_BC: u32 = 1;
pub const F_FLOAT32_FILTER: u32 = 2;
pub const F_D32S8: u32 = 4;
pub const F_DUAL_SRC: u32 = 8;
pub const F_SWIZZLE: u32 = 16;
/// rg11b10ufloat is a render target
pub const F_RG11B10_RENDER: u32 = 32;
/// texture-formats-tier1: the 16-bit normalized formats (r16unorm, ...)
pub const F_TIER1: u32 = 64;

#[repr(C, align(256))]
pub struct Ring {
    pub hdr: [AtomicU32; 64],
    pub data: [u8; DATA],
}

// SAFETY: a zeroed header is valid atomics, zeroed data is valid bytes
pub static RING: Ring = unsafe { std::mem::zeroed() };
static PUSH: Mutex<()> = Mutex::new(());

pub fn hdr(i: usize) -> &'static AtomicU32 {
    &RING.hdr[i]
}

pub fn addr() -> *const u8 {
    &RING as *const Ring as *const u8
}

extern "C" {
    fn emscripten_futex_wait(addr: *const u32, val: u32, max_wait_ms: f64) -> i32;
    fn emscripten_futex_wake(addr: *const u32, count: i32) -> i32;
    /// `src/web/nxgfx.lib.js`, run on the page's thread: hands the ring and the memory to the GPU worker (1: started)
    fn nxgfx_web_start(ring: *const u8) -> i32;
    /// `src/web/nxgfx.lib.js`, on the calling thread: a synchronous GET of a URL relative to the page; the body's
    /// length (kept for `nxgfx_web_fetch_take`), or -1
    fn nxgfx_web_fetch(url: *const u8, url_len: usize) -> i32;
    fn nxgfx_web_fetch_take(dst: *mut u8);
}

/// Sleeps until `word` is no longer `val` or `ms` pass.
pub fn wait_word(word: &AtomicU32, val: u32, ms: f64) {
    // SAFETY: the word lives in linear memory for the program's life
    unsafe { emscripten_futex_wait(word.as_ptr(), val, ms) };
}

pub fn wake(word: &AtomicU32) {
    // SAFETY: as above
    unsafe { emscripten_futex_wake(word.as_ptr(), i32::MAX) };
}

/// Starts the worker (once) and waits until it reports ready or failed. Err: why the page has no WebGPU device.
pub fn start() -> Result<(), String> {
    static STARTED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    STARTED
        .get_or_init(|| {
            hdr(H_MAGIC).store(MAGIC, Ordering::Release);
            hdr(H_DATA_SIZE).store(DATA as u32, Ordering::Release);
            // SAFETY: a plain call into the page's JS
            if unsafe { nxgfx_web_start(addr()) } == 0 {
                return Err("the page has no GPU worker (src/web/gpu.js)".into());
            }
            let t = std::time::Instant::now();
            loop {
                match hdr(H_STATE).load(Ordering::Acquire) {
                    1 => return Ok(()),
                    2 => return Err("the GPU worker has no WebGPU device (navigator.gpu / requestAdapter failed)".into()),
                    s => {
                        if t.elapsed().as_secs() > 30 {
                            return Err("the GPU worker did not answer in 30 s".into());
                        }
                        wait_word(hdr(H_STATE), s, 100.0);
                    }
                }
            }
        })
        .clone()
}

/// Appends one packet and wakes the worker.
pub fn push(op: u32, payload: &[u8]) {
    push_parts(op, &[payload]);
}

/// Appends one packet made of several parts.
pub fn push_parts(op: u32, parts: &[&[u8]]) {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    let need = 8 + len.next_multiple_of(4);
    assert!(need < DATA / 2, "ring packet of {len} bytes");
    let _g = PUSH.lock().unwrap();
    let mut head = hdr(H_HEAD).load(Ordering::Relaxed) as usize;
    // wrap first when the packet does not fit before the end
    if DATA - head < need {
        wait_room(head, DATA - head);
        if DATA - head >= 8 {
            write_u32(head, 0);
            write_u32(head + 4, 0);
        }
        head = 0;
        // the consumer must have left [0, need) too
    }
    wait_room(head, need);
    write_u32(head, op);
    write_u32(head + 4, len as u32);
    let mut at = head + 8;
    for p in parts {
        // SAFETY: [at, at + len) is free ring space (wait_room) and inside the data array
        unsafe {
            std::ptr::copy_nonoverlapping(p.as_ptr(), RING.data.as_ptr().add(at) as *mut u8, p.len());
        }
        at += p.len();
    }
    let next = (head + need) % DATA;
    hdr(H_HEAD).store(next as u32, Ordering::Release);
    if hdr(H_SLEEPING).load(Ordering::Acquire) != 0 {
        wake(hdr(H_HEAD));
    }
}

fn write_u32(at: usize, v: u32) {
    // SAFETY: inside the data array, 4-aligned (packets are padded to 4)
    unsafe { (RING.data.as_ptr().add(at) as *mut u32).write_volatile(v) }
}

/// Waits until [head, head + n) holds no unconsumed packet.
fn wait_room(head: usize, n: usize) {
    loop {
        let tail = hdr(H_TAIL).load(Ordering::Acquire) as usize;
        // used bytes from tail to head (head == tail: empty)
        let used = (head + DATA - tail) % DATA;
        if used + n < DATA {
            // the range [head, head + n) must not reach the tail when the tail is ahead of head
            if tail <= head || head + n < tail {
                return;
            }
        }
        if hdr(H_STATE).load(Ordering::Acquire) != 1 {
            panic!("the GPU worker stopped with the ring full");
        }
        wait_word(hdr(H_TAIL), tail as u32, 50.0);
    }
}

/// A 64-bit completion word the worker writes (`SIGNAL`); lives as long as its object.
pub struct Word(pub Box<AtomicU64>);

impl Word {
    pub fn new(v: u64) -> Word {
        Word(Box::new(AtomicU64::new(v)))
    }
    pub fn addr(&self) -> u64 {
        &*self.0 as *const AtomicU64 as u64
    }
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

/// Waits until `ready()` or `timeout_ns` pass; true when ready.
pub fn wait_until(timeout_ns: u64, ready: impl Fn() -> bool) -> bool {
    let t = std::time::Instant::now();
    loop {
        let gen = hdr(H_GEN).load(Ordering::Acquire);
        if ready() {
            return true;
        }
        let left = timeout_ns.saturating_sub(t.elapsed().as_nanos() as u64);
        if left == 0 {
            return false;
        }
        if hdr(H_STATE).load(Ordering::Acquire) != 1 {
            return false;
        }
        wait_word(hdr(H_GEN), gen, (left as f64 / 1e6).min(100.0));
    }
}

/// A synchronous GET of `url` (relative to the page) on this thread.
pub fn fetch(url: &str) -> Option<Vec<u8>> {
    // SAFETY: the JS side reads url_len bytes of the URL
    let n = unsafe { nxgfx_web_fetch(url.as_ptr(), url.len()) };
    if n < 0 {
        return None;
    }
    let mut v = vec![0u8; n as usize];
    // SAFETY: the JS side writes the n bytes it reported
    unsafe { nxgfx_web_fetch_take(v.as_mut_ptr()) };
    Some(v)
}

/// Little-endian packet payload builder.
#[derive(Default)]
pub struct Pk(pub Vec<u8>);

impl Pk {
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
}

//! The web build's host side (docs/35 W3): what the native build gets from the OS, the page and its workers give
//! here through shared memory. The engine allocates each shared block in linear memory and publishes its address once
//! through a JS-library function (`src/front/web/link/nxrs-lib.js`, run on the page's thread); after that, engine threads and
//! the page's workers meet only through atomics on those blocks, never through the page's thread.
//!
//! | Block | Written by | Read by |
//! |---|---|---|
//! | I/O slots (`io.rs`) | engine threads (requests), the I/O worker (results) | the other side |
//! | audio ring (`audio.rs`) | the engine's audio device thread | the AudioWorklet |
//! | input block (`input/web.rs`) | the page (keyboard, gamepads) | the engine's input thread |

pub mod audio;
pub mod io;
pub mod saves;

use std::sync::atomic::AtomicU32;

/// The page interface this library was built for, kept in the library's bytes for `nxrs-build`'s check
/// (`translator::loader::layout::WEB_HOST_PROTOCOL`).
#[used]
#[no_mangle]
pub static NXWEB_HOST_PROTOCOL: [u8; translator::loader::layout::WEB_HOST_PROTOCOL.len()] = {
    let s = translator::loader::layout::WEB_HOST_PROTOCOL.as_bytes();
    let mut a = [0u8; translator::loader::layout::WEB_HOST_PROTOCOL.len()];
    let mut i = 0;
    while i < s.len() {
        a[i] = s[i];
        i += 1;
    }
    a
};

extern "C" {
    // src/front/web/link/nxrs-lib.js (proxied to the page's thread)
    pub(crate) fn nxweb_io_start(table: *const u32);
    pub(crate) fn nxweb_audio_publish(ring: *const u32, frames: u32);
    pub(crate) fn nxweb_input_publish(block: *const u32, words: u32);
    // Emscripten's futex on a word of linear memory; JS's Atomics.notify / waitAsync meet it on the same word
    fn emscripten_futex_wait(addr: *const u32, val: u32, max_wait_ms: f64) -> i32;
    fn emscripten_futex_wake(addr: *const u32, count: i32) -> i32;
}

/// Blocks while `w` holds `val`, at most `ms` milliseconds.
pub(crate) fn wait(w: &AtomicU32, val: u32, ms: f64) {
    // SAFETY: a word of linear memory that lives for the process
    unsafe { emscripten_futex_wait(w.as_ptr(), val, ms) };
}

/// Wakes every waiter on `w` (engine threads, and the workers' Atomics.waitAsync).
pub(crate) fn wake(w: &AtomicU32) {
    // SAFETY: as above
    unsafe { emscripten_futex_wake(w.as_ptr(), i32::MAX) };
}

/// A zeroed block of `words` 32-bit words for the process lifetime, 64-byte aligned.
pub(crate) fn block(words: usize) -> &'static [AtomicU32] {
    let layout = std::alloc::Layout::from_size_align(words * 4, 64).expect("block layout");
    // SAFETY: a non-zero layout; AtomicU32 is valid for zeroed memory; never freed
    unsafe {
        let p = std::alloc::alloc_zeroed(layout) as *const AtomicU32;
        assert!(!p.is_null(), "web: out of memory for a shared block");
        std::slice::from_raw_parts(p, words)
    }
}

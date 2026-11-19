//! Syncs and events.
//!
//! A fenced sync names a point in the back end's submission timeline; SyncWait returns once the GPU has completed it
//! and every CPU-visible effect of the submissions up to it (event values, counter reports, pool write-backs) has
//! landed. A sync never fenced since its Initialize is signalled: no GPU work stands behind it.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;
use crate::vk;
use std::sync::atomic::{AtomicU32, Ordering};

/// A CPU-side event write with release ordering (the GPU's arrive at their submission's completion).
pub fn event_publish(val: *mut u32, mode: u32, v: u32) {
    // SAFETY: the event's storage is a u32 in a memory pool the game keeps alive while the event is live.
    let a = unsafe { &*(val as *const AtomicU32) };
    if mode == event_signal::ADD {
        a.fetch_add(v, Ordering::Release);
    } else {
        a.store(v, Ordering::Release);
    }
    vk::queue::event_signalled();
}

/// Whether an event's current value passes a wait (EQUAL, or wrapping GEQUAL).
pub fn event_passes(cur: u32, mode: u32, v: u32) -> bool {
    if mode == event_wait::EQUAL {
        cur == v
    } else {
        cur.wrapping_sub(v) < 0x8000_0000
    }
}

nvn_fns! {
    fn nvnSyncInitialize(s: *mut NVNsync, d: *mut NVNdevice) -> NVNboolean {
        let _ = d;
        *(s as *mut sync_t) = sync_t { magic: MAGIC_SYNC, fenced: 0, fence: 0 };
        1
    }
    fn nvnSyncFinalize(s: *mut NVNsync) { (*(s as *mut sync_t)).magic = 0; }
    fn nvnSyncSetDebugLabel(s: *mut NVNsync, l: *const u8) { let _ = (s, l); }
    fn nvnQueueFenceSync(q: *mut NVNqueue, s: *mut NVNsync, cond: i32, flags: i32) {
        let _ = (cond, flags);
        let qs = (*(q as *const queue_t)).serial;
        let f = vk::queue::queue_fence(qs);
        (*(s as *mut sync_t)).fenced = 1;
        (*(s as *mut sync_t)).fence = f;
    }
    fn nvnQueueWaitSync(q: *mut NVNqueue, s: *const NVNsync) -> NVNboolean {
        let o = &*(s as *const sync_t);
        if o.fenced != 0 {
            vk::queue::queue_wait_fence((*(q as *const queue_t)).serial, o.fence);
        }
        1
    }
    fn nvnSyncWait(s: *const NVNsync, timeout: u64) -> u32 {
        // a fence recorded in a command buffer still queued for the replay thread is set once that replays
        if (*(s as *const sync_t)).fenced == 0 && vk::queue::aq_busy() {
            super::blocking(vk::queue::aq_drain);
        }
        let o = &*(s as *const sync_t);
        if o.fenced == 0 || vk::queue::fence_done(o.fence) {
            return sync_wait::ALREADY_SIGNALED;
        }
        let t0 = std::time::Instant::now();
        let fence = o.fence;
        if super::blocking(|| vk::queue::wait_fence(fence, timeout)) {
            sync_wait::CONDITION_SATISFIED
        } else {
            if timeout >= 1_000_000_000 {
                crate::log::red(&format!("nvnSyncWait: a {:.1} s wait on fence {} expired after {:.1} s", timeout as f64 / 1e9, o.fence, t0.elapsed().as_secs_f64()));
            }
            sync_wait::TIMEOUT_EXPIRED
        }
    }
    fn nvnEventBuilderSetDefaults(b: *mut NVNeventBuilder) { *(b as *mut ev_builder_t) = ev_builder_t { pool: core::ptr::null(), off: 0 }; }
    fn nvnEventBuilderSetStorage(b: *mut NVNeventBuilder, p: *const NVNmemoryPool, off: i64) { *(b as *mut ev_builder_t) = ev_builder_t { pool: p as *const pool_t, off }; }
    fn nvnEventInitialize(e: *mut NVNevent, b: *const NVNeventBuilder) -> NVNboolean {
        let s = &*(b as *const ev_builder_t);
        if s.pool.is_null() || (*s.pool).mem.is_null() {
            crate::log::red("EventInitialize has no storage");
            return 0;
        }
        *(e as *mut ev_t) = ev_t { magic: MAGIC_EVENT, val: (*s.pool).mem.add(s.off as usize) as *mut u32, pool: s.pool, off: s.off };
        1
    }
    fn nvnEventFinalize(e: *mut NVNevent) { (*(e as *mut ev_t)).magic = 0; }
    fn nvnEventGetValue(e: *const NVNevent) -> u32 { (*((*(e as *const ev_t)).val as *const AtomicU32)).load(Ordering::Acquire) }
    fn nvnEventSignal(e: *mut NVNevent, mode: u32, v: u32) { event_publish((*(e as *const ev_t)).val, mode, v); }
    fn nvnEventGetMemoryPool(e: *const NVNevent) -> *mut NVNmemoryPool { (*(e as *const ev_t)).pool as *mut NVNmemoryPool }
    fn nvnEventGetMemoryOffset(e: *const NVNevent) -> i64 { (*(e as *const ev_t)).off }
}

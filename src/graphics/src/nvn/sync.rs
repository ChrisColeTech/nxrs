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

/// A CPU-side event write with release ordering (the GPU's arrive at their submission's completion: `cpu` false).
pub fn event_publish(val: *mut u32, mode: u32, v: u32, cpu: bool) {
    // SAFETY: the event's storage is a u32 in a memory pool the game keeps alive while the event is live.
    let a = unsafe { &*(val as *const AtomicU32) };
    // before the word changes: no scan that started earlier may then count as covering work released by it. A GPU
    // signal landing is not one: what the work it releases reads was written by the CPU before the game queued that
    // work or the signalling op, both replayed by now (`queue::resume_due`), or by the GPU
    if cpu {
        vk::mem::note_cpu_signal();
        vk::queue::chain_trace::note("cpu_sig", val as u64, v as u64);
    }
    if mode == event_signal::ADD {
        a.fetch_add(v, Ordering::Release);
    } else {
        a.store(v, Ordering::Release);
    }
    vk::queue::event_signalled();
}

/// The chain trace's note of a SyncWait's return.
struct SwOut(u64);
impl Drop for SwOut {
    fn drop(&mut self) {
        vk::queue::chain_trace::note("sw_out", self.0, 0);
    }
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
        vk::queue::chain_trace::note("fence_sync", s as u64, f);
        (*(s as *mut sync_t)).fenced = 1;
        (*(s as *mut sync_t)).fence = f;
    }
    fn nvnQueueWaitSync(q: *mut NVNqueue, s: *const NVNsync) -> NVNboolean {
        let o = &*(s as *const sync_t);
        vk::queue::chain_trace::note("q_wait", (*(q as *const queue_t)).serial, o.fence);
        if o.fenced != 0 {
            vk::queue::queue_wait_fence((*(q as *const queue_t)).serial, o.fence);
        }
        1
    }
    fn nvnSyncWait(s: *const NVNsync, timeout: u64) -> u32 {
        let diag = vk::queue::wait_diag::Wait::new(s as usize, timeout);
        vk::queue::chain_trace::note("sw_in", s as u64, (*(s as *const sync_t)).fence | ((*(s as *const sync_t)).fenced as u64) << 56);
        let _out = SwOut(s as u64);
        // a fence recorded in a command buffer still queued for the replay thread is set once that replays
        // (until the sync is fenced or the queue has run dry, within the timeout; a poll does not wait)
        if (*(s as *const sync_t)).fenced == 0 && timeout > 0 && vk::queue::aq_busy() {
            diag.set("publication wait: blocking hook entry", None);
            let fenced = &(*(s as *const sync_t)).fenced as *const u32 as usize;
            super::blocking(|| {
                diag.set("await command-buffer fence publication", None);
                let t0 = std::time::Instant::now();
                vk::queue::aq_wait_until(timeout, || unsafe { core::ptr::read_volatile(fenced as *const u32) } != 0);
                vk::perf::add(vk::perf::C::WaitPublishNs, t0.elapsed().as_nanos() as u64);
                diag.set("publication wait returned: blocking hook re-entry", None);
                vk::queue::chain_trace::note("sw_pub", s as u64, 0);
            });
        }
        let o = &*(s as *const sync_t);
        diag.set("check fence completion", Some(o.fence));
        if o.fenced == 0 || vk::queue::fence_done(o.fence) {
            return sync_wait::ALREADY_SIGNALED;
        }
        let t0 = std::time::Instant::now();
        let fence = o.fence;
        diag.set("fence wait: blocking hook entry", Some(fence));
        let ok = super::blocking(|| {
            let _w = vk::queue::GuestWait::new();
            diag.set("wait fence", Some(fence));
            let ok = vk::queue::wait_fence(fence, timeout);
            diag.set(if ok { "fence satisfied: blocking hook re-entry" } else { "fence expired: blocking hook re-entry" }, Some(fence));
            ok
        });
        vk::perf::add(vk::perf::C::WaitSyncNs, t0.elapsed().as_nanos() as u64);
        if ok {
            sync_wait::CONDITION_SATISFIED
        } else {
            if timeout >= 1_000_000_000 {
                crate::vklog!("nvnSyncWait: a {:.1} s timed wait on fence {} returned TIMEOUT_EXPIRED after {:.1} s", timeout as f64 / 1e9, o.fence, t0.elapsed().as_secs_f64());
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
    fn nvnEventSignal(e: *mut NVNevent, mode: u32, v: u32) { event_publish((*(e as *const ev_t)).val, mode, v, true); }
    fn nvnEventGetMemoryPool(e: *const NVNevent) -> *mut NVNmemoryPool { (*(e as *const ev_t)).pool as *mut NVNmemoryPool }
    fn nvnEventGetMemoryOffset(e: *const NVNevent) -> i64 { (*(e as *const ev_t)).off }
}

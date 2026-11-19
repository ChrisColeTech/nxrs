//! Queues: builder sizes, submission of recorded command handles, finish, present.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;
use crate::vk;

unsafe fn qb<'a>(b: *mut NVNqueueBuilder) -> &'a mut queue_builder_t {
    &mut *(b as *mut queue_builder_t)
}
fn qb_size(o: &queue_builder_t, k: usize) -> u64 {
    const DEF: [u32; 4] = [65536, 262144, 16384, 0];
    if o.set[k] != 0 { o.sz[k] as u64 } else { DEF[k] as u64 }
}

nvn_fns! {
    fn nvnQueueBuilderSetDefaults(b: *mut NVNqueueBuilder) {
        let dev = qb(b).dev;
        core::ptr::write_bytes(b as *mut u8, 0, core::mem::size_of::<queue_builder_t>());
        qb(b).dev = dev;
    }
    fn nvnQueueBuilderSetDevice(b: *mut NVNqueueBuilder, d: *mut NVNdevice) { qb(b).dev = d; }
    fn nvnQueueBuilderSetFlags(b: *mut NVNqueueBuilder, f: i32) { qb(b).flags = f as u32; }
    fn nvnQueueBuilderSetCommandMemorySize(b: *mut NVNqueueBuilder, n: usize) { qb(b).sz[QB_CMD] = n as u32; qb(b).set[QB_CMD] = 1; }
    fn nvnQueueBuilderSetComputeMemorySize(b: *mut NVNqueueBuilder, n: usize) { qb(b).sz[QB_COMP] = n as u32; qb(b).set[QB_COMP] = 1; }
    fn nvnQueueBuilderSetControlMemorySize(b: *mut NVNqueueBuilder, n: usize) { qb(b).sz[QB_CTL] = n as u32; qb(b).set[QB_CTL] = 1; }
    fn nvnQueueBuilderSetCommandFlushThreshold(b: *mut NVNqueueBuilder, n: usize) { qb(b).flush = n as u32; }
    fn nvnQueueBuilderGetQueueMemorySize(b: *const NVNqueueBuilder) -> usize {
        let o = &*(b as *const queue_builder_t);
        ((qb_size(o, QB_CMD) + qb_size(o, QB_COMP) + qb_size(o, QB_CTL) + 4095) & !4095) as usize
    }
    fn nvnQueueBuilderSetQueueMemory(b: *mut NVNqueueBuilder, m: *mut u8, n: usize) { qb(b).mem = m; qb(b).mem_sz = n as u64; }
    fn nvnQueueBuilderGetCommandMemorySize(b: *const NVNqueueBuilder, size: *mut i32) -> NVNboolean { let o = &*(b as *const queue_builder_t); *size = o.sz[QB_CMD] as i32; o.set[QB_CMD] }
    fn nvnQueueBuilderGetComputeMemorySize(b: *const NVNqueueBuilder, size: *mut i32) -> NVNboolean { let o = &*(b as *const queue_builder_t); *size = o.sz[QB_COMP] as i32; o.set[QB_COMP] }
    fn nvnQueueBuilderGetControlMemorySize(b: *const NVNqueueBuilder, size: *mut i32) -> NVNboolean { let o = &*(b as *const queue_builder_t); *size = o.sz[QB_CTL] as i32; o.set[QB_CTL] }
    fn nvnQueueInitialize(q: *mut NVNqueue, b: *const NVNqueueBuilder) -> NVNboolean {
        let d = (*(b as *const queue_builder_t)).dev;
        // NVN puts the window origin into the queue's initial state: a later device change does not rewrite it.
        let ll = d.is_null() || (*(d as *const dev_t)).origin == origin::LOWER_LEFT;
        *(q as *mut queue_t) = queue_t { magic: MAGIC_QUEUE, dev: d, submits: 0, origin_ll: ll as i32, serial: super::new_serial() };
        1
    }
    fn nvnQueueFinalize(q: *mut NVNqueue) {
        let o = &mut *(q as *mut queue_t);
        vk::queue::queue_forget(o.serial);
        o.magic = 0;
    }
    fn nvnQueueSetDebugLabel(q: *mut NVNqueue, s: *const u8) { let _ = (q, s); }
    fn nvnQueueGetError(q: *mut NVNqueue, info: *mut u8) -> i32 { let _ = q; if !info.is_null() { *info = 0; } 0 }
    fn nvnQueueSubmitCommands(q: *mut NVNqueue, n: i32, handles: *const u64) {
        let o = &mut *(q as *mut queue_t);
        o.submits += 1;
        let hs = if n > 0 && !handles.is_null() { core::slice::from_raw_parts(handles, n as usize).to_vec() } else { Vec::new() };
        if let Some(h) = super::hooks() {
            (h.submit)(q as u64, n.max(0) as u64);
        }
        vk::queue::submit(o.serial, o.origin_ll != 0, &hs);
    }
    fn nvnQueueFlush(q: *mut NVNqueue) { let _ = q; }
    /// Waits for the GPU work submitted so far (not for the display).
    fn nvnQueueFinish(q: *mut NVNqueue) {
        let _ = q;
        super::blocking(vk::queue::finish);
    }
    fn nvnQueuePresentTexture(q: *mut NVNqueue, w: *mut NVNwindow, idx: i32) {
        let o = &*(q as *const queue_t);
        if let Some(h) = super::hooks() {
            (h.present)(w as u64, idx as u32);
        }
        super::window::present(o, &mut *(w as *mut win_t), idx);
    }
    fn nvnQueueAcquireTexture(q: *mut NVNqueue, w: *mut NVNwindow, idx: *mut i32) -> i32 {
        let _ = q;
        super::window::nvnWindowAcquireTexture(w, core::ptr::null_mut(), idx) as i32
    }
}

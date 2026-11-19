//! Write-back on demand: GPU-written pool pages the CPU does not touch stay on the device.
//!
//! Every GPU write into a mirrored pool is copied back to guest memory when its submission completes. A page GPU
//! writes keep coming to (`STREAK` frames in a row) is handed to the device instead: at completion it is made
//! inaccessible (`PAGE_NOACCESS`) and not copied; the first CPU access to it, by the game or the host, faults, the
//! handler reads the page back from the device (a copy of its own, outside the replay's locks) and lets the access go
//! on. Such a page then stays eager for a while, longer each time (64 frames, doubled per return, at most 4096).
//! `NXGFX_WB_LAZY=0` turns it off. A page the completion thread must write (an eager write-back landing on it) is
//! read back first the same way.

use super::Gpu;
use ash::vk;
use gpu_allocator::MemoryLocation;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

const PAGE: u64 = 4096;
const STREAK: u32 = 3;

#[derive(Clone, Copy)]
struct Lazy {
    buf: vk::Buffer,
    off: u64,
}

#[derive(Default)]
struct Hist {
    streak: u32,
    last_frame: u64,
    eager_until: u64,
    backoff: u64,
}

struct St {
    lazy: HashMap<u64, Lazy>,
    hist: HashMap<u64, Hist>,
}
static ST: Mutex<Option<St>> = Mutex::new(None);
static HANDLER: AtomicBool = AtomicBool::new(false);
pub static PAGES_IN: AtomicU64 = AtomicU64::new(0);
pub static SKIP_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("NXGFX_WB_LAZY").as_deref() != Ok("0"))
}

fn frame() -> u64 {
    super::present::frames()
}

/// Pages now held on the device only.
pub fn pages() -> usize {
    ST.lock().unwrap().as_ref().map(|s| s.lazy.len()).unwrap_or(0)
}

/// A GPU write-back of the whole pages in [a, a + n) is due: the pages that go lazy instead (their write-back is
/// skipped) are returned; the caller copies back the rest. Partial pages are always copied back.
pub fn take_lazy(a: u64, n: u64) -> Vec<u64> {
    if !enabled() {
        return vec![];
    }
    let f = frame();
    let lo = (a + PAGE - 1) & !(PAGE - 1);
    let hi = (a + n) & !(PAGE - 1);
    let mut out = vec![];
    let mut g = ST.lock().unwrap();
    let st = g.get_or_insert_with(|| St { lazy: HashMap::new(), hist: HashMap::new() });
    let mut p = lo;
    while p < hi {
        let h = st.hist.entry(p).or_default();
        if h.last_frame + 1 >= f && h.last_frame != f {
            h.streak += 1;
        } else if h.last_frame != f {
            h.streak = 1;
        }
        h.last_frame = f;
        if h.streak >= STREAK && f >= h.eager_until {
            out.push(p);
        }
        p += PAGE;
    }
    out
}

/// At completion: `pages` (of the pool buffer `buf`, page `p` at `off0 + (p - base)`) stay on the device.
pub fn make_lazy(pages: &[u64], buf: vk::Buffer, base: u64, off0: u64) {
    if pages.is_empty() {
        return;
    }
    install();
    let mut g = ST.lock().unwrap();
    let st = g.as_mut().unwrap();
    for &p in pages {
        st.lazy.insert(p, Lazy { buf, off: off0 + (p - base) });
        protect(p, false);
        SKIP_BYTES.fetch_add(PAGE, Ordering::Relaxed);
    }
}

#[cfg(windows)]
fn protect(p: u64, access: bool) {
    use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_NOACCESS, PAGE_READWRITE};
    let mut old = 0u32;
    unsafe { VirtualProtect(p as *const _, PAGE as usize, if access { PAGE_READWRITE } else { PAGE_NOACCESS }, &mut old) };
}
#[cfg(not(windows))]
fn protect(_p: u64, _access: bool) {}

/// Whether any page of [a, a + n) is held on the device (the host is about to touch it: read it back first).
pub fn touch(a: u64, n: u64) {
    if n == 0 || ST.lock().unwrap().as_ref().is_none_or(|s| s.lazy.is_empty()) {
        return;
    }
    let mut p = a & !(PAGE - 1);
    while p < a + n {
        bring_back(p);
        p += PAGE;
    }
}

/// The page at `p` read back from the device, made accessible again. False when it is not a lazy page.
pub fn bring_back(p: u64) -> bool {
    let p = p & !(PAGE - 1);
    let l = {
        let mut g = ST.lock().unwrap();
        let Some(st) = g.as_mut() else { return false };
        let Some(l) = st.lazy.remove(&p) else { return false };
        let h = st.hist.entry(p).or_default();
        h.backoff = (h.backoff * 2).clamp(64, 4096);
        h.eager_until = frame() + h.backoff;
        h.streak = 0;
        l
    };
    let Some(g) = Gpu::try_get() else { return false };
    protect(p, true);
    let data = read_page(g, l.buf, l.off);
    // SAFETY: the guest page, accessible again
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), p as *mut u8, PAGE as usize) };
    super::mem::reset_watch(p, PAGE);
    PAGES_IN.fetch_add(1, Ordering::Relaxed);
    true
}

struct Reader {
    _pool: vk::CommandPool,
    cb: vk::CommandBuffer,
    fence: vk::Fence,
    stage: super::Buf,
}
unsafe impl Send for Reader {}
static READER: Mutex<Option<Reader>> = Mutex::new(None);

/// One page of a pool buffer, copied by a submission of its own after everything submitted so far.
fn read_page(g: &Gpu, buf: vk::Buffer, off: u64) -> Vec<u8> {
    // what the device holds once the work already submitted is done: a wait on the GPU timeline itself, not on the
    // completion thread's bookkeeping (the completion thread may be the one asking)
    let sems = [g.q.timeline];
    let vals = [g.q.last_fence()];
    unsafe { g.dev().wait_semaphores(&vk::SemaphoreWaitInfo::default().semaphores(&sems).values(&vals), u64::MAX) }.ok();
    let mut rg = READER.lock().unwrap();
    if rg.is_none() {
        let pci = vk::CommandPoolCreateInfo::default().queue_family_index(g.d.qfam).flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let pool = unsafe { g.dev().create_command_pool(&pci, None) }.expect("lazy reader pool");
        let ai = vk::CommandBufferAllocateInfo::default().command_pool(pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1);
        let cb = unsafe { g.dev().allocate_command_buffers(&ai) }.expect("lazy reader cb")[0];
        let fence = unsafe { g.dev().create_fence(&vk::FenceCreateInfo::default(), None) }.expect("lazy reader fence");
        let stage = g.buffer(PAGE, vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuToCpu, "lazy page").expect("lazy page buffer");
        *rg = Some(Reader { _pool: pool, cb, fence, stage });
    }
    let r = rg.as_ref().unwrap();
    unsafe {
        let d = g.dev();
        d.reset_command_buffer(r.cb, vk::CommandBufferResetFlags::empty()).ok();
        d.begin_command_buffer(r.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).ok();
        super::queue::full_barrier(g, r.cb);
        d.cmd_copy_buffer(r.cb, buf, r.stage.buf, &[vk::BufferCopy { src_offset: off, dst_offset: 0, size: PAGE }]);
        let mb = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)];
        d.cmd_pipeline_barrier2(r.cb, &vk::DependencyInfo::default().memory_barriers(&mb));
        d.end_command_buffer(r.cb).ok();
        d.reset_fences(&[r.fence]).ok();
        let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(r.cb)];
        let si = [vk::SubmitInfo2::default().command_buffer_infos(&cbs)];
        super::queue::submit_raw(g, &si, r.fence);
        d.wait_for_fences(&[r.fence], true, u64::MAX).ok();
        core::slice::from_raw_parts(r.stage.ptr, PAGE as usize).to_vec()
    }
}

#[cfg(windows)]
fn install() {
    use windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler;
    if !HANDLER.swap(true, Ordering::SeqCst) {
        // first in the chain: the runtime's crash reporter must not see a lazy page's fault
        unsafe { AddVectoredExceptionHandler(1, Some(veh)) };
    }
}
#[cfg(not(windows))]
fn install() {}

#[cfg(windows)]
unsafe extern "system" fn veh(ep: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS) -> i32 {
    use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    let er = &*(*ep).ExceptionRecord;
    if er.ExceptionCode != EXCEPTION_ACCESS_VIOLATION || er.NumberParameters < 2 {
        return CONTINUE_SEARCH;
    }
    if bring_back(er.ExceptionInformation[1] as u64) {
        CONTINUE_EXECUTION
    } else {
        CONTINUE_SEARCH
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_page_goes_lazy_after_a_streak_of_frames() {
        // frames() is 0 in a unit test: the streak counts calls in the same frame once
        let a = 0x7000_0000u64;
        assert!(super::take_lazy(a, 0x2000).is_empty());
        // partial pages never go lazy
        assert!(super::take_lazy(a + 8, 0x100).is_empty());
    }
}

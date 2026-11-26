//! Write-back on demand: GPU-written pool pages the CPU does not touch stay on the device.
//!
//! Every GPU write into a mirrored pool is copied back to guest memory when its submission completes. A page GPU
//! writes keep coming to (`STREAK` frames in a row) is handed to the device instead: at completion it is made
//! inaccessible (`PAGE_NOACCESS`) and not copied; the first CPU access to it, by the game or the host, faults, the
//! handler reads the page back from the device (a copy of its own, outside the replay's locks) and lets the access go
//! on. Such a page then stays eager for a while, longer each time (64 frames, doubled per return, at most 4096).
//! On by default (`NXGFX_WB_LAZY=0` off) since Pokemon Violet runs clean with it (lane G3 `v12`). A page the completion thread must write (an eager write-back landing on it) is
//! read back first the same way.

use super::Gpu;
use ash::vk;
use gpu_allocator::MemoryLocation;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

const PAGE: u64 = 4096;
const STREAK: u32 = 3;

#[derive(Clone, Copy)]
struct Lazy {
    buf: vk::Buffer,
    off: u64,
    /// the base of the pool whose buffer holds it (the upload skip's record goes to that pool only)
    pool: u64,
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
    /// Pages being read back right now (out of `lazy`, still inaccessible): a second thread's fault on one waits for
    /// the first to finish and then retries its access, instead of falling through to the crash handler
    returning: HashSet<u64>,
}
static ST: Mutex<Option<St>> = Mutex::new(None);
static HANDLER: AtomicBool = AtomicBool::new(false);
pub static PAGES_IN: AtomicU64 = AtomicU64::new(0);
/// time spent bringing pages back (the faulting threads' and the completion thread's)
pub static BACK_NS: AtomicU64 = AtomicU64::new(0);

/// The pools line's part (lazy write-back on): pages held on the device, brought back per frame and their time.
pub fn summary(frames: f64) -> String {
    if !enabled() {
        return String::new();
    }
    let n = PAGES_IN.swap(0, Ordering::Relaxed) as f64 / frames;
    let ms = BACK_NS.swap(0, Ordering::Relaxed) as f64 / 1e6 / frames;
    let skip = SKIP_BYTES.swap(0, Ordering::Relaxed) as f64 / frames / 1048576.0;
    let t = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / 1e6 / frames;
    let (lk, tk, mk, ld, tc) = (t(&T_LOCK), t(&T_TAKE), t(&T_MAKE), t(&T_LAND), t(&T_TOUCH));
    format!(
        "; lazy write-back: {} pages on the device, {skip:.2} MB a frame not copied back, {n:.1} pages a frame brought back in {ms:.2} ms (completion: pool lock {lk:.2} ms, pages chosen {tk:.2}, held {mk:.2}, whole write-backs landed {ld:.2}, pages touched {tc:.2})",
        pages()
    )
}
/// Where the completion's write-back time goes with lazy write-back on (ns): waiting for the pool table, choosing the
/// pages that stay, holding them.
pub static T_LOCK: AtomicU64 = AtomicU64::new(0);
pub static T_TAKE: AtomicU64 = AtomicU64::new(0);
pub static T_MAKE: AtomicU64 = AtomicU64::new(0);
pub static T_LAND: AtomicU64 = AtomicU64::new(0);
pub static T_TOUCH: AtomicU64 = AtomicU64::new(0);
pub static SKIP_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    // on by default since lane G3 rev 8 (Pokemon Violet 150 s clean at its 30 FPS cap, write-back 0.5 ms a frame
    // against 1.3 ms copied back eagerly); NXGFX_WB_LAZY=0: every write-back lands at completion
    *V.get_or_init(|| std::env::var("NXGFX_WB_LAZY").as_deref() != Ok("0"))
}

fn frame() -> u64 {
    super::present::frames()
}

/// Pages now held on the device only.
pub fn pages() -> usize {
    ST.lock()
        .unwrap()
        .as_ref()
        .map(|s| s.lazy.len())
        .unwrap_or(0)
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
    let st = g.get_or_insert_with(|| St {
        lazy: HashMap::new(),
        hist: HashMap::new(),
        returning: HashSet::new(),
    });
    // the committed state of the region around `p` ([lo, hi), committed), asked once per run of like pages
    let mut region = (0u64, 0u64, false);
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
            // A page not committed yet (the heap is committed at first touch) cannot be held: protecting it fails,
            // so it stayed reachable, and its first touch then committed a zero page over the device's bytes
            // (Ocarina of Time: 11,616 pages of one pool lost at once, a red line each). It lands eagerly instead
            // (its first changed byte commits it) and may go lazy later.
            if st.lazy.contains_key(&p) || {
                if p < region.0 || p >= region.1 {
                    region = committed_region(p);
                }
                region.2
            } {
                out.push(p);
            }
        }
        p += PAGE;
    }
    out
}

/// The run of pages around `p` that share its committed state: (start, end, committed).
#[cfg(windows)]
fn committed_region(p: u64) -> (u64, u64, bool) {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    // SAFETY: a query of this process's own address space
    let n = unsafe {
        VirtualQuery(
            p as *const _,
            &mut mbi,
            core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if n == 0 {
        return (p, p + PAGE, false);
    }
    let lo = mbi.BaseAddress as u64;
    (lo, lo + (mbi.RegionSize as u64).max(PAGE), mbi.State == MEM_COMMIT)
}
#[cfg(target_os = "linux")]
fn committed_region(p: u64) -> (u64, u64, bool) {
    let committed = match nxrt::linux::query(p) {
        Some((committed, _, _, _)) => committed,
        None => nxrt::mem::readable(p, 1),
    };
    (p, p + PAGE, committed)
}

/// At completion: `pages` (of the buffer `buf` of the pool at `pool`, page `p` at `off0 + (p - base)`) stay on the
/// device.
pub fn make_lazy(pages: &[u64], buf: vk::Buffer, base: u64, off0: u64, pool: u64) {
    if pages.is_empty() {
        return;
    }
    install();
    let mut g = ST.lock().unwrap();
    let st = g.as_mut().unwrap();
    // pages already held stay protected (they were protected again at every frame: one VirtualProtect a page, ~20 ms a
    // frame in Pokemon Violet); new ones are protected in runs of neighbours
    let mut fresh: Vec<u64> = Vec::new();
    for &p in pages {
        if st.returning.contains(&p) {
            continue; // being read back for a CPU access right now: stays eager
        }
        if st
            .lazy
            .insert(
                p,
                Lazy {
                    buf,
                    off: off0 + (p - base),
                    pool,
                },
            )
            .is_none()
        {
            fresh.push(p);
        }
        SKIP_BYTES.fetch_add(PAGE, Ordering::Relaxed);
    }
    fresh.sort_unstable();
    let mut i = 0;
    while i < fresh.len() {
        let mut j = i + 1;
        while j < fresh.len() && fresh[j] == fresh[j - 1] + PAGE {
            j += 1;
        }
        protect_n(fresh[i], (j - i) as u64, false);
        i = j;
    }
}

#[cfg(windows)]
fn protect_n(p: u64, n: u64, access: bool) {
    use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_NOACCESS, PAGE_READWRITE};
    let mut old = 0u32;
    let prot = if access {
        PAGE_READWRITE
    } else {
        PAGE_NOACCESS
    };
    // a run over two allocations is refused as a whole: page by page then
    if unsafe { VirtualProtect(p as *const _, (n * PAGE) as usize, prot, &mut old) } == 0 {
        for k in 0..n {
            unsafe { VirtualProtect((p + k * PAGE) as *const _, PAGE as usize, prot, &mut old) };
        }
    }
}
#[cfg(target_os = "linux")]
fn protect_n(p: u64, n: u64, access: bool) {
    let prot = if access {
        nxrt::mem::Prot::ReadWrite
    } else {
        nxrt::mem::Prot::None
    };
    if !nxrt::mem::protect(p, n * PAGE, prot) {
        for k in 0..n {
            nxrt::mem::protect(p + k * PAGE, PAGE, prot);
        }
    }
}

#[cfg(windows)]
fn protect(p: u64, access: bool) -> bool {
    use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_NOACCESS, PAGE_READWRITE};
    let mut old = 0u32;
    unsafe {
        VirtualProtect(
            p as *const _,
            PAGE as usize,
            if access {
                PAGE_READWRITE
            } else {
                PAGE_NOACCESS
            },
            &mut old,
        ) != 0
    }
}
#[cfg(target_os = "linux")]
fn protect(p: u64, access: bool) -> bool {
    nxrt::mem::protect(
        p,
        PAGE,
        if access {
            nxrt::mem::Prot::ReadWrite
        } else {
            nxrt::mem::Prot::None
        },
    )
}

/// Whether any page of [a, a + n) is held on the device (the host is about to touch it: read it back first).
/// The parts of [a, a + n) outside the pages held on the device, as (offset, length); None when none is held.
pub fn outside(a: u64, n: u64) -> Option<Vec<(u64, u64)>> {
    let g = ST.lock().unwrap();
    let st = g.as_ref()?;
    if st.lazy.is_empty() && st.returning.is_empty() {
        return None;
    }
    let held = |p: u64| st.lazy.contains_key(&p) || st.returning.contains(&p);
    let mut p = a & !(PAGE - 1);
    let mut any = false;
    while p < a + n {
        any |= held(p);
        p += PAGE;
    }
    if !any {
        return None;
    }
    let mut out: Vec<(u64, u64)> = Vec::new();
    let mut p = a & !(PAGE - 1);
    while p < a + n {
        let (lo, hi) = (p.max(a), (p + PAGE).min(a + n));
        if !held(p) {
            match out.last_mut() {
                Some(l) if l.0 + l.1 == lo - a => l.1 += hi - lo,
                _ => out.push((lo - a, hi - lo)),
            }
        }
        p += PAGE;
    }
    Some(out)
}

pub fn touch(a: u64, n: u64) {
    if n == 0
        || ST
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|s| s.lazy.is_empty())
    {
        return;
    }
    let t0 = std::time::Instant::now();
    // only the held pages (and those coming back) of the range: touching every page of a pool or a texture locked the
    // state and asked the OS about each one (Pokemon Violet: ~30 ms a frame)
    let lo = a & !(PAGE - 1);
    let hi = a + n;
    let mut pages: Vec<u64> = {
        let g = ST.lock().unwrap();
        let Some(st) = g.as_ref() else { return };
        let span = (hi - lo).div_ceil(PAGE) as usize;
        if span <= st.lazy.len() + st.returning.len() {
            (0..span as u64)
                .map(|i| lo + i * PAGE)
                .filter(|p| st.lazy.contains_key(p) || st.returning.contains(p))
                .collect()
        } else {
            st.lazy
                .keys()
                .chain(st.returning.iter())
                .copied()
                .filter(|&p| p >= lo && p < hi)
                .collect()
        }
    };
    pages.sort_unstable();
    pages.dedup();
    for p in pages {
        bring_back(p);
    }
    T_TOUCH.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// The page at `p` read back from the device, made accessible again. False when it is not a lazy page. A page another
/// thread is reading back is waited for (true: the access can go on once it is back).
pub fn bring_back(p: u64) -> bool {
    let p = p & !(PAGE - 1);
    // the page this thread is bringing back right now: a fault on it from inside that (the copy into the page) is not
    // waited for (it waited for itself for ever: Mario Kart World's job worker at ~70 s, lane K2), it goes on to the
    // other handlers
    thread_local! {
        static MINE: std::cell::Cell<u64> = const { std::cell::Cell::new(u64::MAX) };
    }
    if MINE.with(|m| m.get()) == p {
        crate::log::red(&format!(
            "lazy write-back: page {p:#x} faulted again while this thread brought it back"
        ));
        return false;
    }
    let l = loop {
        let mut g = ST.lock().unwrap();
        let Some(st) = g.as_mut() else { return false };
        if st.returning.contains(&p) {
            drop(g);
            std::thread::yield_now();
            continue;
        }
        let Some(l) = st.lazy.remove(&p) else {
            // not ours: or one that came back between the fault and this lookup (accessible again: retry)
            return st.hist.contains_key(&p) && accessible(p);
        };
        st.returning.insert(p);
        let h = st.hist.entry(p).or_default();
        h.backoff = (h.backoff * 2).clamp(64, 4096);
        h.eager_until = frame() + h.backoff;
        h.streak = 0;
        break l;
    };
    let done = || {
        MINE.with(|m| m.set(u64::MAX));
        if let Some(st) = ST.lock().unwrap().as_mut() {
            st.returning.remove(&p);
        }
    };
    MINE.with(|m| m.set(p));
    let Some(g) = Gpu::try_get() else {
        let ok = protect(p, true);
        done();
        return ok;
    };
    let t0 = std::time::Instant::now();
    let data = read_page(g, l.buf, l.off);
    BACK_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    // The write watch is not reset (MKW-RECORD L6): the copy's store below reads as a CPU write, and the upload skip
    // leaves the page out at its pool's next sync while it still holds exactly these bytes (`upskip::brought_back`),
    // so only a real CPU write since uploads it. Recorded before the copy: a scan taking the copy's store finds it (audit
    // H1). A reset here, without the watch lock (which a fault on a thread holding it would deadlock on), let the pool
    // walker take this store for a CPU write anyway, and erased the watch bit of a game store made between the copy and
    // the reset: that write never reached the GPU. With the skip off (`NXGFX_UPLOAD_SKIP=0`) the old reset stays.
    let skip = super::upskip::on();
    if skip {
        super::upskip::brought_back(l.pool, p, &data);
    }
    if !protect(p, true) || !accessible(p) {
        // the page is no longer the one made lazy (unmapped or decommitted since): nothing to bring back; the access
        // goes on to the other handlers
        crate::log::red(&format!("lazy write-back: page {p:#x} can no longer be made accessible (its memory changed while it was held on the device): dropped"));
        if skip {
            super::upskip::not_brought_back(l.pool, p);
        }
        done();
        return false;
    }
    // SAFETY: the guest page, accessible again; other threads' faults on it wait in `returning` until it is filled
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), p as *mut u8, PAGE as usize) };
    if !skip {
        super::mem::reset_watch(p, PAGE);
    }
    done();
    PAGES_IN.fetch_add(1, Ordering::Relaxed);
    true
}

#[cfg(windows)]
fn accessible(p: u64) -> bool {
    use windows_sys::Win32::System::Memory::{
        VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_NOACCESS,
    };
    let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    let n = unsafe {
        VirtualQuery(
            p as *const _,
            &mut mbi,
            core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    n != 0 && mbi.State == MEM_COMMIT && mbi.Protect & PAGE_NOACCESS == 0
}
#[cfg(target_os = "linux")]
fn accessible(p: u64) -> bool {
    match nxrt::linux::query(p) {
        Some((committed, prot, _, _)) => committed && prot != 0,
        None => nxrt::mem::readable(p, 1),
    }
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
    unsafe {
        g.dev().wait_semaphores(
            &vk::SemaphoreWaitInfo::default()
                .semaphores(&sems)
                .values(&vals),
            u64::MAX,
        )
    }
    .ok();
    let mut rg = READER.lock().unwrap();
    if rg.is_none() {
        let pci = vk::CommandPoolCreateInfo::default()
            .queue_family_index(g.d.qfam)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let pool = unsafe { g.dev().create_command_pool(&pci, None) }.expect("lazy reader pool");
        let ai = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let cb = unsafe { g.dev().allocate_command_buffers(&ai) }.expect("lazy reader cb")[0];
        let fence = unsafe { g.dev().create_fence(&vk::FenceCreateInfo::default(), None) }
            .expect("lazy reader fence");
        let stage = g
            .buffer(
                PAGE,
                vk::BufferUsageFlags::TRANSFER_DST,
                MemoryLocation::GpuToCpu,
                "lazy page",
            )
            .expect("lazy page buffer");
        *rg = Some(Reader {
            _pool: pool,
            cb,
            fence,
            stage,
        });
    }
    let r = rg.as_ref().unwrap();
    unsafe {
        let d = g.dev();
        d.reset_command_buffer(r.cb, vk::CommandBufferResetFlags::empty())
            .ok();
        d.begin_command_buffer(
            r.cb,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )
        .ok();
        super::queue::full_barrier(g, r.cb);
        super::lost::checkpoint(g, r.cb, false, || {
            format!(
                "a lazy page read from buffer {:#x} at {off:#x}",
                vk::Handle::as_raw(buf)
            )
        });
        d.cmd_copy_buffer(
            r.cb,
            buf,
            r.stage.buf,
            &[vk::BufferCopy {
                src_offset: off,
                dst_offset: 0,
                size: PAGE,
            }],
        );
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
#[cfg(target_os = "linux")]
fn install() {
    if !HANDLER.swap(true, Ordering::SeqCst) {
        // newest hook first: the runtime's crash reporter must not see a lazy page's fault
        nxrt::linux::add_fault_hook(lazy_fault);
    }
}

#[cfg(target_os = "linux")]
unsafe fn lazy_fault(f: &mut nxrt::linux::Fault) -> bool {
    f.sig == libc::SIGSEGV && bring_back(f.addr)
}

#[cfg(windows)]
unsafe extern "system" fn veh(
    ep: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
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

    /// A reserved page the heap has not committed yet is never held on the device; a committed one may be.
    #[cfg(windows)]
    #[test]
    fn only_committed_pages_can_be_held() {
        use windows_sys::Win32::System::Memory::{
            VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
        };
        let page = super::PAGE as usize;
        // SAFETY: a fresh reservation of four pages, the second committed, released at the end
        unsafe {
            let a = VirtualAlloc(core::ptr::null(), 4 * page, MEM_RESERVE, PAGE_READWRITE) as u64;
            assert_ne!(a, 0);
            assert!(!VirtualAlloc((a + page as u64) as _, page, MEM_COMMIT, PAGE_READWRITE).is_null());
            let r0 = super::committed_region(a);
            assert!(!r0.2 && r0.0 <= a && r0.1 == a + page as u64);
            let r1 = super::committed_region(a + page as u64);
            assert!(r1.2 && r1.1 == a + 2 * page as u64);
            assert!(!super::committed_region(a + 3 * page as u64).2);
            VirtualFree(a as _, 0, MEM_RELEASE);
        }
    }
}

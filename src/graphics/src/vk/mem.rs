//! NVN memory pools on a discrete GPU.
//!
//! On the console a pool is one memory both processors use; here every pool gets a placement:
//!
//! - **Vram** (CPU_NO_ACCESS): a device-local buffer, filled from the storage at its first GPU use (NVN copies a
//!   CPU-inaccessible pool's initial contents), then GPU-only.
//! - **Mirror** (CPU-visible, the default): a device-local copy. CPU writes are found by the OS's write watch on the
//!   guest pages (`GetWriteWatch`; a byte compare against a snapshot where the memory has no write watch) and
//!   uploaded at the start of each submission; ranges the GPU writes (copies, storage buffers, clears, uniform
//!   updates) are copied back into guest memory when the submission's fence completes, before any waiter sees the
//!   fence.
//! - **Host** (`NXGFX_POOL=host`): the guest pages imported into Vulkan (VK_EXT_external_memory_host): zero-copy,
//!   coherent, read by the GPU over PCIe. Falls back to Mirror when the range cannot be imported.
//! - **CpuOnly** (GPU_NO_ACCESS): no GPU buffer.
//!
//! Textures are images of their own (`image`); a pool's pages also tell `image` which CPU-filled textures changed.

use super::{Buf, Gpu};
use crate::nvn::obj::pool_t;
use crate::nvn::types::mem_flags;
use ash::vk;
use gpu_allocator::MemoryLocation;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

pub const PAGE: u64 = 4096;
/// Guest bytes the CPU changed inside a write-back's range while the GPU worked, kept (`land_changed`).
pub static WRITEBACK_KEPT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Vram,
    Mirror,
    Host,
    CpuOnly,
}

pub struct Pool {
    pub base: u64,
    pub size: u64,
    pub flags: u32,
    pub placement: Placement,
    pub buf: Option<Buf>,
    /// the buffer's offset of `base` (an imported range starts at the alignment boundary below it)
    pub buf_off: u64,
    pub watched: bool,
    pub snapshot: Option<Vec<u8>>,
    /// CPU write generation of each page of [page_lo, ...)
    pub page_gen: Vec<u32>,
    pub page_lo: u64,
    /// ranges to upload at the next submission whatever the watch says (flushes, CPU writes kept across a write-back)
    pub pending: Vec<(u64, u64)>,
    /// the device address of `base` in its buffer (0: none yet; raw global memory's pool table, `gmem`)
    pub da: u64,
    /// the allocation (VirtualQuery's allocation base) its first page is in, when it lies in one: watched pools of one
    /// allocation share one GetWriteWatch call a submission
    pub alloc_base: u64,
    /// the frame its CPU writes were last seen (cold pools are asked once a frame: `begin_submit`)
    pub dirty_frame: u64,
}

/// Moves whenever a pool comes, goes or gets its GPU buffer (the raw global memory table is written again).
pub static POOL_GEN: AtomicU64 = AtomicU64::new(1);

/// The pool buffers' usage: with shader device addresses when the device has them (raw global memory).
fn buf_usage(g: &Gpu) -> vk::BufferUsageFlags {
    if g.d.caps.rt {
        // pool memory is also ray-tracing geometry and instance input (vk/rt.rs)
        BUF_USAGE | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS | vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
    } else if g.d.caps.bda {
        BUF_USAGE | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
    } else {
        BUF_USAGE
    }
}

#[derive(Default)]
pub struct Pools {
    pub by_base: BTreeMap<u64, Pool>,
    pub gen: u32,
    /// the frame the last submission's pool sync was in
    pub scan_frame: u64,
}

pub static UPLOAD_BYTES: AtomicU64 = AtomicU64::new(0);
pub static WRITEBACK_BYTES: AtomicU64 = AtomicU64::new(0);
pub static WRITEBACK_NS: AtomicU64 = AtomicU64::new(0);

const BUF_USAGE: vk::BufferUsageFlags = vk::BufferUsageFlags::from_raw(
    vk::BufferUsageFlags::VERTEX_BUFFER.as_raw()
        | vk::BufferUsageFlags::INDEX_BUFFER.as_raw()
        | vk::BufferUsageFlags::UNIFORM_BUFFER.as_raw()
        | vk::BufferUsageFlags::STORAGE_BUFFER.as_raw()
        | vk::BufferUsageFlags::INDIRECT_BUFFER.as_raw()
        | vk::BufferUsageFlags::TRANSFER_SRC.as_raw()
        | vk::BufferUsageFlags::TRANSFER_DST.as_raw()
        | vk::BufferUsageFlags::UNIFORM_TEXEL_BUFFER.as_raw()
        | vk::BufferUsageFlags::STORAGE_TEXEL_BUFFER.as_raw(),
);

// ---------- Windows write watch ----------
#[cfg(windows)]
mod watch {
    use windows_sys::Win32::System::Memory::{GetWriteWatch, ResetWriteWatch};
    const WRITE_WATCH_FLAG_RESET: u32 = 1;
    /// The pages of [lo, hi) (page-aligned) written since the last reset; None when part of the range has no write
    /// watch. A range over several allocations is asked per allocation (GetWriteWatch fails across them: such a pool
    /// fell back to comparing a snapshot of all of it every submission, 40-50 ms a frame in Pokemon Violet).
    pub fn take(lo: u64, hi: u64, reset: bool) -> Option<Vec<u64>> {
        // one allocation (the usual case): one call; the region walk only where that fails (a walk over a pool split
        // into thousands of regions by page protections took 1.9 s a frame in Mario Kart 8 Deluxe)
        if let Some(v) = take_one(lo, hi, reset) {
            return Some(v);
        }
        let mut out = Vec::new();
        for (a, b) in allocations(lo, hi) {
            out.extend(take_one(a, b, reset)?);
        }
        Some(out)
    }
    /// The allocation base of [lo, hi) when the whole range lies in one allocation (0 otherwise).
    pub fn alloc_of(lo: u64, hi: u64) -> u64 {
        let a = allocations_with_base(lo, hi);
        if a.len() == 1 {
            a[0].2
        } else {
            0
        }
    }
    /// [lo, hi) split where its allocations (VirtualQuery's allocation base) change.
    fn allocations(lo: u64, hi: u64) -> Vec<(u64, u64)> {
        allocations_with_base(lo, hi).into_iter().map(|(a, b, _)| (a, b)).collect()
    }
    fn allocations_with_base(lo: u64, hi: u64) -> Vec<(u64, u64, u64)> {
        use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION};
        let mut out: Vec<(u64, u64, u64)> = Vec::new();
        let mut at = lo;
        while at < hi {
            let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
            if unsafe { VirtualQuery(at as *const _, &mut mbi, core::mem::size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
                out.push((at, hi, 0));
                break;
            }
            let e = (mbi.BaseAddress as u64 + mbi.RegionSize as u64).min(hi);
            if e <= at {
                out.push((at, hi, 0));
                break;
            }
            let ab = mbi.AllocationBase as u64;
            match out.last_mut() {
                Some(l) if l.2 == ab && l.1 == at => l.1 = e,
                _ => out.push((at, e, ab)),
            }
            at = e;
        }
        out
    }
    fn take_one(lo: u64, hi: u64, reset: bool) -> Option<Vec<u64>> {
        // one buffer of the thread's, reused (a buffer sized to the pool, zeroed at every call, was most of the cost
        // over hundreds of pools a submission: Pokemon Legends Z-A, ~400 mirrors); a full buffer asks again (with the
        // reset flag the pages it returned are not returned again), a peek takes what fits
        const CAP: usize = 4096;
        thread_local! {
            static BUF: std::cell::RefCell<Vec<usize>> = std::cell::RefCell::new(vec![0; CAP]);
        }
        BUF.with(|b| {
            let mut b = b.borrow_mut();
            let mut out = Vec::new();
            // a full buffer asks again from the page after the last one it returned (pages come in address order):
            // asking from `lo` again walked the whole range once per 4,096 written pages, so a scene that wrote
            // gigabytes of a pool held the replay for 28-34 s (Mario Kart World, lane K2)
            let (lo0, t0, mut calls) = (lo, std::time::Instant::now(), 0u32);
            let slow = |out: &Vec<u64>, calls: u32| {
                static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let ms = t0.elapsed().as_secs_f64() * 1e3;
                if ms > 100.0 && N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 20 {
                    crate::vklog!("write watch: {:#x}+{} MB took {ms:.0} ms ({} written pages, {calls} calls)", lo0, (hi - lo0) >> 20, out.len());
                }
            };
            let mut lo = lo;
            let _t = super::WatchTimer(std::time::Instant::now());
            loop {
                calls += 1;
                super::WATCH_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut count = CAP;
                let mut gran = 0u32;
                let r = unsafe {
                    GetWriteWatch(if reset { WRITE_WATCH_FLAG_RESET } else { 0 }, lo as *const _, (hi - lo) as usize, b.as_mut_ptr() as *mut *mut core::ffi::c_void, &mut count, &mut gran)
                };
                if r != 0 {
                    return None;
                }
                out.extend(b[..count].iter().map(|&p| p as u64));
                if count < CAP || !reset {
                    slow(&out, calls);
                    return Some(out);
                }
                lo = b[count - 1] as u64 + gran.max(4096) as u64;
                if lo >= hi {
                    slow(&out, calls);
                    return Some(out);
                }
            }
        })
    }
    pub fn reset(lo: u64, hi: u64) {
        if unsafe { ResetWriteWatch(lo as *const _, (hi - lo) as usize) } == 0 {
            return;
        }
        for (a, b) in allocations(lo, hi) {
            unsafe { ResetWriteWatch(a as *const _, (b - a) as usize) };
        }
    }
}
// ---------- Linux write watch (the runtime's userfaultfd watch, PAGEMAP_SCAN: one scan over any range) ----------
#[cfg(target_os = "linux")]
mod watch {
    pub fn take(lo: u64, hi: u64, reset: bool) -> Option<Vec<u64>> {
        // only memory a watched reservation holds answers (other memory has no write-protect marker: every page of
        // it would read as written)
        if nxrt::linux::allocation_of(lo, hi) == 0 || !nxrt::linux::watched(lo) {
            return None;
        }
        nxrt::linux::written_pages(lo, hi - lo, reset)
    }
    pub fn alloc_of(lo: u64, hi: u64) -> u64 {
        nxrt::linux::allocation_of(lo, hi)
    }
    pub fn reset(lo: u64, hi: u64) {
        let _ = take(lo, hi, true);
    }
}

/// [a, a + n) of guest memory as it reads, the parts not committed as zeros (reading them would fault).
#[cfg(windows)]
pub fn guest_bytes(a: u64, n: u64) -> std::borrow::Cow<'static, [u8]> {
    // ReadProcessMemory on this process: a read that fails on pages with no storage instead of faulting, with no
    // VirtualQuery (whose region size scan over a large uniform reservation took ~2.5 ms a call: Violet's texture
    // checks, 60 ms a frame)
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut v = vec![0u8; n as usize];
    let rd = |at: u64, dst: &mut [u8]| -> bool {
        let mut got = 0usize;
        unsafe { ReadProcessMemory(GetCurrentProcess(), at as *const _, dst.as_mut_ptr() as *mut _, dst.len(), &mut got) != 0 && got == dst.len() }
    };
    if n == 0 || rd(a, &mut v) {
        return std::borrow::Cow::Owned(v);
    }
    // part of it has no storage: page by page, those as zeros
    let mut at = a;
    while at < a + n {
        let e = ((at & !(PAGE - 1)) + PAGE).min(a + n);
        let o = (at - a) as usize;
        if !rd(at, &mut v[o..o + (e - at) as usize]) {
            v[o..o + (e - at) as usize].fill(0);
        }
        at = e;
    }
    std::borrow::Cow::Owned(v)
}
#[cfg(target_os = "linux")]
pub fn guest_bytes(a: u64, n: u64) -> std::borrow::Cow<'static, [u8]> {
    // process_vm_readv on this process: a read that fails on inaccessible pages (lazily written-back ones, decommitted
    // ones) instead of faulting, as ReadProcessMemory does on Windows
    let mut v = vec![0u8; n as usize];
    if n == 0 || nxrt::linux::read_nofault(a, &mut v) {
        return std::borrow::Cow::Owned(v);
    }
    let mut at = a;
    while at < a + n {
        let e = ((at & !(PAGE - 1)) + PAGE).min(a + n);
        let o = (at - a) as usize;
        let dst = &mut v[o..o + (e - at) as usize];
        if !nxrt::linux::read_nofault(at, dst) {
            dst.fill(0);
        }
        at = e;
    }
    std::borrow::Cow::Owned(v)
}
#[cfg(not(any(windows, target_os = "linux")))]
pub fn guest_bytes(a: u64, n: u64) -> std::borrow::Cow<'static, [u8]> {
    let c = committed(a, n);
    if c.len() == 1 && c[0] == (a, n) {
        // SAFETY: committed guest memory
        return std::borrow::Cow::Borrowed(unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) });
    }
    let mut v = vec![0u8; n as usize];
    for (ca, cn) in c {
        let o = (ca - a) as usize;
        // SAFETY: committed guest memory
        v[o..o + cn as usize].copy_from_slice(unsafe { core::slice::from_raw_parts(ca as *const u8, cn as usize) });
    }
    std::borrow::Cow::Owned(v)
}

/// The committed parts of [a, a + n) (VirtualQuery): what the guest has written or reserved with content.
#[cfg(windows)]
pub fn committed(a: u64, n: u64) -> Vec<(u64, u64)> {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    let mut out: Vec<(u64, u64)> = Vec::new();
    let end = a + n;
    let mut at = a;
    while at < end {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        let r = unsafe { VirtualQuery(at as *const _, &mut mbi, core::mem::size_of::<MEMORY_BASIC_INFORMATION>()) };
        if r == 0 {
            // not queryable: take the rest as it is
            out.push((at, end - at));
            break;
        }
        let rb = mbi.BaseAddress as u64;
        let re = (rb + mbi.RegionSize as u64).min(end);
        if re <= at {
            break;
        }
        if mbi.State == MEM_COMMIT {
            match out.last_mut() {
                Some(l) if l.0 + l.1 == at => l.1 += re - at,
                _ => out.push((at, re - at)),
            }
        }
        at = re;
    }
    out
}
/// The committed parts of [a, a + n) (the runtime's page states); memory outside its reservations is taken whole.
#[cfg(target_os = "linux")]
pub fn committed(a: u64, n: u64) -> Vec<(u64, u64)> {
    if nxrt::linux::query(a).is_none() && nxrt::linux::allocation_of(a, a + n) == 0 {
        return vec![(a, n)];
    }
    nxrt::linux::committed_runs(a, n)
}
#[cfg(not(any(windows, target_os = "linux")))]
pub fn committed(a: u64, n: u64) -> Vec<(u64, u64)> {
    vec![(a, n)]
}

fn page_range(a: u64, n: u64) -> (u64, u64) {
    (a & !(PAGE - 1), (a + n + PAGE - 1) & !(PAGE - 1))
}

fn placement_for(flags: u32) -> Placement {
    if flags & mem_flags::GPU_NO_ACCESS != 0 {
        return Placement::CpuOnly;
    }
    if flags & mem_flags::CPU_NO_ACCESS != 0 {
        return Placement::Vram;
    }
    match std::env::var("NXGFX_POOL").as_deref() {
        Ok("host") => Placement::Host,
        Ok("vram") => Placement::Vram,
        _ => Placement::Mirror,
    }
}

/// nvnMemoryPoolInitialize: the pool's placement (its GPU buffer comes at its first GPU use).
pub fn pool_add(p: &pool_t) {
    super::capture::pool_add(p);
    let Ok(g) = Gpu::get() else { return };
    let base = p.mem as u64;
    let placement = placement_for(p.flags);
    let (lo, hi) = page_range(base, p.size);
    let watched = matches!(placement, Placement::Mirror | Placement::Host) && watch::take(lo, hi, false).is_some();
    let pool = Pool {
        base,
        size: p.size,
        flags: p.flags,
        placement,
        buf: None,
        buf_off: 0,
        watched,
        snapshot: None,
        page_gen: vec![0; ((hi - lo) / PAGE) as usize],
        page_lo: lo,
        pending: Vec::new(),
        da: 0,
        alloc_base: if watched { watch::alloc_of(lo, hi) } else { 0 },
        dirty_frame: super::present::frames(),
    };
    if std::env::var("NXGFX_POOL_LOG").is_ok() {
        crate::vklog!("pool {base:#x} size {:#x} flags {:#x}: {placement:?}{}", p.size, p.flags, if watched { " (write watch)" } else { "" });
    }
    let old = g.mem.lock().unwrap().by_base.insert(base, pool);
    POOL_GEN.fetch_add(1, Ordering::Relaxed);
    if let Some(o) = old {
        release(g, o);
    }
    g.tables.requeue_buf_texs(base, base + p.size);
}

fn release(g: &Gpu, p: Pool) {
    POOL_GEN.fetch_add(1, Ordering::Relaxed);
    // pages of it held on the device (lazy write-back) come back now, from this buffer, while it still exists: left
    // in the lazy table they named a freed buffer, and the next touch of those pages (a new pool's buffer over the
    // same memory brings its range back first) copied from it on the GPU: an invalid read inside a buffer unbound
    // 34 s before (MKW `K2-mkw11`, lane K2)
    if p.buf.is_some() {
        super::lazy::touch(p.base, p.size);
    }
    if let Some(b) = &p.buf {
        if let Some(m) = ADDRS.write().unwrap().as_mut() {
            m.remove(&vk::Handle::as_raw(b.buf));
        }
    }
    if let Some(b) = p.buf {
        super::lost::note_gone(|| format!("pool buffer {:#x} of pool {:#x} (+{:#x}), device address {:#x}", vk::Handle::as_raw(b.buf), p.base, p.size, p.da));
        g.q.defer_after_use(Box::new(move |g: &Gpu| g.free_buffer(b)));
    }
}

/// nvnMemoryPoolFinalize.
pub fn pool_del(base: u64) {
    super::capture::pool_del(base);
    let Some(g) = Gpu::try_get() else { return };
    let p = g.mem.lock().unwrap().by_base.remove(&base);
    POOL_GEN.fetch_add(1, Ordering::Relaxed);
    if let Some(p) = p {
        g.images.lock().unwrap().drop_pool(base, p.size);
        g.tables.requeue_buf_texs(base, base + p.size);
        release(g, p);
    }
}

fn find(m: &Pools, a: u64, n: u64) -> Option<u64> {
    let (b, p) = m.by_base.range(..=a).next_back()?;
    if a + n <= p.base + p.size {
        Some(*b)
    } else {
        None
    }
}

/// Whether [a, a + n) lies in one known memory pool.
pub fn addr_known(a: u64, n: u64) -> bool {
    match Gpu::try_get() {
        Some(g) => find(&g.mem.lock().unwrap(), a, n).is_some(),
        None => false,
    }
}

/// The bytes from `a` to the end of its memory pool (0 when `a` is in none).
pub fn room(a: u64) -> u64 {
    let Some(g) = Gpu::try_get() else { return 0 };
    let a = if crate::nvn::vpool::is_sparse(a) { crate::nvn::vpool::translate(a, 1).unwrap_or(0) } else { a };
    let m = g.mem.lock().unwrap();
    match find(&m, a, 1) {
        Some(b) => {
            let p = &m.by_base[&b];
            p.base + p.size - a
        }
        None => 0,
    }
}

/// The OS write watch of [a, a + n) reset (the host wrote it, not the game).
/// Every page of [a, a + n) is committed and writable by the host now (a texture write-back writes only then: a
/// guest page read-only or not committed faulted in the host copy, lane K2).
#[cfg(windows)]
pub fn host_writable(a: u64, n: u64) -> bool {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_READWRITE};
    let (mut at, end) = (a, a.saturating_add(n.max(1)));
    while at < end {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        if unsafe { VirtualQuery(at as *const _, &mut mbi, core::mem::size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 || mbi.State != MEM_COMMIT {
            return false;
        }
        if mbi.Protect & 0xff != PAGE_READWRITE || mbi.Protect & PAGE_GUARD != 0 {
            return false;
        }
        at = mbi.BaseAddress as u64 + mbi.RegionSize as u64;
    }
    true
}
#[cfg(not(windows))]
pub fn host_writable(a: u64, n: u64) -> bool {
    nxrt::mem::readable(a, n) && matches!(nxrt::linux::query(a), Some((true, prot, _, _)) if prot & 2 != 0)
}

/// Whether the CPU wrote any page of [a, a + n) since the write watch was last taken there (a peek: nothing reset).
pub fn cpu_written(a: u64, n: u64) -> bool {
    let (lo, hi) = page_range(a, n);
    watch::take(lo, hi, false).is_some_and(|p| !p.is_empty())
}

pub fn reset_watch(a: u64, n: u64) {
    let (lo, hi) = page_range(a, n);
    watch::reset(lo, hi);
}

/// Flushed by the CPU (nvnMemoryPoolFlushMappedRange): uploaded at the next submission even without a write watch.
pub fn cpu_flushed(a: u64, n: u64) {
    let Some(g) = Gpu::try_get() else { return };
    let mut m = g.mem.lock().unwrap();
    if let Some(b) = find(&m, a, n.max(1)) {
        m.by_base.get_mut(&b).unwrap().pending.push((a, n));
    }
}

/// The CPU's writes to [a, a + n) of a cold pool, taken now: a buffer -> texture copy reads them in this submission.
/// `begin_submit` asks a cold pool (no CPU write for 60 frames) once a frame only, so a texture copied from memory the
/// CPU wrote since kept the old bytes for good (the copy's texture is the GPU's from then on): Mario Kart World's menu
/// text lost letters, the glyphs it writes into a cold pool just before copying them (mkw-recomp #250, lane K2).
/// `NXGFX_COPY_SYNC=0` off.
pub fn sync_range(g: &Gpu, sub: &mut super::queue::Sub, a: u64, n: u64) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("NXGFX_COPY_SYNC").as_deref() != Ok("0")) || n == 0 {
        return;
    }
    let frame = super::present::frames();
    let mut m = g.mem.lock().unwrap();
    let gen = m.gen;
    let Some(b) = find(&m, a, n) else { return };
    let p = m.by_base.get_mut(&b).unwrap();
    // a hot pool was asked at this submission's start; only a watched mirror's pages are taken here
    if !p.watched || p.placement != Placement::Mirror || frame.saturating_sub(p.dirty_frame) <= 60 {
        return;
    }
    let (lo, hi) = page_range(a, n);
    let Some(pages) = watch::take(lo, hi, true) else { return };
    if pages.is_empty() {
        return;
    }
    let ranges = p.take_cpu_writes(gen, Some(pages));
    p.dirty_frame = frame;
    if let Some(buf) = p.buf.as_ref() {
        for &(ra, rn) in &ranges {
            let src = unsafe { core::slice::from_raw_parts(ra as *const u8, rn as usize) };
            sub.upload(g, src, buf.buf, p.buf_off + (ra - p.base));
            UPLOAD_BYTES.fetch_add(rn, Ordering::Relaxed);
        }
    }
    static N: AtomicU64 = AtomicU64::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 4 {
        crate::vklog!("a texture copy's source {a:#x} (+{n:#x}) in a cold pool: {} CPU-written range(s) taken first", ranges.len());
    }
}

/// nvnMemoryPoolInvalidateMappedRange: GPU writes reach guest memory at their fence, before SyncWait returns.
pub fn cpu_invalidate(_a: u64, _n: u64) {}

impl Pool {
    fn ensure_buffer(&mut self, g: &Gpu, sub: &mut super::queue::Sub) -> Result<(), String> {
        if self.buf.is_some() {
            return Ok(());
        }
        match self.placement {
            Placement::CpuOnly => return Err(format!("pool {:#x} has no GPU access", self.base)),
            Placement::Host => match import(g, self.base, self.size) {
                Ok((b, off)) => {
                    self.buf = Some(b);
                    self.buf_off = off;
                    self.note_address(g);
                    return Ok(());
                }
                Err(e) => {
                    crate::log::red_once(&format!("pool {:#x}: host import failed ({e}); mirrored instead", self.base));
                    self.placement = Placement::Mirror;
                    let (lo, hi) = page_range(self.base, self.size);
                    self.watched = watch::take(lo, hi, false).is_some();
                }
            },
            _ => {}
        }
        let b = g.buffer(self.size, buf_usage(g), MemoryLocation::GpuOnly, "pool")?;
        // The storage now: a Vram pool's initial content, a mirror's current one. Only the committed parts of the
        // guest's memory hold anything (a large pool the game has not written yet is zero and is not read here: reading
        // it would commit it, and staging it would double it in host memory; Mario Kart World's pools)
        super::lazy::touch(self.base, self.size);
        unsafe { g.dev().cmd_fill_buffer(sub.pre, b.buf, 0, vk::WHOLE_SIZE, 0) };
        super::queue::full_barrier(g, sub.pre);
        let mut staged = 0u64;
        for (a, n) in committed(self.base, self.size) {
            let src = unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) };
            // in pieces of at most 64 MB (staging chunks)
            for (k, part) in src.chunks(64 << 20).enumerate() {
                sub.upload(g, part, b.buf, a - self.base + (k as u64) * (64 << 20));
            }
            staged += n;
        }
        UPLOAD_BYTES.fetch_add(staged, Ordering::Relaxed);
        if self.size >= 256 << 20 {
            crate::vklog!("pool {:#x}: {} MB on the GPU, {} MB of it committed and uploaded ({:?})", self.base, self.size >> 20, staged >> 20, self.placement);
        }
        if self.placement == Placement::Mirror {
            let (lo, hi) = page_range(self.base, self.size);
            if self.watched {
                let _ = watch::take(lo, hi, true);
            } else {
                self.snapshot = Some(guest_bytes(self.base, self.size).into_owned());
            }
        }
        self.buf = Some(b);
        self.note_address(g);
        Ok(())
    }

    /// The buffer's device address (raw global memory) once it has one.
    fn note_address(&mut self, g: &Gpu) {
        if g.d.caps.bda {
            if let Some(b) = &self.buf {
                let ai = vk::BufferDeviceAddressInfo::default().buffer(b.buf);
                let start = unsafe { g.dev().get_buffer_device_address(&ai) };
                self.da = start + self.buf_off;
                ADDRS.write().unwrap().get_or_insert_with(HashMap::new).insert(vk::Handle::as_raw(b.buf), start);
            }
        }
        POOL_GEN.fetch_add(1, Ordering::Relaxed);
    }

    /// CPU-written ranges since the last call (absolute addresses), page generations bumped. `watched_pages`: this
    /// pool's written pages already taken from the write watch (`begin_submit` asks once for neighbouring pools).
    fn take_cpu_writes(&mut self, gen: u32, watched_pages: Option<Vec<u64>>) -> Vec<(u64, u64)> {
        let mut out: Vec<(u64, u64)> = std::mem::take(&mut self.pending);
        if !matches!(self.placement, Placement::Mirror | Placement::Host) {
            return out;
        }
        let (lo, hi) = page_range(self.base, self.size);
        if self.watched {
            if let Some(pages) = watched_pages.or_else(|| watch::take(lo, hi, true)) {
                for p in pages {
                    out.push((p, PAGE));
                }
            }
        } else if let Some(snap) = self.snapshot.as_mut() {
            // only the committed parts can have been written (reading the rest faults)
            for (ca, cn) in committed(self.base, self.size) {
                let cur = unsafe { core::slice::from_raw_parts(ca as *const u8, cn as usize) };
                let o0 = (ca - self.base) as usize;
                let mut i = 0usize;
                while i < cur.len() {
                    let e = (i + PAGE as usize).min(cur.len());
                    if cur[i..e] != snap[o0 + i..o0 + e] {
                        snap[o0 + i..o0 + e].copy_from_slice(&cur[i..e]);
                        out.push((ca + i as u64, (e - i) as u64));
                    }
                    i = e;
                }
            }
        }
        // clip to the pool, mark generations, merge
        let mut v: Vec<(u64, u64)> = out
            .into_iter()
            .filter_map(|(a, n)| {
                let s = a.max(self.base);
                let e = (a + n).min(self.base + self.size);
                (e > s).then_some((s, e))
            })
            .collect();
        v.sort();
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for (s, e) in v {
            match merged.last_mut() {
                Some(l) if s <= l.1 => l.1 = l.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        for &(s, e) in &merged {
            let p0 = ((s - self.page_lo) / PAGE) as usize;
            let p1 = ((e - self.page_lo).div_ceil(PAGE)) as usize;
            let np = self.page_gen.len();
            for pg in &mut self.page_gen[p0.min(np)..p1.min(np)] {
                *pg = gen;
            }
        }
        merged.into_iter().map(|(s, e)| (s, e - s)).collect()
    }
}

/// Imports [base, base + size), widened to the import alignment, as a buffer; returns it and base's offset in it.
fn import(g: &Gpu, base: u64, size: u64) -> Result<(Buf, u64), String> {
    if !g.d.caps.host_import {
        return Err("no VK_EXT_external_memory_host".into());
    }
    let al = g.d.caps.import_align.max(PAGE);
    let lo = base & !(al - 1);
    let hi = (base + size + al - 1) & !(al - 1);
    let ht = vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT;
    let mut hp = vk::MemoryHostPointerPropertiesEXT::default();
    let r = unsafe { (g.d.emh.fp().get_memory_host_pointer_properties_ext)(g.dev().handle(), ht, lo as *const core::ffi::c_void, &mut hp) };
    if r != vk::Result::SUCCESS {
        return Err(format!("host pointer properties: {r}"));
    }
    let mut ext = vk::ExternalMemoryBufferCreateInfo::default().handle_types(ht);
    let ci = vk::BufferCreateInfo::default().size(hi - lo).usage(buf_usage(g)).push_next(&mut ext);
    let buf = unsafe { g.dev().create_buffer(&ci, None) }.map_err(|e| format!("{e}"))?;
    let req = unsafe { g.dev().get_buffer_memory_requirements(buf) };
    let ty = g.d.mem_type(req.memory_type_bits & hp.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE).ok_or("no memory type")?;
    let mut imp = vk::ImportMemoryHostPointerInfoEXT::default().handle_type(ht).host_pointer(lo as *mut core::ffi::c_void);
    let mut fl = vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
    let mut ai = vk::MemoryAllocateInfo::default().allocation_size(hi - lo).memory_type_index(ty).push_next(&mut imp);
    if g.d.caps.bda {
        ai = ai.push_next(&mut fl);
    }
    let mem = unsafe { g.dev().allocate_memory(&ai, None) }.map_err(|e| {
        unsafe { g.dev().destroy_buffer(buf, None) };
        format!("import: {e}")
    })?;
    unsafe { g.dev().bind_buffer_memory(buf, mem, 0) }.map_err(|e| format!("{e}"))?;
    let addr = if g.d.caps.bda { unsafe { g.dev().get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buf)) } } else { 0 };
    Ok((Buf { buf, alloc: None, mem, size: hi - lo, ptr: lo as *mut u8, addr }, base - lo))
}

/// Pool buffers' device addresses by handle (entered when a pool gets its buffer, removed when it goes).
static ADDRS: std::sync::RwLock<Option<HashMap<u64, u64>>> = std::sync::RwLock::new(None);

/// The device address of a pool's buffer (descriptor buffers).
pub fn buffer_address(buf: vk::Buffer) -> Option<u64> {
    ADDRS.read().unwrap().as_ref().and_then(|m| m.get(&vk::Handle::as_raw(buf)).copied())
}

/// A GPU address range as (buffer, offset, pool base); creates the pool's buffer at its first use.
pub fn resolve(g: &Gpu, sub: &mut super::queue::Sub, a: u64, n: u64) -> Option<(vk::Buffer, u64, u64)> {
    super::perf::timed_fine(super::perf::C::ResolveNs, || resolve_in(g, sub, a, n))
}

fn resolve_in(g: &Gpu, sub: &mut super::queue::Sub, a: u64, n: u64) -> Option<(vk::Buffer, u64, u64)> {
    // a sparse virtual pool's range: the physical memory its page table maps there (contiguous pages only)
    let a = if crate::nvn::vpool::is_sparse(a) {
        match crate::nvn::vpool::translate(a, n) {
            Some(p) => p,
            None => {
                crate::log::red_once(&format!("GPU address {a:#x} (+{n:#x}) of a sparse virtual pool is not mapped to contiguous memory"));
                return None;
            }
        }
    } else {
        a
    };
    // the 16 pools this thread resolved last, while no pool came, went or got its buffer (thousands of draws a frame bind
    // ranges of the same few (Donkey Kong Bananza: dozens, 7.5 ms of lookups a frame with 4 kept): the pool lock and lookup were most of the replay's 1.1 ms of `memory` in Z-A; Turbo's
    // vkm_use fast path)
    thread_local! {
        // (generation, pool base, pool size, the next pool's base, buffer, buffer offset of base)
        static LAST: std::cell::Cell<[(u64, u64, u64, u64, vk::Buffer, u64); 16]> = const { std::cell::Cell::new([(0, 0, 0, 0, vk::Buffer::null(), 0); 16]) };
    }
    let gen = POOL_GEN.load(Ordering::Acquire);
    let n1 = n.max(1);
    // `find` takes the pool with the highest base at or below the address (pools laid over each other): a hit only
    // below the next pool's base
    let hit = LAST.with(|l| l.get().iter().find(|e| e.0 == gen && a >= e.1 && a + n1 <= e.1 + e.2 && a < e.3 && e.4 != vk::Buffer::null()).copied());
    if let Some((_, base, _, _, buf, off)) = hit {
        return Some((buf, off + (a - base), base));
    }
    let mut m = g.mem.lock().unwrap();
    let Some(b) = find(&m, a, n1) else {
        static N: AtomicU64 = AtomicU64::new(0);
        if N.fetch_add(1, Ordering::Relaxed) < 8 {
            crate::log::red(&format!("GPU address {a:#x} (+{n:#x}) is in no memory pool"));
        }
        return None;
    };
    let p = m.by_base.get_mut(&b).unwrap();
    if let Err(e) = p.ensure_buffer(g, sub) {
        crate::log::red_once(&e);
        return None;
    }
    let (buf, off) = (p.buf.as_ref().unwrap().buf, p.buf_off);
    // the generation after the buffer exists (its creation moved it)
    let gen = POOL_GEN.load(Ordering::Acquire);

    let (pb, ps) = (p.base, p.size);
    let next = m.by_base.range(pb + 1..).next().map(|(k, _)| *k).unwrap_or(u64::MAX);
    LAST.with(|l| {
        let mut v = l.get();
        v.rotate_right(1);
        v[0] = (gen, pb, ps, next, buf, off);
        l.set(v);
    });
    Some((buf, off + (a - pb), b))
}

/// Guest ranges a submission in flight writes on the GPU and lands back in guest memory at its completion (its
/// write-backs): (address, bytes, count of submissions writing it). A CPU-dirty page over one is not uploaded while
/// it is in flight: the guest bytes there are older than the GPU's, and uploading them overwrote the GPU's result
/// before it came back (asynchronous replay made the window wide: Mario Kart 8 Deluxe's GPU-computed values read as
/// stale, the black stretches). Those pieces upload once it has landed (NXGFX_WB_GUARD=0: uploaded at once, as before).
static INFLIGHT: std::sync::Mutex<Inflight> = std::sync::Mutex::new(Inflight { by_start: std::collections::BTreeMap::new(), max_len: 0 });

/// The in-flight write-back ranges by start: ((start, end) -> how many submissions), and the longest one's length.
struct Inflight {
    by_start: std::collections::BTreeMap<(u64, u64), u32>,
    max_len: u64,
}
impl Inflight {
    /// The ranges overlapping [lo, hi).
    fn overlapping(&self, lo: u64, hi: u64) -> Vec<(u64, u64)> {
        self.by_start
            .range((lo.saturating_sub(self.max_len), 0)..(hi, 0))
            .filter(|((s, e), _)| *s < hi && *e > lo)
            .map(|((s, e), _)| (*s, e - s))
            .collect()
    }
}

fn wb_guard() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_WB_GUARD").as_deref() != Ok("0"))
}

/// A submission with these write-backs is going to the GPU.
pub fn inflight_add(wbs: &[(u64, u64)]) {
    if wb_guard() && !wbs.is_empty() {
        let mut v = INFLIGHT.lock().unwrap();
        for &(a, n) in wbs {
            *v.by_start.entry((a, a + n)).or_insert(0) += 1;
            v.max_len = v.max_len.max(n);
        }
    }
}

/// Its write-backs landed.
fn inflight_remove(wbs: &[(u64, u64)]) {
    if !wb_guard() || wbs.is_empty() {
        return;
    }
    let mut v = INFLIGHT.lock().unwrap();
    for &(a, n) in wbs {
        if let Some(c) = v.by_start.get_mut(&(a, a + n)) {
            *c -= 1;
            if *c == 0 {
                v.by_start.remove(&(a, a + n));
            }
        }
    }
    if v.by_start.is_empty() {
        v.max_len = 0;
    }
}

/// [a, a + n) split into what may upload now and what waits for in-flight write-backs.
fn split_inflight(a: u64, n: u64, inflight: &[(u64, u64)]) -> (Vec<(u64, u64)>, Vec<(u64, u64)>) {
    let mut now = vec![(a, a + n)];
    let mut later = Vec::new();
    for &(wa, wn) in inflight {
        let (ws, we) = (wa, wa + wn);
        let mut next = Vec::with_capacity(now.len() + 1);
        for (s, e) in now {
            if we <= s || ws >= e {
                next.push((s, e));
                continue;
            }
            if s < ws {
                next.push((s, ws));
            }
            if we < e {
                next.push((we, e));
            }
            later.push((s.max(ws), e.min(we)));
        }
        now = next;
    }
    (now.into_iter().map(|(s, e)| (s, e - s)).collect(), later.into_iter().map(|(s, e)| (s, e - s)).collect())
}

/// Uploads held back for an in-flight write-back (submissions that had some).
pub static WB_DEFERRED: AtomicU64 = AtomicU64::new(0);
/// Removes landed write-backs from INFLIGHT when the landing is over (after the guest bytes are written).
struct InflightDone(Vec<(u64, u64)>);
impl Drop for InflightDone {
    fn drop(&mut self) {
        inflight_remove(&self.0);
    }
}

/// GetWriteWatch calls and their time (pool sync's breakdown, logged with NXGFX_PROFILE=1).
pub static WATCH_CALLS: AtomicU64 = AtomicU64::new(0);
pub static WATCH_NS: AtomicU64 = AtomicU64::new(0);
pub static WATCH_SUBS: AtomicU64 = AtomicU64::new(0);
pub static WATCH_POOLS: AtomicU64 = AtomicU64::new(0);
struct WatchTimer(std::time::Instant);
impl Drop for WatchTimer {
    fn drop(&mut self) {
        WATCH_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

/// Start of a submission: every CPU write since the last one goes up to the mirrors (recorded into the
/// submission's upload command buffer). Returns the written ranges (textures over them may need a new upload).
pub fn begin_submit(g: &Gpu, sub: &mut super::queue::Sub) -> Vec<(u64, u64)> {
    let mut m = g.mem.lock().unwrap();
    m.gen = m.gen.wrapping_add(1);
    let gen = m.gen;
    let mut all = Vec::new();
    // the write watch asked once per run of watched pools whose pages touch (Pokemon Legends Z-A has ~400 mirrors,
    // most of them neighbours: one GetWriteWatch each was 3.5 ms a frame of `pools`; Mario Kart World 21 ms)
    let mut taken: HashMap<u64, Vec<u64>> = HashMap::new();
    let cold_skipped: std::collections::HashSet<u64>;
    {
        // cold pools (no CPU write for 60 frames) are asked once a frame, not at every submission: GetWriteWatch walks
        // every page of its range, and Donkey Kong Bananza's ~1,300 mirrors (1.9 GB) at 9 submissions a frame cost
        // ~33 ms a frame. A write to a cold pool reaches the GPU at the next frame's first submission (the pool is hot
        // from then on). NXGFX_POOL_COLD=0: every pool at every submission.
        let frame = super::present::frames();
        static COLD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let cold_on = *COLD.get_or_init(|| std::env::var("NXGFX_POOL_COLD").as_deref() != Ok("0"));
        let new_frame = m.scan_frame != frame;
        m.scan_frame = frame;
        let skip = |p: &Pool| cold_on && !new_frame && frame.saturating_sub(p.dirty_frame) > 60;
        cold_skipped = m.by_base.values().filter(|p| p.watched && skip(p)).map(|p| p.base).collect();
        let watched: Vec<(u64, u64, u64, u64)> = m
            .by_base
            .values()
            .filter(|p| p.watched && matches!(p.placement, Placement::Mirror | Placement::Host) && !skip(p))
            .map(|p| {
                let (lo, hi) = page_range(p.base, p.size);
                (p.base, lo, hi, p.alloc_base)
            })
            .collect();
        WATCH_POOLS.fetch_add(watched.len() as u64, Ordering::Relaxed);
        let subs = WATCH_SUBS.fetch_add(1, Ordering::Relaxed) + 1;
        if subs % 600 == 0 && super::perf::fine() {
            let (c, ns, np) = (WATCH_CALLS.swap(0, Ordering::Relaxed), WATCH_NS.swap(0, Ordering::Relaxed), WATCH_POOLS.swap(0, Ordering::Relaxed));
            crate::vklog!("pool sync, last 600 submissions: {:.0} watched pools, {:.0} write-watch calls, {:.2} ms in them a submission", np as f64 / 600.0, c as f64 / 600.0, ns as f64 / 600.0 / 1e6);
        }
        // a run: pools whose pages touch, or pools of one allocation with gaps between them (one call over the run
        // instead of one per pool: Donkey Kong Bananza has ~1,300 mirrors, 9 submissions a frame)
        let mut i = 0;
        while i < watched.len() {
            let (mut j, lo, mut hi, ab) = (i + 1, watched[i].1, watched[i].2, watched[i].3);
            // a gap inside one allocation joins the run only while the pools cover most of the span (GetWriteWatch
            // walks every page of its range: Mario Kart World's pools sit far apart in one large reservation)
            let mut covered = hi - lo;
            while j < watched.len() && (watched[j].1 <= hi || (ab != 0 && watched[j].3 == ab && watched[j].1 - hi <= (covered / 2).max(4 << 20))) {
                covered += watched[j].2.saturating_sub(hi.max(watched[j].1));
                hi = hi.max(watched[j].2);
                j += 1;
            }
            if j - i > 1 {
                if let Some(pages) = watch::take(lo, hi, true) {
                    // each pool its own pages (a page two pools share goes to both)
                    for &(base, plo, phi, _) in &watched[i..j] {
                        let a = pages.partition_point(|&p| p < plo);
                        let b = pages.partition_point(|&p| p < phi);
                        taken.insert(base, pages[a..b].to_vec());
                    }
                }
            }
            i = j;
        }
    }
    for p in m.by_base.values_mut() {
        if !matches!(p.placement, Placement::Mirror | Placement::Host) && p.pending.is_empty() {
            continue;
        }
        if p.pending.is_empty() && cold_skipped.contains(&p.base) {
            continue;
        }
        let pre = taken.remove(&p.base);
        let mut ranges = p.take_cpu_writes(gen, pre);
        // pieces under a write-back still in flight wait (in `pending`) until it has landed
        if wb_guard() {
            let inflight = INFLIGHT.lock().unwrap();
            if !inflight.by_start.is_empty() && !ranges.is_empty() {
                let mine = inflight.overlapping(p.base, p.base + p.size);
                if !mine.is_empty() {
                    let mut keep = Vec::new();
                    for &(a, n) in &ranges {
                        let (now, later) = split_inflight(a, n, &mine);
                        keep.extend(now);
                        p.pending.extend(later);
                    }
                    if !p.pending.is_empty() {
                        WB_DEFERRED.fetch_add(1, Ordering::Relaxed);
                    }
                    ranges = keep;
                }
            }
        }
        if !ranges.is_empty() {
            p.dirty_frame = super::present::frames();
        }
        if let Some(b) = p.buf.as_ref() {
            if p.placement == Placement::Mirror || p.placement == Placement::Vram {
                for &(a, n) in &ranges {
                    let src = unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) };
                    sub.upload(g, src, b.buf, p.buf_off + (a - p.base));
                    UPLOAD_BYTES.fetch_add(n, Ordering::Relaxed);
                }
            }
        }
        all.extend(ranges);
    }
    all
}

/// The GPU writes [a, a + n) in this submission: a mirrored range is copied back into guest memory at its fence.
pub fn note_gpu_write(g: &Gpu, sub: &mut super::queue::Sub, a: u64, n: u64) {
    if n == 0 {
        return;
    }
    {
        static W: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
        if let Some(w) = *W.get_or_init(|| std::env::var("NXGFX_WATCH_ADDR").ok().and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok())) {
            if a < w + 0x2000000 && w < a + n {
                static N: AtomicU64 = AtomicU64::new(0);
                if N.fetch_add(1, Ordering::Relaxed) < 40 {
                    crate::vklog!("watch {w:#x}: the GPU writes {a:#x} (+{n:#x})");
                }
            }
        }
    }
    let m = g.mem.lock().unwrap();
    let Some(b) = find(&m, a, n) else { return };
    let p = &m.by_base[&b];
    if p.placement == Placement::Mirror && p.buf.is_some() {
        super::perf::inc(super::perf::C::Writebacks);
        let (buf, off) = (p.buf.as_ref().unwrap().buf, p.buf_off + (a - p.base));
        // ranges that follow each other (or one inside another) in the same pool are one copy
        for w in sub.writebacks.iter_mut().rev() {
            if w.0 != b {
                continue;
            }
            if a >= w.1 && a + n <= w.1 + w.2 {
                return;
            }
            if a == w.1 + w.2 {
                w.2 += n;
                return;
            }
            if a + n == w.1 {
                w.1 = a;
                w.2 += n;
                w.4 = off;
                return;
            }
        }
        sub.writebacks.push((b, a, n, buf, off));
    }
}

/// Completion of a submission: its write-backs land in guest memory. CPU writes the game made meanwhile to the same
/// pages stay marked for the next upload; the write-back's own stores are not taken for CPU writes.
pub fn apply_writebacks(g: &Gpu, wbs: &[(u64, u64, u64, *const u8, *const u8)]) {
    if wbs.is_empty() {
        return;
    }
    let landed: Vec<(u64, u64)> = wbs.iter().map(|w| (w.1, w.2)).collect();
    let _done = InflightDone(landed);
    let t0 = std::time::Instant::now();
    let mut m = g.mem.lock().unwrap();
    super::lazy::T_LOCK.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    for &(pool, a, n, src, pre) in wbs {
        let Some(p) = m.by_base.get_mut(&pool) else { continue };
        // pages GPU writes keep coming to stay on the device (write-back on demand, `lazy`); the rest lands here
        let tl = std::time::Instant::now();
        let mut lazy = if p.watched { super::lazy::take_lazy(a, n) } else { vec![] };
        // a page the CPU wrote since its pool was last asked (a cold pool is asked once a frame) is not held on the
        // device: its buffer copy lacks those bytes, and bringing it back would put the buffer's page over them (lane
        // K2: the bytes of a page the GPU and the CPU both write, as MKW's glyph pages may be; mkw-recomp #250)
        if !lazy.is_empty() && std::env::var("NXGFX_LAZY_CPU_CHECK").as_deref() != Ok("0") {
            let (lo, hi) = page_range(a, n);
            if let Some(cpu) = watch::take(lo, hi, false) {
                if !cpu.is_empty() {
                    lazy.retain(|pg| cpu.binary_search(pg).is_err());
                }
            }
        }
        super::lazy::T_TAKE.fetch_add(tl.elapsed().as_nanos() as u64, Ordering::Relaxed);
        if !lazy.is_empty() {
            let (buf, off0) = (p.buf.as_ref().map(|b| b.buf).unwrap_or_default(), p.buf_off);
            let base = p.base;
            let mut at = a;
            for &pg in lazy.iter().chain(std::iter::once(&(a + n))) {
                let end = pg.min(a + n);
                if end > at {
                    land(p, at, end - at, unsafe { src.add((at - a) as usize) }, if pre.is_null() { pre } else { unsafe { pre.add((at - a) as usize) } });
                }
                at = (pg + PAGE).min(a + n).max(at);
            }
            let tm = std::time::Instant::now();
            super::lazy::make_lazy(&lazy, buf, base, off0);
            super::lazy::T_MAKE.fetch_add(tm.elapsed().as_nanos() as u64, Ordering::Relaxed);
            continue;
        }
        let tl = std::time::Instant::now();
        land(p, a, n, src, pre);
        super::lazy::T_LAND.fetch_add(tl.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
    WRITEBACK_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// One write-back's bytes into guest memory (pages held on the device are read back first).
fn land(p: &mut Pool, a: u64, n: u64, src: *const u8, base: *const u8) {
    // pages held on the device (lazy write-back): its buffer has their newest bytes, this write-back's among them, so
    // they stay there; bringing them back here made the completion thread wait for all the GPU work submitted since
    // (Pokemon Violet: 48 ms of write-back a frame, 12 FPS)
    if let Some(runs) = super::lazy::outside(a, n) {
        for (o, m) in runs {
            let at = |q: *const u8| if q.is_null() { q } else { unsafe { q.add(o as usize) } };
            land(p, a + o, m, at(src), at(base));
        }
        return;
    }
    if !base.is_null() {
        return land_changed(p, a, n, src, base);
    }
    {
        super::lazy::touch(a, n);
        let (lo, hi) = page_range(a, n);
        if p.watched {
            if let Some(pages) = watch::take(lo, hi, true) {
                for pg in pages {
                    p.pending.push((pg, PAGE));
                }
            }
        }
        unsafe { core::ptr::copy_nonoverlapping(src, a as *mut u8, n as usize) };
        if p.watched {
            watch::reset(lo, hi);
        } else if let Some(s) = p.snapshot.as_mut() {
            let o = (a - p.base) as usize;
            s[o..o + n as usize].copy_from_slice(unsafe { core::slice::from_raw_parts(src, n as usize) });
        }
        WRITEBACK_BYTES.fetch_add(n, Ordering::Relaxed);
    }
}

/// Bytes the GPU changed in a write-back's range (where `src`, the device after the submission, differs from `base`,
/// the device before its work) land in guest memory; the rest of the range keeps what the CPU has, including what it
/// wrote while the GPU worked (Turbo's `wr_land`: a storage buffer is written in part). Pages the GPU left alone are
/// not touched. Counts the guest bytes the CPU had changed meanwhile that a whole-range landing would have reverted.
fn land_changed(p: &mut Pool, a: u64, n: u64, src: *const u8, base: *const u8) {
    let (s, b) = unsafe { (core::slice::from_raw_parts(src, n as usize), core::slice::from_raw_parts(base, n as usize)) };
    let mut at = 0u64;
    let mut kept = 0u64;
    while at < n {
        let pg_end = (((a + at) / PAGE + 1) * PAGE - a).min(n);
        let (lo, hi) = (at as usize, pg_end as usize);
        if s[lo..hi] != b[lo..hi] {
            super::lazy::touch(a + at, pg_end - at);
            let (plo, phi) = page_range(a + at, pg_end - at);
            if p.watched {
                if let Some(pages) = watch::take(plo, phi, true) {
                    for pg in pages {
                        p.pending.push((pg, PAGE));
                    }
                }
            }
            let g = unsafe { core::slice::from_raw_parts_mut((a + at) as *mut u8, hi - lo) };
            for i in 0..hi - lo {
                let (nv, ov) = (s[lo + i], b[lo + i]);
                if nv != ov {
                    g[i] = nv;
                } else if g[i] != ov {
                    kept += 1;
                }
            }
            if p.watched {
                watch::reset(plo, phi);
            } else if let Some(snap) = p.snapshot.as_mut() {
                let o = (a + at - p.base) as usize;
                snap[o..o + (hi - lo)].copy_from_slice(&g[..]);
            }
            WRITEBACK_BYTES.fetch_add(pg_end - at, Ordering::Relaxed);
        }
        at = pg_end;
    }
    if kept > 0 {
        WRITEBACK_KEPT.fetch_add(kept, Ordering::Relaxed);
    }
}

/// One line of placement and traffic (with the pacing line).
pub fn summary(g: &Gpu, frames: u64) -> String {
    let m = g.mem.lock().unwrap();
    let mut cnt = [0usize; 4];
    let mut bytes = [0u64; 4];
    for p in m.by_base.values() {
        let i = p.placement as usize;
        cnt[i] += 1;
        bytes[i] += p.size;
    }
    // mirrors without a write watch: compared with a snapshot of all of them at every submission
    let (sn, sb) = m.by_base.values().filter(|p| p.snapshot.is_some()).fold((0usize, 0u64), |(n, b), p| (n + 1, b + p.size));
    let f = frames.max(1) as f64;
    let up = UPLOAD_BYTES.swap(0, Ordering::Relaxed) as f64 / f / 1048576.0;
    let wb = WRITEBACK_BYTES.swap(0, Ordering::Relaxed) as f64 / f / 1048576.0;
    let wbms = WRITEBACK_NS.swap(0, Ordering::Relaxed) as f64 / f / 1e6;
    let kept = WRITEBACK_KEPT.load(Ordering::Relaxed);
    format!(
        "pools: {} vram ({} MB), {} mirror ({} MB), {} host ({} MB), {} cpu-only; per frame upload {up:.2} MB, write-back {wb:.2} MB in {wbms:.3} ms ({kept} CPU-written bytes inside write-backs kept so far); {sn} mirrors without write watch ({} MB)",
        cnt[0],
        bytes[0] >> 20,
        cnt[1],
        bytes[1] >> 20,
        cnt[2],
        bytes[2] >> 20,
        cnt[3],
        sb >> 20
    ) + &super::lazy::summary(f)
}

#[cfg(test)]
mod inflight_tests {
    /// A CPU-dirty range over in-flight write-backs: the parts outside upload now, the parts over them later.
    #[test]
    fn dirty_ranges_split_around_inflight_writebacks() {
        let (now, later) = super::split_inflight(0x1000, 0x3000, &[(0x1800, 0x800), (0x3800, 0x1000)]);
        assert_eq!(now, vec![(0x1000, 0x800), (0x2000, 0x1800)]);
        assert_eq!(later, vec![(0x1800, 0x800), (0x3800, 0x800)]);
        let (now, later) = super::split_inflight(0x1000, 0x1000, &[]);
        assert_eq!((now, later), (vec![(0x1000, 0x1000)], vec![]));
    }
}

/// For a capture (`capture.rs`): every pool's pages held on the device come back to guest memory first, then what the
/// GPU holds of the device-local pools (their guest memory is not their content): (base, bytes) of each.
pub fn gpu_contents(g: &Gpu) -> Vec<(u64, Vec<u8>)> {
    let pools: Vec<(u64, u64, Placement, Option<(vk::Buffer, u64)>)> = {
        let m = g.mem.lock().unwrap();
        m.by_base.values().map(|p| (p.base, p.size, p.placement, p.buf.as_ref().map(|b| (b.buf, p.buf_off)))).collect()
    };
    for &(base, size, _, _) in &pools {
        super::lazy::touch(base, size);
    }
    let mut out = Vec::new();
    for (base, size, placement, buf) in pools {
        let (Placement::Vram, Some((buf, off))) = (placement, buf) else { continue };
        let Ok(rb) = g.buffer(size, vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuToCpu, "capture readback") else { continue };
        let f = super::queue::oneoff(g, |g, sub, _| unsafe {
            g.dev().cmd_copy_buffer(sub.main, buf, rb.buf, &[vk::BufferCopy { src_offset: off, dst_offset: 0, size }]);
        });
        super::queue::wait_fence(f, u64::MAX);
        let bytes = unsafe { core::slice::from_raw_parts(rb.ptr as *const u8, size as usize) }.to_vec();
        g.free_buffer(rb);
        out.push((base, bytes));
    }
    out
}

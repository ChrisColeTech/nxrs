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
use gpu_allocator::vulkan::{AllocationCreateDesc, AllocationScheme};
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
    /// Registration identity, distinct even when guest base and Vulkan handles are reused.
    pub identity: u64,
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
    /// A pool larger than the device's largest buffer (maintenance4 `maxBufferSize`; AMD: 2 GiB) has several buffers,
    /// windows side by side over it: window k holds pool bytes [k * win, (k + 1) *
    /// win), window 0 is `buf`, the others are here. `win` 0: one buffer for all of it.
    pub win: u64,
    pub wins: Vec<Buf>,
    /// the device address of each extra window's start (raw global memory's table lists every window)
    pub wins_da: Vec<u64>,
    /// a sparse-residency buffer backed in granules on first use (`Granules`), None: backed whole
    pub gran: Option<Granules>,
    /// CPU-written ranges past the frame's upload budget (`upload_budget`): uploaded when a use resolves them, or by
    /// the next submissions within their budget
    pub deferred: Vec<(u64, u64)>,
    /// CPU-written ranges uploaded outside a sync (`sync_range`): the next sync hands them to the texture check, which
    /// never saw them otherwise (their watch bits are gone)
    pub tex_dirty: Vec<(u64, u64)>,
}

/// The per-frame upload budget (64 MB): CPU writes uploaded at the start of a submission count against it; past it,
/// a range waits in its pool (`Pool::deferred`) and goes when a draw, dispatch or copy of this submission resolves an
/// address in it (a use syncs what it needs whatever the budget), when raw global memory's table is bound (every pool
/// at once), or at the next submissions within theirs. Without the budget every CPU write was uploaded before each
/// submission, so a frame that rewrote hundreds of MB the GPU did not read (a load's staging areas) staged all of it
/// at once.
/// `NXGFX_UPLOAD_MB` (default 64; 0: no budget).
pub fn upload_budget() -> u64 {
    match BUDGET_OVERRIDE.load(Ordering::Relaxed) {
        0 => {
            static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
            *V.get_or_init(|| {
                std::env::var("NXGFX_UPLOAD_MB")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(64)
                    << 20
            })
        }
        o => o,
    }
}
/// Tests: another budget (bytes; 0: the configured one).
pub static BUDGET_OVERRIDE: AtomicU64 = AtomicU64::new(0);
/// (frame, bytes uploaded in it so far)
static BUDGET_USED: std::sync::Mutex<(u64, u64)> = std::sync::Mutex::new((0, 0));
/// Bytes left for later by the budget, and taken by a use since.
pub static DEFERRED_BYTES: AtomicU64 = AtomicU64::new(0);
pub static DEFERRED_USED: AtomicU64 = AtomicU64::new(0);

/// Whether a range of `n` CPU-written bytes goes now under this frame's budget (nothing taken: `budget_charge` takes
/// what is uploaded of it, which the upload skip may make less).
fn budget_fits(n: u64) -> bool {
    let b = upload_budget();
    if b == 0 {
        return true;
    }
    let f = super::present::frames();
    let mut u = BUDGET_USED.lock().unwrap();
    if u.0 != f {
        *u = (f, 0);
    }
    // a frame's first range always goes (a range larger than the budget is not starved)
    !(u.1 > 0 && u.1 + n > b)
}

/// `n` bytes uploaded count against this frame's budget.
fn budget_charge(n: u64) {
    if upload_budget() == 0 || n == 0 {
        return;
    }
    let f = super::present::frames();
    let mut u = BUDGET_USED.lock().unwrap();
    if u.0 != f {
        *u = (f, 0);
    }
    u.1 += n;
}

/// A pool buffer backed in granules on first use: device memory for a
/// granule is allocated and bound (before the submission that first uses it, `sparse::bind_buffer`) when a range in it
/// is first resolved, and filled then with the guest memory's committed bytes there. A large pool the game uses a
/// little of holds only that much device memory (Mario Kart World's pools of gigabytes). Raw global memory's table
/// names whole pools: a pool it lists is bound whole. Pools of `GRAN_MIN` and up, Mirror / Vram, one window;
/// `NXGFX_POOL_SPARSE=0` backs every pool whole.
pub struct Granules {
    pub size: u64,
    pub mem: Vec<Option<gpu_allocator::vulkan::Allocation>>,
    pub req: vk::MemoryRequirements,
    pub bound: u64,
}

/// The granule (2 MiB, a whole number of the device's sparse blocks).
pub const GRAN: u64 = 2 << 20;
/// Pools smaller than this are backed whole (a bind per first use of a 16 KB pool cost more than its memory).
pub const GRAN_MIN: u64 = 8 << 20;
/// Bytes of pool memory bound in granules so far.
pub static GRAN_BOUND: AtomicU64 = AtomicU64::new(0);

fn gran_on(g: &Gpu) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    g.d.caps.sparse_buffer
        && *ON.get_or_init(|| std::env::var("NXGFX_POOL_SPARSE").as_deref() != Ok("0"))
}

/// The granules [first, end) a range [off, off + n) of a pool touches.
pub fn granules_of(gran: u64, off: u64, n: u64) -> (usize, usize) {
    (
        (off / gran) as usize,
        (off + n.max(1)).div_ceil(gran) as usize,
    )
}

/// The largest buffer the device makes (maintenance4 `maxBufferSize`), or less when a test or
/// `NXGFX_MAX_BUFFER_MB` asks for it (to try pool windows on a device whose largest buffer is larger than any pool).
pub static MAX_BUFFER_OVERRIDE: AtomicU64 = AtomicU64::new(0);

pub fn max_buffer(g: &Gpu) -> u64 {
    static DEV: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let dev = *DEV.get_or_init(|| {
        let mut m4 = vk::PhysicalDeviceMaintenance4Properties::default();
        let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut m4);
        unsafe {
            g.d.instance
                .get_physical_device_properties2(g.d.pdev, &mut p2)
        };
        let max = if m4.max_buffer_size > 0 {
            m4.max_buffer_size
        } else {
            u64::MAX
        };
        let env = std::env::var("NXGFX_MAX_BUFFER_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v << 20)
            .unwrap_or(u64::MAX);
        max.min(env)
    });
    match MAX_BUFFER_OVERRIDE.load(Ordering::Relaxed) {
        0 => dev,
        o => o.min(dev),
    }
}

/// The window size for a pool of `size` bytes when the device's largest buffer is `max`: 0 (one buffer) when it
/// fits, else `max` rounded down to whole pages.
pub fn window_size(size: u64, max: u64) -> u64 {
    if size <= max {
        0
    } else {
        (max & !(PAGE - 1)).max(PAGE)
    }
}

/// [off, off + n) of a pool split at its windows: (window index, offset in the window, bytes).
pub fn split_windows(win: u64, off: u64, n: u64) -> Vec<(usize, u64, u64)> {
    if win == 0 {
        return vec![(0, off, n)];
    }
    let mut out = vec![];
    let (mut at, end) = (off, off + n);
    while at < end {
        let k = at / win;
        let e = ((k + 1) * win).min(end);
        out.push((k as usize, at - k * win, e - at));
        at = e;
    }
    out
}

/// Moves whenever a pool comes, goes or gets its GPU buffer (the raw global memory table is written again).
pub static POOL_GEN: AtomicU64 = AtomicU64::new(1);

/// The pool buffers' usage: with shader device addresses when the device has them (raw global memory).
fn buf_usage(g: &Gpu) -> vk::BufferUsageFlags {
    if g.d.caps.rt {
        // pool memory is also ray-tracing geometry and instance input (vk/rt.rs)
        BUF_USAGE
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
            | vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
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
    /// As `take`, keeping what the allocations that answered gave (their pages were reset: dropping them would lose
    /// those writes) when another does not: (pages, every allocation answered). Audit L1.
    pub fn take_partial(lo: u64, hi: u64, reset: bool) -> (Vec<u64>, bool) {
        if let Some(v) = take_one(lo, hi, reset) {
            return (v, true);
        }
        let (mut out, mut ok) = (Vec::new(), true);
        for (a, b) in allocations(lo, hi) {
            match take_one(a, b, reset) {
                Some(v) => out.extend(v),
                None => ok = false,
            }
        }
        (out, ok)
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
        allocations_with_base(lo, hi)
            .into_iter()
            .map(|(a, b, _)| (a, b))
            .collect()
    }
    fn allocations_with_base(lo: u64, hi: u64) -> Vec<(u64, u64, u64)> {
        use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION};
        let mut out: Vec<(u64, u64, u64)> = Vec::new();
        let mut at = lo;
        while at < hi {
            let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
            if unsafe {
                VirtualQuery(
                    at as *const _,
                    &mut mbi,
                    core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            } == 0
            {
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
            let (k0, u0) = thread_times();
            let slow = |out: &Vec<u64>, calls: u32| {
                static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let ms = t0.elapsed().as_secs_f64() * 1e3;
                if ms > 100.0 && N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 20 {
                    let (k1, u1) = thread_times();
                    crate::vklog!("write watch: {:#x}+{} MB took {ms:.0} ms ({} written pages, {calls} calls; thread CPU kernel {:.0} ms, user {:.0} ms)", lo0, (hi - lo0) >> 20, out.len(), (k1 - k0) as f64 / 1e4, (u1 - u0) as f64 / 1e4);
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
    /// This thread's kernel and user CPU time (100 ns units).
    fn thread_times() -> (u64, u64) {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) };
        let ft = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
        (ft(k), ft(u))
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
    /// As `take`: (pages, answered). One scan answers for the whole range or not at all.
    pub fn take_partial(lo: u64, hi: u64, reset: bool) -> (Vec<u64>, bool) {
        match take(lo, hi, reset) {
            Some(v) => (v, true),
            None => (Vec::new(), false),
        }
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
        unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                at as *const _,
                dst.as_mut_ptr() as *mut _,
                dst.len(),
                &mut got,
            ) != 0
                && got == dst.len()
        }
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
        return std::borrow::Cow::Borrowed(unsafe {
            core::slice::from_raw_parts(a as *const u8, n as usize)
        });
    }
    let mut v = vec![0u8; n as usize];
    for (ca, cn) in c {
        let o = (ca - a) as usize;
        // SAFETY: committed guest memory
        v[o..o + cn as usize]
            .copy_from_slice(unsafe { core::slice::from_raw_parts(ca as *const u8, cn as usize) });
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
        let r = unsafe {
            VirtualQuery(
                at as *const _,
                &mut mbi,
                core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
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
    // A physical pool is GPU_NO_ACCESS by NVN's rule, yet the GPU reaches it through the virtual pools mapped onto it
    // (an alias resolves its buffers into this pool, a sparse pool its contiguous ranges): it needs a GPU buffer, made
    // at its first GPU use like any pool's (Ocarina of Time: a 305 MB physical pool behind a whole-pool alias; its
    // compute shaders' storage buffers were bound to zero stand-ins, "pool has no GPU access").
    if flags & mem_flags::GPU_NO_ACCESS != 0 && flags & mem_flags::PHYSICAL == 0 {
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
    let watched = matches!(placement, Placement::Mirror | Placement::Host)
        && watch::take(lo, hi, false).is_some();
    let pool = Pool {
        identity: POOL_GEN.fetch_add(1, Ordering::AcqRel) + 1,
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
        win: 0,
        wins: Vec::new(),
        wins_da: Vec::new(),
        gran: None,
        deferred: Vec::new(),
        tex_dirty: Vec::new(),
    };
    if std::env::var("NXGFX_POOL_LOG").is_ok() {
        crate::vklog!(
            "pool {base:#x} size {:#x} flags {:#x}: {placement:?}{}",
            p.size,
            p.flags,
            if watched { " (write watch)" } else { "" }
        );
    }
    // an entry of the same extent cannot tell the two pools apart: it goes now (the replaced pool's held pages then come
    // back unrecorded: uploaded once more); one of another size goes in the replaced pool's `release`
    super::upskip::drop_pool(base, base + p.size);
    let old = {
        let mut m = g.mem.lock().unwrap();
        let old = m.by_base.insert(base, pool);
        POOL_GEN.fetch_add(1, Ordering::Release);
        old
    };
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
    // then its upload-skip entry, with the records those returns made (UPSYNC2 L5: dropped first, they recorded into
    // nothing; a pool registered anew at this base with another size, and nested pools' raw logs, are kept)
    super::upskip::drop_pool(p.base, p.base + p.size);
    if let Some(m) = ADDRS.write().unwrap().as_mut() {
        for b in p.buf.iter().chain(p.wins.iter()) {
            m.remove(&vk::Handle::as_raw(b.buf));
        }
    }
    for b in p.buf.iter().chain(p.wins.iter()) {
        super::lost::ring::note_free(2, b.addr, b.size, vk::Handle::as_raw(b.buf), g.q.last_fence());
    }
    for b in p.wins {
        g.q.defer_after_use(Box::new(move |g: &Gpu| g.free_buffer(b)));
    }
    if let Some(gr) = p.gran {
        let mems: Vec<_> = gr.mem.into_iter().flatten().collect();
        GRAN_BOUND.fetch_sub(mems.len() as u64 * gr.size, Ordering::Relaxed);
        g.q.defer_after_use(Box::new(move |g: &Gpu| {
            let mut a = g.alloc.lock().unwrap();
            for m in mems {
                let _ = a.free(m);
            }
        }));
    }
    if let Some(b) = p.buf {
        super::lost::note_gone(|| {
            format!(
                "pool buffer {:#x} of pool {:#x} (+{:#x}), device address {:#x}",
                vk::Handle::as_raw(b.buf),
                p.base,
                p.size,
                p.da
            )
        });
        g.q.defer_after_use(Box::new(move |g: &Gpu| g.free_buffer(b)));
    }
}

/// nvnMemoryPoolFinalize.
pub fn pool_del(base: u64) {
    super::capture::pool_del(base);
    let Some(g) = Gpu::try_get() else { return };
    let p = {
        let mut m = g.mem.lock().unwrap();
        let p = m.by_base.remove(&base);
        POOL_GEN.fetch_add(1, Ordering::Release);
        p
    };
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
    let a = if crate::nvn::vpool::is_sparse(a) {
        crate::nvn::vpool::translate(a, 1).unwrap_or(0)
    } else {
        a
    };
    // the windows this thread asked about last, while no pool came, went or got its buffer: (generation, window
    // start, window end, the next pool's base); every read-only storage buffer of a stage set asks (MKW: thousands a
    // frame, each taking the pool lock the completion thread holds ~9 ms a frame)
    thread_local! {
        static LAST: std::cell::Cell<[(u64, u64, u64, u64); 8]> = const { std::cell::Cell::new([(0, 0, 0, 0); 8]) };
    }
    let gen = POOL_GEN.load(Ordering::Acquire);
    if let Some(e) = LAST.with(|l| l.get().into_iter().find(|e| room_hit(*e, gen, a))) {
        return e.2 - a;
    }
    let m = g.mem.lock().unwrap();
    match find(&m, a, 1) {
        Some(b) => {
            let p = &m.by_base[&b];
            // to the end of the window holding `a` (a pool over the device's largest buffer has several)
            let (lo, hi) = if p.win == 0 {
                (p.base, p.base + p.size)
            } else {
                let k = (a - p.base) / p.win;
                (p.base + k * p.win, p.base + ((k + 1) * p.win).min(p.size))
            };
            let next = m
                .by_base
                .range(b + 1..)
                .next()
                .map(|(k, _)| *k)
                .unwrap_or(u64::MAX);
            // the generation read before the lookup: a pool change since makes the entry miss
            LAST.with(|l| {
                let mut v = l.get();
                v.rotate_right(1);
                v[0] = (gen, lo, hi, next);
                l.set(v);
            });
            hi - a
        }
        None => 0,
    }
}

/// Whether `room`'s remembered window `e` (generation, start, end, the next pool's base) holds `a` now: the same
/// pools, `a` in the window and below the next pool (a pool laid over this one owns what lies past its base).
fn room_hit(e: (u64, u64, u64, u64), gen: u64, a: u64) -> bool {
    e.0 == gen && e.0 != 0 && a >= e.1 && a < e.2 && a < e.3
}

/// The OS write watch of [a, a + n) reset (the host wrote it, not the game).
/// Every page of [a, a + n) is committed and writable by the host now (a texture write-back writes only then: a
/// guest page read-only or not committed faulted in the host copy, lane K2).
#[cfg(windows)]
pub fn host_writable(a: u64, n: u64) -> bool {
    use windows_sys::Win32::System::Memory::{
        VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_READWRITE,
    };
    let (mut at, end) = (a, a.saturating_add(n.max(1)));
    while at < end {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        if unsafe {
            VirtualQuery(
                at as *const _,
                &mut mbi,
                core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
            || mbi.State != MEM_COMMIT
        {
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
    nxrt::mem::readable(a, n)
        && matches!(nxrt::linux::query(a), Some((true, prot, _, _)) if prot & 2 != 0)
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
    // a sparse virtual pool's range: the physical memory it maps (as `resolve_in` binds it)
    let a = if crate::nvn::vpool::is_sparse(a) {
        match crate::nvn::vpool::translate(a, n) {
            Some(p) => p,
            None => return,
        }
    } else {
        a
    };
    let frame = super::present::frames();
    let mut m = g.mem.lock().unwrap();
    let _watch = watch_lock(&mut m);
    let gen = m.gen;
    let Some(b) = find(&m, a, n) else { return };
    let p = m.by_base.get_mut(&b).unwrap();
    // a hot pool was asked at this submission's start; only a watched mirror's pages are taken here
    if !p.watched || p.placement != Placement::Mirror || frame.saturating_sub(p.dirty_frame) <= 60 {
        return;
    }
    // taking the watch and uploading the CPU writes: a first-use initialization in the upload buffer (`gate::pass_pre`)
    if !super::gate::pass_pre(true) {
        return;
    }
    // taken and uploaded under the shared upload buffer's lock in a parallel run, so the upload lands after any older
    // upload of the same pages (audit H1)
    let _pre = super::gate::pre_lock();
    let (lo, hi) = page_range(a, n);
    let Some(pages) = watch::take(lo, hi, true) else {
        return;
    };
    // pages a scan took and left for the next sync (the pool walker's, a landing's) are CPU writes too: the ones in the
    // copy's range go now; the rest of the pool's pending ranges stay for the submission's own sync, which also checks
    // the textures over them (`image_begin_submit`) and keeps pieces under an in-flight write-back back (audit M1)
    let (inside, outside) = split_pending(std::mem::take(&mut p.pending), lo, hi);
    if pages.is_empty() && inside.is_empty() {
        p.pending = outside;
        return;
    }
    p.pending = inside;
    let mut ranges = p.take_cpu_writes(gen, Some(pages));
    p.pending = outside;
    if wb_guard() {
        let inflight = INFLIGHT.lock().unwrap();
        let mine = inflight.overlapping(p.base, p.base + p.size);
        if !mine.is_empty() {
            let mut keep = Vec::new();
            for &(ra, rn) in &ranges {
                let (now, later) = split_inflight(ra, rn, &mine);
                keep.extend(now);
                p.pending.extend(later);
            }
            ranges = keep;
        }
    }
    // through the upload skip as the submission's sync does (audit H1/L2: a plain upload left lazy write-back records
    // standing and could send the device's older bytes back over newer GPU writes); pages that only came back from
    // the device unchanged are no CPU write: not uploaded, not told to the textures (M2), and the pool stays cold (L5)
    if p.buf.is_some() {
        let mut back = Vec::new();
        for &(ra, rn) in &ranges {
            if super::upskip::on() {
                let (base, size) = (p.base, p.size);
                super::upskip::upload_changed(base, size, ra, rn, &mut back, |at, b| p.upload_from(g, sub, at, b));
            } else {
                p.upload_now(g, sub, ra, rn);
            }
        }
        if !back.is_empty() {
            ranges = without_pages(&ranges, &mut back);
        }
    }
    if ranges.is_empty() {
        return;
    }
    // (a cold pool written: a walk that left it out covers no sync now)
    COLD_GEN.fetch_add(1, Ordering::AcqRel);
    p.dirty_frame = frame;
    p.tex_dirty.extend(ranges.iter().copied());
    static N: AtomicU64 = AtomicU64::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 4 {
        crate::vklog!("a texture copy's source {a:#x} (+{n:#x}) in a cold pool: {} CPU-written range(s) taken first", ranges.len());
    }
}

/// nvnMemoryPoolInvalidateMappedRange: GPU writes reach guest memory at their fence, before SyncWait returns.
pub fn cpu_invalidate(_a: u64, _n: u64) {}

impl Pool {
    /// The buffer of window `k` and the buffer offset of the window's start.
    fn window(&self, k: usize) -> Option<(vk::Buffer, u64)> {
        if k == 0 {
            self.buf.as_ref().map(|b| (b.buf, self.buf_off))
        } else {
            self.wins.get(k - 1).map(|b| (b.buf, 0))
        }
    }

    /// [a, a + n) of guest memory in this pool as pieces of its buffers: (buffer, buffer offset, address, bytes).
    pub fn pieces(&self, a: u64, n: u64) -> Vec<(vk::Buffer, u64, u64, u64)> {
        split_windows(self.win, a - self.base, n)
            .into_iter()
            .filter_map(|(k, o, m)| {
                let (b, b0) = self.window(k)?;
                Some((b, b0 + o, self.base + k as u64 * self.win + o, m))
            })
            .collect()
    }

    /// The one buffer holding all of [a, a + n): (buffer, its offset of `a`, the window's first address, the window's
    /// bytes). None when the range spans two windows.
    pub fn one(&self, a: u64, n: u64) -> Option<(vk::Buffer, u64, u64, u64)> {
        let ps = split_windows(self.win, a - self.base, n.max(1));
        if ps.len() != 1 {
            return None;
        }
        let (k, o, _) = ps[0];
        let (b, b0) = self.window(k)?;
        let wlen = if self.win == 0 {
            self.size
        } else {
            self.win.min(self.size - k as u64 * self.win)
        };
        Some((b, b0 + o, self.base + k as u64 * self.win, wlen))
    }

    /// The ranges raw global memory's table lists for this pool: (first address, end, device address of the first).
    pub fn da_ranges(&self) -> Vec<(u64, u64, u64)> {
        if self.da == 0 {
            return vec![];
        }
        if self.win == 0 {
            return vec![(self.base, self.base + self.size, self.da)];
        }
        let mut v = vec![(self.base, self.base + self.win.min(self.size), self.da)];
        for (i, &d) in self.wins_da.iter().enumerate() {
            let lo = self.base + (i as u64 + 1) * self.win;
            v.push((lo, (lo + self.win).min(self.base + self.size), d));
        }
        v
    }

    fn ensure_buffer(&mut self, g: &Gpu, sub: &mut super::queue::Sub) -> Result<(), String> {
        if self.buf.is_some() {
            return Ok(());
        }
        let win = window_size(self.size, max_buffer(g));
        if win != 0 && self.placement == Placement::Host {
            // an imported range is one buffer: a pool larger than the device's largest buffer is mirrored instead
            self.placement = Placement::Mirror;
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
                    crate::log::red_once(&format!(
                        "pool {:#x}: host import failed ({e}); mirrored instead",
                        self.base
                    ));
                    self.placement = Placement::Mirror;
                    let (lo, hi) = page_range(self.base, self.size);
                    self.watched = watch::take(lo, hi, false).is_some();
                }
            },
            _ => {}
        }
        if win == 0
            && self.size >= GRAN_MIN
            && matches!(self.placement, Placement::Mirror | Placement::Vram)
            && gran_on(g)
        {
            return self.make_sparse(g);
        }
        let b = g.buffer(
            if win == 0 { self.size } else { win },
            buf_usage(g),
            MemoryLocation::GpuOnly,
            "pool",
        )?;
        self.win = win;
        if win != 0 {
            let n = self.size.div_ceil(win);
            for k in 1..n {
                let len = win.min(self.size - k * win);
                match g.buffer(len, buf_usage(g), MemoryLocation::GpuOnly, "pool window") {
                    Ok(w) => self.wins.push(w),
                    Err(e) => {
                        for w in self.wins.drain(..) {
                            g.free_buffer(w);
                        }
                        g.free_buffer(b);
                        self.win = 0;
                        return Err(format!("pool {:#x}: window {k} of {n}: {e}", self.base));
                    }
                }
            }
            crate::vklog!("memory pool {:#x}+{:#x}: {n} buffers of up to {} MB side by side (the device's largest buffer is {} MB)", self.base, self.size, win >> 20, max_buffer(g) >> 20);
        }
        // The storage now: a Vram pool's initial content, a mirror's current one. Only the committed parts of the
        // guest's memory hold anything (a large pool the game has not written yet is zero and is not read here: reading
        // it would commit it, and staging it would double it in host memory; Mario Kart World's pools)
        super::lazy::touch(self.base, self.size);
        // a new GPU copy: no page of it was uploaded by the pool sync yet (an empty entry: a page held on this buffer
        // and brought back before the first sync is recorded there, `upskip::brought_back`)
        super::upskip::new_pool(self.base, self.base + self.size);
        // filled, uploaded and its watch taken under the shared upload buffer's lock in a parallel run: a later sync of
        // a page of it lands after this (audit H1)
        let _pre = super::gate::pre_lock();
        unsafe {
            g.dev()
                .cmd_fill_buffer(sub.pre, b.buf, 0, vk::WHOLE_SIZE, 0)
        };
        for w in &self.wins {
            unsafe {
                g.dev()
                    .cmd_fill_buffer(sub.pre, w.buf, 0, vk::WHOLE_SIZE, 0)
            };
        }
        super::queue::full_barrier(g, sub.pre);
        self.buf = Some(b);
        self.buf_off = 0;
        let mut staged = 0u64;
        for (a, n) in committed(self.base, self.size) {
            // in pieces of at most 64 MB (staging chunks), split at the windows
            let mut at = a;
            while at < a + n {
                let m = (a + n - at).min(64 << 20);
                for (buf, boff, pa, pn) in self.pieces(at, m) {
                    let src = unsafe { core::slice::from_raw_parts(pa as *const u8, pn as usize) };
                    sub.upload(g, src, buf, boff);
                }
                at += m;
            }
            staged += n;
        }
        UPLOAD_BYTES.fetch_add(staged, Ordering::Relaxed);
        if self.size >= 256 << 20 {
            crate::vklog!(
                "pool {:#x}: {} MB on the GPU, {} MB of it committed and uploaded ({:?})",
                self.base,
                self.size >> 20,
                staged >> 20,
                self.placement
            );
        }
        if self.placement == Placement::Mirror {
            let (lo, hi) = page_range(self.base, self.size);
            if self.watched {
                let _ = watch::take(lo, hi, true);
            } else {
                self.snapshot = Some(guest_bytes(self.base, self.size).into_owned());
            }
        }
        self.note_address(g);
        Ok(())
    }

    /// A sparse-residency buffer with no memory yet; granules come at first use (`bind`).
    fn make_sparse(&mut self, g: &Gpu) -> Result<(), String> {
        let dev = g.dev();
        let size = self.size.div_ceil(GRAN) * GRAN;
        let bda = vk::BufferCreateInfo::default()
            .size(size)
            .usage(buf_usage(g))
            .flags(vk::BufferCreateFlags::SPARSE_BINDING | vk::BufferCreateFlags::SPARSE_RESIDENCY)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buf = unsafe { dev.create_buffer(&bda, None) }
            .map_err(|e| format!("pool {:#x}: sparse buffer: {e}", self.base))?;
        let req = unsafe { dev.get_buffer_memory_requirements(buf) };
        let gran = GRAN.div_ceil(req.alignment.max(1)) * req.alignment.max(1);
        let n = size.div_ceil(gran) as usize;
        let addr = if g.d.caps.bda {
            unsafe {
                dev.get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buf))
            }
        } else {
            0
        };
        super::lost::ring::note_mem(11, addr, size, vk::Handle::as_raw(buf), g.q.last_fence(), "granules");
        self.buf = Some(Buf {
            buf,
            alloc: None,
            mem: vk::DeviceMemory::null(),
            size,
            ptr: core::ptr::null_mut(),
            addr,
        });
        self.buf_off = 0;
        self.gran = Some(Granules {
            size: gran,
            mem: (0..n).map(|_| None).collect(),
            req,
            bound: 0,
        });
        static N: AtomicU64 = AtomicU64::new(0);
        if N.fetch_add(1, Ordering::Relaxed) < 32 {
            crate::vklog!(
                "memory pool {:#x}+{:#x}: device-local, backed in {} KB granules on first use",
                self.base,
                self.size,
                gran >> 10
            );
        }
        if self.placement == Placement::Mirror {
            let (lo, hi) = page_range(self.base, self.size);
            if self.watched {
                let _ = watch::take(lo, hi, true);
            } else {
                self.snapshot = Some(guest_bytes(self.base, self.size).into_owned());
            }
        }
        self.note_address(g);
        Ok(())
    }

    /// The granules of [a, a + n) that are not bound yet get memory, a bind before this submission, and the guest
    /// memory's committed bytes there (zeros elsewhere), recorded into `sub`.
    pub fn bind(
        &mut self,
        g: &Gpu,
        sub: &mut super::queue::Sub,
        a: u64,
        n: u64,
    ) -> Result<(), String> {
        let Some(gr) = self.gran.as_mut() else {
            return Ok(());
        };
        let (g0, g1) = granules_of(gr.size, a - self.base, n);
        let g1 = g1.min(gr.mem.len());
        if (g0..g1).all(|i| gr.mem[i].is_some()) {
            return Ok(());
        }
        let buf = self.buf.as_ref().unwrap().buf;
        let mut fresh = vec![];
        for i in g0..g1 {
            if gr.mem[i].is_some() {
                continue;
            }
            let req = vk::MemoryRequirements {
                size: gr.size,
                alignment: gr.req.alignment,
                memory_type_bits: gr.req.memory_type_bits,
            };
            let al = g
                .alloc
                .lock()
                .unwrap()
                .allocate(&AllocationCreateDesc {
                    name: "pool granule",
                    requirements: req,
                    location: MemoryLocation::GpuOnly,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| {
                    format!(
                        "no device memory to back pool {:#x} at {:#x}: {e}",
                        self.base,
                        i as u64 * gr.size
                    )
                })?;
            super::sparse::bind_buffer(
                buf,
                vk::SparseMemoryBind {
                    resource_offset: i as u64 * gr.size,
                    size: gr.size,
                    memory: unsafe { al.memory() },
                    memory_offset: al.offset(),
                    flags: vk::SparseMemoryBindFlags::empty(),
                },
            );
            gr.mem[i] = Some(al);
            gr.bound += gr.size;
            GRAN_BOUND.fetch_add(gr.size, Ordering::Relaxed);
            fresh.push(i);
        }
        let gsz = gr.size;
        // (in the upload buffer: a parallel run's shared one, audit H1)
        let _pre = super::gate::pre_lock();
        // the new granules' content: zeros, then what the guest memory holds there (its committed parts)
        for &i in &fresh {
            unsafe {
                g.dev()
                    .cmd_fill_buffer(sub.pre, buf, i as u64 * gsz, gsz, 0)
            };
        }
        super::queue::full_barrier(g, sub.pre);
        if self.placement == Placement::Mirror || self.placement == Placement::Vram {
            for &i in &fresh {
                let lo = self.base + i as u64 * gsz;
                let hi = (lo + gsz).min(self.base + self.size);
                if hi <= lo {
                    continue;
                }
                super::lazy::touch(lo, hi - lo);
                super::upskip::forget(lo, hi - lo);
                for (ca, cn) in committed(lo, hi - lo) {
                    let src = unsafe { core::slice::from_raw_parts(ca as *const u8, cn as usize) };
                    sub.upload(g, src, buf, ca - self.base);
                    UPLOAD_BYTES.fetch_add(cn, Ordering::Relaxed);
                }
            }
        }
        Ok(())
    }

    /// Uploads this pool's deferred CPU writes overlapping [a, a + n) (all of them with `n` = u64::MAX), from guest
    /// memory as it is now.
    pub fn upload_deferred(&mut self, g: &Gpu, sub: &mut super::queue::Sub, a: u64, n: u64) {
        let end = a.saturating_add(n);
        let mut keep = vec![];
        for (da, dn) in std::mem::take(&mut self.deferred) {
            if da + dn <= a || da >= end {
                keep.push((da, dn));
                continue;
            }
            super::upskip::forget(da, dn);
            self.upload_now(g, sub, da, dn);
            DEFERRED_USED.fetch_add(dn, Ordering::Relaxed);
        }
        self.deferred = keep;
    }

    fn upload_now(&self, g: &Gpu, sub: &mut super::queue::Sub, a: u64, n: u64) {
        // SAFETY: committed guest memory of this pool (the CPU-written range being uploaded)
        let src = unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) };
        self.upload_from(g, sub, a, src);
    }

    /// Uploads `src`, the bytes of [a, a + src.len()) of this pool (a copy taken by the caller, or guest memory).
    fn upload_from(&self, g: &Gpu, sub: &mut super::queue::Sub, a: u64, src: &[u8]) {
        // (in the upload buffer: a parallel run's shared one, audit H1)
        let _pre = super::gate::pre_lock();
        for (buf, boff, pa, pn) in self.pieces(a, src.len() as u64) {
            // granules not bound yet take the guest bytes when they are bound
            for (qa, qn) in self.bound_parts(pa, pn) {
                let o = (qa - a) as usize;
                sub.upload(g, &src[o..o + qn as usize], buf, boff + (qa - pa));
            }
        }
        UPLOAD_BYTES.fetch_add(src.len() as u64, Ordering::Relaxed);
    }

    /// [a, a + n) as the runs that lie in bound granules (all of it for a pool backed whole).
    fn bound_parts(&self, a: u64, n: u64) -> Vec<(u64, u64)> {
        let Some(gr) = self.gran.as_ref() else {
            return vec![(a, n)];
        };
        let mut out: Vec<(u64, u64)> = vec![];
        let (g0, g1) = granules_of(gr.size, a - self.base, n);
        for i in g0..g1.min(gr.mem.len()) {
            if gr.mem[i].is_none() {
                continue;
            }
            let lo = (self.base + i as u64 * gr.size).max(a);
            let hi = (self.base + (i as u64 + 1) * gr.size).min(a + n);
            match out.last_mut() {
                Some(l) if l.0 + l.1 == lo => l.1 += hi - lo,
                _ => out.push((lo, hi - lo)),
            }
        }
        out
    }

    /// The bound span around [a, a + n) after `bind` (a resolve's cache entry stays inside memory that exists).
    fn bound_span(&self, a: u64, n: u64) -> (u64, u64) {
        let Some(gr) = self.gran.as_ref() else {
            return (self.base, self.size);
        };
        let (mut g0, mut g1) = granules_of(gr.size, a - self.base, n);
        while g0 > 0 && gr.mem[g0 - 1].is_some() {
            g0 -= 1;
        }
        while g1 < gr.mem.len() && gr.mem[g1].is_some() {
            g1 += 1;
        }
        let lo = self.base + g0 as u64 * gr.size;
        (
            lo,
            (self.base + g1 as u64 * gr.size).min(self.base + self.size) - lo,
        )
    }

    /// The buffer's device address (raw global memory) once it has one.
    fn note_address(&mut self, g: &Gpu) {
        if g.d.caps.bda {
            if let Some(b) = &self.buf {
                let ai = vk::BufferDeviceAddressInfo::default().buffer(b.buf);
                let start = unsafe { g.dev().get_buffer_device_address(&ai) };
                self.da = start + self.buf_off;
                ADDRS
                    .write()
                    .unwrap()
                    .get_or_insert_with(HashMap::new)
                    .insert(vk::Handle::as_raw(b.buf), start);
            }
            self.wins_da.clear();
            for w in &self.wins {
                let start = unsafe {
                    g.dev().get_buffer_device_address(
                        &vk::BufferDeviceAddressInfo::default().buffer(w.buf),
                    )
                };
                self.wins_da.push(start);
                ADDRS
                    .write()
                    .unwrap()
                    .get_or_insert_with(HashMap::new)
                    .insert(vk::Handle::as_raw(w.buf), start);
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
    let hi = base.checked_add(size).and_then(|end| end.checked_add(al - 1))
        .ok_or("import range overflow")? & !(al - 1);
    if size == 0 { return Err("empty import range".into()); }
    let ht = vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT;
    let mut hp = vk::MemoryHostPointerPropertiesEXT::default();
    let r = unsafe {
        (g.d.emh.fp().get_memory_host_pointer_properties_ext)(
            g.dev().handle(),
            ht,
            lo as *const core::ffi::c_void,
            &mut hp,
        )
    };
    if r != vk::Result::SUCCESS {
        return Err(format!("host pointer properties: {r}"));
    }
    let mut ext = vk::ExternalMemoryBufferCreateInfo::default().handle_types(ht);
    let ci = vk::BufferCreateInfo::default()
        .size(hi - lo)
        .usage(buf_usage(g))
        .push_next(&mut ext);
    let (buf, mem) = super::queue::submission_safety::create_bound_buffer(
    || unsafe { g.dev().create_buffer(&ci, None) }.map_err(|e| format!("{e}")), |buf| {
    let req = unsafe { g.dev().get_buffer_memory_requirements(*buf) };
    let ty =
        g.d.mem_type(
            req.memory_type_bits & hp.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE,
        )
        .ok_or("no memory type")?;
    let mut imp = vk::ImportMemoryHostPointerInfoEXT::default()
        .handle_type(ht)
        .host_pointer(lo as *mut core::ffi::c_void);
    let mut fl =
        vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
    let mut ai = vk::MemoryAllocateInfo::default()
        .allocation_size(hi - lo)
        .memory_type_index(ty)
        .push_next(&mut imp);
    if g.d.caps.bda {
        ai = ai.push_next(&mut fl);
    }
    unsafe { g.dev().allocate_memory(&ai, None) }.map_err(|e| format!("import: {e}"))
    }, |buf, mem| unsafe { g.dev().bind_buffer_memory(*buf, *mem, 0) }.map_err(|e| format!("{e}")),
        |buf| unsafe { g.dev().destroy_buffer(buf, None) },
        |mem| unsafe { g.dev().free_memory(mem, None) })?;
    let addr = if g.d.caps.bda {
        unsafe {
            g.dev()
                .get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buf))
        }
    } else {
        0
    };
    Ok((
        Buf {
            buf,
            alloc: None,
            mem,
            size: hi - lo,
            ptr: lo as *mut u8,
            addr,
        },
        base - lo,
    ))
}

/// Pool buffers' device addresses by handle (entered when a pool gets its buffer, removed when it goes).
static ADDRS: std::sync::RwLock<Option<HashMap<u64, u64>>> = std::sync::RwLock::new(None);

/// The device address of a pool's buffer (descriptor buffers).
pub fn buffer_address(buf: vk::Buffer) -> Option<u64> {
    ADDRS
        .read()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&vk::Handle::as_raw(buf)).copied())
}

/// Read-only preflight: no allocation, upload, sparse bind or cache publication.
/// Pool release is deferred through the open recording by `release`/`defer_after_use`.
pub(crate) fn prepared_span(g: &Gpu, a: u64, n: u64) -> Option<(vk::Buffer, u64)> {
    let end = a.checked_add(n)?;
    if a == 0 || n == 0 || crate::nvn::vpool::is_sparse(a) { return None; }
    let m = g.mem.lock().unwrap();
    let (&base, p) = m.by_base.range(..=a).next_back()?;
    if end > p.base.checked_add(p.size)? || p.gran.is_some() || !p.deferred.is_empty() {
        return None;
    }
    if m.by_base.range((std::ops::Bound::Excluded(base), std::ops::Bound::Unbounded))
        .next().is_some_and(|(&next, _)| end > next) { return None; }
    let (buffer, offset, _, _) = p.one(a, n)?;
    let owner = p.buf.iter().chain(p.wins.iter()).find(|b| b.buf == buffer)?;
    (offset.checked_add(n)? <= owner.size).then_some((buffer, offset))
}

/// A GPU address range as (buffer, offset, pool base); creates the pool's buffer at its first use.
pub fn resolve(
    g: &Gpu,
    sub: &mut super::queue::Sub,
    a: u64,
    n: u64,
) -> Option<(vk::Buffer, u64, u64)> {
    super::perf::timed_fine(super::perf::C::ResolveNs, || resolve_in(g, sub, a, n))
}

fn resolve_in(
    g: &Gpu,
    sub: &mut super::queue::Sub,
    a: u64,
    n: u64,
) -> Option<(vk::Buffer, u64, u64)> {
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
    // ranges of the same few (Donkey Kong Bananza: dozens, 7.5 ms of lookups a frame with 4 kept): the pool lock and lookup were most of the replay's 1.1 ms of `memory` in Z-A)
    thread_local! {
        // (generation, pool base, pool size, the next pool's base, buffer, buffer offset of base)
        static LAST: std::cell::Cell<[(u64, u64, u64, u64, vk::Buffer, u64, u64); 16]> = const { std::cell::Cell::new([(0, 0, 0, 0, vk::Buffer::null(), 0, 0); 16]) };
    }
    let gen = POOL_GEN.load(Ordering::Acquire);
    let n1 = n.max(1);
    // `find` takes the pool with the highest base at or below the address (pools laid over each other): a hit only
    // below the next pool's base
    let hit = LAST.with(|l| {
        l.get()
            .iter()
            .find(|e| {
                e.0 == gen
                    && a >= e.1
                    && a + n1 <= e.1 + e.2
                    && a < e.3
                    && e.4 != vk::Buffer::null()
            })
            .copied()
    });
    if let Some((_, lo, _, _, buf, off, pool)) = hit {
        return Some((buf, off + (a - lo), pool));
    }
    let mut m = super::gate::lock(&g.mem, 1);
    let Some(b) = find(&m, a, n1) else {
        static N: AtomicU64 = AtomicU64::new(0);
        if N.fetch_add(1, Ordering::Relaxed) < 8 {
            crate::log::red(&format!(
                "GPU address {a:#x} (+{n:#x}) is in no memory pool"
            ));
        }
        return None;
    };
    let p = m.by_base.get_mut(&b).unwrap();
    // the pool's buffer made, granules bound or deferred CPU writes uploaded: first-use initialization in the upload
    // buffer (`gate::pass_pre`; checked only in the gate's reference mode)
    if super::gate::all() {
        let end = a.saturating_add(n1);
        let need = p.buf.is_none()
            || p.deferred.iter().any(|&(da, dn)| da < end && a < da + dn)
            || p.gran.as_ref().is_some_and(|gr| {
                let (g0, g1) = granules_of(gr.size, a.saturating_sub(p.base), n1);
                !(g0..g1.min(gr.mem.len())).all(|i| gr.mem[i].is_some())
            });
        if !super::gate::pass(need) {
            return None;
        }
    }
    if let Err(e) = p.ensure_buffer(g, sub) {
        crate::log::red_once(&e);
        return None;
    }
    if let Err(e) = p.bind(g, sub, a, n1) {
        crate::log::red_once(&e);
        return None;
    }
    // the window holding the range (one buffer for most pools)
    let Some((buf, boff, wlo, wlen)) = p.one(a, n1) else {
        crate::log::red_once(&format!(
            "a range {a:#x}+{n:#x} spans two of memory pool {:#x}'s buffers (the device's largest buffer is {} MB): it is not bound (this is logged once)",
            p.base,
            max_buffer(g) >> 20
        ));
        return None;
    };
    // a pool backed in granules: the cache entry covers the bound granules around the range only
    let (wlo, wlen) = if p.gran.is_some() {
        p.bound_span(a, n1)
    } else {
        (wlo, wlen)
    };
    let off = boff - (a - wlo);
    // CPU writes the frame's upload budget left for later: the ones in this range go now (this use needs them)
    if !p.deferred.is_empty() {
        p.upload_deferred(g, sub, a, n1);
    }
    // the generation after the buffer exists (its creation moved it)
    let gen = POOL_GEN.load(Ordering::Acquire);

    let (pb, ps) = (wlo, wlen);
    let cache = p.deferred.is_empty();
    let next = m
        .by_base
        .range(b + 1..)
        .next()
        .map(|(k, _)| *k)
        .unwrap_or(u64::MAX);
    // a pool with deferred writes is not cached: each use looks at them
    if cache {
        LAST.with(|l| {
            let mut v = l.get();
            v.rotate_right(1);
            v[0] = (gen, pb, ps, next, buf, off, b);
            l.set(v);
        });
    }
    Some((buf, off + (a - pb), b))
}

/// Guest ranges a submission in flight writes on the GPU and lands back in guest memory at its completion (its
/// write-backs): (address, bytes, count of submissions writing it). A CPU-dirty page over one is not uploaded while
/// it is in flight: the guest bytes there are older than the GPU's, and uploading them overwrote the GPU's result
/// before it came back (asynchronous replay made the window wide: Mario Kart 8 Deluxe's GPU-computed values read as
/// stale, the black stretches). Those pieces upload once it has landed (NXGFX_WB_GUARD=0: uploaded at once, as before).
static INFLIGHT: std::sync::Mutex<Inflight> = std::sync::Mutex::new(Inflight {
    by_start: std::collections::BTreeMap::new(),
    max_len: 0,
});

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

/// `ranges` (sorted, not overlapping) without the whole pages starting at `pages` (sorted here).
fn without_pages(ranges: &[(u64, u64)], pages: &mut [u64]) -> Vec<(u64, u64)> {
    pages.sort_unstable();
    let mut out = Vec::with_capacity(ranges.len());
    for &(a, n) in ranges {
        let mut at = a;
        let e = a + n;
        let i0 = pages.partition_point(|&p| p + PAGE <= a);
        for &p in pages[i0..].iter().take_while(|&&p| p < e) {
            if p > at {
                out.push((at, p - at));
            }
            at = at.max(p + PAGE);
        }
        if e > at {
            out.push((at, e - at));
        }
    }
    out
}

#[cfg(test)]
mod without_pages_tests {
    #[test]
    fn brought_back_pages_are_cut_out_of_the_ranges() {
        let p = super::PAGE;
        let r = [(0, 4 * p), (10 * p, p), (20 * p + 8, 2 * p)];
        let mut pages = vec![10 * p, 2 * p, 0];
        assert_eq!(super::without_pages(&r, &mut pages), vec![(p, p), (3 * p, p), (20 * p + 8, 2 * p)]);
        assert_eq!(super::without_pages(&r, &mut []), r.to_vec());
    }
}

/// A pool's pending CPU-written ranges split at [lo, hi): (the parts inside, the parts outside).
fn split_pending(pending: Vec<(u64, u64)>, lo: u64, hi: u64) -> (Vec<(u64, u64)>, Vec<(u64, u64)>) {
    let (mut inside, mut outside) = (Vec::new(), Vec::new());
    for (a, n) in pending {
        let e = a + n;
        if e <= lo || a >= hi {
            outside.push((a, n));
            continue;
        }
        if a < lo {
            outside.push((a, lo - a));
        }
        if e > hi {
            outside.push((hi, e - hi));
        }
        let (s, t) = (a.max(lo), e.min(hi));
        inside.push((s, t - s));
    }
    (inside, outside)
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
    (
        now.into_iter().map(|(s, e)| (s, e - s)).collect(),
        later.into_iter().map(|(s, e)| (s, e - s)).collect(),
    )
}

/// Uploads held back for an in-flight write-back (submissions that had some), and their bytes (counted at every sync
/// that holds them back again).
pub static WB_DEFERRED: AtomicU64 = AtomicU64::new(0);
pub static WB_HELD_B: AtomicU64 = AtomicU64::new(0);
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
/// Bytes scanned in pools the CPU wrote in this or the last frame (warm) and in the others (lukewarm: 2-60 frames ago);
/// writes found in a lukewarm pool after a frame's first submission (what scanning lukewarm pools only once a frame would
/// have delayed to the next frame). Pool sync's breakdown with NXGFX_PROFILE=1.
pub static WATCH_WARM_B: AtomicU64 = AtomicU64::new(0);
pub static WATCH_LUKE_B: AtomicU64 = AtomicU64::new(0);
pub static WATCH_LUKE_HITS: AtomicU64 = AtomicU64::new(0);
/// Pool sync's parts a submission (`begin_submit`): waiting for the pool lock, the write-watch scan with its set-up,
/// the pass over every pool (logged with the line above).
pub static SYNC_LOCK_NS: AtomicU64 = AtomicU64::new(0);
pub static SYNC_SCAN_NS: AtomicU64 = AtomicU64::new(0);
pub static SYNC_LOOP_NS: AtomicU64 = AtomicU64::new(0);
/// Pool syncs by the submission that asked: a stream's replay, a present, a one-off (texel reads, tools, decodes)
pub static SYNC_BY: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
struct SyncLoopTimer(std::time::Instant);
impl Drop for SyncLoopTimer {
    fn drop(&mut self) {
        SYNC_LOOP_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}
struct WatchTimer(std::time::Instant);
impl Drop for WatchTimer {
    fn drop(&mut self) {
        WATCH_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

/// (pool base, page start, page end, allocation base), ordered by pool base.
type WatchedPool = (u64, u64, u64, u64);

/// The largest gap between two watched pools of one allocation that one write-watch call spans. GetWriteWatch walks
/// every page of its range, written or not, at a few nanoseconds a page (~1-1.7 ms a GB here), and a call costs about
/// as much as walking a megabyte: a gap past this is cheaper asked as two calls. (The old rule let the gaps of a run
/// grow to half the pools' bytes: a run over pools of 400 MB walked up to 200 MB that no pool holds, at every scan.)
const WATCH_GAP: u64 = 1 << 20;

fn watch_batch_end(watched: &[WatchedPool], i: usize) -> (usize, u64) {
    let (mut j, mut hi, ab) = (i + 1, watched[i].2, watched[i].3);
    while j < watched.len() {
        let (_, next_lo, next_hi, next_ab) = watched[j];
        if next_lo > hi && (ab == 0 || next_ab != ab || next_lo - hi > WATCH_GAP) {
            break;
        }
        WATCH_GAP_B.fetch_add(next_lo.saturating_sub(hi), Ordering::Relaxed);
        hi = hi.max(next_hi);
        j += 1;
    }
    (j, hi)
}

/// Bytes of gaps between pools the write-watch calls walked (pool sync's breakdown with NXGFX_PROFILE=1).
pub static WATCH_GAP_B: AtomicU64 = AtomicU64::new(0);

/// Resetting a scan clears pages belonging to *every* overlapping pool, including
/// cold pools between the requested ones and aliases sharing a boundary page.
/// Distribute the union only after all scans, so a later empty scan cannot replace
/// a dirty result from an earlier overlapping range.
fn distribute_watched_pages(
    pools: &[WatchedPool],
    pages: &mut Vec<u64>,
    taken: &mut HashMap<u64, Vec<u64>>,
) {
    pages.sort_unstable();
    pages.dedup();
    for &(base, lo, hi, _) in pools {
        let a = pages.partition_point(|&p| p < lo);
        let b = pages.partition_point(|&p| p < hi);
        if a != b {
            taken
                .entry(base)
                .or_default()
                .extend_from_slice(&pages[a..b]);
        }
    }
}

#[cfg(test)]
mod watch_batch_tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn real_write_watch_reset_consumes_a_cold_pools_notification() {
        use windows_sys::Win32::System::Memory::{
            VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
        };
        const MEM_WRITE_WATCH: u32 = 0x20_0000;
        struct Allocation(*mut core::ffi::c_void);
        impl Drop for Allocation {
            fn drop(&mut self) {
                unsafe {
                    VirtualFree(self.0, 0, MEM_RELEASE);
                }
            }
        }
        // No Vulkan/device work: exercise Windows' actual reset semantics.
        let a = Allocation(unsafe {
            VirtualAlloc(
                core::ptr::null(),
                (4 * PAGE) as usize,
                MEM_COMMIT | MEM_RESERVE | MEM_WRITE_WATCH,
                PAGE_READWRITE,
            )
        });
        assert!(!a.0.is_null());
        let lo = a.0 as u64;
        unsafe {
            (a.0 as *mut u8).add(PAGE as usize).write_volatile(7);
        }
        let mut pages = watch::take(lo, lo + 4 * PAGE, true).unwrap();
        assert!(pages.contains(&(lo + PAGE)));
        assert!(
            watch::take(lo + PAGE, lo + 2 * PAGE, true)
                .unwrap()
                .is_empty(),
            "a later scan cannot recover the notification reset by its warm neighbours"
        );
        let mut taken = HashMap::new();
        distribute_watched_pages(
            &[(lo + PAGE, lo + PAGE, lo + 2 * PAGE, lo)],
            &mut pages,
            &mut taken,
        );
        assert_eq!(taken[&(lo + PAGE)], [lo + PAGE]);
    }

    #[test]
    fn batched_reset_keeps_cold_gap_and_overlapping_pool_writes() {
        let pools = [
            (0x1000, 0x1000, 0x2000, 1),
            (0x2000, 0x2000, 0x4000, 1),
            (0x2800, 0x2000, 0x3000, 1),
            (0x4000, 0x4000, 0x5000, 1),
        ];
        // Only first/last are warm. One batch spans the cold pools and resets
        // their flags, so those pages must wake BOTH owners in this submission.
        let warm = [pools[0], pools[3]];
        assert_eq!(watch_batch_end(&warm, 0), (2, 0x5000));
        let mut taken = HashMap::from([(0x1000, vec![]), (0x4000, vec![])]);
        distribute_watched_pages(&pools, &mut vec![0x4000, 0x2000, 0x2000], &mut taken);
        assert_eq!(taken[&0x2000], [0x2000]);
        assert_eq!(taken[&0x2800], [0x2000]);
        assert_eq!(taken[&0x4000], [0x4000]);
        assert!(taken[&0x1000].is_empty());
    }

    #[test]
    fn individual_gap_allowances_cannot_accumulate_into_a_giant_scan() {
        let pools: Vec<_> = (0..100)
            .map(|i| {
                let base = i * (4 << 20);
                (base, base, base + PAGE, 1)
            })
            .collect();
        // a gap of nearly 4 MiB costs more to walk than a second call
        assert_eq!(watch_batch_end(&pools, 0), (1, PAGE));
        let contiguous = [(0, 0, PAGE, 1), (PAGE, PAGE, 2 * PAGE, 1)];
        assert_eq!(watch_batch_end(&contiguous, 0), (2, 2 * PAGE));
    }

    /// Two pools of one allocation share a call while the gap between them is at most `WATCH_GAP`, however large the
    /// pools are (the old rule let a run over large pools walk gaps of up to half their bytes).
    #[test]
    fn a_run_spans_only_small_gaps() {
        let mb = 1u64 << 20;
        let near = [(0, 0, 400 * mb, 1), (401 * mb, 401 * mb, 800 * mb, 1)];
        assert_eq!(watch_batch_end(&near, 0), (2, 800 * mb), "a 1 MiB gap is walked");
        let far = [(0, 0, 400 * mb, 1), (401 * mb + PAGE, 401 * mb + PAGE, 800 * mb, 1)];
        assert_eq!(watch_batch_end(&far, 0), (1, 400 * mb), "past 1 MiB: two calls");
        let three = [(0, 0, PAGE, 1), (mb, mb, mb + PAGE, 1), (2 * mb + PAGE, 2 * mb + PAGE, 3 * mb, 1)];
        assert_eq!(watch_batch_end(&three, 0), (3, 3 * mb), "each gap is judged by itself");
        let other_allocation = [(0, 0, PAGE, 1), (2 * PAGE, 2 * PAGE, 3 * PAGE, 2)];
        assert_eq!(watch_batch_end(&other_allocation, 0), (1, PAGE));
    }

    #[test]
    fn distant_allocations_are_not_joined_and_nested_spans_do_not_count_twice() {
        let pools = [
            (0, 0, 8 << 20, 1),
            (PAGE, PAGE, 8 << 20, 1),
            (14 << 20, 14 << 20, (14 << 20) + PAGE, 1),
        ];
        assert_eq!(watch_batch_end(&pools, 0), (2, 8 << 20));
        assert_eq!(
            watch_batch_end(&[(0, 0, PAGE, 1), (2 * PAGE, 2 * PAGE, 3 * PAGE, 2)], 0),
            (1, PAGE)
        );
    }
}

/// Start of a submission: every CPU write since the last one goes up to the mirrors (recorded into the
/// submission's upload command buffer). Returns the written ranges (textures over them may need a new upload).
/// The write-watch scans' coverage, as replay-queue sequence numbers (`queue::aq_tail` read as a scan started): the
/// last scan's, and the last one that asked every pool (cold ones too). 0: none yet.
static SCAN_TAIL_ANY: AtomicU64 = AtomicU64::new(0);
static SCAN_TAIL_FULL: AtomicU64 = AtomicU64::new(0);
/// Submissions whose pool sync needed no scan (`scan_covers`).
pub static SCANS_SKIPPED: AtomicU64 = AtomicU64::new(0);

/// CPU event signals so far (`note_cpu_signal`), and their count when the last scans started.
static CPU_SIGNALS: AtomicU64 = AtomicU64::new(0);
static SCAN_SIG_ANY: AtomicU64 = AtomicU64::new(u64::MAX);
static SCAN_SIG_FULL: AtomicU64 = AtomicU64::new(u64::MAX);

/// The game's CPU signals an event, before the event's word changes. Work already queued may wait on it and then
/// read what the CPU wrote just before signalling: a scan that started before the signal does not cover that work
/// (MKW-RACE audit: a wait that already passes at replay does not park, so the "parked resumes scan" rule misses it).
pub fn note_cpu_signal() {
    CPU_SIGNALS.fetch_add(1, Ordering::SeqCst);
}

fn cpu_signals() -> u64 {
    CPU_SIGNALS.load(Ordering::SeqCst)
}

/// A scan that started once the replay queue's tail was `tail` and `signals` CPU event signals had been made (`full`:
/// no pool left out).
fn note_scan(tail: u64, signals: u64, full: bool) {
    let _l = SCAN_MU.lock().unwrap();
    SCAN_TAIL_ANY.store(tail, Ordering::Release);
    SCAN_SIG_ANY.store(signals, Ordering::Release);
    if full {
        SCAN_TAIL_FULL.store(tail, Ordering::Release);
        SCAN_SIG_FULL.store(signals, Ordering::Release);
    }
}

/// The coverage pairs (tail, signals) change together: the replay's scans and the pool walker's walks both note them.
static SCAN_MU: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A complete walk of every watched pool by the pool walker, started once the queue's tail was `tail` and `signals`
/// CPU signals had been made, counts as a scan of every pool (`scan_covers`): it took every page written before it
/// started, and the next sync drains what it took into the pools' pending writes and uploads them before its own
/// work (`watch_lock`). Kept only when newer than the scans noted already.
/// `full` false: a walk that left the cold pools out counts as a scan that did (a mid-frame sync's).
fn note_walk(tail: u64, signals: u64, full: bool) {
    let _l = SCAN_MU.lock().unwrap();
    for (t, s) in [(&SCAN_TAIL_FULL, &SCAN_SIG_FULL), (&SCAN_TAIL_ANY, &SCAN_SIG_ANY)] {
        if !full && std::ptr::eq(t, &SCAN_TAIL_FULL) {
            continue;
        }
        if tail > t.load(Ordering::Acquire) {
            t.store(tail, Ordering::Release);
            s.store(signals, Ordering::Release);
        }
    }
}

/// `NXGFX_WALKER_HELP=1`: a sync no scan covers finishes a walk in progress instead of scanning (`begin_submit_ex`).
fn walker_help() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_WALKER_HELP").as_deref() == Ok("1"))
}

/// `NXGFX_WALKER_SYNCS=0`: the walker walks only for presents and its walks cover presents' syncs only.
fn walker_syncs() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_WALKER_SYNCS").as_deref() != Ok("0"))
}

/// Whether work the game queued as replay op `due` needs no write-watch scan of its own: a scan (of every pool when
/// `full` is asked for) started after the op was queued, so it took every CPU write the game made before queuing it;
/// the uploads it made are in an earlier submission, which the GPU runs first. Mario Kart World replays ~10 streams a
/// frame, each scanning ~2.8 GB of mirrors (~4 ms): ~44 ms of its ~107 ms race frame (MKW-RACE). A parked queue's
/// rest counts as the newest op the replay thread has taken (`queue::resume_due`; a CPU event signal that released it
/// is refused below). Work not from the queue (`None`: replay on the game's thread, one-offs) always scans.
/// `NXGFX_SCAN_COVER=0`: every submission scans.
fn scan_covers(due: Option<u64>, full: bool) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("NXGFX_SCAN_COVER").as_deref() != Ok("0")) {
        return false;
    }
    let last = if full { &SCAN_TAIL_FULL } else { &SCAN_TAIL_ANY }.load(Ordering::Acquire);
    let sig = if full { &SCAN_SIG_FULL } else { &SCAN_SIG_ANY }.load(Ordering::Acquire);
    // no CPU event signal since that scan started (a signal may release work that reads newer CPU writes)
    due.is_some_and(|d| d != 0 && d <= last) && sig == cpu_signals()
}

// ---------- the pool walker (MKW-RECORD step 1) ----------
//
// A present's pool sync is the frame's first (`present_frame` counts the frame before it), so it asked every watched
// pool, cold ones too: ~4.4 ms of write-watch walks a frame on the replay thread in Mario Kart World, under the pool
// lock (completion's write-backs waited behind it). The game queues the present ~20 ms before the replay thread takes
// it: when it does, a `vk pool walker` thread starts that walk at once, in pieces of `WALK_PIECE` bytes, each under the
// watch lock `WATCH_MU` only (a piece's pages wait in its stash, which every holder of the pool lock that takes or
// resets the watch drains into the pools' `pending` first, so no sync, landing or lazy check misses a reset page). A
// walk that started after the present was queued and found every range covers the present's sync (`walker_covers`;
// a walk still running then is finished by the sync itself): every CPU write the game made before queuing the
// present was taken and is uploaded by the next sync, the present's included. Cold pools keep their rule (asked once
// a frame, at the walk). `NXGFX_POOL_WALKER=0`: off (the present walks every pool itself).

/// Bytes of guest memory one piece of the walker's walk asks under the pool lock (~25 us).
const WALK_PIECE: u64 = 16 << 20;

fn walker_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("NXGFX_POOL_WALKER").as_deref() != Ok("0")
            && std::env::var("NXGFX_SCAN_COVER").as_deref() != Ok("0")
    })
}

struct WalkState {
    /// a walk is asked for (the present queued after the last walk started)
    want: bool,
    running: bool,
    /// `queue::aq_tail` when the last complete walk started (0: none)
    done_tail: u64,
    /// the walk in progress: a present's sync takes its pieces left (`walker_covers`)
    job: Option<std::sync::Arc<WalkJob>>,
    /// a present was queued and its sync has not asked yet: the walker walks again when it ends a walk, so the sync
    /// is covered by the newest walk (cold writes made during the wait reach the GPU this frame; audit M2), at most
    /// this many more times (a present behind parked work never asks)
    waiting: u8,
}
static WALK: std::sync::Mutex<WalkState> = std::sync::Mutex::new(WalkState {
    want: false,
    running: false,
    done_tail: 0,
    job: None,
    waiting: 0,
});

/// One walk: its pieces, taken in order by the walker and by a present's sync that comes while it runs.
struct WalkJob {
    /// `queue::aq_tail` before any page was taken
    tail: u64,
    /// CPU signals made before it started (`cpu_signals`)
    signals: u64,
    pieces: Vec<(u64, u64)>,
    next: std::sync::atomic::AtomicUsize,
    done: std::sync::atomic::AtomicUsize,
    /// every range answered
    ok: std::sync::atomic::AtomicBool,
    /// each piece walked and answered (`walk_covers_pool`)
    piece_done: Vec<std::sync::atomic::AtomicBool>,
    /// every watched pool, cold ones too (a present's walk); false: a walk for syncs, which leaves the cold pools out
    /// as a mid-frame scan does, and covers only what such a scan covers (`note_walk`)
    full: bool,
}

impl WalkJob {
    fn new(tail: u64, signals: u64, pieces: Vec<(u64, u64)>) -> WalkJob {
        WalkJob {
            tail,
            signals,
            piece_done: pieces.iter().map(|_| Default::default()).collect(),
            pieces,
            next: Default::default(),
            done: Default::default(),
            ok: std::sync::atomic::AtomicBool::new(true),
            full: true,
        }
    }

    /// Whether pieces this walk has walked (and answered) cover [lo, hi) whole.
    fn covers(&self, lo: u64, hi: u64) -> bool {
        let first = self.pieces.partition_point(|p| p.1 <= lo);
        let mut at = lo;
        for (i, &(plo, phi)) in self.pieces.iter().enumerate().skip(first) {
            if at >= hi || plo > at {
                break;
            }
            if !self.piece_done[i].load(Ordering::Acquire) {
                return false;
            }
            at = at.max(phi);
        }
        at >= hi
    }
}

/// `NXGFX_WALKER_POOLS=0`: a sync that scans asks every watched pool, also those the walk in progress has walked.
fn walker_pools() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_WALKER_POOLS").as_deref() != Ok("0"))
}

/// The walk in progress, when its pieces may stand for a sync's scan of op `due`: it started after the op was queued
/// and no CPU signal came since (the `scan_covers` rule, per piece).
fn walk_for(due: Option<u64>) -> Option<std::sync::Arc<WalkJob>> {
    let d = due.filter(|&d| d != 0)?;
    if !walker_on() || !walker_pools() {
        return None;
    }
    WALK.lock()
        .unwrap()
        .job
        .clone()
        .filter(|j| j.tail >= d && j.signals == cpu_signals())
}

/// Walks pieces of `job` until none is left to take. `step_aside`: the walker lets a sync or landing waiting for the
/// watch lock go first.
fn walk_some(job: &WalkJob, step_aside: bool) {
    loop {
        // a walk for syncs gives way to a present's full walk: the pieces left count as done but unwalked (the walk
        // covers nothing as a whole; its walked pieces still stand for a scanning sync's pools, `WalkJob::covers`)
        if !job.full && PRESENT_ASKED.load(Ordering::Acquire) {
            let i = job.next.swap(job.pieces.len(), Ordering::AcqRel);
            if i < job.pieces.len() {
                job.ok.store(false, Ordering::Release);
                job.done.fetch_add(job.pieces.len() - i, Ordering::AcqRel);
            }
            return;
        }
        // stepping aside before a piece is claimed (audit L3: a claimed piece a sync waits for in `walk_finish` was
        // held back up to 50 ms by the walker's stepping aside)
        if step_aside && job.next.load(Ordering::Acquire) < job.pieces.len() {
            let t = std::time::Instant::now();
            while WATCH_WANT.load(Ordering::Acquire) != 0 && t.elapsed().as_millis() < 50 {
                std::thread::yield_now();
            }
        }
        let i = job.next.fetch_add(1, Ordering::AcqRel);
        let Some(&(lo, hi)) = job.pieces.get(i) else {
            return;
        };
        let mut s = WATCH_MU.lock().unwrap();
        // a range no longer mapped (a pool gone meanwhile) reads as failed: this walk covers nothing; the pages it did
        // take (reset already) are kept (audit L1)
        let (pages, ok) = watch::take_partial(lo, hi, true);
        s.extend(pages);
        if ok {
            job.piece_done[i].store(true, Ordering::Release);
        }
        drop(s);
        if !ok {
            job.ok.store(false, Ordering::Release);
        }
        job.done.fetch_add(1, Ordering::AcqRel);
    }
}

/// Waits until every taken piece of `job` was walked (at most one piece each by the others). False when the walk
/// covers nothing: a range could not be read, or a piece another thread took did not end within `WALK_FINISH` (audit
/// L3: a walker that died or stalled mid-piece made the waiting sync spin forever, holding the submit lock); the
/// caller then scans itself, which waits for the watch lock as any scan does.
fn walk_finish(job: &WalkJob) -> bool {
    walk_finish_within(job, WALK_FINISH)
}

const WALK_FINISH: std::time::Duration = std::time::Duration::from_millis(100);

/// Walks whose pieces did not end in time (`walk_finish`).
pub static WALK_LATE: AtomicU64 = AtomicU64::new(0);

fn walk_finish_within(job: &WalkJob, limit: std::time::Duration) -> bool {
    let t = std::time::Instant::now();
    while job.done.load(Ordering::Acquire) < job.pieces.len() {
        if t.elapsed() > limit {
            if WALK_LATE.fetch_add(1, Ordering::Relaxed) < 4 {
                crate::vklog!(
                    "pool walker: a walk's pieces did not end within {} ms ({} of {}): the sync scans itself",
                    limit.as_millis(),
                    job.done.load(Ordering::Acquire),
                    job.pieces.len()
                );
            }
            return false;
        }
        std::thread::yield_now();
    }
    job.ok.load(Ordering::Acquire)
}
static WALK_CV: std::sync::Condvar = std::sync::Condvar::new();
/// Walks made, their time, walks that could not read some range (they cover nothing), presents whose sync a walk
/// covered, and the replay thread's time waiting for a walk to end.
pub static WALKS: AtomicU64 = AtomicU64::new(0);
/// Walks for syncs without the cold pools, and those that covered nothing (`COLD_GEN` moved).
pub static WALKS_WARM: AtomicU64 = AtomicU64::new(0);
pub static WALKS_WARM_VOID: AtomicU64 = AtomicU64::new(0);
pub static WALK_NS: AtomicU64 = AtomicU64::new(0);
pub static WALK_FAILED: AtomicU64 = AtomicU64::new(0);
pub static WALK_COVERED: AtomicU64 = AtomicU64::new(0);
pub static WALK_WAIT_NS: AtomicU64 = AtomicU64::new(0);

/// A present is being queued (replay queue op `seq`, before the replay thread can see it): the walker walks every
/// watched pool. Its walk starts after the op is queued (the queue's tail is past `seq` already).
pub fn walker_kick() {
    if !walker_on() {
        return;
    }
    static SPAWN: std::sync::Once = std::sync::Once::new();
    SPAWN.call_once(|| {
        if let Err(e) = std::thread::Builder::new()
            .name("vk pool walker".into())
            .spawn(walker_main)
        {
            crate::log::red_once(&format!("the pool walker thread did not start: {e}"));
        }
    });
    {
        let mut w = WALK.lock().unwrap();
        w.want = true;
        w.waiting = 3;
    }
    // a walk for syncs in progress stops at its next piece: the present's full walk starts sooner
    PRESENT_ASKED.store(true, Ordering::Release);
    WALK_CV.notify_all();
}

/// A present asked for a walk and no full walk has started since (`walk_some` ends a walk for syncs early).
static PRESENT_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn walker_main() {
    let mut last_tail = 0;
    loop {
        let full;
        {
            let mut w = WALK.lock().unwrap();
            // a present asked; or (`walker_syncs`) ops were queued since the last walk started and wait for the
            // replay: a new walk covers their syncs, which then scan nothing themselves (MKW: ~9 ms a frame of
            // warm scans on the replay thread)
            while !w.want
                && !(walker_syncs()
                    && super::queue::aq_backlog() > 0
                    && super::queue::aq_tail() > last_tail)
            {
                w = WALK_CV
                    .wait_timeout(w, std::time::Duration::from_micros(500))
                    .unwrap()
                    .0;
            }
            // a present asked: every pool; else a walk for the syncs of ops waiting, which leaves the cold pools out
            // (a mid-frame scan asks no cold pool either: MKW's full walks took ~10 ms each, ~3 a frame, so ops queued
            // during one waited for the next and scanned for themselves)
            full = w.want || !warm_walks();
            if w.want {
                PRESENT_ASKED.store(false, Ordering::Release);
            }
            w.want = false;
            w.running = true;
        }
        // read before any page is taken: every op queued up to `tail` was queued before the walk took its pages, and
        // no CPU signal after `signals` came before it
        let signals = cpu_signals();
        let tail = super::queue::aq_tail();
        let cold_gen = COLD_GEN.load(Ordering::Acquire);
        last_tail = tail;
        let t0 = std::time::Instant::now();
        let ok = match super::Gpu::try_get() {
            Some(g) => {
                let list = {
                    let m = g.mem.lock().unwrap();
                    if full {
                        watched_pools(&m)
                    } else {
                        // the pools a sync of this frame would scan (`begin_submit`'s cold rule)
                        let frame = super::present::frames();
                        watched_pools_where(&m, |p| frame.saturating_sub(p.dirty_frame) <= 60)
                    }
                };
                let mut job = WalkJob::new(tail, signals, walk_pieces(&list, WALK_PIECE));
                job.full = full;
                let job = std::sync::Arc::new(job);
                WALK.lock().unwrap().job = Some(job.clone());
                walk_some(&job, true);
                walk_finish(&job)
            }
            None => false,
        };
        WALK_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let n = WALKS.fetch_add(1, Ordering::Relaxed) + 1;
        if !ok {
            WALK_FAILED.fetch_add(1, Ordering::Relaxed);
        }
        {
            let mut w = WALK.lock().unwrap();
            w.running = false;
            w.job = None;
            if ok {
                if full {
                    w.done_tail = w.done_tail.max(tail);
                }
                // a walk without the cold pools stands for no scan once one of them was found written since it started
                // (its pages are still drained by the next sync; it just covers none)
                if walker_syncs() && (full || COLD_GEN.load(Ordering::Acquire) == cold_gen) {
                    note_walk(tail, signals, full);
                } else if !full {
                    WALKS_WARM_VOID.fetch_add(1, Ordering::Relaxed);
                }
            }
            if !full {
                WALKS_WARM.fetch_add(1, Ordering::Relaxed);
            }
            if w.waiting > 0 && rewalk() {
                w.waiting -= 1;
                w.want = true;
            }
        }
        WALK_CV.notify_all();
        if n % 600 == 0 {
            crate::vklog!(
                "pool walker, last 600 walks: {} without the cold pools ({} of them covering no sync: a cold pool written meanwhile), {:.2} ms a walk, {} could not read a range, {} presents' syncs covered ({} finishing a walk), {} other syncs finished one, the replay's syncs walked {:.3} ms a walk of them, scanning syncs left {} MB to walks in progress; syncs and landings waited {:.3} ms a walk for the watch lock (with the drain)",
                WALKS_WARM.swap(0, Ordering::Relaxed),
                WALKS_WARM_VOID.swap(0, Ordering::Relaxed),
                WALK_NS.swap(0, Ordering::Relaxed) as f64 / 600.0 / 1e6,
                WALK_FAILED.swap(0, Ordering::Relaxed),
                WALK_COVERED.swap(0, Ordering::Relaxed),
                WALK_HELPED.swap(0, Ordering::Relaxed),
                WALK_HELPED_SYNCS.swap(0, Ordering::Relaxed),
                WALK_WAIT_NS.swap(0, Ordering::Relaxed) as f64 / 600.0 / 1e6,
                WALK_POOLS_B.swap(0, Ordering::Relaxed) >> 20,
                WATCH_WAIT_NS.swap(0, Ordering::Relaxed) as f64 / 600.0 / 1e6
            );
        }
    }
}

fn watched_pools(m: &Pools) -> Vec<WatchedPool> {
    watched_pools_where(m, |_| true)
}

fn watched_pools_where(m: &Pools, keep: impl Fn(&Pool) -> bool) -> Vec<WatchedPool> {
    m.by_base
        .values()
        .filter(|p| p.watched && matches!(p.placement, Placement::Mirror | Placement::Host) && keep(p))
        .map(|p| {
            let (lo, hi) = page_range(p.base, p.size);
            (p.base, lo, hi, p.alloc_base)
        })
        .collect()
}

/// Moves whenever a cold pool (no CPU write for 60 frames) is found written: a walk that left it out as cold does not
/// stand for a scan that would ask it now.
static COLD_GEN: AtomicU64 = AtomicU64::new(0);

/// `NXGFX_WALKER_WARM=1`: the walker's walks for syncs leave the cold pools out. **Off by default** (UPLOAD-SYNC): no
/// FPS gain measured in MKW (its walker is busy with the present's walks) or DKB, and a one-frame OoT render fault was
/// seen only in builds with it on (3 in ~67 dense shots against 0 in 60 without; not proven either way).
fn warm_walks() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("NXGFX_WALKER_WARM").as_deref() == Ok("1")
            && std::env::var("NXGFX_POOL_COLD").as_deref() != Ok("0")
    })
}

/// [lo, hi) of each run of watched pools (`watch_batch_end`, as a sync asks them), cut into `piece` bytes.
fn walk_pieces(list: &[WatchedPool], piece: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < list.len() {
        let (j, hi) = watch_batch_end(list, i);
        let mut lo = list[i].1;
        while lo < hi {
            let e = (lo + piece).min(hi);
            out.push((lo, e));
            lo = e;
        }
        i = j;
    }
    out
}

/// The write watch's resets outside the pool lock: the walker's taken pages wait here until the next holder of the
/// pool lock drains them into their pools (`drain_walk`). Lock order: `g.mem`, then this. Everything that takes the
/// watch with a reset under the pool lock and relies on what it took (a sync's scan, a landing's take / copy /
/// reset, `sync_range`) holds this too, and drains it first. (The walker under the pool lock held up the replay's
/// draws, which take the pool lock for every storage buffer they write: MKW 7 -> 13 ms of sets a frame.)
static WATCH_MU: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

/// The walker's stashed pages into their pools' `pending` (the pools there now). Under the pool lock.
fn drain_walk(m: &mut Pools, stash: &mut Vec<u64>) {
    if stash.is_empty() {
        return;
    }
    let list = watched_pools(m);
    let mut taken = HashMap::new();
    distribute_watched_pages(&list, stash, &mut taken);
    stash.clear();
    for (base, pgs) in taken {
        if let Some(p) = m.by_base.get_mut(&base) {
            p.pending.extend(pgs.into_iter().map(|pg| (pg, PAGE)));
        }
    }
}

/// Threads waiting for `WATCH_MU` other than the walker: the walker steps aside between its pieces while any wait
/// (the lock is not fair: the walker took it again before a woken sync ran, 3 ms a frame of syncs waiting in MKW).
static WATCH_WANT: AtomicU64 = AtomicU64::new(0);
/// Syncs' and landings' time waiting for the watch lock (the walker's piece), with the drain.
pub static WATCH_WAIT_NS: AtomicU64 = AtomicU64::new(0);

/// The pool lock's holder: the watch lock with the walker's pages drained.
fn watch_lock(m: &mut Pools) -> std::sync::MutexGuard<'static, Vec<u64>> {
    let t0 = std::time::Instant::now();
    WATCH_WANT.fetch_add(1, Ordering::AcqRel);
    let mut s = WATCH_MU.lock().unwrap();
    WATCH_WANT.fetch_sub(1, Ordering::AcqRel);
    drain_walk(m, &mut s);
    WATCH_WAIT_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    s
}

/// `NXGFX_WALKER_REWALK=0`: the walker walks once per present (no new walk while the present still waits).
fn rewalk() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_WALKER_REWALK").as_deref() != Ok("0"))
}

/// A present's sync (op `due`, taken straight from the replay queue): whether a complete walk started after it was
/// queued. A walk that started after it and is still running is finished here: this thread walks the pieces left
/// and waits only for the walker's piece in progress (audit M3: never for a whole walk; with no such walk the sync
/// scans every pool itself, as without the walker). The CPU-signal rule of `scan_covers` does not apply: a present
/// waits on no event, and work a signal releases has syncs of its own.
fn walker_covers(due: Option<u64>) -> bool {
    walker_covers_ex(due, true)
}

/// As `walker_covers`; `present` false: any other sync, whose scan a walk in progress replaces only when it started
/// after the op was queued and after the last CPU signal (the `scan_covers` rule; a complete walk counts through
/// `note_walk`).
fn walker_covers_ex(due: Option<u64>, present: bool) -> bool {
    if !walker_on() {
        return false;
    }
    let Some(d) = due.filter(|&d| d != 0) else {
        return false;
    };
    let t_wait = std::time::Instant::now();
    let job = loop {
        let mut w = WALK.lock().unwrap();
        if present {
            w.waiting = 0;
            if w.done_tail >= d {
                return true;
            }
        }
        // a present's sync asks every pool (and the help path stands in for a full scan): a full walk only
        let job = w
            .job
            .clone()
            .filter(|j| j.full && j.tail >= d && (present || j.signals == cpu_signals()));
        // a walk for syncs is ending early for this present's full walk (`walk_some`), or that one is about to start:
        // wait for it (a piece at most) rather than scan every pool here; not longer than the scan would take (the
        // walker builds its list under the pool lock, which a landing may hold for milliseconds)
        let coming = (w.running && !w.job.as_ref().is_some_and(|j| j.full)) || (w.want && !w.running);
        if job.is_none() && present && coming && t_wait.elapsed() < std::time::Duration::from_millis(2) {
            drop(w);
            std::thread::yield_now();
            continue;
        }
        break job;
    };
    let Some(job) = job else {
        return false;
    };
    let t0 = std::time::Instant::now();
    walk_some(&job, false);
    let ok = walk_finish(&job);
    WALK_WAIT_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    WALK_HELPED.fetch_add(1, Ordering::Relaxed);
    ok
}

/// Bytes of watched pools a scanning sync left to the walk in progress (`walk_for`).
pub static WALK_POOLS_B: AtomicU64 = AtomicU64::new(0);

/// Presents' syncs that finished a walk in progress (`walker_covers`), and other syncs that did (`walker_covers_ex`).
pub static WALK_HELPED: AtomicU64 = AtomicU64::new(0);
pub static WALK_HELPED_SYNCS: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod walker_tests {
    use super::*;

    /// A walk asks the runs a sync asks, in pieces, and nothing between runs that a sync would not walk.
    #[test]
    fn pieces_cover_each_run_and_nothing_else() {
        let mb = 1u64 << 20;
        let list = [
            (0, 0, 40 * mb, 1),
            (40 * mb, 40 * mb, 41 * mb, 1),
            (100 * mb, 100 * mb, 100 * mb + PAGE, 1),
        ];
        let p = walk_pieces(&list, 16 * mb);
        assert_eq!(
            p,
            vec![
                (0, 16 * mb),
                (16 * mb, 32 * mb),
                (32 * mb, 41 * mb),
                (100 * mb, 100 * mb + PAGE)
            ]
        );
        assert!(walk_pieces(&[], 16 * mb).is_empty());
    }

    /// A present is covered only by a complete walk that started after it was queued; a present not from the queue
    /// never is.
    #[test]
    fn coverage_needs_a_complete_walk_started_after_the_present() {
        if !walker_on() {
            return;
        }
        let keep = std::mem::replace(
            &mut *WALK.lock().unwrap(),
            WalkState { want: false, running: false, done_tail: 10, job: None, waiting: 0 },
        );
        assert!(walker_covers(Some(10)));
        assert!(walker_covers(Some(3)));
        assert!(!walker_covers(Some(11)), "queued after the walk started");
        assert!(!walker_covers(None));
        assert!(!walker_covers(Some(0)));
        // a walk in progress that started after the present: the sync finishes it (no wait for a whole walk); one
        // that started before it does not cover it
        let job = |tail, signals| std::sync::Arc::new(WalkJob::new(tail, signals, vec![]));
        WALK.lock().unwrap().job = Some(job(12, 0));
        assert!(walker_covers(Some(12)));
        WALK.lock().unwrap().job = Some(job(11, 0));
        assert!(!walker_covers(Some(12)));
        // another sync: also only when no CPU signal came after the walk started
        let now = cpu_signals();
        WALK.lock().unwrap().job = Some(job(12, now));
        assert!(walker_covers_ex(Some(12), false));
        WALK.lock().unwrap().job = Some(job(12, now.wrapping_sub(1)));
        assert!(!walker_covers_ex(Some(12), false), "a CPU signal since the walk started");
        *WALK.lock().unwrap() = keep;
    }

    /// Pieces taken by two threads: each is walked once, and finishing waits for the other's piece in progress.
    #[test]
    fn a_walk_is_shared_piece_by_piece() {
        let j = WalkJob::new(1, 0, vec![(0, 0); 64]);
        // empty ranges: nothing is read, every piece counts as walked
        std::thread::scope(|s| {
            s.spawn(|| walk_some(&j, false));
            walk_some(&j, false);
        });
        assert_eq!(j.done.load(Ordering::Acquire), 64);
        assert!(j.next.load(Ordering::Acquire) >= 64);
        let _ = walk_finish(&j);
    }

    /// A piece another thread took and never ends (a walker that died mid-piece): finishing gives up after its limit
    /// and the walk covers nothing (audit L3), instead of spinning forever.
    #[test]
    fn a_piece_that_never_ends_does_not_hold_the_sync() {
        let j = WalkJob::new(1, 0, vec![(0, 0); 2]);
        // both pieces claimed, none done
        j.next.store(2, Ordering::Release);
        walk_some(&j, true);
        let t = std::time::Instant::now();
        assert!(!walk_finish_within(&j, std::time::Duration::from_millis(5)));
        assert!(t.elapsed() < std::time::Duration::from_secs(1));
        // once they end, it covers as before
        j.done.store(2, Ordering::Release);
        assert!(walk_finish_within(&j, std::time::Duration::from_millis(5)));
    }

    /// A pool counts as walked only when walked pieces cover all of it, without a gap.
    #[test]
    fn walked_pieces_cover_a_pool_whole() {
        let j = WalkJob::new(1, 0, vec![(0, 100), (100, 200), (300, 400)]);
        let done = |i: usize| j.piece_done[i].store(true, Ordering::Release);
        assert!(!j.covers(10, 50));
        done(0);
        assert!(j.covers(10, 50));
        assert!(!j.covers(50, 150), "the second piece is not walked yet");
        done(1);
        assert!(j.covers(50, 150));
        done(2);
        assert!(!j.covers(150, 350), "200..300 is in no piece");
        assert!(j.covers(300, 400));
        assert!(!j.covers(350, 450), "past the last piece");
    }
}

pub fn begin_submit(g: &Gpu, sub: &mut super::queue::Sub, due: Option<u64>) -> Vec<(u64, u64)> {
    begin_submit_ex(g, sub, due, false)
}

/// As `begin_submit`; `present`: the sync of a present taken straight from the replay queue (the pool walker's walk
/// may cover it).
pub fn begin_submit_ex(
    g: &Gpu,
    sub: &mut super::queue::Sub,
    due: Option<u64>,
    present: bool,
) -> Vec<(u64, u64)> {
    // before the pool lock: the walker takes it for each piece
    let walked = present && walker_covers(due);
    // another sync that no scan covers may finish a walk in progress that does, instead of scanning itself
    // (`NXGFX_WALKER_HELP=1`; off by default: MKW measured it slower, 12.5 vs 10.0 ms of pool sync a frame, since the
    // rest of a walk asks the cold pools too, which the sync's own scan leaves out)
    let helped = !present
        && walker_help()
        && walker_syncs()
        && !scan_covers(due, true)
        && walker_covers_ex(due, false);
    if helped {
        WALK_HELPED_SYNCS.fetch_add(1, Ordering::Relaxed);
    }
    let t_lock = std::time::Instant::now();
    let mut m = g.mem.lock().unwrap();
    // the walker's pages first, and none taken by it while this sync scans and uploads
    let _watch = watch_lock(&mut m);
    SYNC_LOCK_NS.fetch_add(t_lock.elapsed().as_nanos() as u64, Ordering::Relaxed);
    let t_scan = std::time::Instant::now();
    m.gen = m.gen.wrapping_add(1);
    let gen = m.gen;
    let mut all = Vec::new();
    // the write watch asked once per run of watched pools whose pages touch (Pokemon Legends Z-A has ~400 mirrors,
    // most of them neighbours: one GetWriteWatch each was 3.5 ms a frame of `pools`; Mario Kart World 21 ms)
    let mut taken: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut retry_watch = std::collections::HashSet::new();
    let mut cold_skipped: std::collections::HashSet<u64> = std::collections::HashSet::new();
    // cold pools (no CPU write for 60 frames) are asked once a frame, not at every submission: GetWriteWatch walks
    // every page of its range, and Donkey Kong Bananza's ~1,300 mirrors (1.9 GB) at 9 submissions a frame cost
    // ~33 ms a frame. A write to a cold pool reaches the GPU at the next frame's first submission (the pool is hot
    // from then on). NXGFX_POOL_COLD=0: every pool at every submission.
    let frame = super::present::frames();
    static COLD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let cold_on = *COLD.get_or_init(|| std::env::var("NXGFX_POOL_COLD").as_deref() != Ok("0"));
    let new_frame = m.scan_frame != frame;
    // a later submission of this frame (its first one asked every pool already)
    let mid_frame = !new_frame;
    // the scan this one would make (every pool, or the cold ones left out) already started after the game queued this
    // work: every CPU write made before it was queued has been taken (`scan_covers`)
    let covered = walked || helped || scan_covers(due, new_frame || !cold_on);
    if walked {
        // the walk asked every pool, cold ones too: this frame's later syncs leave the cold ones out
        WALK_COVERED.fetch_add(1, Ordering::Relaxed);
        m.scan_frame = frame;
    }
    if covered {
        SCANS_SKIPPED.fetch_add(1, Ordering::Relaxed);
    } else {
        let signals = cpu_signals();
        let tail = super::queue::aq_tail();
        m.scan_frame = frame;
        let skip = |p: &Pool| cold_on && !new_frame && frame.saturating_sub(p.dirty_frame) > 60;
        cold_skipped = m
            .by_base
            .values()
            .filter(|p| p.watched && skip(p))
            .map(|p| p.base)
            .collect();
        let all_watched: Vec<WatchedPool> = m
            .by_base
            .values()
            .filter(|p| p.watched && matches!(p.placement, Placement::Mirror | Placement::Host))
            .map(|p| {
                let (lo, hi) = page_range(p.base, p.size);
                (p.base, lo, hi, p.alloc_base)
            })
            .collect();
        // pools the walk in progress has walked since this op was queued, with no CPU signal since: their pages were
        // taken and drained into their pending writes when this sync took the watch lock (which it holds, so no piece
        // changes meanwhile); they need no scan here (`NXGFX_WALKER_POOLS=0` off)
        let walk = walk_for(due).filter(|j| j.signals == signals);
        let mut by_walk = 0u64;
        let watched: Vec<_> = all_watched
            .iter()
            .copied()
            .filter(|p| !cold_skipped.contains(&p.0))
            .filter(|p| {
                let w = walk.as_ref().is_some_and(|j| j.covers(p.1, p.2));
                if w {
                    taken.entry(p.0).or_default();
                    by_walk += p.2 - p.1;
                }
                !w
            })
            .collect();
        WALK_POOLS_B.fetch_add(by_walk, Ordering::Relaxed);
        // the scan covers no newer op than the walk that stood for part of it
        let tail = match &walk {
            Some(j) if by_walk > 0 => tail.min(j.tail),
            _ => tail,
        };
        WATCH_POOLS.fetch_add(watched.len() as u64, Ordering::Relaxed);
        if super::perf::fine() {
            for p in m.by_base.values().filter(|p| {
                p.watched && matches!(p.placement, Placement::Mirror | Placement::Host) && !skip(p)
            }) {
                let warm = frame.saturating_sub(p.dirty_frame) <= 1;
                (if warm { &WATCH_WARM_B } else { &WATCH_LUKE_B })
                    .fetch_add(p.size, Ordering::Relaxed);
            }
        }
        let subs = WATCH_SUBS.fetch_add(1, Ordering::Relaxed) + 1;
        if subs % 600 == 0 && super::perf::fine() {
            let (c, ns, np) = (
                WATCH_CALLS.swap(0, Ordering::Relaxed),
                WATCH_NS.swap(0, Ordering::Relaxed),
                WATCH_POOLS.swap(0, Ordering::Relaxed),
            );
            let (wb, lb, lh) = (
                WATCH_WARM_B.swap(0, Ordering::Relaxed),
                WATCH_LUKE_B.swap(0, Ordering::Relaxed),
                WATCH_LUKE_HITS.swap(0, Ordering::Relaxed),
            );
            let gap = WATCH_GAP_B.swap(0, Ordering::Relaxed);
            crate::vklog!(
                "pool sync, last 600 submissions: {:.0} watched pools, {:.0} write-watch calls, {:.2} ms in them a submission; scanned a submission {:.0} MB in pools written this / last frame, {:.0} MB in pools written 2-60 frames ago, {:.0} MB of gaps between pools ({lh} later-submission writes found in those)",
                np as f64 / 600.0,
                c as f64 / 600.0,
                ns as f64 / 600.0 / 1e6,
                wb as f64 / 600.0 / 1048576.0,
                lb as f64 / 600.0 / 1048576.0,
                gap as f64 / 600.0 / 1048576.0
            );
        }
        // a run: pools whose pages touch, or pools of one allocation with gaps between them (one call over the run
        // instead of one per pool: Donkey Kong Bananza has ~1,300 mirrors, 9 submissions a frame)
        let mut i = 0;
        let mut dirty_pages = Vec::new();
        while i < watched.len() {
            let (j, hi) = watch_batch_end(&watched, i);
            if let Some(pages) = watch::take(watched[i].1, hi, true) {
                dirty_pages.extend(pages);
                for &(base, _, _, _) in &watched[i..j] {
                    taken.entry(base).or_default();
                }
            } else {
                retry_watch.extend(watched[i..j].iter().map(|p| p.0));
            }
            i = j;
        }
        distribute_watched_pages(&all_watched, &mut dirty_pages, &mut taken);
        let recovered = cold_skipped
            .iter()
            .filter(|base| taken.get(base).is_some_and(|p| !p.is_empty()))
            .count();
        if recovered != 0 {
            nxconfig::metrics::add(
                nxconfig::metrics::counter("pool_cold_writes_recovered"),
                recovered as u64,
            );
        }
        // a scan whose write-watch read failed for some batch covers nothing (those pools are asked one by one below,
        // and their writes may come later than this scan claims)
        if retry_watch.is_empty() {
            note_scan(tail, signals, new_frame || !cold_on);
        }
    }
    SYNC_SCAN_NS.fetch_add(t_scan.elapsed().as_nanos() as u64, Ordering::Relaxed);
    super::queue::chain_trace::note("scan", t_scan.elapsed().as_micros() as u64, covered as u64);
    let _loop_t = SyncLoopTimer(std::time::Instant::now());
    for p in m.by_base.values_mut() {
        if !p.tex_dirty.is_empty() {
            all.append(&mut p.tex_dirty);
        }
        if !matches!(p.placement, Placement::Mirror | Placement::Host) && p.pending.is_empty() {
            continue;
        }
        if p.pending.is_empty() && cold_skipped.contains(&p.base) && !taken.contains_key(&p.base) {
            continue;
        }
        // no scan this time: a watched pool's writes were taken by the last one (what it left pending or deferred
        // still goes now)
        if covered && p.watched && p.pending.is_empty() && p.deferred.is_empty() {
            continue;
        }
        let mut pre = if covered && p.watched {
            Some(Vec::new())
        } else {
            taken.remove(&p.base)
        };
        if retry_watch.contains(&p.base) {
            // Pages recovered from another scan are only partial coverage here.
            // Retain them while allowing the existing individual-scan fallback.
            p.pending
                .extend(pre.take().into_iter().flatten().map(|page| (page, PAGE)));
        }
        // a watched pool the scan found unwritten, with nothing pending or deferred: nothing below would do anything
        // (it cost a few hundred ns a pool: ~4,000 mirrors at each scanning sync in MKW)
        if p.watched
            && pre.as_ref().is_some_and(|v| v.is_empty())
            && p.pending.is_empty()
            && p.deferred.is_empty()
        {
            continue;
        }
        let mut ranges = p.take_cpu_writes(gen, pre);
        // what the budget left for later comes first (the guest bytes as they are now)
        if !p.deferred.is_empty() {
            ranges.extend(std::mem::take(&mut p.deferred));
            ranges.sort_unstable();
            ranges.dedup();
        }
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
                        WB_HELD_B.fetch_add(later.iter().map(|r| r.1).sum::<u64>(), Ordering::Relaxed);
                        p.pending.extend(later);
                    }
                    if !p.pending.is_empty() {
                        WB_DEFERRED.fetch_add(1, Ordering::Relaxed);
                    }
                    ranges = keep;
                }
            }
        }
        if p.buf.is_some() && (p.placement == Placement::Mirror || p.placement == Placement::Vram) {
            let mut deferred_now = false;
            // pages left out as a lazy write-back's return, unchanged by the CPU since (`upskip::brought_back`)
            let mut back = Vec::new();
            for &(a, n) in &ranges {
                if !budget_fits(n) {
                    p.deferred.push((a, n));
                    DEFERRED_BYTES.fetch_add(n, Ordering::Relaxed);
                    deferred_now = true;
                    continue;
                }
                if super::uptrace::on() {
                    super::uptrace::note(p.base, p.size, a, n);
                }
                // written pages whose bytes the GPU copy already holds stay as they are (`upskip`); the budget is
                // charged with what is uploaded (audit M2)
                let sent = if super::upskip::on() {
                    let (base, size) = (p.base, p.size);
                    super::upskip::upload_changed(base, size, a, n, &mut back, |at, b| p.upload_from(g, sub, at, b))
                } else {
                    p.upload_now(g, sub, a, n);
                    n
                };
                budget_charge(sent);
            }
            if deferred_now {
                // resolves of this pool cached before must look at the deferred ranges
                POOL_GEN.fetch_add(1, Ordering::Relaxed);
            }
            // those pages are no CPU write: the texture check does not reload a texture over them from guest memory
            // (a GPU-owned texture over held pages got the pool's bytes instead of its rendered content, UPLOAD-SYNC
            // audit M2), and they do not make the pool warm (L5)
            if !back.is_empty() {
                ranges = without_pages(&ranges, &mut back);
            }
        }
        if !ranges.is_empty() {
            let f = super::present::frames();
            if super::perf::fine() && f.saturating_sub(p.dirty_frame) > 1 && mid_frame {
                WATCH_LUKE_HITS.fetch_add(1, Ordering::Relaxed);
            }
            if f.saturating_sub(p.dirty_frame) > 60 {
                COLD_GEN.fetch_add(1, Ordering::AcqRel);
            }
            p.dirty_frame = f;
        }
        all.extend(ranges);
    }
    all
}

/// The GPU writes [a, a + n) in this submission: a mirrored range is copied back into guest memory at its fence.
pub fn note_gpu_write(g: &Gpu, sub: &mut super::queue::Sub, a: u64, n: u64) {
    note_gpu_write_version(g, sub, a, n, None);
}

pub(crate) fn note_gpu_write_version(g: &Gpu, sub: &mut super::queue::Sub, a: u64, n: u64,
    identity: Option<u64>) {
    if n == 0 {
        return;
    }
    {
        static W: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
        if let Some(w) = *W.get_or_init(|| {
            std::env::var("NXGFX_WATCH_ADDR")
                .ok()
                .and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok())
        }) {
            if a < w + 0x2000000 && w < a + n {
                static N: AtomicU64 = AtomicU64::new(0);
                if N.fetch_add(1, Ordering::Relaxed) < 40 {
                    crate::vklog!("watch {w:#x}: the GPU writes {a:#x} (+{n:#x})");
                }
            }
        }
    }
    // a sparse virtual pool's range was bound at the physical memory it maps (`resolve_in`): the write-back goes there
    // too, or the GPU's bytes are lost and a later CPU write to the page uploads stale guest memory over them
    // the GPU copy changes: its pages' hashes are parked until this submission's write-back lands (`upskip`)
    super::upskip::gpu_writes(a, n);
    let a = if crate::nvn::vpool::is_sparse(a) {
        match crate::nvn::vpool::translate(a, n) {
            Some(p) => p,
            None => return,
        }
    } else {
        a
    };
    super::upskip::gpu_writes(a, n);
    let m = super::gate::lock(&g.mem, 0);
    let Some(b) = find(&m, a, n) else { return };
    let p = &m.by_base[&b];
    if !writeback_identity_matches(identity, p.identity) { return; }
    if p.placement == Placement::Mirror && p.buf.is_some() {
        super::perf::inc(super::perf::C::Writebacks);
        // one write-back per window the range lies in
        for (buf, off, a, n) in p.pieces(a, n) {
            note_writeback(sub, b, a, n, buf, off, p.identity);
        }
    }
}

/// A child submission's write-backs join its parent's (`queue::child_join`), merged as one submission's are: a range
/// segments wrote one after another is copied and landed once.
pub(crate) fn join_writebacks(parent: &mut super::queue::Sub, wbs: Vec<(u64, u64, u64, vk::Buffer, u64, u64)>) {
    for (b, a, n, buf, off, identity) in wbs {
        note_writeback(parent, b, a, n, buf, off, identity);
    }
}

fn note_writeback(sub: &mut super::queue::Sub, b: u64, a: u64, n: u64, buf: vk::Buffer, off: u64, identity: u64) {
    {
        // ranges that follow each other (or one inside another) in the same pool's buffer are one copy
        for w in sub.writebacks.iter_mut().rev() {
            if w.0 != b || w.3 != buf || w.5 != identity {
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
        sub.writebacks.push((b, a, n, buf, off, identity));
    }
}

/// Completion of a submission: its write-backs land in guest memory. CPU writes the game made meanwhile to the same
/// pages stay marked for the next upload; the write-back's own stores are not taken for CPU writes.
/// `epoch`: the submission's recording epoch (`upskip::next_epoch`): pages it noted as GPU-written get the hash of the
/// GPU's bytes back (`upskip::landed`); `value`: its GPU timeline value.
pub fn apply_writebacks(g: &Gpu, wbs: &[(u64, u64, u64, *const u8, *const u8, u64)], epoch: u64, value: u64) {
    if wbs.is_empty() {
        return;
    }
    let landed: Vec<(u64, u64)> = wbs.iter().map(|w| (w.1, w.2)).collect();
    let _done = InflightDone(landed);
    let t0 = std::time::Instant::now();
    let mut m = g.mem.lock().unwrap();
    // landings take, write and reset pages: the walker must not take a landing's own stores for CPU writes, and the
    // lazy check below sees the walker's pages as pending
    let _watch = watch_lock(&mut m);
    super::lazy::T_LOCK.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    super::queue::chain_trace::note("wb_lock", t0.elapsed().as_micros() as u64, wbs.len() as u64);
    for &(pool, a, n, src, pre, identity) in wbs {
        let Some(p) = m.by_base.get_mut(&pool) else {
            continue;
        };
        if !writeback_identity_matches(Some(identity), p.identity) { continue; }
        // the GPU copy of the range is `src` now (unless a later submission changed it: then it restamped the pages);
        // the pages it found the GPU left alone are not compared again below
        let same = super::upskip::landed(pool, a, n, src, pre, epoch, value);
        // pages GPU writes keep coming to stay on the device (write-back on demand, `lazy`); the rest lands here
        let tl = std::time::Instant::now();
        let mut lazy = if p.watched {
            super::lazy::take_lazy(a, n)
        } else {
            vec![]
        };
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
            // pages a scan took and left for the next sync (the pool walker's, a landing's) are CPU writes too
            if !p.pending.is_empty() {
                lazy.retain(|&pg| !p.pending.iter().any(|&(pa, pn)| pg < pa + pn && pa < pg + PAGE));
            }
            // and so are the ones the upload budget left for later (taken from the watch, not on the device yet: held,
            // a bring-back before their upload put the buffer's page over them, UPSYNC2 L6)
            if !p.deferred.is_empty() {
                lazy.retain(|&pg| !p.deferred.iter().any(|&(pa, pn)| pg < pa + pn && pa < pg + PAGE));
            }
        }
        super::lazy::T_TAKE.fetch_add(tl.elapsed().as_nanos() as u64, Ordering::Relaxed);
        if !lazy.is_empty() {
            let windows: Vec<(u64, u64, vk::Buffer, u64)> = if p.win == 0 {
                vec![(
                    p.base,
                    p.size,
                    p.buf.as_ref().map(|b| b.buf).unwrap_or_default(),
                    p.buf_off,
                )]
            } else {
                (0..=p.wins.len())
                    .filter_map(|k| {
                        p.window(k)
                            .map(|(b, o)| (p.base + k as u64 * p.win, p.win, b, o))
                    })
                    .collect()
            };
            let mut at = a;
            for &pg in lazy.iter().chain(std::iter::once(&(a + n))) {
                let end = pg.min(a + n);
                if end > at {
                    land(
                        p,
                        at,
                        end - at,
                        unsafe { src.add((at - a) as usize) },
                        if pre.is_null() {
                            pre
                        } else {
                            unsafe { pre.add((at - a) as usize) }
                        },
                        &same,
                    );
                }
                at = (pg + PAGE).min(a + n).max(at);
            }
            let tm = std::time::Instant::now();
            for &(wlo, wn, buf, off0) in &windows {
                let pages: Vec<u64> = lazy
                    .iter()
                    .copied()
                    .filter(|&pg| pg >= wlo && pg < wlo + wn)
                    .collect();
                super::lazy::make_lazy(&pages, buf, wlo, off0, pool);
            }
            super::lazy::T_MAKE.fetch_add(tm.elapsed().as_nanos() as u64, Ordering::Relaxed);
            continue;
        }
        let tl = std::time::Instant::now();
        land(p, a, n, src, pre, &same);
        super::lazy::T_LAND.fetch_add(tl.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
    WRITEBACK_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// One write-back's bytes into guest memory (pages held on the device are read back first). `same`: whole pages
/// already found unchanged by the GPU (`src` equal to `base`, ascending): nothing of them lands.
fn land(p: &mut Pool, a: u64, n: u64, src: *const u8, base: *const u8, same: &[u64]) {
    // pages held on the device (lazy write-back): its buffer has their newest bytes, this write-back's among them, so
    // they stay there; bringing them back here made the completion thread wait for all the GPU work submitted since
    // (Pokemon Violet: 48 ms of write-back a frame, 12 FPS)
    if let Some(runs) = super::lazy::outside(a, n) {
        for (o, m) in runs {
            let at = |q: *const u8| {
                if q.is_null() {
                    q
                } else {
                    unsafe { q.add(o as usize) }
                }
            };
            land(p, a + o, m, at(src), at(base), same);
        }
        return;
    }
    if !base.is_null() {
        return land_changed(p, a, n, src, base, same);
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
            s[o..o + n as usize]
                .copy_from_slice(unsafe { core::slice::from_raw_parts(src, n as usize) });
        }
        WRITEBACK_BYTES.fetch_add(n, Ordering::Relaxed);
    }
}

/// Bytes the GPU changed in a write-back's range (where `src`, the device after the submission, differs from `base`,
/// the device before its work) land in guest memory; the rest of the range keeps what the CPU has, including what it
/// wrote while the GPU worked (a storage buffer is written in part). Pages the GPU left alone are
/// not touched. Counts the guest bytes the CPU had changed meanwhile that a whole-range landing would have reverted.
pub(crate) fn writeback_identity_matches(expected: Option<u64>, actual: u64) -> bool {
    expected.is_none_or(|expected| expected == actual)
}

/// The production byte merge. Only producer-changed bytes replace CPU bytes;
/// callers must supply the correct producer baseline, not a later snapshot.
pub(crate) fn merge_changed_bytes(guest: &mut [u8], after: &[u8], before: &[u8]) -> u64 {
    assert_eq!(guest.len(), after.len());
    assert_eq!(after.len(), before.len());
    let mut kept = 0;
    for ((guest, &new), &old) in guest.iter_mut().zip(after).zip(before) {
        if new != old { *guest = new; }
        else if *guest != old { kept += 1; }
    }
    kept
}

fn land_changed(p: &mut Pool, a: u64, n: u64, src: *const u8, base: *const u8, same: &[u64]) {
    let (s, b) = unsafe {
        (
            core::slice::from_raw_parts(src, n as usize),
            core::slice::from_raw_parts(base, n as usize),
        )
    };
    let mut at = 0u64;
    let mut kept = 0u64;
    while at < n {
        let pg_end = (((a + at) / PAGE + 1) * PAGE - a).min(n);
        let (lo, hi) = (at as usize, pg_end as usize);
        // a whole page the upload skip's re-arm already compared and found unchanged (`upskip::landed`)
        let known_same = pg_end - at == PAGE && same.binary_search(&(a + at)).is_ok();
        if !known_same && s[lo..hi] != b[lo..hi] {
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
            kept += merge_changed_bytes(g, &s[lo..hi], &b[lo..hi]);
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
    let (sn, sb) = m
        .by_base
        .values()
        .filter(|p| p.snapshot.is_some())
        .fold((0usize, 0u64), |(n, b), p| (n + 1, b + p.size));
    let f = frames.max(1) as f64;
    let up = UPLOAD_BYTES.swap(0, Ordering::Relaxed) as f64 / f / 1048576.0;
    let wb = WRITEBACK_BYTES.swap(0, Ordering::Relaxed) as f64 / f / 1048576.0;
    let wbms = WRITEBACK_NS.swap(0, Ordering::Relaxed) as f64 / f / 1e6;
    let kept = WRITEBACK_KEPT.load(Ordering::Relaxed);
    let t = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / f / 1e6;
    let (lk, sc, lp) = (t(&SYNC_LOCK_NS), t(&SYNC_SCAN_NS), t(&SYNC_LOOP_NS));
    let by = |i: usize| SYNC_BY[i].swap(0, Ordering::Relaxed) as f64 / f;
    let (bs, bp, bo) = (by(0), by(1), by(2));
    let skipped = SCANS_SKIPPED.swap(0, Ordering::Relaxed) as f64 / f;
    let resumes = super::queue::RESUMES.swap(0, Ordering::Relaxed) as f64 / f;
    let at_once = super::queue::PARKED_AT_ONCE.swap(0, Ordering::Relaxed) as f64 / f;
    let held = WB_HELD_B.swap(0, Ordering::Relaxed) as f64 / f / 1048576.0;
    let held_n = WB_DEFERRED.swap(0, Ordering::Relaxed) as f64 / f;
    let deferred = DEFERRED_BYTES.swap(0, Ordering::Relaxed) as f64 / f / 1048576.0;
    format!(
        "pools: {} vram ({} MB), {} mirror ({} MB), {} host ({} MB), {} cpu-only; per frame upload {up:.2} MB, write-back {wb:.2} MB in {wbms:.3} ms ({kept} CPU-written bytes inside write-backs kept so far); {sn} mirrors without write watch ({} MB); pool sync a frame: lock wait {lk:.2} ms, write-watch scan {sc:.2} ms, per-pool pass {lp:.2} ms; syncs a frame {bs:.1} replays, {bp:.1} presents, {bo:.1} one-offs, {skipped:.1} of them covered by an earlier scan, {resumes:.1} of them parked work run on; {at_once:.1} submits a frame parked at their first wait (no sync); CPU writes held back a frame: {held:.2} MB under in-flight write-backs ({held_n:.1} syncs), {deferred:.2} MB past the upload budget",
        cnt[0],
        bytes[0] >> 20,
        cnt[1],
        bytes[1] >> 20,
        cnt[2],
        bytes[2] >> 20,
        cnt[3],
        sb >> 20
    ) + &super::lazy::summary(f)
        + &(if super::uptrace::on() { super::uptrace::report(f) } else { String::new() })
        + &super::upskip::report(f)
}

#[cfg(test)]
mod room_cache_tests {
    use super::*;

    /// `room`'s remembered window answers only for the same pools (generation), inside the window and below the next
    /// pool's base (a pool laid over it owns the rest); never for an empty slot.
    #[test]
    fn a_remembered_window_answers_only_inside_it_under_the_same_pools() {
        let e = (7, 0x1000, 0x5000, 0x3000);
        assert!(room_hit(e, 7, 0x1000));
        assert!(room_hit(e, 7, 0x2fff));
        assert!(!room_hit(e, 7, 0x3000), "past the next pool's base");
        assert!(!room_hit(e, 7, 0xfff));
        assert!(!room_hit(e, 8, 0x2000), "a pool came, went or got its buffer");
        assert!(!room_hit((0, 0, 0, 0), 0, 0));
        let e = (7, 0x1000, 0x5000, u64::MAX);
        assert!(room_hit(e, 7, 0x4fff));
        assert!(!room_hit(e, 7, 0x5000), "the window ends there");
    }
}

#[cfg(test)]
mod scan_cover_tests {
    use super::*;

    /// A submission needs no write-watch scan only when a scan began after the game queued its op (one of every
    /// pool when it would ask every pool); work from outside the queue always scans.
    #[test]
    fn a_scan_after_the_op_was_queued_covers_it() {
        let s = cpu_signals();
        assert!(!scan_covers(Some(1), false), "no scan yet");
        note_scan(5, s, false);
        assert!(scan_covers(Some(5), false), "a scan began once op 5 was queued");
        assert!(scan_covers(Some(3), false), "and covers the ops before it");
        assert!(!scan_covers(Some(6), false), "op 6 was queued after the scan began");
        assert!(!scan_covers(Some(5), true), "that scan left cold pools out: a frame's first submission scans");
        assert!(!scan_covers(None, false), "not from the queue (parked rest, one-off): scans");
        assert!(!scan_covers(Some(0), false));
        note_scan(9, s, true);
        assert!(scan_covers(Some(9), true), "a scan of every pool covers a frame's first submission");
        assert!(scan_covers(Some(9), false));
        note_scan(12, s, false);
        assert!(!scan_covers(Some(10), true), "the last full scan began before op 10");
        assert!(scan_covers(Some(10), false));
        // The audit's case: op 10 waits on an event the CPU signals after this scan, having written what the op's
        // draws read; the wait passes at replay, so nothing parks. The scan no longer covers it.
        note_cpu_signal();
        assert!(!scan_covers(Some(10), false), "a CPU event signal since the scan: scan again");
        assert!(!scan_covers(Some(9), true));
        note_scan(13, cpu_signals(), false);
        assert!(scan_covers(Some(10), false), "a scan after the signal covers it again");
        // A parked queue's rest (op 14 parked at a wait) runs on as the newest op taken (`queue::resume_due`): op 15
        // of another queue signals its event on the GPU, and op 15's own sync scanned once it was queued.
        note_scan(15, cpu_signals(), false);
        assert!(scan_covers(Some(15), false), "released by op 15's GPU signal: op 15's scan covers the rest");
        assert!(
            !scan_covers(Some(16), false),
            "released by op 16, queued after that scan began (the CPU may have written before queuing it)"
        );
        note_cpu_signal();
        assert!(
            !scan_covers(Some(15), false),
            "released by a CPU event signal after the scan began: what the CPU wrote before it is not taken yet"
        );
        // A GPU signal landing at its submission's completion (the event word written by the completion thread) is not
        // a CPU signal: it leaves the coverage alone; nvnEventSignal's does not.
        note_scan(20, cpu_signals(), false);
        let mut word = 0u32;
        crate::nvn::sync::event_publish(&mut word, 0, 3, false);
        assert_eq!(word, 3);
        assert!(scan_covers(Some(20), false), "a GPU signal landed: op 20's scan still covers");
        crate::nvn::sync::event_publish(&mut word, 0, 4, true);
        assert!(!scan_covers(Some(20), false), "the game's nvnEventSignal: scan again");
        // a walk for syncs (cold pools left out) stands for a mid-frame scan only, never a frame's first
        note_walk(30, cpu_signals(), false);
        assert!(scan_covers(Some(30), false), "a walk without the cold pools covers a mid-frame sync");
        assert!(!scan_covers(Some(30), true), "but not a frame's first sync, which asks the cold pools too");
        note_walk(31, cpu_signals(), true);
        assert!(scan_covers(Some(31), true), "a full walk covers both");
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    /// Audit M2: the upload budget is charged with the bytes uploaded, not with the bytes the CPU wrote (the upload
    /// skip leaves most of them out); a range still goes only when the bytes written would fit.
    #[test]
    fn the_budget_counts_what_is_uploaded() {
        BUDGET_OVERRIDE.store(100, Ordering::Relaxed);
        *BUDGET_USED.lock().unwrap() = (super::super::present::frames(), 0);
        assert!(budget_fits(1000), "a frame's first range always goes");
        budget_charge(10); // 10 of its 1000 bytes uploaded
        assert!(budget_fits(80), "90 bytes left");
        budget_charge(80);
        assert!(!budget_fits(20), "past the budget: deferred");
        BUDGET_OVERRIDE.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod inflight_tests {
    /// A copy from a cold pool takes only the pending CPU writes in its own range (audit M1): the rest of the pool's
    /// pending ranges, and the parts of one that straddles the range, stay for the submission's sync.
    #[test]
    fn copy_sync_takes_only_its_range_of_pending() {
        let (inside, outside) = super::split_pending(
            vec![(0x0, 0x1000), (0x1800, 0x1000), (0x4000, 0x1000), (0x2000, 0x400)],
            0x2000,
            0x3000,
        );
        assert_eq!(inside, vec![(0x2000, 0x800), (0x2000, 0x400)]);
        assert_eq!(outside, vec![(0x0, 0x1000), (0x1800, 0x800), (0x4000, 0x1000)]);
        let (inside, outside) = super::split_pending(vec![(0x1000, 0x4000)], 0x2000, 0x3000);
        assert_eq!(inside, vec![(0x2000, 0x1000)]);
        assert_eq!(outside, vec![(0x1000, 0x1000), (0x3000, 0x2000)]);
    }

    /// A CPU-dirty range over in-flight write-backs: the parts outside upload now, the parts over them later.
    #[test]
    fn dirty_ranges_split_around_inflight_writebacks() {
        let (now, later) =
            super::split_inflight(0x1000, 0x3000, &[(0x1800, 0x800), (0x3800, 0x1000)]);
        assert_eq!(now, vec![(0x1000, 0x800), (0x2000, 0x1800)]);
        assert_eq!(later, vec![(0x1800, 0x800), (0x3800, 0x800)]);
        let (now, later) = super::split_inflight(0x1000, 0x1000, &[]);
        assert_eq!((now, later), (vec![(0x1000, 0x1000)], vec![]));
    }
}

/// For a capture (`capture.rs`): every pool's pages held on the device come back to guest memory first, then what the
/// GPU holds of the device-local pools (their guest memory is not their content): (base, bytes) of each.
pub fn gpu_contents(g: &Gpu) -> Vec<(u64, Vec<u8>)> {
    let pools: Vec<(
        u64,
        u64,
        Placement,
        Option<Vec<(vk::Buffer, u64, u64, u64)>>,
    )> = {
        let m = g.mem.lock().unwrap();
        m.by_base
            .values()
            .map(|p| {
                (
                    p.base,
                    p.size,
                    p.placement,
                    p.buf.as_ref().map(|_| p.pieces(p.base, p.size)),
                )
            })
            .collect()
    };
    for &(base, size, _, _) in &pools {
        super::lazy::touch(base, size);
    }
    let mut out = Vec::new();
    for (base, size, placement, buf) in pools {
        let (Placement::Vram, Some(pieces)) = (placement, buf) else {
            continue;
        };
        let Ok(rb) = g.buffer(
            size,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
            "capture readback",
        ) else {
            continue;
        };
        let f = super::queue::oneoff(g, |g, sub, _| unsafe {
            for &(buf, off, a, n) in &pieces {
                g.dev().cmd_copy_buffer(
                    sub.main,
                    buf,
                    rb.buf,
                    &[vk::BufferCopy {
                        src_offset: off,
                        dst_offset: a - base,
                        size: n,
                    }],
                );
            }
        });
        super::queue::wait_fence(f, u64::MAX);
        let bytes =
            unsafe { core::slice::from_raw_parts(rb.ptr as *const u8, size as usize) }.to_vec();
        g.free_buffer(rb);
        out.push((base, bytes));
    }
    out
}

#[cfg(test)]
mod window_tests {
    #[test]
    fn ranges_split_at_the_windows() {
        assert_eq!(
            super::window_size(100 << 20, 1 << 30),
            0,
            "fits: one buffer"
        );
        assert_eq!(
            super::window_size(3 << 30, (2 << 30) + 123),
            2 << 30,
            "AMD's 2 GiB, in whole pages"
        );
        assert_eq!(super::split_windows(0, 5, 10), vec![(0, 5, 10)]);
        assert_eq!(
            super::split_windows(100, 90, 30),
            vec![(0, 90, 10), (1, 0, 20)]
        );
        assert_eq!(super::split_windows(100, 250, 10), vec![(2, 50, 10)]);
    }
}

/// The device memory bound to pool `base` in granules (None: not a pool backed in granules). Tests and diagnostics.
pub fn pool_bound(base: u64) -> Option<u64> {
    let g = Gpu::try_get()?;
    let m = g.mem.lock().unwrap();
    m.by_base
        .get(&base)
        .and_then(|p| p.gran.as_ref().map(|gr| gr.bound))
}

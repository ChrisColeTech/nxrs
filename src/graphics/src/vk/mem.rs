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
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub const PAGE: u64 = 4096;

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
}

#[derive(Default)]
pub struct Pools {
    pub by_base: BTreeMap<u64, Pool>,
    pub gen: u32,
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
    /// The pages of [lo, hi) (page-aligned) written since the last reset; None when the range has no write watch.
    pub fn take(lo: u64, hi: u64, reset: bool) -> Option<Vec<u64>> {
        let n = ((hi - lo) / super::PAGE) as usize;
        let mut addrs: Vec<*mut core::ffi::c_void> = vec![core::ptr::null_mut(); n.max(1)];
        let mut count = addrs.len();
        let mut gran = 0u32;
        let r = unsafe {
            GetWriteWatch(if reset { WRITE_WATCH_FLAG_RESET } else { 0 }, lo as *const _, (hi - lo) as usize, addrs.as_mut_ptr(), &mut count, &mut gran)
        };
        if r != 0 {
            return None;
        }
        Some(addrs[..count].iter().map(|p| *p as u64).collect())
    }
    pub fn reset(lo: u64, hi: u64) {
        unsafe { ResetWriteWatch(lo as *const _, (hi - lo) as usize) };
    }
}
#[cfg(not(windows))]
mod watch {
    pub fn take(_lo: u64, _hi: u64, _reset: bool) -> Option<Vec<u64>> {
        None
    }
    pub fn reset(_lo: u64, _hi: u64) {}
}

/// [a, a + n) of guest memory as it reads, the parts not committed as zeros (reading them would fault).
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
#[cfg(not(windows))]
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
    };
    if std::env::var("NXGFX_POOL_LOG").is_ok() {
        crate::vklog!("pool {base:#x} size {:#x} flags {:#x}: {placement:?}{}", p.size, p.flags, if watched { " (write watch)" } else { "" });
    }
    let old = g.mem.lock().unwrap().by_base.insert(base, pool);
    if let Some(o) = old {
        release(g, o);
    }
}

fn release(g: &Gpu, p: Pool) {
    if let Some(b) = p.buf {
        g.q.defer_after_use(Box::new(move |g: &Gpu| g.free_buffer(b)));
    }
}

/// nvnMemoryPoolFinalize.
pub fn pool_del(base: u64) {
    let Some(g) = Gpu::try_get() else { return };
    let p = g.mem.lock().unwrap().by_base.remove(&base);
    if let Some(p) = p {
        g.images.lock().unwrap().drop_pool(base, p.size);
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
        let b = g.buffer(self.size, BUF_USAGE, MemoryLocation::GpuOnly, "pool")?;
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
        Ok(())
    }

    /// CPU-written ranges since the last call (absolute addresses), page generations bumped.
    fn take_cpu_writes(&mut self, gen: u32) -> Vec<(u64, u64)> {
        let mut out: Vec<(u64, u64)> = std::mem::take(&mut self.pending);
        if !matches!(self.placement, Placement::Mirror | Placement::Host) {
            return out;
        }
        let (lo, hi) = page_range(self.base, self.size);
        if self.watched {
            if let Some(pages) = watch::take(lo, hi, true) {
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
    let ci = vk::BufferCreateInfo::default().size(hi - lo).usage(BUF_USAGE).push_next(&mut ext);
    let buf = unsafe { g.dev().create_buffer(&ci, None) }.map_err(|e| format!("{e}"))?;
    let req = unsafe { g.dev().get_buffer_memory_requirements(buf) };
    let ty = g.d.mem_type(req.memory_type_bits & hp.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE).ok_or("no memory type")?;
    let mut imp = vk::ImportMemoryHostPointerInfoEXT::default().handle_type(ht).host_pointer(lo as *mut core::ffi::c_void);
    let ai = vk::MemoryAllocateInfo::default().allocation_size(hi - lo).memory_type_index(ty).push_next(&mut imp);
    let mem = unsafe { g.dev().allocate_memory(&ai, None) }.map_err(|e| {
        unsafe { g.dev().destroy_buffer(buf, None) };
        format!("import: {e}")
    })?;
    unsafe { g.dev().bind_buffer_memory(buf, mem, 0) }.map_err(|e| format!("{e}"))?;
    Ok((Buf { buf, alloc: None, mem, size: hi - lo, ptr: lo as *mut u8 }, base - lo))
}

/// A GPU address range as (buffer, offset, pool base); creates the pool's buffer at its first use.
pub fn resolve(g: &Gpu, sub: &mut super::queue::Sub, a: u64, n: u64) -> Option<(vk::Buffer, u64, u64)> {
    super::perf::timed(super::perf::C::ResolveNs, || resolve_in(g, sub, a, n))
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
    let mut m = g.mem.lock().unwrap();
    let Some(b) = find(&m, a, n.max(1)) else {
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
    Some((p.buf.as_ref().unwrap().buf, p.buf_off + (a - p.base), b))
}

/// Start of a submission: every CPU write since the last one goes up to the mirrors (recorded into the
/// submission's upload command buffer). Returns the written ranges (textures over them may need a new upload).
pub fn begin_submit(g: &Gpu, sub: &mut super::queue::Sub) -> Vec<(u64, u64)> {
    let mut m = g.mem.lock().unwrap();
    m.gen = m.gen.wrapping_add(1);
    let gen = m.gen;
    let mut all = Vec::new();
    for p in m.by_base.values_mut() {
        if !matches!(p.placement, Placement::Mirror | Placement::Host) && p.pending.is_empty() {
            continue;
        }
        let ranges = p.take_cpu_writes(gen);
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
pub fn apply_writebacks(g: &Gpu, wbs: &[(u64, u64, u64, *const u8)]) {
    if wbs.is_empty() {
        return;
    }
    let t0 = std::time::Instant::now();
    let mut m = g.mem.lock().unwrap();
    for &(pool, a, n, src) in wbs {
        let Some(p) = m.by_base.get_mut(&pool) else { continue };
        // pages GPU writes keep coming to stay on the device (write-back on demand, `lazy`); the rest lands here
        let lazy = if p.watched { super::lazy::take_lazy(a, n) } else { vec![] };
        if !lazy.is_empty() {
            let (buf, off0) = (p.buf.as_ref().map(|b| b.buf).unwrap_or_default(), p.buf_off);
            let base = p.base;
            let mut at = a;
            for &pg in lazy.iter().chain(std::iter::once(&(a + n))) {
                let end = pg.min(a + n);
                if end > at {
                    land(p, at, end - at, unsafe { src.add((at - a) as usize) });
                }
                at = (pg + PAGE).min(a + n).max(at);
            }
            super::lazy::make_lazy(&lazy, buf, base, off0);
            continue;
        }
        land(p, a, n, src);
    }
    WRITEBACK_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// One write-back's bytes into guest memory (pages held on the device are read back first).
fn land(p: &mut Pool, a: u64, n: u64, src: *const u8) {
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
    let f = frames.max(1) as f64;
    let up = UPLOAD_BYTES.swap(0, Ordering::Relaxed) as f64 / f / 1048576.0;
    let wb = WRITEBACK_BYTES.swap(0, Ordering::Relaxed) as f64 / f / 1048576.0;
    let wbms = WRITEBACK_NS.swap(0, Ordering::Relaxed) as f64 / f / 1e6;
    format!(
        "pools: {} vram ({} MB), {} mirror ({} MB), {} host ({} MB), {} cpu-only; per frame upload {up:.2} MB, write-back {wb:.2} MB in {wbms:.3} ms",
        cnt[0],
        bytes[0] >> 20,
        cnt[1],
        bytes[1] >> 20,
        cnt[2],
        bytes[2] >> 20,
        cnt[3]
    )
}

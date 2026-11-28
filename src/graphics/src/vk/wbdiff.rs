//! Write-backs compared on the GPU: only the pages a submission changed come back to host memory.
//!
//! A storage buffer bound writable may be written anywhere in its range, so every such range is written back at the
//! submission's completion: its bytes as the GPU left them (the result) and as the GPU had them before the
//! submission's work (the baseline), compared page by page on the completion thread, and only the words the GPU
//! changed land in guest memory (`mem::land_changed`). Ocarina of Time binds ~59 MB a frame this way and its shaders
//! change ~1.3 MB of it (OOT-FPS: `re-armed at landings` 58.8 MB, `write-back` 1.31 MB a frame, run `of-final2`):
//! 118 MB a frame went over the bus into host memory and ~5.7 ms a frame of the completion thread compared it, before
//! the submission's events reach the game.
//!
//! Here the baseline is copied into device memory instead (in the submission's `pre`, where the host copy was made),
//! and at the end of its `main` a kernel (`shaders/wb_diff.comp`) compares the range with it, one workgroup per guest
//! page, and copies to the host buffers only the pages the GPU changed and the range's partial first and last pages;
//! a flag a page says which (`landing flags`: 1 left as it was, 2 copied). The landing reads only copied pages: an
//! unchanged page's bytes in the host buffers are whatever an earlier submission left there and are never read
//! (`upskip::landing_pre_flags`, the same pages handed to `mem::land_changed`).
//!
//! `NXGFX_WB_DIFF=0`: every range copied back whole and compared on the host, as before.

use super::{Buf, Gpu};
use ash::vk;
use gpu_allocator::MemoryLocation;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const PAGE: u64 = 4096;

pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_WB_DIFF").as_deref() != Ok("0"))
}

/// The chunks (guest pages touched) of the range [a, a + n): one workgroup and one flag each.
pub fn chunks(a: u64, n: u64) -> u64 {
    let lo = a & !(PAGE - 1);
    (a + n - lo).div_ceil(PAGE)
}

/// Whether the range [a, a + n) at byte `off` of its buffer can be compared by the kernel: word-aligned (it reads and
/// writes 32-bit words) and at most 65,535 pages (one dispatch's workgroups).
pub fn fits(a: u64, n: u64, off: u64) -> bool {
    n > 0 && (a | n | off) & 3 == 0 && chunks(a, n) <= 65_535
}

static SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/wb_diff_comp.spv"));

/// The kernel: (layout, pipeline), made once; None when it cannot be (no buffer device addresses).
pub fn pipe(g: &Gpu) -> Option<(vk::PipelineLayout, vk::Pipeline)> {
    static P: std::sync::OnceLock<Option<(vk::PipelineLayout, vk::Pipeline)>> = std::sync::OnceLock::new();
    *P.get_or_init(|| unsafe {
        if !g.d.caps.bda {
            return None;
        }
        let pcr = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::COMPUTE).size(48)];
        let layout = g
            .dev()
            .create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&pcr), None)
            .ok()?;
        let w: Vec<u32> = SPV.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let m = g.dev().create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&w), None).ok()?;
        let st = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(m)
            .name(c"main");
        let flags = if g.d.caps.descbuf {
            vk::PipelineCreateFlags::DESCRIPTOR_BUFFER_EXT
        } else {
            vk::PipelineCreateFlags::empty()
        };
        let p = g
            .dev()
            .create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default().flags(flags).stage(st).layout(layout)],
                None,
            )
            .ok()?[0];
        g.dev().destroy_shader_module(m, None);
        Some((layout, p))
    })
}

/// One range's compare: device addresses of the range in the pool's buffer, of its baseline copy, of its place in the
/// host result and baseline buffers and of its first flag; its size and its first byte's offset in its page.
pub struct Job {
    pub src: u64,
    pub base: u64,
    pub rb: u64,
    pub bf: u64,
    pub flags: u64,
    pub n: u64,
    pub a: u64,
}

/// Records the compares of `jobs` into `cb` (after a full barrier that made the submission's work and the baselines
/// visible; the caller's host barrier follows).
pub fn record(g: &Gpu, cb: vk::CommandBuffer, layout: vk::PipelineLayout, pipe: vk::Pipeline, jobs: &[Job]) {
    if jobs.is_empty() {
        return;
    }
    let dev = g.dev();
    unsafe { dev.cmd_bind_pipeline(cb, vk::PipelineBindPoint::COMPUTE, pipe) };
    let lo = |v: u64| v as u32;
    let hi = |v: u64| (v >> 32) as u32;
    for j in jobs {
        let pc: [u32; 12] = [
            lo(j.src),
            hi(j.src),
            lo(j.base),
            hi(j.base),
            lo(j.rb),
            hi(j.rb),
            lo(j.bf),
            hi(j.bf),
            lo(j.flags),
            hi(j.flags),
            j.n as u32,
            (j.a & (PAGE - 1)) as u32,
        ];
        let bytes: Vec<u8> = pc.iter().flat_map(|w| w.to_le_bytes()).collect();
        unsafe {
            dev.cmd_push_constants(cb, layout, vk::ShaderStageFlags::COMPUTE, 0, &bytes);
            dev.cmd_dispatch(cb, chunks(j.a, j.n) as u32, 1, 1);
        }
        RANGES.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(j.n, Ordering::Relaxed);
    }
}

/// (GPU tests) The kernel run once on `after` against `before` as a range at address `a`: its flags and the host
/// buffers' bytes (`rb`, `bf`, filled with 0xee / 0x11 first: what the kernel did not copy keeps them). None when the
/// kernel or a buffer cannot be made.
pub fn compare_once(g: &'static Gpu, a: u64, after: &[u8], before: &[u8]) -> Option<(Vec<u32>, Vec<u8>, Vec<u8>)> {
    let n = after.len() as u64;
    if !fits(a, n, 0) || before.len() != after.len() {
        return None;
    }
    let (layout, pipe) = pipe(g)?;
    let usage = vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST;
    let up = |b: &[u8], name: &str| -> Option<Buf> {
        let x = g.buffer(n, usage, MemoryLocation::CpuToGpu, name).ok()?;
        unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), x.ptr, b.len()) };
        Some(x)
    };
    let src = up(after, "wbdiff test src")?;
    let base = up(before, "wbdiff test base")?;
    let rb = up(&vec![0xee; n as usize], "wbdiff test rb")?;
    let bf = up(&vec![0x11; n as usize], "wbdiff test bf")?;
    let k = chunks(a, n);
    let fl = up(&vec![0u8; (k * 4) as usize], "wbdiff test flags")?;
    let job = Job { src: src.addr, base: base.addr, rb: rb.addr, bf: bf.addr, flags: fl.addr, n, a };
    let f = super::queue::oneoff(g, |g, sub, _imgs| {
        super::queue::full_barrier(g, sub.pre);
        record(g, sub.pre, layout, pipe, std::slice::from_ref(&job));
        super::queue::full_barrier(g, sub.pre);
    });
    super::queue::wait_fence(f, u64::MAX);
    let read = |b: &Buf, len: usize| unsafe { core::slice::from_raw_parts(b.ptr, len) }.to_vec();
    let flags: Vec<u32> =
        read(&fl, (k * 4) as usize).chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let out = (flags, read(&rb, n as usize), read(&bf, n as usize));
    for b in [src, base, rb, bf, fl] {
        g.free_buffer(b);
    }
    Some(out)
}

/// Device buffers for baselines, kept for the next submissions (as `queue::wb_bufs` keeps the host ones): handed out
/// again only after the submission that used one has completed (`give` runs after its landing).
static SHELF: Mutex<super::queue::wb_bufs::Shelf<Buf>> = Mutex::new(super::queue::wb_bufs::Shelf::new());
const KEEP: u64 = 512 << 20;
const PER_SIZE: usize = 16;

/// A device buffer of at least `n` bytes for a submission's baselines.
pub fn take(g: &Gpu, n: u64) -> Option<Buf> {
    if let Some(b) = SHELF.lock().unwrap().take(n) {
        return Some(b);
    }
    let usage = vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::STORAGE_BUFFER;
    let b = g.buffer(super::queue::wb_bufs::class(n), usage, MemoryLocation::GpuOnly, "write-back base (device)").ok()?;
    if b.addr == 0 {
        g.free_buffer(b);
        return None;
    }
    Some(b)
}

/// A completed submission's baseline buffer.
pub fn give(g: &Gpu, b: Buf) {
    let size = b.size;
    if let Some(b) = SHELF.lock().unwrap().put(size, b, KEEP, PER_SIZE) {
        g.free_buffer(b);
    }
}

/// Ranges and bytes compared on the GPU, and of them the bytes the landings found copied back (changed or partial
/// pages), since the last report.
static RANGES: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static COPIED: AtomicU64 = AtomicU64::new(0);

/// Landed ranges since the last report: (address, size) -> (landings, bytes copied back).
static SEEN: Mutex<Option<std::collections::HashMap<(u64, u64), (u64, u64)>>> = Mutex::new(None);

/// A landing of the range [a, a + n) the GPU compared, `copied` bytes of it copied back.
pub fn landed(a: u64, n: u64, copied: u64) {
    COPIED.fetch_add(copied, Ordering::Relaxed);
    let mut s = SEEN.lock().unwrap();
    let e = s.get_or_insert_with(Default::default).entry((a, n)).or_default();
    e.0 += 1;
    e.1 += copied;
}

/// The pools line's part: what the GPU compare did a frame, and the ranges landing the most bytes.
pub fn report(frames: f64) -> String {
    if !on() {
        return String::new();
    }
    let r = RANGES.swap(0, Ordering::Relaxed) as f64 / frames;
    let b = BYTES.swap(0, Ordering::Relaxed) as f64 / frames / 1048576.0;
    let c = COPIED.swap(0, Ordering::Relaxed) as f64 / frames / 1048576.0;
    let mut top: Vec<((u64, u64), (u64, u64))> = SEEN.lock().unwrap().take().unwrap_or_default().into_iter().collect();
    top.sort_by_key(|&((_, n), (k, _))| std::cmp::Reverse(n * k));
    let top: Vec<String> = top
        .iter()
        .take(6)
        .map(|&((a, n), (k, cp))| {
            format!("{a:#x}+{:.2} MB x{:.2} ({:.1} KB copied)", n as f64 / 1048576.0, k as f64 / frames, cp as f64 / frames / 1024.0)
        })
        .collect();
    format!(
        "; write-backs compared on the GPU a frame: {r:.1} ranges, {b:.2} MB, {c:.2} MB of changed or partial pages copied back (largest a frame: {})",
        top.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A range's chunks are the guest pages it touches; ranges the kernel cannot read word by word are left to the
    /// host compare.
    #[test]
    fn chunks_follow_guest_pages_and_unaligned_ranges_do_not_fit() {
        assert_eq!(chunks(0x1000, 0x1000), 1);
        assert_eq!(chunks(0x1800, 0x1000), 2, "half of two pages");
        assert_eq!(chunks(0x1ffc, 8), 2);
        assert_eq!(chunks(0x1000, 1), 1);
        assert!(fits(0x1000, 0x40, 0x10));
        assert!(!fits(0x1002, 0x40, 0), "address");
        assert!(!fits(0x1000, 0x42, 0), "size");
        assert!(!fits(0x1000, 0x40, 2), "offset in its buffer");
        assert!(!fits(0x1000, 0, 0), "empty");
        assert!(fits(0, 65_535 * PAGE, 0));
        assert!(!fits(0, 65_535 * PAGE + 4, 0), "one dispatch at most");
    }
}

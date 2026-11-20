//! Submissions on one Vulkan queue with a timeline semaphore.
//!
//! - Every NVN submission becomes one Vulkan submission of two command buffers: the uploads it needs (CPU writes to
//!   mirrored pools, texture contents) and its replay. Its fence is its timeline value.
//! - A completion thread waits for each submission in order and then makes its CPU-visible effects: write-backs of
//!   GPU-written pool ranges, event values, counter reports. Only then does the fence count as reached: a SyncWait
//!   never returns before the bytes the game will read are in its memory.
//! - Command-buffer bindings belong to each NVN queue (`exec::QState`).
//! - An event wait no CPU value or in-flight GPU signal satisfies parks its NVN queue: the rest of the stream is kept
//!   and replayed when a signal arrives, while other queues go on. Fences taken on a parked queue are receipts that
//!   resolve to the timeline value of the submission that finally runs its work.

use super::exec::{self, QState};
use super::{Buf, Gpu};
use ash::vk;
use gpu_allocator::MemoryLocation;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

const STAGE_CHUNK: u64 = 8 << 20;
const RECEIPT: u64 = 1 << 63;

/// One submission being recorded: its two command buffers, staging memory, descriptor pools and what its completion
/// must do.
pub struct Sub {
    pub pre: vk::CommandBuffer,
    pub main: vk::CommandBuffer,
    pub stage: Vec<Buf>,
    stage_used: u64,
    pub dpools: Vec<vk::DescriptorPool>,
    pub dpool_i: usize,
    /// (pool base, guest address, bytes, pool buffer, buffer offset)
    pub writebacks: Vec<(u64, u64, u64, vk::Buffer, u64)>,
    /// textures this submission wrote that go back to their storage at completion (`image.rs` `tex_writeback`)
    pub tex_wb: Vec<u64>,
    pub post: Vec<Post>,
    pub query: vk::QueryPool,
    /// timestamp queries used (0 and 1: the submission's start and end)
    pub queries: u32,
    /// occlusion queries (SAMPLES_PASSED counters)
    pub occl: vk::QueryPool,
    pub occl_n: u32,
    pub images_new: Vec<vk::Image>,
    pub transient: Vec<Buf>,
    /// device-local scratch chunks handed out by `gpu_scratch` (kept with the submission, reused)
    pub arena: Vec<Buf>,
    pub arena_used: u64,
    pub transient_views: Vec<vk::ImageView>,
    /// texel buffer views of buffer textures, destroyed at completion
    pub buffer_views: Vec<vk::BufferView>,
    /// the presents this submission renders, shown in this order (generated frames first, then the game's)
    pub present: Vec<super::present::PendingPresent>,
    /// staged bytes by content, and descriptor sets by their writes (reused within the submission)
    pub memo_bytes: std::collections::HashMap<u128, (vk::Buffer, u64)>,
    pub memo_sets: std::collections::HashMap<u128, vk::DescriptorSet>,
    /// texture views by (texture ID, sampler type, storage) within the submission
    pub memo_views: std::collections::HashMap<(i32, u32, bool), vk::ImageView>,
    /// which recording this is (a stage's staged driver constants are reused by it while they stand)
    pub serial: u64,
    /// raw global memory (`gmem`): the staged memory pool table, the write log and the table's ranges
    pub gmem: Option<((vk::Buffer, u64), (vk::Buffer, u64), std::sync::Arc<Vec<(u64, u64)>>)>,
    /// descriptor buffers: the stage sets written for this submission (chunks), the bytes used of the current one,
    /// and which chunk the main command buffer has bound
    pub db: Vec<super::tables::DescBuf>,
    pub db_cur: usize,
    pub db_used: u64,
    pub db_bound: Option<usize>,
}

const DB_CHUNK: u64 = 4 << 20;
unsafe impl Send for Sub {}

/// A 128-bit content hash (two FNV-1a lanes) for the submission's memos.
const ARENA_CHUNK: u64 = 64 << 20;

pub fn memo_hash(b: &[u8]) -> u128 {
    let (mut a, mut c) = (0xcbf29ce484222325u64, 0x84222325cbf29ce4u64);
    for ch in b.chunks(8) {
        let mut w = [0u8; 8];
        w[..ch.len()].copy_from_slice(ch);
        let v = u64::from_le_bytes(w);
        a = (a ^ v).wrapping_mul(0x100000001b3);
        c = (c ^ v.rotate_left(29)).wrapping_mul(0x1000193_0000_01b3);
    }
    ((a as u128) << 64 | c as u128) ^ b.len() as u128
}

pub enum Post {
    Event { addr: u64, mode: u32, value: u32 },
    Counter { addr: u64, kind: u32, query: u32 },
    /// an occlusion query's samples join the SAMPLES_PASSED sum
    Occl { q: u32 },
    OcclReset,
    OcclReport { addr: u64 },
    Readback { dst: *mut u8, src: *const u8, n: usize },
    Callback(Box<dyn FnOnce(&Gpu) + Send>),
}
unsafe impl Send for Post {}

struct Inflight {
    value: u64,
    sub: Sub,
    wb: Vec<(u64, u64, u64, *const u8, *const u8)>,
    readback: Option<Buf>,
    before: Option<Buf>,
}
unsafe impl Send for Inflight {}

struct Parked {
    /// canonical records still to run, owned
    stream: Vec<u8>,
    receipts: Vec<u64>,
    /// a sync of another parked queue this queue waits for first (nvnQueueWaitSync)
    wait_receipt: Option<u64>,
    /// presents queued behind the parked work: (texture, crop), shown when it has run
    presents: Vec<(crate::nvn::obj::tex_t, Option<[i32; 4]>)>,
    /// waits queued while parked: (stream offset, receipt): the work from that offset on runs once the receipt resolved
    later: Vec<(usize, u64)>,
}

pub struct SubmitState {
    pub qstates: HashMap<u64, QState>,
    parked: HashMap<u64, Parked>,
    cmd_pool: vk::CommandPool,
    /// the main buffers' pool of their own: with deferred recording (vk/defer.rs) a main buffer is recorded on the
    /// recorder thread while the replay thread records the upload buffer, and a pool's buffers may not be recorded at once
    main_pool: vk::CommandPool,
    free_subs: Vec<Sub>,
}

pub struct Queue {
    pub timeline: vk::Semaphore,
    /// latest GPU WRITE signal encoded per event word and not yet landed: (mode, value, fence)
    pub pending_signals: Mutex<HashMap<u64, (u32, u32, u64)>>,
    retry: AtomicBool,
    retry_mu: Mutex<bool>,
    retry_cv: Condvar,
    pub sub: Mutex<SubmitState>,
    vkq: Mutex<()>,
    last: AtomicU64,
    done: Mutex<u64>,
    done_cv: Condvar,
    inflight: Mutex<VecDeque<Inflight>>,
    inflight_cv: Condvar,
    recycled: Mutex<Vec<Sub>>,
    deferred: Mutex<Vec<(u64, Box<dyn FnOnce(&Gpu) + Send>)>>,
    /// a submission is being recorded (under `sub`): releases wait for its fence, which it does not have yet
    recording: AtomicBool,
    after_current: Mutex<Vec<Box<dyn FnOnce(&Gpu) + Send>>>,
    receipts: Mutex<HashMap<u64, u64>>,
    next_receipt: AtomicU64,
    thread_started: AtomicBool,
}

impl Queue {
    pub fn new(d: &super::device::Device) -> Result<Queue, String> {
        let mut tci = vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE).initial_value(0);
        let sci = vk::SemaphoreCreateInfo::default().push_next(&mut tci);
        let timeline = unsafe { d.dev.create_semaphore(&sci, None) }.map_err(|e| format!("timeline: {e}"))?;
        let pci = vk::CommandPoolCreateInfo::default().queue_family_index(d.qfam).flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let cmd_pool = unsafe { d.dev.create_command_pool(&pci, None) }.map_err(|e| format!("command pool: {e}"))?;
        let main_pool = unsafe { d.dev.create_command_pool(&pci, None) }.map_err(|e| format!("command pool: {e}"))?;
        Ok(Queue {
            timeline,
            sub: Mutex::new(SubmitState { qstates: HashMap::new(), parked: HashMap::new(), cmd_pool, main_pool, free_subs: Vec::new() }),
            pending_signals: Mutex::new(HashMap::new()),
            retry: AtomicBool::new(false),
            retry_mu: Mutex::new(false),
            retry_cv: Condvar::new(),
            vkq: Mutex::new(()),
            last: AtomicU64::new(0),
            done: Mutex::new(0),
            done_cv: Condvar::new(),
            inflight: Mutex::new(VecDeque::new()),
            inflight_cv: Condvar::new(),
            recycled: Mutex::new(Vec::new()),
            deferred: Mutex::new(Vec::new()),
            recording: AtomicBool::new(false),
            after_current: Mutex::new(Vec::new()),
            receipts: Mutex::new(HashMap::new()),
            next_receipt: AtomicU64::new(1),
            thread_started: AtomicBool::new(false),
        })
    }

    /// The Vulkan queue's lock (vkQueueSubmit / vkQueuePresentKHR from several threads).
    pub fn vkq_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.vkq.lock().unwrap()
    }
    pub fn last_fence(&self) -> u64 {
        self.last.load(Ordering::Acquire)
    }
    pub fn completed(&self) -> u64 {
        *self.done.lock().unwrap()
    }
    /// Runs `f` once `fence` has completed (resources a submission may still use).
    pub fn defer(&self, fence: u64, f: Box<dyn FnOnce(&Gpu) + Send>) {
        if fence <= self.completed() {
            if let Some(g) = Gpu::try_get() {
                f(g);
                return;
            }
        }
        self.deferred.lock().unwrap().push((fence, f));
    }

    /// Runs `f` once the GPU can no longer use what it frees: after everything submitted so far, and after the
    /// submission being recorded now (which may have referenced it before its owner let it go).
    pub fn defer_after_use(&self, f: Box<dyn FnOnce(&Gpu) + Send>) {
        // ops still queued for the replay thread may use it: after them
        if aq_busy() && !on_replay_thread() {
            after_queued(Box::new(move |g: &'static Gpu| g.q.defer_after_use(f)));
            return;
        }
        if self.recording.load(Ordering::Acquire) {
            let mut l = self.after_current.lock().unwrap();
            // re-check under the list lock: submit_sub drains it under the same lock before clearing the flag
            if self.recording.load(Ordering::Acquire) {
                l.push(f);
                return;
            }
        }
        self.defer(self.last_fence(), f);
    }
}

impl Sub {
    /// Copies `src` into staging memory; returns (buffer, offset). Aligned to 256 (usable as a uniform buffer).
    pub fn stage_bytes(&mut self, g: &Gpu, src: &[u8]) -> (vk::Buffer, u64) {
        // the same bytes staged earlier in this submission are bound again (a stage's driver constants, handle table
        // and support buffer repeat from draw to draw)
        let h = memo_hash(src);
        if let Some(&r) = self.memo_bytes.get(&h) {
            return r;
        }
        let r = self.stage_bytes_new(g, src);
        self.memo_bytes.insert(h, r);
        r
    }
    /// Staged bytes of their own (not shared with equal bytes staged earlier): the GPU writes into them.
    pub fn stage_bytes_fresh(&mut self, g: &Gpu, src: &[u8]) -> (vk::Buffer, u64) {
        self.stage_bytes_new(g, src)
    }
    fn stage_bytes_new(&mut self, g: &Gpu, src: &[u8]) -> (vk::Buffer, u64) {
        let n = src.len() as u64;
        let (b, off) = self.stage_alloc(g, n);
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), self.stage[b].ptr.add(off as usize), src.len()) };
        (self.stage[b].buf, off)
    }
    /// Device-local scratch of n bytes for this submission's own GPU work (storage, transfer source and
    /// destination), 256-aligned: (buffer, offset). One allocation per chunk instead of one per use (the ASTC
    /// decoder allocated two buffers for each texture level: Mario Kart World's first submission, 29,000 textures, 10 s).
    pub fn gpu_scratch(&mut self, g: &Gpu, n: u64) -> Option<(vk::Buffer, u64)> {
        let off = (self.arena_used + 255) & !255;
        if let Some(last) = self.arena.last() {
            if off + n <= last.size {
                self.arena_used = off + n;
                return Some((last.buf, off));
            }
        }
        let size = n.max(ARENA_CHUNK);
        let b = g
            .buffer(size, vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuOnly, "scratch")
            .ok()?;
        let buf = b.buf;
        // older chunks stay allocated until the submission is recycled
        self.arena.push(b);
        self.arena_used = n;
        Some((buf, 0))
    }
    /// Staging space of n bytes: (chunk index, offset), 256-aligned.
    pub fn stage_alloc(&mut self, g: &Gpu, n: u64) -> (usize, u64) {
        let off = (self.stage_used + 255) & !255;
        if let Some(last) = self.stage.len().checked_sub(1) {
            if off + n <= self.stage[last].size {
                self.stage_used = off + n;
                return (last, off);
            }
        }
        let size = n.max(STAGE_CHUNK);
        let mk = |g: &Gpu| {
            g.buffer(
                size,
                vk::BufferUsageFlags::TRANSFER_SRC
                    | vk::BufferUsageFlags::UNIFORM_BUFFER
                    | vk::BufferUsageFlags::STORAGE_BUFFER
                    | vk::BufferUsageFlags::TRANSFER_DST
                    | vk::BufferUsageFlags::INDIRECT_BUFFER,
                MemoryLocation::CpuToGpu,
                "staging",
            )
        };
        // over the system memory cap: what completed submissions free first, then once more
        let b = mk(g).or_else(|_| {
            wait_fence(g.q.last_fence(), 5_000_000_000);
            mk(g)
        })
        .expect("staging memory");
        self.stage.push(b);
        self.stage_used = n;
        (self.stage.len() - 1, 0)
    }
    pub fn stage_ptr(&self, chunk: usize) -> (*mut u8, vk::Buffer) {
        (self.stage[chunk].ptr, self.stage[chunk].buf)
    }
    /// Uploads `src` into `dst` at `off`, in the submission's upload command buffer.
    pub fn upload(&mut self, g: &Gpu, src: &[u8], dst: vk::Buffer, off: u64) {
        if src.is_empty() {
            return;
        }
        let (sb, so) = self.stage_bytes(g, src);
        let r = [vk::BufferCopy { src_offset: so, dst_offset: off, size: src.len() as u64 }];
        super::lost::checkpoint(g, self.pre, true, || format!("an upload of {:#x} bytes into buffer {:#x} at {off:#x}", src.len(), vk::Handle::as_raw(dst)));
        unsafe { g.dev().cmd_copy_buffer(self.pre, sb, dst, &r) };
    }
    /// A descriptor set of the given stage set layout, from this submission's pools.
    pub fn alloc_set(&mut self, g: &Gpu, layout: vk::DescriptorSetLayout) -> vk::DescriptorSet {
        loop {
            if self.dpool_i == self.dpools.len() {
                let sizes = [
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::UNIFORM_BUFFER, descriptor_count: 32 * 512 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 32 * 512 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER, descriptor_count: 64 * 512 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_IMAGE, descriptor_count: 16 * 512 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::UNIFORM_TEXEL_BUFFER, descriptor_count: 16 * 512 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_TEXEL_BUFFER, descriptor_count: 16 * 512 },
                ];
                let ci = vk::DescriptorPoolCreateInfo::default().max_sets(512).pool_sizes(&sizes);
                let p = unsafe { g.dev().create_descriptor_pool(&ci, None) }.expect("descriptor pool");
                self.dpools.push(p);
            }
            let l = [layout];
            let ai = vk::DescriptorSetAllocateInfo::default().descriptor_pool(self.dpools[self.dpool_i]).set_layouts(&l);
            match unsafe { g.dev().allocate_descriptor_sets(&ai) } {
                Ok(s) => return s[0],
                Err(_) => self.dpool_i += 1,
            }
        }
    }
    /// Descriptor buffers: room for `n` bytes of stage sets in the current chunk (a new chunk when it is full; the main
    /// command buffer binds it at its next draw).
    pub fn db_reserve(&mut self, g: &Gpu, n: u64) {
        if self.db.is_empty() || self.db_used + n > self.db[self.db_cur].size {
            if !self.db.is_empty() {
                self.db_cur += 1;
            }
            if self.db_cur == self.db.len() {
                let b = super::tables::desc_buffer(&g.d, DB_CHUNK.max(n)).expect("descriptor buffer chunk");
                self.db.push(b);
            }
            self.db_used = 0;
        }
    }
    /// A stage set's place in the current chunk (after `db_reserve`): its offset.
    pub fn db_alloc(&mut self, size: u64) -> u64 {
        let off = self.db_used;
        self.db_used += size;
        off
    }
    /// A staging or scratch buffer of this submission's own.
    pub fn owns(&self, buf: vk::Buffer) -> bool {
        self.stage.iter().chain(self.arena.iter()).any(|b| b.buf == buf)
    }
    /// The device address of a buffer of this submission's own (staging, scratch).
    pub fn addr_of(&self, buf: vk::Buffer) -> Option<u64> {
        self.stage.iter().chain(self.arena.iter()).find(|b| b.buf == buf).map(|b| b.addr).filter(|&a| a != 0)
    }
    pub fn query(&mut self) -> u32 {
        let q = self.queries;
        self.queries += 1;
        q
    }
}

fn barrier_all(g: &Gpu, cb: vk::CommandBuffer) {
    let mb = [vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)];
    let di = vk::DependencyInfo::default().memory_barriers(&mb);
    unsafe { g.dev().cmd_pipeline_barrier2(cb, &di) };
}
pub fn full_barrier(g: &Gpu, cb: vk::CommandBuffer) {
    barrier_all(g, cb)
}

fn new_sub(g: &Gpu, st: &mut SubmitState) -> Sub {
    let mut s = if let Some(s) = st.free_subs.pop() {
        s
    } else {
        let ai = vk::CommandBufferAllocateInfo::default().command_pool(st.cmd_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1);
        let pre = unsafe { g.dev().allocate_command_buffers(&ai) }.expect("command buffers")[0];
        let ai = vk::CommandBufferAllocateInfo::default().command_pool(st.main_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1);
        let main = unsafe { g.dev().allocate_command_buffers(&ai) }.expect("command buffers")[0];
        let cbs = [pre, main];
        let qi = vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(256);
        let query = unsafe { g.dev().create_query_pool(&qi, None) }.expect("query pool");
        let oi = vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::OCCLUSION).query_count(256);
        let occl = unsafe { g.dev().create_query_pool(&oi, None) }.expect("occlusion query pool");
        Sub {
            pre: cbs[0],
            main: cbs[1],
            stage: Vec::new(),
            stage_used: 0,
            dpools: Vec::new(),
            dpool_i: 0,
            writebacks: Vec::new(),
            tex_wb: Vec::new(),
            post: Vec::new(),
            query,
            queries: 2,
            occl,
            occl_n: 0,
            images_new: Vec::new(),
            transient: Vec::new(),
            arena: Vec::new(),
            arena_used: 0,
            transient_views: Vec::new(),
            buffer_views: Vec::new(),
            present: Vec::new(),
            memo_bytes: Default::default(),
            memo_sets: Default::default(),
            memo_views: Default::default(),
            serial: 0,
            gmem: None,
            db: Vec::new(),
            db_cur: 0,
            db_used: 0,
            db_bound: None,
        }
    };
    static SUB_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    s.serial = SUB_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
    g.q.recording.store(true, Ordering::Release);
    let bi = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe {
        g.dev().reset_command_buffer(s.pre, vk::CommandBufferResetFlags::empty()).unwrap();
        g.dev().reset_command_buffer(s.main, vk::CommandBufferResetFlags::empty()).unwrap();
        g.dev().begin_command_buffer(s.pre, &bi).unwrap();
        g.dev().begin_command_buffer(s.main, &bi).unwrap();
        super::defer::begin(s.main);
        g.dev().cmd_reset_query_pool(s.pre, s.query, 0, 256);
        g.dev().cmd_reset_query_pool(s.pre, s.occl, 0, 256);
        g.dev().cmd_write_timestamp2(s.main, vk::PipelineStageFlags2::TOP_OF_PIPE, s.query, 0);
    }
    s.queries = 2;
    s.occl_n = 0;
    barrier_all(g, s.main);
    s.stage_used = 0;
    // keep one staging chunk of the ordinary size; the rest (and an outsized one: a big upload's) go.
    // NXGFX_STAGE_HOLD=<s> (diagnosis, lane K2): they are freed that many seconds later instead (MKW's 128x128x5 pass
    // read a staging chunk freed here 3.6 s before: something keeps a staged range past its submission)
    static HOLD: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    static HELD: Mutex<Vec<(f64, Buf)>> = Mutex::new(Vec::new());
    let hold = *HOLD.get_or_init(|| std::env::var("NXGFX_STAGE_HOLD").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0));
    let now = crate::log::elapsed();
    while s.stage.len() > 1 || s.stage.last().is_some_and(|b| b.size > STAGE_CHUNK) {
        let b = s.stage.pop().unwrap();
        if hold > 0.0 {
            HELD.lock().unwrap().push((now, b));
        } else {
            g.free_buffer(b);
        }
    }
    if hold > 0.0 {
        let mut h = HELD.lock().unwrap();
        while h.first().is_some_and(|(t, _)| now - *t > hold) {
            let (_, b) = h.remove(0);
            g.free_buffer(b);
        }
    }
    s
}

/// A submission of the host's own (the lazy page reader), in queue order with the replay's.
pub fn submit_raw(g: &Gpu, si: &[vk::SubmitInfo2], fence: vk::Fence) {
    let _vq = g.q.vkq.lock().unwrap();
    unsafe { g.dev().queue_submit2(g.d.queue, si, fence) }.unwrap_or_else(|e| {
        super::lost::check(g, e, "vkQueueSubmit2");
        panic!("vkQueueSubmit2: {e}")
    });
}

/// Ends and submits a recorded submission; returns its timeline value.
fn submit_sub(g: &'static Gpu, mut s: Sub) -> u64 {
    let q = &g.q;
    super::gmem::end(g, &mut s);
    // write-backs: copy the GPU-written ranges into one readback buffer at the end of the replay
    let total: u64 = s.writebacks.iter().map(|w| (w.2 + 15) & !15).sum();
    let mut readback = None;
    let mut before: Option<Buf> = None;
    let mut wb = Vec::new();
    if total > 0 {
        barrier_all(g, s.main);
        let rb = g.buffer(total, vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuToCpu, "write-back").expect("readback");
        // the same ranges as the device had them before this submission's work (after its uploads, in `pre`): only
        // the words the GPU changed land in guest memory, so what the CPU wrote beside them meanwhile stays (Turbo's
        // vk_mem.c wr_land). Landing the whole range put back stale bytes the CPU had changed since the upload: a
        // storage buffer bound read-write over a pool the CPU also writes (a size-0 one runs to the end of its pool)
        let bf = g.buffer(total, vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuToCpu, "write-back base").ok();
        if bf.is_some() {
            barrier_all(g, s.pre);
        }
        let mut off = 0;
        super::lost::checkpoint(g, s.main, false, || format!("the write-back copies of {} range(s)", s.writebacks.len()));
        for &(pool, a, n, buf, boff) in &s.writebacks {
            let r = [vk::BufferCopy { src_offset: boff, dst_offset: off, size: n }];
            unsafe { g.dev().cmd_copy_buffer(s.main, buf, rb.buf, &r) };
            let base = match &bf {
                Some(b) => {
                    unsafe { g.dev().cmd_copy_buffer(s.pre, buf, b.buf, &r) };
                    unsafe { b.ptr.add(off as usize) as *const u8 }
                }
                None => core::ptr::null(),
            };
            wb.push((pool, a, n, unsafe { rb.ptr.add(off as usize) } as *const u8, base));
            off += (n + 15) & !15;
        }
        readback = Some(rb);
        before = bf;
        super::mem::inflight_add(&wb.iter().map(|w| (w.1, w.2)).collect::<Vec<_>>());
    }
    // small textures this submission wrote: copied out for their storage at completion
    if !s.tex_wb.is_empty() {
        let list = std::mem::take(&mut s.tex_wb);
        g.tex_writeback(&mut s, &list);
    }
    // host reads of anything this submission wrote
    let mb = [vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::HOST)
        .dst_access_mask(vk::AccessFlags2::HOST_READ)];
    unsafe { g.dev().cmd_pipeline_barrier2(s.main, &vk::DependencyInfo::default().memory_barriers(&mb)) };
    super::sparse::fills(g, s.pre);
    barrier_all(g, s.pre);
    unsafe {
        g.dev().cmd_write_timestamp2(s.main, vk::PipelineStageFlags2::ALL_COMMANDS, s.query, 1);
        g.dev().end_command_buffer(s.pre).unwrap();
        super::defer::end(s.main);
        g.dev().end_command_buffer(s.main).unwrap();
    }
    let _x = super::pipe::driver_exclusive(g).then(|| super::pipe::EXCLUSIVE.lock().unwrap());
    let _vq = q.vkq.lock().unwrap();
    let value = q.last.load(Ordering::Acquire) + 1;
    let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(s.pre), vk::CommandBufferSubmitInfo::default().command_buffer(s.main)];
    let mut sig = vec![vk::SemaphoreSubmitInfo::default().semaphore(q.timeline).value(value).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
    let mut wait = vec![];
    // sparse binds changed since the last submission go first (after every earlier submission); this one waits for them
    if let Some((sem, v)) = super::sparse::flush(g, value - 1, q.timeline, value) {
        wait.push(vk::SemaphoreSubmitInfo::default().semaphore(sem).value(v).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS));
    }
    for p in &s.present {
        wait.push(vk::SemaphoreSubmitInfo::default().semaphore(p.acquired).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS));
        sig.push(vk::SemaphoreSubmitInfo::default().semaphore(p.rendered).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS));
    }
    let si = [vk::SubmitInfo2::default().command_buffer_infos(&cbs).signal_semaphore_infos(&sig).wait_semaphore_infos(&wait)];
    unsafe { g.dev().queue_submit2(g.d.queue, &si, vk::Fence::null()) }.unwrap_or_else(|e| {
        super::lost::check(g, e, "vkQueueSubmit2");
        panic!("vkQueueSubmit2: {e}")
    });
    q.last.store(value, Ordering::Release);
    {
        let mut l = q.after_current.lock().unwrap();
        let mut d = q.deferred.lock().unwrap();
        for f in l.drain(..) {
            d.push((value, f));
        }
        q.recording.store(false, Ordering::Release);
    }
    let presents = std::mem::take(&mut s.present);
    if presents.len() == 1 {
        super::present::queue_present(g, presents.into_iter().next().unwrap());
    } else if !presents.is_empty() {
        // generated frames: spaced over the game's frame interval by frame generation's presenting thread
        drop(_vq);
        super::framegen::schedule(g, presents);
        start_completion(g);
        q.inflight.lock().unwrap().push_back(Inflight { value, sub: s, wb, readback, before });
        q.inflight_cv.notify_one();
        return value;
    }
    drop(_vq);
    start_completion(g);
    q.inflight.lock().unwrap().push_back(Inflight { value, sub: s, wb, readback, before });
    q.inflight_cv.notify_one();
    value
}

fn start_completion(g: &'static Gpu) {
    if g.q.thread_started.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::Builder::new()
        .name("nvn-completion".into())
        .spawn(move || completion_loop(g))
        .expect("completion thread");
}

fn completion_loop(g: &'static Gpu) {
    let q = &g.q;
    loop {
        let item = {
            let mut l = q.inflight.lock().unwrap();
            while l.is_empty() {
                l = q.inflight_cv.wait(l).unwrap();
            }
            l.pop_front().unwrap()
        };
        let sems = [q.timeline];
        let vals = [item.value];
        let wi = vk::SemaphoreWaitInfo::default().semaphores(&sems).values(&vals);
        loop {
            match unsafe { g.dev().wait_semaphores(&wi, 5_000_000_000) } {
                Ok(()) => break,
                Err(vk::Result::TIMEOUT) => crate::log::red(&format!("GPU submission {} has not completed after 5 s", item.value)),
                Err(e) => {
                    super::lost::check(g, e, "the wait for a submission");
                    panic!("vkWaitSemaphores: {e}")
                }
            }
        }
        let mut signalled = false;
        // CPU-visible effects, in order: write-backs first (an event may tell the game to read them)
        super::mem::apply_writebacks(g, &item.wb);
        let mut sub = item.sub;
        let ts: Vec<u64> = {
            let mut v = vec![0u64; sub.queries.max(2) as usize];
            unsafe { g.dev().get_query_pool_results(sub.query, 0, &mut v, vk::QueryResultFlags::TYPE_64) }.ok();
            let period = g.timestamp_period() as f64;
            v.iter().map(|&t| (t as f64 * period) as u64).collect()
        };
        let (t0, t1) = (ts[0], ts[1]);
        if t1 >= t0 && t1 > 0 {
            super::counters::part(t0, t1);
            super::perf::gpu_time(t1 - t0);
            super::scale::note_gpu(t1 - t0);
        }
        let occl: Vec<u64> = if sub.occl_n > 0 {
            let mut v = vec![0u64; sub.occl_n as usize];
            unsafe { g.dev().get_query_pool_results(sub.occl, 0, &mut v, vk::QueryResultFlags::TYPE_64) }.ok();
            v
        } else {
            vec![]
        };
        for p in sub.post.drain(..) {
            match p {
                Post::Event { addr, mode, value } => {
                    crate::nvn::sync::event_publish(addr as *mut u32, mode, value);
                    signalled = true;
                }
                Post::Counter { addr, kind, query } => {
                    let stamp = if query != u32::MAX { ts.get(query as usize).copied() } else { None };
                    let t = super::counters::report_time(stamp, t0, t1);
                    // a timestamp's counter field is 0 (Turbo; agl's "already cleared" records compare it), other kinds 0 too
                    let ts = kind == crate::nvn::types::counter::TIMESTAMP || kind == crate::nvn::types::counter::TIMESTAMP_TOP;
                    super::counters::write_record(addr, 0, t);
                    if ts {
                        super::counters::rc_landed(addr);
                    }
                }
                Post::Occl { q } => super::counters::vis_add(occl.get(q as usize).copied().unwrap_or(0)),
                Post::OcclReset => super::counters::vis_reset(),
                Post::OcclReport { addr } => {
                    let t = super::counters::report_time(None, t0, t1);
                    super::counters::write_record(addr, super::counters::vis(), t);
                }
                Post::Readback { dst, src, n } => unsafe { core::ptr::copy_nonoverlapping(src, dst, n) },
                Post::Callback(f) => f(g),
            }
        }
        if let Some(rb) = item.readback {
            g.free_buffer(rb);
        }
        if let Some(b) = item.before {
            g.free_buffer(b);
        }
        for b in sub.transient.drain(..) {
            g.free_buffer(b);
        }
        // scratch: one ordinary chunk kept for the next use of this submission
        sub.arena_used = 0;
        while sub.arena.len() > 1 || sub.arena.last().is_some_and(|b| b.size > ARENA_CHUNK) {
            let b = sub.arena.pop().unwrap();
            g.free_buffer(b);
        }
        for v in sub.transient_views.drain(..) {
            unsafe { g.dev().destroy_image_view(v, None) };
        }
        for v in sub.buffer_views.drain(..) {
            unsafe { g.dev().destroy_buffer_view(v, None) };
        }
        for p in &sub.dpools {
            unsafe { g.dev().reset_descriptor_pool(*p, vk::DescriptorPoolResetFlags::empty()) }.ok();
        }
        sub.dpool_i = 0;
        sub.db_cur = 0;
        sub.db_used = 0;
        sub.db_bound = None;
        sub.memo_bytes.clear();
        sub.memo_sets.clear();
        sub.memo_views.clear();
        sub.writebacks.clear();
        sub.tex_wb.clear();
        sub.queries = 2;
        sub.occl_n = 0;
        q.recycled.lock().unwrap().push(sub);
        {
            let mut dl = q.deferred.lock().unwrap();
            let mut keep = Vec::new();
            let mut run = Vec::new();
            for (f, cb) in dl.drain(..) {
                if f <= item.value {
                    run.push(cb);
                } else {
                    keep.push((f, cb));
                }
            }
            *dl = keep;
            drop(dl);
            for cb in run {
                cb(g);
            }
        }
        q.pending_signals.lock().unwrap().retain(|_, v| v.2 > item.value);
        *q.done.lock().unwrap() = item.value;
        q.done_cv.notify_all();
        if signalled {
            // never block on the submit lock here (its holder may be waiting for this thread): the retry thread does
            request_retry(g);
        }
    }
}

/// Wakes the thread that lets parked queues run on once a GPU signal has landed.
fn request_retry(g: &'static Gpu) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    g.q.retry.store(true, Ordering::Release);
    if !STARTED.swap(true, Ordering::AcqRel) {
        std::thread::Builder::new()
            .name("nvn-parked".into())
            .spawn(move || loop {
                {
                    let mut f = g.q.retry_mu.lock().unwrap();
                    while !*f {
                        // every second: a queue parked for 2 s is logged once with what it waits for (diagnostics only)
                        let (ff, to) = g.q.retry_cv.wait_timeout(f, std::time::Duration::from_secs(1)).unwrap();
                        f = ff;
                        if to.timed_out() {
                            drop(f);
                            log_parked(g);
                            f = g.q.retry_mu.lock().unwrap();
                        }
                    }
                    *f = false;
                }
                if g.q.retry.swap(false, Ordering::AcqRel) {
                    retry_parked(g);
                }
            })
            .expect("parked queue thread");
    }
    *g.q.retry_mu.lock().unwrap() = true;
    g.q.retry_cv.notify_one();
}

fn take_recycled(g: &Gpu, st: &mut SubmitState) {
    let mut r = g.q.recycled.lock().unwrap();
    st.free_subs.append(&mut r);
}

// ---------- asynchronous submission ----------
//
// The game's thread only copies a submission's records (or notes a present) and goes on; the replay thread records
// and submits them in order (Turbo's translation thread). Fences the game takes meanwhile name the op (ASYNC | its
// sequence number) and resolve to its timeline value once it ran. `NXGFX_ASYNC=0`: replay on the game's thread.

const ASYNC: u64 = 1 << 62;

enum Op {
    Submit { q: u64, ll: bool, stream: Vec<u8> },
    Present { q: u64, ll: bool, t: crate::nvn::obj::tex_t, crop: Option<[i32; 4]> },
    Call(Box<dyn FnOnce(&'static Gpu) + Send>),
    Retry,
    WaitReceipt { q: u64, r: u64 },
}

/// Queue `q` waits for receipt `r` (a sync of another queue's parked work) before anything else it was given.
fn park_on_receipt(st: &mut SubmitState, q: u64, r: u64) {
    match st.parked.get_mut(&q) {
        Some(p) if p.wait_receipt.is_none() && p.stream.is_empty() && p.later.is_empty() => p.wait_receipt = Some(r),
        // already parked with work: the wait sits at the end of that work (the work queued after it runs once it passed)
        Some(p) => p.later.push((p.stream.len(), r)),
        None => {
            st.parked.insert(q, Parked { stream: Vec::new(), receipts: Vec::new(), wait_receipt: Some(r), presents: Vec::new(), later: Vec::new() });
        }
    }
}

struct Aq {
    ops: Mutex<VecDeque<(u64, Op)>>,
    cv: Condvar,
    tail: AtomicU64,
    done: AtomicU64,
    /// op sequence number -> the timeline value after it (recent ones)
    fences: Mutex<std::collections::BTreeMap<u64, u64>>,
    idle_mu: Mutex<()>,
    idle_cv: Condvar,
    ns: AtomicU64,
}

static AQ: std::sync::OnceLock<Aq> = std::sync::OnceLock::new();

pub fn aq_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        // on by default (Turbo's default): Pokemon Legends Z-A in Lumiose 49.5-52.4 FPS against 42.5-48 replayed on the
        // game's thread (lane G3 z6 / z5), MK8D's demo 300 s at 60 with no stall (a1). NXGFX_ASYNC=0: on the game's thread
        let on = std::env::var("NXGFX_ASYNC").as_deref() != Ok("0");
        crate::vklog!("asynchronous submission: {}", if on { "submissions and presents are replayed on a thread of their own (NXGFX_ASYNC=0: on the game's thread)" } else { "off (NXGFX_ASYNC=0)" });
        on
    })
}

fn on_replay_thread() -> bool {
    std::thread::current().name() == Some("vk replay")
}

fn aq(g: &'static Gpu) -> &'static Aq {
    let mut made = false;
    let a = AQ.get_or_init(|| {
        made = true;
        Aq {
            ops: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
            tail: AtomicU64::new(0),
            done: AtomicU64::new(0),
            fences: Mutex::new(Default::default()),
            idle_mu: Mutex::new(()),
            idle_cv: Condvar::new(),
            ns: AtomicU64::new(0),
        }
    });
    if made {
        std::thread::Builder::new().name("vk replay".into()).spawn(move || aq_main(g, a)).expect("replay thread");
    }
    a
}

/// Ops queued and not replayed at most (Turbo's AQ_N), presents among them at most (AQ_PRESENTS: frames in flight).
const AQ_MAX: u64 = 64;
/// `NXGFX_ASYNC_PRESENTS` (default 2): presents queued at most.
fn aq_max_presents() -> u64 {
    static N: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *N.get_or_init(|| std::env::var("NXGFX_ASYNC_PRESENTS").ok().and_then(|v| v.parse().ok()).unwrap_or(2u64).max(1))
}
static AQ_PRESENTS: AtomicU64 = AtomicU64::new(0);

/// Before a submit or present is queued: waits while the replay thread is that far behind (the game's thread, inside
/// the OS's blocking hook). Without it a game that does not wait for its frames ran ahead with copies of its records.
pub fn aq_wait_room(present: bool) {
    if !aq_on() {
        return;
    }
    let Some(a) = AQ.get() else { return };
    // a game whose frames read occlusion (samples-passed) results: before its presents every earlier op has been
    // replayed, so those results come back as soon as with replay on the game's thread (a game that culls on last
    // frame's results otherwise read them two presents late: Mario Kart 8 Deluxe's black stretches). NXGFX_OCCL_SYNC=0 off
    // (presents only: waiting at every submit too serialized the game with the replay: Donkey Kong Bananza 17.8 -> 13
    // FPS, and did not help Mario Kart 8 Deluxe)
    let occl = present && occl_sync();
    let full = || {
        let behind = a.tail.load(Ordering::Acquire) - a.done.load(Ordering::Acquire);
        behind >= AQ_MAX - 1 || (present && AQ_PRESENTS.load(Ordering::Acquire) >= aq_max_presents()) || (occl && behind > 0)
    };
    if !full() {
        return;
    }
    let mut l = a.idle_mu.lock().unwrap();
    while full() {
        l = a.idle_cv.wait_timeout(l, std::time::Duration::from_millis(20)).unwrap().0;
    }
}

fn aq_push(g: &'static Gpu, op: Op) -> u64 {
    aq_push_with(g, op, |_| {})
}

/// The present frame a samples-passed (occlusion) report was last replayed at (0: never).
pub static OCCL_FRAME: AtomicU64 = AtomicU64::new(0);

/// Occlusion results in use lately (a report in the last 120 presents): presents wait for the replay to catch up.
fn occl_sync() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("NXGFX_OCCL_SYNC").as_deref() != Ok("0")) {
        return false;
    }
    let f = OCCL_FRAME.load(Ordering::Relaxed);
    f != 0 && super::present::frames().saturating_sub(f) < 120
}

/// As `aq_push`; `before` runs with the op's sequence number before the replay thread can see the op.
fn aq_push_with(g: &'static Gpu, op: Op, before: impl FnOnce(u64)) -> u64 {
    let a = aq(g);
    if matches!(op, Op::Present { .. }) {
        AQ_PRESENTS.fetch_add(1, Ordering::AcqRel);
    }
    let mut q = a.ops.lock().unwrap();
    let seq = a.tail.fetch_add(1, Ordering::AcqRel) + 1;
    before(seq);
    q.push_back((seq, op));
    drop(q);
    a.cv.notify_one();
    seq
}

/// Ops queued and not yet replayed.
pub fn aq_busy() -> bool {
    AQ.get().is_some_and(|a| a.done.load(Ordering::Acquire) < a.tail.load(Ordering::Acquire))
}

/// Waits until every op queued so far has been replayed (not on the replay thread itself).
pub fn aq_drain() {
    if on_replay_thread() {
        return;
    }
    let Some(a) = AQ.get() else { return };
    let want = a.tail.load(Ordering::Acquire);
    let mut l = a.idle_mu.lock().unwrap();
    while a.done.load(Ordering::Acquire) < want {
        l = a.idle_cv.wait_timeout(l, std::time::Duration::from_millis(20)).unwrap().0;
    }
}

/// Waits until `ready()`, or every op queued so far has been replayed, or `timeout_ns` has passed.
pub fn aq_wait_until(timeout_ns: u64, ready: impl Fn() -> bool) {
    if on_replay_thread() {
        return;
    }
    let Some(a) = AQ.get() else { return };
    let want = a.tail.load(Ordering::Acquire);
    let t0 = std::time::Instant::now();
    let limit = std::time::Duration::from_nanos(timeout_ns.min(3_600_000_000_000));
    let mut l = a.idle_mu.lock().unwrap();
    while a.done.load(Ordering::Acquire) < want && !ready() && t0.elapsed() < limit {
        l = a.idle_cv.wait_timeout(l, std::time::Duration::from_millis(2)).unwrap().0;
    }
}

/// Ops queued and not replayed yet.
pub fn aq_backlog() -> u64 {
    AQ.get().map(|a| a.tail.load(Ordering::Acquire) - a.done.load(Ordering::Acquire)).unwrap_or(0)
}

/// Replay time on the replay thread (ns, cumulative).
pub fn aq_ns() -> u64 {
    AQ.get().map(|a| a.ns.load(Ordering::Relaxed)).unwrap_or(0)
}

fn aq_main(g: &'static Gpu, a: &'static Aq) {
    loop {
        // the op, and the sequence numbers of the submits of the same queue queued right behind it and taken with it
        let mut merged: Vec<u64> = Vec::new();
        let (mut seq, mut op) = {
            let mut q = a.ops.lock().unwrap();
            loop {
                if let Some(x) = q.pop_front() {
                    break x;
                }
                q = a.cv.wait(q).unwrap();
            }
        };
        // submits of one queue already waiting one after another replay as one submission: one pool sync (a write-
        // watch scan of every mirror: Mario Kart World's ~3,500 pools cost ~10 ms a submission) and one queue submit
        // for all of them. Their fences all resolve to the merged submission's (later for the first: conservative).
        // NXGFX_AQ_MERGE=0: one submission each.
        if let Op::Submit { q: oq, stream: ref mut s0, .. } = op {
            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *ON.get_or_init(|| std::env::var("NXGFX_AQ_MERGE").as_deref() != Ok("0")) {
                let mut q = a.ops.lock().unwrap();
                while merged.len() < 7 && s0.len() < 64 << 20 {
                    match q.front() {
                        Some((_, Op::Submit { q: nq, .. })) if *nq == oq => {}
                        _ => break,
                    }
                    let Some((ns, Op::Submit { stream, .. })) = q.pop_front() else { break };
                    merged.push(seq);
                    seq = ns;
                    s0.extend_from_slice(&stream);
                    AQ_MERGED.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let t0 = std::time::Instant::now();
        // the NVN queue of a submit or present: its fence is a receipt while that queue stays parked
        let opq = match &op {
            Op::Submit { q, .. } | Op::Present { q, .. } | Op::WaitReceipt { q, .. } => Some(*q),
            _ => None,
        };
        match op {
            Op::Submit { q, ll, stream } => {
                let mut st = g.q.sub.lock().unwrap();
                take_recycled(g, &mut st);
                if let Some(p) = st.parked.get_mut(&q) {
                    p.stream.extend_from_slice(&stream);
                } else {
                    run_stream(g, &mut st, q, ll, stream, Vec::new());
                }
            }
            Op::Present { q, ll, t, crop } => {
                present_now(g, q, ll, &t, crop);
                AQ_PRESENTS.fetch_sub(1, Ordering::AcqRel);
            }
            Op::Call(f) => f(g),
            Op::Retry => retry_parked(g),
            Op::WaitReceipt { q, r } => {
                // an async fence: the receipt its op left when its queue was parked (else the timeline orders it)
                let was_async = r & RECEIPT == 0;
                let r = if was_async { a.fences.lock().unwrap().range((r & !ASYNC)..).next().map(|(_, v)| *v).unwrap_or(0) } else { r };
                let mut st = g.q.sub.lock().unwrap();
                if r & RECEIPT != 0 && !g.q.receipts.lock().unwrap().contains_key(&r) {
                    if was_async {
                        if !ASYNC_WAIT_LOGGED.swap(true, Ordering::Relaxed) { crate::vklog!("a queue waits on another queue's parked work (an async fence of a submit made while that queue waited on an event)"); }
                    }
                    park_on_receipt(&mut st, q, r);
                }
            }
        }
        // parked queues whose wait now passes run on (a receipt resolved, an event signalled)
        {
            let mut st = g.q.sub.lock().unwrap();
            if !st.parked.is_empty() {
                retry_locked(g, &mut st);
            }
        }
        a.ns.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
        // the op's fence: the timeline after it, or, when its queue is parked at an event wait, a receipt of that
        // queue's (resolved once its parked work has run: a SyncWait on it must not pass before)
        let fence = {
            let mut st = g.q.sub.lock().unwrap();
            match opq.and_then(|q| st.parked.get_mut(&q)) {
                Some(p) => {
                    let id = RECEIPT | g.q.next_receipt.fetch_add(1, Ordering::Relaxed);
                    p.receipts.push(id);
                    id
                }
                None => g.q.last_fence(),
            }
        };
        {
            let mut f = a.fences.lock().unwrap();
            for &m in &merged {
                f.insert(m, fence);
            }
            f.insert(seq, fence);
            while f.len() > 4096 {
                let k = *f.keys().next().unwrap();
                f.remove(&k);
            }
        }
        a.done.store(seq, Ordering::Release);
        {
            let _l = a.idle_mu.lock().unwrap();
            a.idle_cv.notify_all();
        }
        // fence waiters look again
        let _d = g.q.done.lock().unwrap();
        g.q.done_cv.notify_all();
    }
}

/// The timeline value an async fence resolves to, once its op ran.
fn aq_physical(f: u64) -> Option<u64> {
    let a = AQ.get()?;
    let seq = f & !ASYNC;
    if a.done.load(Ordering::Acquire) < seq {
        return None;
    }
    let v = {
        let m = a.fences.lock().unwrap();
        // an op older than the ones kept: long done, its fence at most the oldest kept one's
        m.range(seq..).next().map(|(_, v)| *v).unwrap_or(0)
    };
    if v & RECEIPT != 0 {
        // a parked queue's: its timeline value once that work has run
        return Gpu::try_get().and_then(|g| g.q.receipts.lock().unwrap().get(&v).copied());
    }
    Some(v)
}

/// Runs `f` after every op queued so far (on the replay thread), or now when nothing is queued.
pub fn after_queued(f: Box<dyn FnOnce(&'static Gpu) + Send>) {
    let Some(g) = Gpu::try_get() else { return };
    if aq_busy() && !on_replay_thread() {
        aq_push(g, Op::Call(f));
    } else {
        f(g);
    }
}

/// Submits replayed together with the one before them (`aq_main`).
pub static AQ_MERGED: AtomicU64 = AtomicU64::new(0);
static ASYNC_WAIT_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CB_FENCE_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The last op queued for each NVN queue (its fences name it).
static LAST_SEQ: Mutex<Option<HashMap<u64, u64>>> = Mutex::new(None);

/// nvnQueueSubmitCommands.
pub fn submit(qserial: u64, origin_ll: bool, handles: &[u64]) {
    let Some(g) = Gpu::try_get() else { return };
    if aq_on() || super::capture::on() {
        let mut stream = Vec::new();
        let mut syncs = Vec::new();
        unsafe {
            crate::nvn::cmd::for_each_record(handles, |op, r| {
                if op == crate::nvn::obj::op::FENCE_SYNC && r.len() >= 16 {
                    syncs.push(core::ptr::read_unaligned(r.as_ptr().add(8) as *const u64));
                }
                stream.extend_from_slice(r)
            })
        };
        super::capture::submit(qserial, origin_ll, &stream);
        if !aq_on() {
            submit_stream(qserial, origin_ll, stream);
            return;
        }
        if !syncs.is_empty() && !CB_FENCE_LOGGED.swap(true, Ordering::Relaxed) {
            crate::vklog!("command-buffer fence syncs: fenced when submitted (their replay sets the exact value)");
        }
        // a sync fenced inside these command buffers covers this submission from now: one reused every frame still held
        // last frame's (done) fence until the replay thread reached it, so a wait on it passed while this frame's work was
        // still queued and the game rewrote memory that work reads (Turbo fences at submit; NXGFX_ASYNC=0 replays at
        // once). Written before the replay thread can see the op: its exact value (set when the replay reaches the
        // record, before a later event wait parks the queue) must not be overwritten by this one (that waited for the
        // parked rest: Donkey Kong Bananza's main thread held 15 s in nvnSyncWait)
        let seq = aq_push_with(g, Op::Submit { q: qserial, ll: origin_ll, stream }, |seq| {
            for &s in syncs.iter().filter(|&&s| s != 0) {
                let so = s as *mut crate::nvn::obj::sync_t;
                unsafe {
                    core::ptr::write_volatile(&mut (*so).fence, ASYNC | seq);
                    core::ptr::write_volatile(&mut (*so).fenced, 1);
                }
            }
        });
        LAST_SEQ.lock().unwrap().get_or_insert_with(HashMap::new).insert(qserial, seq);
        return;
    }
    let mut st = g.q.sub.lock().unwrap();
    take_recycled(g, &mut st);
    if let Some(p) = st.parked.get_mut(&qserial) {
        // queue order: behind the parked work
        unsafe { crate::nvn::cmd::for_each_record(handles, |_, r| p.stream.extend_from_slice(r)) };
        return;
    }
    let mut stream = Vec::new();
    unsafe { crate::nvn::cmd::for_each_record(handles, |_, r| stream.extend_from_slice(r)) };
    run_stream(g, &mut st, qserial, origin_ll, stream, Vec::new());
}

/// A submission given as its flattened records (the capture's replay; `submit` without async replay).
pub fn submit_stream(qserial: u64, origin_ll: bool, stream: Vec<u8>) {
    let Some(g) = Gpu::try_get() else { return };
    if aq_on() && !on_replay_thread() {
        aq_push(g, Op::Submit { q: qserial, ll: origin_ll, stream });
        return;
    }
    let mut st = g.q.sub.lock().unwrap();
    take_recycled(g, &mut st);
    if let Some(p) = st.parked.get_mut(&qserial) {
        p.stream.extend_from_slice(&stream);
        return;
    }
    run_stream(g, &mut st, qserial, origin_ll, stream, Vec::new());
}

/// Replays a stream for a queue in one submission; parks the queue at an unsatisfied wait.
fn run_stream(g: &'static Gpu, st: &mut SubmitState, qserial: u64, origin_ll: bool, stream: Vec<u8>, receipts: Vec<u64>) {
    let mut sub = new_sub(g, st);
    // a changed resolution scale makes the scaled render targets again first
    super::scale::begin_submit(g, &mut sub);
    let t0 = std::time::Instant::now();
    let mut nd = 0;
    let mut tp = 0.0;
    super::perf::timed(super::perf::C::PrepNs, || {
        let dirty = super::perf::timed(super::perf::C::PoolSyncNs, || super::mem::begin_submit(g, &mut sub));
        tp = t0.elapsed().as_secs_f64();
        nd = dirty.len();
        super::gmem::begin(g, &mut sub);
        g.image_begin_submit(&mut sub, &dirty);
    });
    let tt = t0.elapsed().as_secs_f64();
    if tt > 1.0 {
        crate::vklog!("a submission's set-up took {tt:.2} s: pools {tp:.2} s ({nd} CPU-written ranges), textures {:.2} s", tt - tp);
    }
    let mut qs = st.qstates.remove(&qserial).unwrap_or_else(|| QState::new(origin_ll));
    let rest = super::perf::timed(super::perf::C::ReplayNs, || exec::run(g, &mut sub, &mut qs, &stream));
    exec::end(g, &mut sub, &mut qs);
    st.qstates.insert(qserial, qs);
    let value = submit_sub(g, sub);
    match rest {
        // parked again: its receipts wait for the rest
        Some(off) => {
            st.parked.insert(qserial, Parked { stream: stream[off..].to_vec(), receipts, wait_receipt: None, presents: Vec::new(), later: Vec::new() });
            // the retry thread (it also logs a queue parked for long)
            request_retry(g);
        }
        None if !receipts.is_empty() => {
            let mut r = g.q.receipts.lock().unwrap();
            for id in receipts {
                r.insert(id, value);
            }
            drop(r);
            g.q.done_cv.notify_all();
        }
        None => {}
    }
}

/// A queue parked for 2 s or more, logged once per park with its wait (the retry thread, every second).
fn log_parked(g: &Gpu) {
    static SEEN: Mutex<Option<HashMap<u64, (std::time::Instant, bool)>>> = Mutex::new(None);
    let Ok(st) = g.q.sub.try_lock() else { return };
    let mut seen = SEEN.lock().unwrap();
    let m = seen.get_or_insert_with(HashMap::new);
    m.retain(|q, _| st.parked.contains_key(q));
    let now = std::time::Instant::now();
    for (q, p) in st.parked.iter() {
        let e = m.entry(*q).or_insert((now, false));
        if !e.1 && now.duration_since(e.0).as_secs() >= 2 {
            e.1 = true;
            let what = if p.wait_receipt.is_some() && p.stream.is_empty() { "a sync of another parked queue".to_string() } else { exec::wait_desc(&p.stream) };
            crate::log::red(&format!("queue {q:#x} parked for 2 s: {what}"));
        }
    }
}

/// Parked queues whose wait now passes run on.
pub fn retry_parked(g: &'static Gpu) {
    let mut st = g.q.sub.lock().unwrap();
    retry_locked(g, &mut st);
}

fn retry_locked(g: &'static Gpu, st: &mut SubmitState) {
    take_recycled(g, st);
    loop {
        let ready: Vec<u64> = st
            .parked
            .iter()
            .filter(|(_, p)| p.wait_receipt.is_none_or(|r| g.q.receipts.lock().unwrap().contains_key(&r)) && (p.stream.is_empty() || exec::wait_passes(g, &p.stream)))
            .map(|(k, _)| *k)
            .collect();
        if ready.is_empty() {
            return;
        }
        for q in ready {
            let mut p = st.parked.remove(&q).unwrap();
            let ll = st.qstates.get(&q).map(|s| s.origin_ll).unwrap_or(true);
            if let Some(&(off, r)) = p.later.first() {
                // the work up to a queued wait runs; the rest parks behind that wait (its receipts and presents last)
                let tail = p.stream.split_off(off.min(p.stream.len()));
                let later: Vec<(usize, u64)> = p.later[1..].iter().map(|&(o, x)| (o - off, x)).collect();
                if !p.stream.is_empty() {
                    run_stream(g, st, q, ll, p.stream, Vec::new());
                }
                match st.parked.get_mut(&q) {
                    Some(again) => {
                        let base = again.stream.len();
                        again.later.push((base, r));
                        again.later.extend(later.into_iter().map(|(o, x)| (o + base, x)));
                        again.stream.extend_from_slice(&tail);
                        again.receipts.extend(p.receipts);
                        again.presents.extend(p.presents);
                    }
                    None => {
                        st.parked.insert(q, Parked { stream: tail, receipts: p.receipts, wait_receipt: Some(r), presents: p.presents, later });
                    }
                }
                continue;
            }
            if !p.stream.is_empty() || !p.receipts.is_empty() {
                run_stream(g, st, q, ll, p.stream, p.receipts);
            }
            match st.parked.get_mut(&q) {
                // parked again: its presents stay behind
                Some(again) => again.presents.extend(p.presents),
                None => {
                    for (t, crop) in p.presents {
                        let frame = present_frame(&t, crop, ll, false);
                        present_locked(g, st, q, ll, &t, crop, frame);
                    }
                }
            }
        }
    }
}

/// A CPU event signal: a parked queue may run on.
pub fn event_signalled() {
    if let Some(g) = Gpu::try_get() {
        if AQ.get().is_some() && !on_replay_thread() {
            aq_push(g, Op::Retry);
            return;
        }
        let any = !g.q.sub.lock().unwrap().parked.is_empty();
        if any {
            retry_parked(g);
        }
    }
}

/// nvnQueueFenceSync: the point covering everything submitted to `qserial` (0 = every queue) so far.
pub fn queue_fence(qserial: u64) -> u64 {
    let Some(g) = Gpu::try_get() else { return 0 };
    if let Some(a) = AQ.get() {
        // the queue's own last op (Turbo's last_seq: another queue's parked work is not waited for); 0: every queue's
        let t = a.tail.load(Ordering::Acquire);
        let s = if qserial != 0 { LAST_SEQ.lock().unwrap().as_ref().and_then(|m| m.get(&qserial).copied()).unwrap_or(t) } else { t };
        if !on_replay_thread() && (a.done.load(Ordering::Acquire) < s || aq_physical(ASYNC | s).is_none()) {
            return ASYNC | s;
        }
    }
    let mut st = g.q.sub.lock().unwrap();
    if qserial != 0 {
        if let Some(p) = st.parked.get_mut(&qserial) {
            let id = RECEIPT | g.q.next_receipt.fetch_add(1, Ordering::Relaxed);
            p.receipts.push(id);
            return id;
        }
    }
    g.q.last_fence()
}

/// nvnQueueWaitSync: one physical queue runs submissions in order (each starts with a full barrier), so a fence
/// already submitted orders itself; a receipt of a parked queue parks this one too.
pub fn queue_wait_fence(_qserial: u64, fence: u64) {
    let Some(g) = Gpu::try_get() else { return };
    if fence & RECEIPT == 0 {
        // an async fence of a submit that found its queue parked at an event wait resolves to that queue's receipt: the
        // waiting queue must not run ahead of the parked work (the replay thread resolves it in queue order)
        if fence & ASYNC != 0 && AQ.get().is_some() && !on_replay_thread() && aq_physical(fence).is_none() {
            aq_push(g, Op::WaitReceipt { q: _qserial, r: fence });
        }
        return;
    }
    if g.q.receipts.lock().unwrap().contains_key(&fence) {
        return;
    }
    // the waiting queue's later work runs once that sync's submission has (it parks behind it)
    if aq_on() {
        aq_push(g, Op::WaitReceipt { q: _qserial, r: fence });
    } else {
        let mut st = g.q.sub.lock().unwrap();
        park_on_receipt(&mut st, _qserial, fence);
    }
}

fn physical(g: &Gpu, f: u64) -> Option<u64> {
    if f & RECEIPT == 0 && f & ASYNC != 0 {
        return aq_physical(f);
    }
    if f & RECEIPT == 0 {
        Some(f)
    } else {
        g.q.receipts.lock().unwrap().get(&f).copied()
    }
}

pub fn fence_done(f: u64) -> bool {
    let Some(g) = Gpu::try_get() else { return true };
    match physical(g, f) {
        Some(p) => g.q.completed() >= p,
        None => false,
    }
}

/// Waits until `f` and its CPU-visible effects are done; false on timeout.
pub fn wait_fence(f: u64, timeout_ns: u64) -> bool {
    let Some(g) = Gpu::try_get() else { return true };
    let deadline = std::time::Instant::now() + std::time::Duration::from_nanos(timeout_ns.min(3600_000_000_000));
    let mut d = g.q.done.lock().unwrap();
    loop {
        if let Some(p) = physical(g, f) {
            if *d >= p {
                return true;
            }
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return false;
        }
        d = g.q.done_cv.wait_timeout(d, (deadline - now).min(std::time::Duration::from_millis(50))).unwrap().0;
    }
}

/// nvnQueueFinish: the GPU work submitted so far, with its effects.
pub fn finish() {
    let Some(g) = Gpu::try_get() else { return };
    aq_drain();
    let f = g.q.last_fence();
    wait_fence(f, u64::MAX);
}

pub fn queue_forget(qserial: u64) {
    if let Some(g) = Gpu::try_get() {
        let mut st = g.q.sub.lock().unwrap();
        st.qstates.remove(&qserial);
        st.parked.remove(&qserial);
    }
}

/// nvnQueuePresentTexture: in queue order after the work submitted before it. Returns a ticket for `present_wait`.
pub fn present(qserial: u64, origin_ll: bool, t: &crate::nvn::obj::tex_t, crop: Option<[i32; 4]>) -> u64 {
    let Some(g) = Gpu::try_get() else { return 0 };
    super::capture::present(qserial, origin_ll, t, crop);
    if aq_on() {
        return ASYNC | aq_push(g, Op::Present { q: qserial, ll: origin_ll, t: *t, crop });
    }
    present_now(g, qserial, origin_ll, t, crop)
}

fn present_now(g: &'static Gpu, qserial: u64, origin_ll: bool, t: &crate::nvn::obj::tex_t, crop: Option<[i32; 4]>) -> u64 {
    {
        let mut st = g.q.sub.lock().unwrap();
        take_recycled(g, &mut st);
        if let Some(p) = st.parked.get_mut(&qserial) {
            // in queue order: shown once the queue's parked work has run (retry_locked)
            p.presents.push((*t, crop));
            return g.q.last_fence();
        }
    }
    // the frame's bookkeeping and a requested screenshot before the submission state is locked: a screenshot reads
    // the texture back through its own submission (oneoff locks it; under the lock the game froze in its present)
    let frame = present_frame(t, crop, origin_ll, true);
    let mut st = g.q.sub.lock().unwrap();
    take_recycled(g, &mut st);
    present_locked(g, &mut st, qserial, origin_ll, t, crop, frame)
}

/// A present's frame number, telemetry and pass-dump bookkeeping, and the screenshot requested for it (`shot`:
/// false where the submission state is held).
fn present_frame(t: &crate::nvn::obj::tex_t, crop: Option<[i32; 4]>, origin_ll: bool, shot: bool) -> u64 {
    let frame = super::present::note_present();
    super::perf::frame(frame);
    super::scale::frame();
    super::exec::passdump::frame(frame, crate::log::elapsed());
    if shot {
        super::shot::on_present(t, frame, crop, origin_ll);
    }
    frame
}

/// A present in queue order (the submission state locked).
fn present_locked(g: &'static Gpu, st: &mut SubmitState, _qserial: u64, origin_ll: bool, t: &crate::nvn::obj::tex_t, crop: Option<[i32; 4]>, _frame: u64) -> u64 {
    let mut sub = new_sub(g, st);
    super::perf::timed(super::perf::C::PrepNs, || {
        let dirty = super::perf::timed(super::perf::C::PoolSyncNs, || super::mem::begin_submit(g, &mut sub));
        g.image_begin_submit(&mut sub, &dirty);
    });
    super::present::record_present(g, &mut sub, t, origin_ll, crop);
    super::shot::record_sink(g, &mut sub, t, crop, origin_ll);
    submit_sub(g, sub)
}

pub fn present_wait(ticket: u64) {
    wait_fence(ticket, u64::MAX);
}

/// Waits for everything (tests and teardown).
pub fn idle() {
    finish();
}

/// A submission of host work (texel reads, tools): `f` records into it with the images lock held. Returns its fence.
pub fn oneoff(_g: &Gpu, f: impl FnOnce(&Gpu, &mut Sub, &mut super::image::Images)) -> u64 {
    let g: &'static Gpu = Gpu::try_get().expect("no Vulkan device");
    // after what the game submitted before (a read of what it drew)
    aq_drain();
    let mut st = g.q.sub.lock().unwrap();
    take_recycled(g, &mut st);
    let mut sub = new_sub(g, &mut st);
    super::perf::timed(super::perf::C::PrepNs, || {
        let dirty = super::perf::timed(super::perf::C::PoolSyncNs, || super::mem::begin_submit(g, &mut sub));
        g.image_begin_submit(&mut sub, &dirty);
    });
    {
        let mut imgs = g.images.lock().unwrap();
        f(g, &mut sub, &mut imgs);
    }
    submit_sub(g, sub)
}

#[cfg(test)]
mod occl_tests {
    use super::*;
    /// Presents wait for the replay to catch up only while occlusion results were used in the last 120 presents.
    #[test]
    fn occlusion_results_keep_the_replay_caught_up() {
        OCCL_FRAME.store(0, Ordering::Relaxed);
        assert!(!occl_sync(), "never used: async lag as usual");
        OCCL_FRAME.store(super::super::present::frames().max(1), Ordering::Relaxed);
        assert!(occl_sync(), "used just now: presents wait for the replay");
        OCCL_FRAME.store(0, Ordering::Relaxed);
    }
}
